//! Secrets about to be committed: API keys, tokens and private keys in the
//! added lines of a diff, and environment files being added.

use std::sync::LazyLock;
use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Finding {
    pub path: String,
    /// Line in the new file (0 when the whole file is the problem).
    pub line: usize,
    pub kind: String,
    /// The matched text with most of it masked.
    pub preview: String,
}

static RULES: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    [
        ("Anthropic API key", r"\bsk-ant-[A-Za-z0-9_\-]{20,}"),
        ("OpenAI API key", r"\bsk-(?:proj-|svcacct-)?[A-Za-z0-9_\-]{32,}"),
        ("AWS access key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
        ("GitHub token", r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36}\b|\bgithub_pat_[A-Za-z0-9_]{60,}"),
        ("GitLab token", r"\bglpat-[A-Za-z0-9_\-]{20}\b"),
        ("Slack token", r"\bxox[abprs]-[A-Za-z0-9-]{10,}"),
        ("Google API key", r"\bAIza[0-9A-Za-z_\-]{35}\b"),
        ("Stripe secret key", r"\b(?:sk|rk)_live_[0-9A-Za-z]{20,}"),
        ("Private key", r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY( BLOCK)?-----"),
        ("JSON web token", r"\beyJ[A-Za-z0-9_\-]{10,}\.eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}"),
        ("Password in a URL", r"[a-z][a-z0-9+.\-]*://[^/\s:@]+:[^/\s:@]{6,}@[^\s/]+"),
    ]
    .into_iter()
    .map(|(k, r)| (k, Regex::new(r).unwrap()))
    .collect()
});

/// `ab…yz` style masking that still lets you recognise which key it is.
fn mask(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 12 {
        return "•".repeat(chars.len().min(8));
    }
    let head: String = chars[..6].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

/// Environment files that usually hold secrets (examples / templates are fine).
pub fn is_env_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let template = ["example", "sample", "template", "dist", "defaults"].iter().any(|w| name.contains(w));
    (name == ".env" || name.starts_with(".env.") || name.ends_with(".env")) && !template
        || matches!(name.as_str(), "credentials.json" | "id_rsa" | "id_ed25519" | ".npmrc" | ".pypirc" | ".netrc")
}

/// Findings in a unified diff (`git diff` output): only added lines count.
pub fn scan_diff(diff: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut path = String::new();
    let mut new_line = 0usize;
    let mut new_file = false;
    for l in diff.lines() {
        if let Some(p) = l.strip_prefix("+++ ") {
            path = p.strip_prefix("b/").unwrap_or(p).to_string();
            if path != "/dev/null" && new_file && is_env_file(&path) {
                out.push(Finding { path: path.clone(), line: 0, kind: "Environment / credentials file".into(), preview: String::new() });
            }
            continue;
        }
        if l.starts_with("diff --git") {
            new_file = false;
            continue;
        }
        if l.starts_with("new file mode") {
            new_file = true;
            continue;
        }
        if let Some(rest) = l.strip_prefix("@@ ") {
            // @@ -a,b +c,d @@
            new_line = rest
                .split_whitespace()
                .find_map(|w| w.strip_prefix('+'))
                .and_then(|w| w.split(',').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            continue;
        }
        if l.starts_with("+++") || l.starts_with("---") {
            continue;
        }
        if let Some(added) = l.strip_prefix('+') {
            if added.len() < 4000 {
                for (kind, re) in RULES.iter() {
                    if let Some(m) = re.find(added) {
                        out.push(Finding { path: path.clone(), line: new_line, kind: (*kind).into(), preview: mask(m.as_str()) });
                        break;
                    }
                }
            }
            new_line += 1;
        } else if !l.starts_with('-') && !l.starts_with('\\') {
            new_line += 1;
        }
        if out.len() >= 50 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_added_secrets_only() {
        let key = format!("sk-ant-api03-{}", "A".repeat(40));
        let aws = "AKIAIOSFODNN7EXAMPLE";
        let diff = format!(
            "diff --git a/src/cfg.py b/src/cfg.py\n--- a/src/cfg.py\n+++ b/src/cfg.py\n@@ -1,3 +1,4 @@\n import os\n-OLD = \"{aws}\"\n+API_KEY = \"{key}\"\n+name = \"ok\"\n x = 1\n+url = \"postgres://admin:hunter2pass@db.local/app\"\ndiff --git a/.env b/.env\nnew file mode 100644\n--- /dev/null\n+++ b/.env\n@@ -0,0 +1 @@\n+DEBUG=1\ndiff --git a/.env.example b/.env.example\nnew file mode 100644\n--- /dev/null\n+++ b/.env.example\n@@ -0,0 +1 @@\n+KEY=\n"
        );
        let f = scan_diff(&diff);
        let kinds: Vec<(&str, usize, &str)> = f.iter().map(|x| (x.path.as_str(), x.line, x.kind.as_str())).collect();
        assert_eq!(kinds, vec![("src/cfg.py", 2, "Anthropic API key"), ("src/cfg.py", 5, "Password in a URL"), (".env", 0, "Environment / credentials file")], "{f:?}");
        assert!(f[0].preview.starts_with("sk-ant") && f[0].preview.contains('…') && !f[0].preview.contains(&"A".repeat(20)));
    }

    #[test]
    fn rules_and_env_files() {
        assert!(RULES.iter().any(|(_, r)| r.is_match(&format!("ghp_{}", "a".repeat(36)))));
        assert!(RULES.iter().any(|(_, r)| r.is_match("-----BEGIN OPENSSH PRIVATE KEY-----")));
        assert!(!RULES.iter().any(|(_, r)| r.is_match("sk-short")));
        assert!(!RULES.iter().any(|(_, r)| r.is_match("https://example.com/path:with@colon")));
        assert!(is_env_file(".env.production") && is_env_file("config/prod.env") && is_env_file("id_rsa"));
        assert!(!is_env_file(".env.example") && !is_env_file(".env.sample") && !is_env_file("environment.ts"));
    }
}
