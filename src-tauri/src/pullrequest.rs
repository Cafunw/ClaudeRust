//! Pull requests from a session's branch: what would go into one, a title
//! and description drafted by Claude, and creating it with the GitHub CLI
//! (`gh`) — or opening the host's compare page when `gh` isn't installed.

use crate::error::{AppError, AppResult};
use crate::gitops::{git, git_raw};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// `owner/repo` path and host of a GitHub or GitLab remote URL.
fn remote_parts(url: &str) -> Option<(String, String)> {
    let u = url.trim().trim_end_matches('/');
    let u = u.strip_suffix(".git").unwrap_or(u);
    let (host, path) = if let Some(rest) = u.strip_prefix("git@") {
        rest.split_once(':')?
    } else {
        let rest = u.split_once("://")?.1;
        let rest = rest.rsplit_once('@').map(|(_, r)| r).unwrap_or(rest);
        rest.split_once('/')?
    };
    let host = host.split(':').next()?.to_ascii_lowercase();
    if path.split('/').filter(|s| !s.is_empty()).count() < 2 {
        return None;
    }
    Some((host, path.trim_matches('/').to_string()))
}

/// Web page that starts a pull / merge request for `branch` into `base`.
pub fn compare_url(remote: &str, base: &str, branch: &str) -> Option<String> {
    let (host, path) = remote_parts(remote)?;
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    if host == "github.com" || host.starts_with("github.") {
        Some(format!("https://{host}/{path}/compare/{}...{}?expand=1", enc(base), enc(branch)))
    } else if host == "gitlab.com" || host.starts_with("gitlab.") {
        Some(format!(
            "https://{host}/{path}/-/merge_requests/new?merge_request%5Bsource_branch%5D={}&merge_request%5Btarget_branch%5D={}",
            enc(branch),
            enc(base)
        ))
    } else {
        None
    }
}

/// A branch name for a pull request titled `title`.
pub fn branch_for(title: &str) -> String {
    let mut slug = String::new();
    for c in title.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    format!("clauderust/{}", if slug.is_empty() { "changes" } else { slug })
}

/// Split Claude's draft ("first line = title, rest = body").
pub fn split_draft(text: &str) -> (String, String) {
    let mut t = text.trim();
    // a reply wrapped in a code fence (```markdown … ```)
    if t.starts_with("```") {
        t = t.split_once('\n').map(|(_, rest)| rest).unwrap_or("");
        t = t.trim_end().strip_suffix("```").unwrap_or(t);
    }
    let t = t.trim().trim_matches('`').trim();
    let mut lines = t.lines();
    let title = lines
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('#')
        .trim()
        .trim_start_matches("Title:")
        .trim()
        .to_string();
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    (title, body)
}

async fn out(root: &Path, args: &[&str]) -> Option<String> {
    let o = git_raw(root, args).await.ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

/// The default branch of `origin` (without the remote prefix).
pub async fn base_branch(root: &Path) -> String {
    if let Some(r) = out(root, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]).await {
        if let Some(b) = r.strip_prefix("origin/") {
            return b.to_string();
        }
    }
    for b in ["main", "master", "trunk", "develop"] {
        if out(root, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/origin/{b}")]).await.is_some() {
            return b.to_string();
        }
    }
    "main".into()
}

