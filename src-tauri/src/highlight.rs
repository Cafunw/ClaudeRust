//! Syntax highlighting with syntect (pure-Rust regex engine), emitting CSS
//! classes so light/dark themes are pure CSS.

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use syntect::html::{css_for_theme_with_class_style, ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::util::{escape_html, hash64, ByteCache};

static SYNTAXES: Lazy<SyntaxSet> = Lazy::new(two_face::syntax::extra_newlines);
/// Bounded by size, not count: a code block re-highlighted as it streams in
/// leaves many large, near-identical entries. Very large results aren't kept.
static CACHE: Lazy<Mutex<ByteCache>> = Lazy::new(|| Mutex::new(ByteCache::new(CACHE_BYTES, CACHE_ENTRY_BYTES)));
const CACHE_BYTES: usize = 24 * 1024 * 1024;
const CACHE_ENTRY_BYTES: usize = 1024 * 1024;
const STYLE: ClassStyle = ClassStyle::SpacedPrefixed { prefix: "hl-" };
const MAX_HIGHLIGHT_BYTES: usize = 300_000;

fn alias(lang: &str) -> &str {
    match lang {
        "js" | "jsx" | "javascript" | "mjs" | "cjs" | "node" => "js",
        "ts" | "typescript" | "mts" | "cts" => "ts",
        "tsx" => "tsx",
        "py" | "python" | "python3" | "py3" => "py",
        "rs" | "rust" => "rs",
        "sh" | "bash" | "shell" | "zsh" | "console" | "shellscript" | "terminal" => "bash",
        "ps" | "ps1" | "powershell" | "pwsh" => "ps1",
        "c++" | "cpp" | "cxx" | "hpp" => "cpp",
        "c#" | "csharp" | "cs" => "cs",
        "golang" | "go" => "go",
        "yml" | "yaml" => "yaml",
        "md" | "markdown" => "md",
        "kt" | "kotlin" => "kt",
        "objc" | "objective-c" => "m",
        "html" | "xhtml" | "htm" => "html",
        "xml" | "svg" => "xml",
        "dockerfile" | "docker" => "Dockerfile",
        "make" | "makefile" => "Makefile",
        "tf" | "hcl" | "terraform" => "tf",
        "jsonc" | "json5" => "json",
        "vue" => "vue",
        "sql" | "postgres" | "postgresql" | "mysql" | "sqlite" => "sql",
        other => other,
    }
}

fn find_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let l = lang.trim().to_ascii_lowercase();
    if l.is_empty() || l == "text" || l == "plain" || l == "plaintext" || l == "txt" {
        return None;
    }
    let a = alias(&l);
    SYNTAXES
        .find_syntax_by_token(a)
        .or_else(|| SYNTAXES.find_syntax_by_token(&l))
        .or_else(|| SYNTAXES.find_syntax_by_extension(a))
}

/// Highlight `code` and return the inner HTML for a `<code>` element.
pub fn highlight(code: &str, lang: &str) -> String {
    if code.len() > MAX_HIGHLIGHT_BYTES {
        return escape_html(code);
    }
    let Some(syntax) = find_syntax(lang) else {
        return escape_html(code);
    };
    let key = hash64(&format!("{lang}\u{0}{code}"));
    if let Some(h) = CACHE.lock().get(key) {
        return h.clone();
    }
    let mut gen = ClassedHTMLGenerator::new_with_class_style(syntax, &SYNTAXES, STYLE);
    for line in LinesWithEndings::from(code) {
        if gen.parse_html_for_line_which_includes_newline(line).is_err() {
            return escape_html(code);
        }
    }
    let html = gen.finalize();
    CACHE.lock().insert(key, html.clone());
    html
}

/// CSS for the highlight classes, scoped to light and dark themes.
pub fn theme_css() -> String {
    let themes = two_face::theme::extra();
    let light = themes.get(two_face::theme::EmbeddedThemeName::OneHalfLight);
    let dark = themes.get(two_face::theme::EmbeddedThemeName::OneHalfDark);
    let mut out = String::new();
    if let Ok(css) = css_for_theme_with_class_style(light, STYLE) {
        out.push_str(&scope_css(&css, ":root[data-theme=\"light\"]"));
    }
    if let Ok(css) = css_for_theme_with_class_style(dark, STYLE) {
        out.push_str(&scope_css(&css, ":root[data-theme=\"dark\"]"));
    }
    out
}

fn scope_css(css: &str, scope: &str) -> String {
    let mut out = String::new();
    // strip comments
    let mut clean = String::new();
    let mut rest = css;
    while let Some(i) = rest.find("/*") {
        clean.push_str(&rest[..i]);
        match rest[i..].find("*/") {
            Some(j) => rest = &rest[i + j + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    clean.push_str(rest);
    for rule in clean.split('}') {
        let Some((sel, body)) = rule.split_once('{') else { continue };
        let sel = sel.trim();
        if sel.is_empty() {
            continue;
        }
        // Drop the theme's background so code blocks use our own surface colours.
        let body: String = body
            .split(';')
            .filter(|d| !d.trim_start().starts_with("background-color"))
            .collect::<Vec<_>>()
            .join(";");
        let scoped: Vec<String> = sel
            .split(',')
            .map(|s| format!("{scope} {}", s.trim()))
            .collect();
        out.push_str(&scoped.join(", "));
        out.push_str(" {");
        out.push_str(&body);
        out.push_str("}\n");
    }
    out
}

pub fn language_label(lang: &str) -> String {
    let l = lang.trim();
    if l.is_empty() {
        return "text".into();
    }
    l.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn highlights_rust_and_ts() {
        let h = highlight("fn main() { let x = 1; }\n", "rust");
        assert!(h.contains("hl-"), "{h}");
        let h = highlight("const x: number = 1;\n", "ts");
        assert!(h.contains("hl-"), "{h}");
        let plain = highlight("<b>", "text");
        assert_eq!(plain, "&lt;b&gt;");
        assert!(theme_css().contains(":root[data-theme=\"dark\"]"));
    }
}

