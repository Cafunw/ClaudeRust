//! Syntax highlighting with syntect (pure-Rust regex engine), emitting CSS
//! classes so light/dark themes are pure CSS.

use std::sync::LazyLock;
use parking_lot::Mutex;
use syntect::html::{css_for_theme_with_class_style, line_tokens_to_classed_spans, ClassStyle};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::util::{escape_html, ByteCache};

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);
/// Bounded by size, not count: a code block re-highlighted as it streams in
/// leaves many large, near-identical entries. Very large results aren't kept.
static CACHE: LazyLock<Mutex<ByteCache>> = LazyLock::new(|| Mutex::new(ByteCache::new(CACHE_BYTES, CACHE_ENTRY_BYTES)));
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

fn cache_key(code: &str, lang: &str) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write_usize(lang.len());
    h.write(lang.as_bytes());
    h.write(code.as_bytes());
    h.finish()
}

/// How far highlighting a code block got: the parser state after its
/// complete lines, so that once the block has grown only the new lines
/// are parsed. The HTML is the same as from `ClassedHTMLGenerator` (this is
/// what it does, line by line), but a block streaming in used to be parsed
/// from its start on every update (tens of ms for a long one).
struct Progress {
    lang: String,
    /// the code parsed so far: whole lines
    code: String,
    parse: ParseState,
    scopes: ScopeStack,
    open_spans: isize,
    html: String,
}

/// Code blocks still streaming in, most recent last: a few, for replies
/// streaming at the same time. Each holds up to `MAX_HIGHLIGHT_BYTES` of
/// code and `PROGRESS_HTML_BYTES` of HTML.
static GROWING: LazyLock<Mutex<Vec<Progress>>> = LazyLock::new(|| Mutex::new(Vec::new()));
const GROWING_BLOCKS: usize = 4;
const PROGRESS_HTML_BYTES: usize = 1024 * 1024;

impl Progress {
    fn new(syntax: &SyntaxReference, lang: &str) -> Progress {
        Progress {
            lang: lang.to_string(),
            code: String::new(),
            parse: ParseState::new(syntax),
            scopes: ScopeStack::new(),
            open_spans: 0,
            html: String::new(),
        }
    }

    /// The saved progress `code` (a longer version of the same block) can
    /// go on from, taken out of the list while it's in use.
    fn take(code: &str, lang: &str) -> Option<Progress> {
        let mut saved = GROWING.lock();
        let i = (0..saved.len())
            .filter(|&i| saved[i].lang == lang && code.starts_with(saved[i].code.as_str()))
            .max_by_key(|&i| saved[i].code.len())?;
        Some(saved.remove(i))
    }

    fn keep(self) {
        if self.code.is_empty() || self.html.len() > PROGRESS_HTML_BYTES {
            return;
        }
        let mut saved = GROWING.lock();
        saved.push(self);
        if saved.len() > GROWING_BLOCKS {
            saved.remove(0);
        }
    }

    /// Parse the whole lines of `code` past the ones parsed already; returns
    /// its last line when that has no newline yet. `None`: a parse error.
    fn advance<'c>(&mut self, code: &'c str) -> Option<Option<&'c str>> {
        let mut tail = None;
        for line in LinesWithEndings::from(&code[self.code.len()..]) {
            if !line.ends_with('\n') {
                tail = Some(line);
                break;
            }
            let ops = self.parse.parse_line(line, &SYNTAXES).ok()?;
            let (h, delta) = line_tokens_to_classed_spans(line, &ops, STYLE, &mut self.scopes).ok()?;
            self.open_spans += delta;
            self.html.push_str(&h);
            self.code.push_str(line);
        }
        Some(tail)
    }

    /// The HTML of the parsed lines, then `tail` (parsed on a copy of the
    /// state: it will be parsed again once its line is complete), with the
    /// spans still open closed.
    fn html_with(&self, mut html: String, tail: Option<&str>) -> Option<String> {
        let mut open = self.open_spans;
        if let Some(line) = tail {
            let mut parse = self.parse.clone();
            let mut scopes = self.scopes.clone();
            let ops = parse.parse_line(line, &SYNTAXES).ok()?;
            let (h, delta) = line_tokens_to_classed_spans(line, &ops, STYLE, &mut scopes).ok()?;
            open += delta;
            html.push_str(&h);
        }
        for _ in 0..open {
            html.push_str("</span>");
        }
        Some(html)
    }
}