async fn gh(root: &Path, args: &[&str], stdin: Option<&str>) -> AppResult<String> {
    crate::shellenv::ready().await;
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut cmd = crate::shellenv::command("gh", &args);
    cmd.current_dir(root)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .stdin(if stdin.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| AppError::msg(format!("Could not run gh: {e}")))?;
    if let (Some(text), Some(mut w)) = (stdin, child.stdin.take()) {
        use tokio::io::AsyncWriteExt;
        let _ = w.write_all(text.as_bytes()).await;
    }
    let o = tokio::time::timeout(Duration::from_secs(90), child.wait_with_output())
        .await
        .map_err(|_| AppError::msg("gh took too long and was stopped."))?
        .map_err(|e| AppError::msg(e.to_string()))?;
    if !o.status.success() {
        let e = String::from_utf8_lossy(&o.stderr).trim().to_string();
        return Err(AppError::msg(if e.is_empty() { "gh failed".to_string() } else { e }));
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// What a pull request from the current branch would contain.
pub async fn info(root: &Path) -> AppResult<Value> {
    let branch = out(root, &["rev-parse", "--abbrev-ref", "HEAD"]).await.unwrap_or_default();
    if branch.is_empty() || branch == "HEAD" {
        return Err(AppError::msg("The repository isn't on a branch (detached HEAD)."));
    }
    let remote = out(root, &["remote", "get-url", "origin"]).await;
    let base = base_branch(root).await;
    let base_ref = format!("origin/{base}");
    let has_base = out(root, &["rev-parse", "--verify", "--quiet", &base_ref]).await.is_some();
    let range = format!("{}..HEAD", if has_base { base_ref.as_str() } else { base.as_str() });
    let ahead: u64 = out(root, &["rev-list", "--count", &range]).await.and_then(|s| s.parse().ok()).unwrap_or(0);
    let commits = out(root, &["log", "--format=%h %s", "-20", &range]).await.unwrap_or_default();
    let dirty = out(root, &["status", "--porcelain"]).await.is_some();
    let gh_ok = crate::shellenv::which("gh");
    let mut existing = Value::Null;
    if gh_ok && remote.is_some() && branch != base {
        if let Ok(Ok(s)) = tokio::time::timeout(Duration::from_secs(15), gh(root, &["pr", "view", &branch, "--json", "url,state,title"], None)).await {
            if let Ok(v) = serde_json::from_str::<Value>(&s) {
                if v["state"] == "OPEN" {
                    existing = v;
                }
            }
        }
    }
    Ok(json!({
        "branch": branch,
        "base": base,
        "on_base": branch == base,
        "remote": remote,
        "ahead": ahead,
        "commits": commits.lines().collect::<Vec<_>>(),
        "dirty": dirty,
        "gh": gh_ok,
        "existing": existing,
        "compare_url": remote.as_deref().and_then(|r| compare_url(r, &base, &branch)),
    }))
}

pub const PR_PROMPT: &str = "Write a pull request title and description for the commits and diff below. First line: the title only (under 70 characters, no prefix like \"Title:\"). Then a blank line and the description in Markdown: a short summary of what changed and why, then a \"## Changes\" list of the notable changes, and a \"## Testing\" section if the diff shows how it was tested. Be concise and factual; don't invent details. Reply with the title and description only.";

/// The material Claude drafts the pull request from.
pub async fn draft_input(root: &Path, base: &str) -> String {
    let base_ref = format!("origin/{base}");
    let from = if out(root, &["rev-parse", "--verify", "--quiet", &base_ref]).await.is_some() { base_ref } else { base.to_string() };
    let log = git(root, &["log", "--format=%h %s%n%b", "-30", &format!("{from}..HEAD")]).await.unwrap_or_default();
    let diff = git(root, &["diff", "--no-color", "--no-ext-diff", "--stat", "--patch", &format!("{from}...HEAD")]).await.unwrap_or_default();
    format!("<commits>\n{}\n</commits>\n\n<diff>\n{}\n</diff>", log.trim(), crate::util::truncate_middle(&diff, 60_000))
}

/// Push the branch (creating `new_branch` first when given) and open the
/// pull request. Returns `{url, created}`; `created` is false when only the
/// compare page could be offered.
pub async fn create(root: &Path, title: &str, body: &str, base: &str, draft: bool, new_branch: Option<&str>) -> AppResult<Value> {
    if title.trim().is_empty() {
        return Err(AppError::msg("Give the pull request a title."));
    }
    if let Some(b) = new_branch.map(str::trim).filter(|b| !b.is_empty()) {
        git(root, &["check-ref-format", "--branch", b]).await.map_err(|_| AppError::msg(format!("“{b}” isn't a valid branch name.")))?;
        git(root, &["switch", "-c", b]).await?;
    }
    let branch = out(root, &["rev-parse", "--abbrev-ref", "HEAD"]).await.unwrap_or_default();
    if branch == base {
        return Err(AppError::msg(format!("You're on {base}; give a new branch name for the pull request.")));
    }
    git(root, &["push", "-u", "origin", "HEAD"]).await.map_err(|e| AppError::msg(format!("Push failed: {e}")))?;
    if crate::shellenv::which("gh") {
        let mut args = vec!["pr", "create", "--base", base, "--head", branch.as_str(), "--title", title.trim(), "--body-file", "-"];
        if draft {
            args.push("--draft");
        }
        let url = gh(root, &args, Some(body)).await?;
        let url = url.lines().rev().find(|l| l.starts_with("http")).unwrap_or(url.as_str()).trim().to_string();
        return Ok(json!({"url": url, "created": true, "branch": branch}));
    }
    let remote = out(root, &["remote", "get-url", "origin"]).await.unwrap_or_default();
    match compare_url(&remote, base, &branch) {
        Some(u) => Ok(json!({"url": u, "created": false, "branch": branch})),
        None => Err(AppError::msg("Pushed. Install the GitHub CLI (gh) to create pull requests from here, or open one on your Git host.")),
    }
}

/// Whether `pr` names a pull request: a number (`42`, `#42`) or a pull
/// request URL. Anything else (notably text starting with `-`) is refused.
pub fn pr_ref(pr: &str) -> Option<String> {
    let p = pr.trim().trim_start_matches('#');
    if !p.is_empty() && p.len() <= 10 && p.chars().all(|c| c.is_ascii_digit()) {
        return Some(p.to_string());
    }
    let u = pr.trim();
    if (u.starts_with("https://") || u.starts_with("http://")) && (u.contains("/pull/") || u.contains("/merge_requests/")) && !u.contains(char::is_whitespace) {
        return Some(u.to_string());
    }
    None
}

/// A pull request's details and diff, through `gh`.
pub async fn fetch(root: &Path, pr: &str) -> AppResult<Value> {
    let Some(r) = pr_ref(pr) else { return Err(AppError::msg("Give a pull request number or URL.")) };
    crate::shellenv::ready().await;
    if !crate::shellenv::which("gh") {
        return Err(AppError::msg("Reviewing a pull request needs the GitHub CLI (gh), signed in with `gh auth login`."));
    }
    let meta = gh(root, &["pr", "view", &r, "--json", "number,title,body,url,author,baseRefName,headRefName,additions,deletions,changedFiles"], None).await?;
    let mut v: Value = serde_json::from_str(&meta).map_err(|_| AppError::msg("gh returned something unexpected."))?;
    let diff = gh(root, &["pr", "diff", &r, "--color", "never"], None).await?;
    v["diff"] = json!(diff);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_urls() {
        let gh = "https://github.com/acme/app/compare/main...feat%2Fx?expand=1";
        assert_eq!(compare_url("git@github.com:acme/app.git", "main", "feat/x").as_deref(), Some(gh));
        assert_eq!(compare_url("https://github.com/acme/app", "main", "feat/x").as_deref(), Some(gh));
        assert_eq!(compare_url("https://token@github.com/acme/app.git/", "main", "feat/x").as_deref(), Some(gh));
        assert_eq!(compare_url("ssh://git@github.com:22/acme/app.git", "main", "feat/x").as_deref(), Some(gh));
        assert!(compare_url("git@gitlab.com:grp/sub/app.git", "main", "b").unwrap().starts_with("https://gitlab.com/grp/sub/app/-/merge_requests/new?"));
        assert_eq!(compare_url("/srv/git/app.git", "main", "b"), None);
        assert_eq!(compare_url("https://example.org/acme/app.git", "main", "b"), None);
    }

    #[test]
    fn pr_refs() {
        assert_eq!(pr_ref("42").as_deref(), Some("42"));
        assert_eq!(pr_ref("#7").as_deref(), Some("7"));
        assert_eq!(pr_ref("https://github.com/a/b/pull/9").as_deref(), Some("https://github.com/a/b/pull/9"));
        assert_eq!(pr_ref("--repo=x"), None);
        assert_eq!(pr_ref("focus on security"), None);
        assert_eq!(pr_ref("https://github.com/a/b/pull/9 --web"), None);
        assert_eq!(pr_ref(""), None);
    }

    #[test]
    fn branch_names_and_drafts() {
        assert_eq!(branch_for("Fix the login bug!"), "clauderust/fix-the-login-bug");
        assert_eq!(branch_for("  "), "clauderust/changes");
        assert!(branch_for(&"word ".repeat(30)).len() <= "clauderust/".len() + 41);
        assert_eq!(split_draft("Add caching\n\nSpeeds up reads.\n\n## Changes\n- x"), ("Add caching".into(), "Speeds up reads.\n\n## Changes\n- x".into()));
        assert_eq!(split_draft("# Title: Add caching").0, "Add caching");
        assert_eq!(split_draft("").0, "");
        assert_eq!(split_draft("```markdown\nAdd caching\n\nBody text\n```"), ("Add caching".into(), "Body text".into()));
    }

    #[tokio::test]
    async fn info_and_push_with_a_local_remote() {
        let d = std::env::temp_dir().join(format!("cr-pr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let origin = d.join("origin.git");
        let work = d.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let run = |dir: &Path, args: &[&str]| {
            let o = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        };
        run(&d, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        run(&work, &["init", "-q", "-b", "main"]);
        run(&work, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init"]);
        run(&work, &["remote", "add", "origin", origin.to_str().unwrap()]);
        run(&work, &["push", "-q", "-u", "origin", "main"]);
        run(&work, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "feature work"]);
        let i = info(&work).await.unwrap();
        assert_eq!(i["branch"], "main");
        assert_eq!(i["base"], "main");
        assert_eq!(i["on_base"], true);
        assert_eq!(i["ahead"], 1);
        assert!(i["commits"][0].as_str().unwrap().ends_with("feature work"));
        assert!(draft_input(&work, "main").await.contains("feature work"));
        // on the default branch without a new branch name: refused
        assert!(create(&work, "T", "B", "main", false, None).await.is_err());
        // a local remote has no web page, so after pushing the new branch it
        // reports that (unless gh is installed, which can't reach it anyway)
        if !crate::shellenv::which("gh") {
            let e = create(&work, "Feature", "Body", "main", false, Some("clauderust/feature")).await.unwrap_err();
            assert!(e.to_string().starts_with("Pushed."));
            assert!(out(&origin, &["rev-parse", "--verify", "--quiet", "refs/heads/clauderust/feature"]).await.is_some());
        }
        assert!(create(&work, "T", "B", "main", false, Some("bad..name")).await.is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