/// Highlight `code` and return the inner HTML for a `<code>` element.
pub fn highlight(code: &str, lang: &str) -> String {
    if code.len() > MAX_HIGHLIGHT_BYTES {
        return escape_html(code);
    }
    let Some(syntax) = find_syntax(lang) else {
        return escape_html(code);
    };
    let key = cache_key(code, lang);
    if let Some(h) = CACHE.lock().get(key) {
        return h.clone();
    }
    // a block that streamed in and is now complete goes on from where it got
    let mut p = Progress::take(code, lang).unwrap_or_else(|| Progress::new(syntax, lang));
    let Some(tail) = p.advance(code) else {
        return escape_html(code);
    };
    let html = std::mem::take(&mut p.html);
    let Some(html) = p.html_with(html, tail) else {
        return escape_html(code);
    };
    CACHE.lock().insert(key, html.clone());
    html
}

/// Highlight a code block that is most likely still streaming in (it has
/// no closing fence yet): the same HTML as `highlight`, but kept out of the
/// cache (the next update has more code), and from where the previous
/// version of the block got to.
pub fn highlight_growing(code: &str, lang: &str) -> String {
    if code.len() > MAX_HIGHLIGHT_BYTES {
        return escape_html(code);
    }
    let Some(syntax) = find_syntax(lang) else {
        return escape_html(code);
    };
    let mut p = Progress::take(code, lang).unwrap_or_else(|| Progress::new(syntax, lang));
    let html = p.advance(code).and_then(|tail| p.html_with(p.html.clone(), tail));
    match html {
        Some(h) => {
            p.keep();
            h
        }
        None => escape_html(code),
    }
}

/// Tests: how a block was highlighted before (from its start, every time,
/// no cache), the reference `highlight` and `highlight_growing` must match.
#[cfg(test)]
pub(crate) fn from_scratch(code: &str, lang: &str) -> String {
    use syntect::html::ClassedHTMLGenerator;
    if code.len() > MAX_HIGHLIGHT_BYTES {
        return escape_html(code);
    }
    let Some(syntax) = find_syntax(lang) else {
        return escape_html(code);
    };
    let mut gen = ClassedHTMLGenerator::new_with_class_style(syntax, &SYNTAXES, STYLE);
    for line in LinesWithEndings::from(code) {
        if gen.parse_html_for_line_which_includes_newline(line).is_err() {
            return escape_html(code);
        }
    }
    gen.finalize()
}

/// Tests: whether `code` is in the cache.
#[cfg(test)]
pub(crate) fn is_cached(code: &str, lang: &str) -> bool {
    CACHE.lock().get(cache_key(code, lang)).is_some()
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

    #[test]
    fn growing_blocks_highlight_as_from_scratch() {
        // constructs whose state carries over lines: block comments, multi
        // line strings, template literals, heredocs, nested tags
        let samples = [
            ("rust", "/* a\n * comment */\nfn main() {\n    let s = \"multi\nline\";\n    let r = r#\"raw\n\"#;\n}\n"),
            ("ts", "const t = `tpl ${a +\n b} end`;\n/** doc\n */\nexport class A<T> { x: T }\r\nlet y = 1;\n"),
            ("python", "def f():\n    \"\"\"doc\n    string\"\"\"\n    return {'a': [1,\n 2]}\n"),
            ("bash", "cat <<EOF\nhello $USER\nEOF\nfor i in 1 2; do echo \"$i\"; done\n"),
            ("html", "<div class=\"a\">\n<script>\nlet x = 1 < 2;\n</script>\n<style>\na { color: red }\n</style></div>\n"),
            ("json", "{\n  \"a\": [1, 2,\n  3],\n  \"b\": \"x\"\n}\n"),
            ("plaintext", "not <highlighted>\n"),
        ];
        for (lang, code) in samples {
            for step in [1, 3, 7, 40] {
                let mut end = 0;
                while end < code.len() {
                    end = (end + step).min(code.len());
                    while !code.is_char_boundary(end) {
                        end += 1;
                    }
                    let part = &code[..end];
                    assert_eq!(highlight_growing(part, lang), from_scratch(part, lang), "{lang}, {end} bytes, step {step}");
                    // kept out of the cache: the next update has more code
                    assert!(CACHE.lock().get(cache_key(part, lang)).is_none() || part == code);
                }
                // then the block is closed
                assert_eq!(highlight(code, lang), from_scratch(code, lang), "{lang} complete");
            }
        }
        // two blocks growing at once (two replies streaming), and one that
        // was edited rather than extended
        let (a, b) = (samples[0].1, samples[2].1);
        for end in 1..a.len().min(b.len()) {
            if a.is_char_boundary(end) && b.is_char_boundary(end) {
                assert_eq!(highlight_growing(&a[..end], "rust"), from_scratch(&a[..end], "rust"));
                assert_eq!(highlight_growing(&b[..end], "python"), from_scratch(&b[..end], "python"));
            }
        }
        let edited = "fn main() {\n    let changed = 1;\n";
        assert_eq!(highlight_growing(edited, "rust"), from_scratch(edited, "rust"));
    }
}

