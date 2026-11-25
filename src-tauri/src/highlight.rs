//! Syntax highlighting with syntect (pure-Rust regex engine). The HTML is
//! flat: each run of code that looks the same gets one span, whose short
//! class names its style in the light and in the dark theme, so the themes
//! are pure CSS.

use std::fmt::Write;
use std::ops::Range;
use std::sync::LazyLock;
use parking_lot::Mutex;
use syntect::highlighting::{Color, FontStyle, StyleModifier, Theme};
use syntect::parsing::{BasicScopeStackOp, ParseState, Scope, ScopeStack, ScopeStackOp, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;
use two_face::theme::EmbeddedThemeName;

use crate::util::{escape_html, ByteCache};

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);
/// Bounded by size, not count: a code block re-highlighted as it streams in
/// leaves many large, near-identical entries. Very large results aren't kept.
static CACHE: LazyLock<Mutex<ByteCache>> = LazyLock::new(|| Mutex::new(ByteCache::new(CACHE_BYTES, CACHE_ENTRY_BYTES)));
const CACHE_BYTES: usize = 24 * 1024 * 1024;
const CACHE_ENTRY_BYTES: usize = 1024 * 1024;
const MAX_HIGHLIGHT_BYTES: usize = 300_000;

/// Light, then dark.
static PALETTES: LazyLock<[Palette; 2]> = LazyLock::new(|| {
    let set = two_face::theme::extra();
    [(EmbeddedThemeName::OneHalfLight, 0), (EmbeddedThemeName::OneHalfDark, 1)].map(|(n, t)| Palette::new(set.get(n), BACKGROUNDS[t]))
});
/// The surfaces code is shown on, light then dark, each the one that gives
/// the least contrast: app.css `--code-bg` (code blocks, attachment preview)
/// in light, `--bg-1` (file viewer) in dark.
const BACKGROUNDS: [Color; 2] = [Color { r: 0xf6, g: 0xf6, b: 0xf4, a: 0xff }, Color { r: 0x1f, g: 0x1f, b: 0x1f, a: 0xff }];
/// WCAG AA for normal text, with a little room for rounding.
const MIN_CONTRAST: f64 = 4.6;

fn luminance(c: Color) -> f64 {
    let lin = |v: u8| {
        let v = f64::from(v) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

fn contrast(a: Color, b: Color) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Tests: the contrast of two `#rrggbb` colours.
#[cfg(test)]
pub(crate) fn contrast_hex(a: &str, b: &str) -> f64 {
    let c = |h: &str| {
        let n = u32::from_str_radix(h.trim_start_matches('#'), 16).unwrap();
        Color { r: (n >> 16) as u8, g: (n >> 8) as u8, b: n as u8, a: 0xff }
    };
    contrast(c(a), c(b))
}

/// `c`, darkened toward black on a light background or lightened toward
/// white on a dark one just enough to be readable: the theme's hue stays.
fn readable(c: Color, bg: Color) -> Color {
    let to = if luminance(bg) > 0.5 { 0.0 } else { 255.0 };
    let mix = |v: u8, t: f64| (f64::from(v) + (to - f64::from(v)) * t).round() as u8;
    (0..=100)
        .map(|i| f64::from(i) / 100.0)
        .map(|t| Color { r: mix(c.r, t), g: mix(c.g, t), b: mix(c.b, t), a: c.a })
        .find(|&x| contrast(x, bg) >= MIN_CONTRAST)
        .unwrap_or(c)
}
const THEME_SCOPES: [&str; 2] = [":root[data-theme=\"light\"]", ":root[data-theme=\"dark\"]"];

/// How a run of code looks in one theme.
type Look = (Color, FontStyle);

/// A theme rule, applied the way the browser applied it to the nested
/// scope spans (a class per atom) this module emitted before, so that code
/// looks the same: a part of the selector matches a scope that has all of
/// its atoms, anywhere. (The theme's own prefix matching would give JS
/// constants or CSS values, for example, other colours.)
struct Rule {
    /// the atoms of each part of the selector, outermost first
    parts: Vec<Vec<u16>>,
    /// `mask` of the last part's atoms: a quick test that rules most out
    last: u64,
    style: StyleModifier,
}

fn mask(atoms: impl Iterator<Item = u16>) -> u64 {
    atoms.fold(0, |m, a| m | 1 << (a % 64))
}

fn atoms(s: Scope) -> impl Iterator<Item = u16> {
    (0..s.len() as usize).map(move |i| s.atom_at(i))
}

impl Rule {
    /// Whether the rule applies to the innermost scope of `stack`, whose
    /// atoms' `mask` is `top_mask`.
    fn matches(&self, stack: &[Scope], top_mask: u64) -> bool {
        let has = |s: Scope, part: &[u16]| part.iter().all(|&a| atoms(s).any(|b| b == a));
        let (Some((last, outer)), Some((&top, mut above))) = (self.parts.split_last(), stack.split_last()) else {
            return false;
        };
        self.last & !top_mask == 0
            && has(top, last)
            && outer.iter().rev().all(|part| match above.iter().rposition(|&s| has(s, part)) {
                Some(i) => {
                    above = &above[..i];
                    true
                }
                None => false,
            })
    }
}

/// A theme: its rules, most specific first, and every look they can give a
/// run of code (each colour with each font style) in a fixed order: slot 0
/// is plain text.
struct Palette {
    rules: Vec<Rule>,
    looks: Vec<Look>,
}

impl Palette {
    fn new(theme: &Theme, bg: Color) -> Palette {
        let mut colors = vec![readable(theme.settings.foreground.unwrap_or(Color::BLACK), bg)];
        let mut fonts = vec![FontStyle::empty()];
        let mut rules = vec![];
        for (order, item) in theme.scopes.iter().enumerate() {
            let style = StyleModifier { foreground: item.style.foreground.map(|c| readable(c, bg)), ..item.style };
            if let Some(c) = style.foreground.filter(|c| !colors.contains(c)) {
                colors.push(c);
            }
            // font styles add up, down the scopes
            for f in fonts.clone() {
                let f = f | item.style.font_style.unwrap_or(FontStyle::empty());
                if !fonts.contains(&f) {
                    fonts.push(f);
                }
            }
            for sel in &item.scope.selectors {
                let parts: Vec<Vec<u16>> = sel.extract_scopes().into_iter().map(|s| atoms(s).collect()).collect();
                let last = mask(parts.last().into_iter().flatten().copied());
                rules.push((parts.iter().map(Vec::len).sum::<usize>(), order, Rule { parts, last, style }));
            }
        }
        // as in CSS: the most specific rule first, then the last one
        rules.sort_by_key(|&(specificity, order, _)| std::cmp::Reverse((specificity, order)));
        Palette {
            rules: rules.into_iter().map(|(.., r)| r).collect(),
            looks: colors.iter().flat_map(|&c| fonts.iter().map(move |&f| (c, f))).collect(),
        }
    }

    /// The look of the innermost scope of `stack`, given its parent's: the
    /// colour of the best rule that has one, else the parent's, and the font
    /// styles of all its rules added to the parent's.
    fn look(&self, stack: &[Scope], parent: Look) -> Look {
        let (mut color, mut font) = (None, parent.1);
        let top_mask = mask(stack.last().into_iter().flat_map(|&s| atoms(s)));
        for rule in self.rules.iter().filter(|r| r.matches(stack, top_mask)) {
            color = color.or(rule.style.foreground);
            font |= rule.style.font_style.unwrap_or(FontStyle::empty());
        }
        (color.unwrap_or(parent.0), font)
    }

    fn slot(&self, look: Look) -> usize {
        self.looks.iter().position(|&x| x == look).unwrap_or(0)
    }
}

/// The class of a run of code: its light and its dark palette slot as two
/// capital letters (the app's own classes are lower case).
fn push_class(out: &mut String, (light, dark): (usize, usize)) {
    out.push(char::from(b'A' + light as u8));
    out.push(char::from(b'A' + dark as u8));
}

/// One line of code as flat spans, one per run of text that looks the same
/// in both themes. `scopes`, and `looks` (each scope's in both themes), go
/// on to the next line. `None`: the scope changes don't fit the stack.
fn line_html(line: &str, ops: &[(usize, ScopeStackOp)], scopes: &mut ScopeStack, looks: &mut Vec<[Look; 2]>) -> Option<String> {
    let plain = PALETTES.each_ref().map(|p| p.looks[0]);
    let mut tokens: Vec<((usize, usize), Range<usize>)> = vec![];
    let mut pos = 0;
    for (end, op) in ops.iter().map(|(i, op)| (*i, op)).chain([(line.len(), &ScopeStackOp::Noop)]) {
        if end > pos {
            let [l, d] = looks.last().copied().unwrap_or(plain);
            tokens.push(((PALETTES[0].slot(l), PALETTES[1].slot(d)), pos..end));
            pos = end;
        }
        scopes
            .apply_with_hook(op, |basic, stack| match basic {
                BasicScopeStackOp::Push(_) => {
                    let parent = looks.last().copied().unwrap_or(plain);
                    looks.push([0, 1].map(|t| PALETTES[t].look(stack, parent[t])));
                }
                BasicScopeStackOp::Pop => {
                    looks.pop();
                }
            })
            .ok()?;
    }
    let mut out = String::with_capacity(line.len() + tokens.len() * 8);
    for run in tokens.chunk_by(|a, b| a.0 == b.0) {
        let text = escape_html(&line[run[0].1.start..run[run.len() - 1].1.end]);
        if run[0].0 == (0, 0) {
            out.push_str(&text);
            continue;
        }
        out.push_str("<span class=\"");
        push_class(&mut out, run[0].0);
        out.push_str("\">");
        out.push_str(&text);
        out.push_str("</span>");
    }
    Some(out)
}

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
/// are parsed. The HTML is made line by line, so it is the same as from
/// the start, but a block streaming in used to be parsed from its start on
/// every update (tens of ms for a long one).
struct Progress {
    lang: String,
    /// the code parsed so far: whole lines
    code: String,
    parse: ParseState,
    /// the scopes open at the end of `code`, and their looks
    scopes: ScopeStack,
    looks: Vec<[Look; 2]>,
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
            looks: vec![],
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
            self.html.push_str(&line_html(line, &ops, &mut self.scopes, &mut self.looks)?);
            self.code.push_str(line);
        }
        Some(tail)
    }

    /// The HTML of the parsed lines, then `tail` (parsed on a copy of the
    /// state: it will be parsed again once its line is complete).
    fn html_with(&self, mut html: String, tail: Option<&str>) -> Option<String> {
        if let Some(line) = tail {
            let ops = self.parse.clone().parse_line(line, &SYNTAXES).ok()?;
            html.push_str(&line_html(line, &ops, &mut self.scopes.clone(), &mut self.looks.clone())?);
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
    if code.len() > MAX_HIGHLIGHT_BYTES {
        return escape_html(code);
    }
    let Some(syntax) = find_syntax(lang) else {
        return escape_html(code);
    };
    let mut p = Progress::new(syntax, lang);
    p.advance(code).and_then(|tail| p.html_with(p.html.clone(), tail)).unwrap_or_else(|| escape_html(code))
}

/// Tests: whether `code` is in the cache.
#[cfg(test)]
pub(crate) fn is_cached(code: &str, lang: &str) -> bool {
    CACHE.lock().get(cache_key(code, lang)).is_some()
}

/// CSS for the highlight classes, scoped to light and dark themes. (The
/// themes' backgrounds are left out: code blocks use our own surfaces.)
pub fn theme_css() -> String {
    let mut out = String::new();
    for (t, (scope, palette)) in THEME_SCOPES.iter().zip(PALETTES.iter()).enumerate() {
        let _ = writeln!(out, "{scope} .hl-code {{ {}}}", declarations(palette.looks[0]));
        for (slot, &look) in palette.looks.iter().enumerate().skip(1) {
            // every class with this slot in this theme, whatever its slot in the other
            let selectors: Vec<String> = (0..PALETTES[1 - t].looks.len())
                .map(|other| {
                    let mut s = format!("{scope} .");
                    push_class(&mut s, if t == 0 { (slot, other) } else { (other, slot) });
                    s
                })
                .collect();
            let _ = writeln!(out, "{} {{ {}}}", selectors.join(", "), declarations(look));
        }
    }
    out
}

fn declarations((c, font): Look) -> String {
    let mut out = format!("color: #{:02x}{:02x}{:02x}; ", c.r, c.g, c.b);
    for (flag, css) in [(FontStyle::BOLD, "font-weight: bold; "), (FontStyle::ITALIC, "font-style: italic; "), (FontStyle::UNDERLINE, "text-decoration: underline; ")] {
        if font.contains(flag) {
            out.push_str(css);
        }
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

    // constructs whose state carries over lines: block comments, multi line
    // strings, template literals, heredocs, nested tags; and scopes the
    // light and the dark theme style differently (quoted keys, headings)
    const SAMPLES: [(&str, &str); 9] = [
        ("rust", "/* a\n * comment */\nfn main() {\n    let s = \"multi\nline\";\n    let r = r#\"raw\n\"#;\n}\n"),
        ("ts", "const t = `tpl ${a +\n b} end`;\n/** doc\n */\nexport class A<T> { x: T }\r\nlet y = 1;\n"),
        ("python", "def f():\n    \"\"\"doc\n    string\"\"\"\n    return {'a': [1,\n 2]}\n"),
        ("bash", "cat <<EOF\nhello $USER\nEOF\nfor i in 1 2; do echo \"$i\"; done\n"),
        ("html", "<div class=\"a\">\n<script>\nlet x = 1 < 2;\n</script>\n<style>\na { color: red }\n</style></div>\n"),
        ("json", "{\n  \"a\": [1, 2,\n  3],\n  \"b\": \"x\"\n}\n"),
        ("plaintext", "not <highlighted>\n"),
        ("yaml", "key: 'value'\nlist:\n  - \"a\" # note\n"),
        ("markdown", "# Title\n\n**bold** and `code` & <tags>\n"),
    ];

    /// The characters of highlighted HTML, each with the class of its span
    /// ("" outside one). Panics unless the spans are flat, closed, and
    /// adjacent spans on a line have different classes.
    fn classed_chars(html: &str) -> Vec<(char, &str)> {
        let mut out = vec![];
        let (mut class, mut closed) = ("", "");
        let mut rest = html;
        while let Some(c) = rest.chars().next() {
            if let Some(r) = rest.strip_prefix("<span class=\"") {
                assert!(class.is_empty(), "nested span in {html}");
                let (c, r) = r.split_once("\">").unwrap();
                assert_ne!(c, closed, "the same class twice in a row in {html}");
                (class, rest) = (c, r);
            } else if let Some(r) = rest.strip_prefix("</span>") {
                assert!(!class.is_empty(), "{html}");
                let line_end = out.last().is_some_and(|&(c, _)| c == '\n');
                (closed, class, rest) = (if line_end { "" } else { class }, "", r);
            } else if c == '&' {
                let (entity, r) = rest.split_once(';').unwrap();
                let c = match entity {
                    "&amp" => '&',
                    "&lt" => '<',
                    "&gt" => '>',
                    "&quot" => '"',
                    "&#39" => '\'',
                    _ => panic!("{entity} in {html}"),
                };
                out.push((c, class));
                (closed, rest) = ("", r);
            } else {
                assert!(c != '<' && c != '>', "{html}");
                out.push((c, class));
                (closed, rest) = ("", &rest[c.len_utf8()..]);
            }
        }
        assert!(class.is_empty(), "a span left open in {html}");
        out
    }

    #[test]
    fn highlights_rust_and_ts() {
        let h = highlight("fn main() { let x = 1; }\n", "rust");
        assert!(h.contains("<span class=\""), "{h}");
        let h = highlight("const x: number = 1;\n", "ts");
        assert!(h.contains("<span class=\""), "{h}");
        let plain = highlight("<b>", "text");
        assert_eq!(plain, "&lt;b&gt;");
        assert!(theme_css().contains(":root[data-theme=\"dark\"]"));
    }

    /// Flat spans with a two-letter class each, adjacent runs that look the
    /// same merged; a token's newline stays in its span.
    #[test]
    fn output_shape() {
        assert_eq!(
            highlight("fn main() {\n    // hi\n    let s = \"a<b\";\n}\n", "rust"),
            "<span class=\"CC\">fn</span> <span class=\"EE\">main</span>() {\n    <span class=\"BB\">// hi\n</span>    \
             <span class=\"CC\">let</span> s <span class=\"CC\">=</span> <span class=\"GG\">&quot;a&lt;b&quot;</span>;\n}\n"
        );
        // a key is a string in the light theme, a keyword in the dark one
        assert_eq!(highlight("{\"a\": 1}\n", "json"), "{<span class=\"GC\">&quot;a&quot;</span>: <span class=\"FF\">1</span>}\n");
        // as the nested spans' CSS showed them: `constant` anywhere in a
        // scope (variable.other.constant, support.constant.color), and a
        // rule for some JS scopes inside others (the parameter)
        assert_eq!(
            highlight("const JOIN_MS = 10;\nfunction f(conv) {}\n", "js"),
            "<span class=\"CC\">const</span> <span class=\"FF\">JOIN_MS</span> <span class=\"CC\">=</span> <span class=\"FF\">10</span>;\n\
             <span class=\"CC\">function</span> <span class=\"EE\">f</span>(<span class=\"DD\">conv</span>) {}\n"
        );
        assert!(highlight("a { color: red; }\n", "css").contains("color: <span class=\"FF\">red</span>;"));
        let [l, d] = &*PALETTES;
        assert!(l.looks.len() <= 26 && d.looks.len() <= 26, "a slot is one letter");
    }

    /// Every colour a token class can have reads at WCAG AA (4.5:1) on each
    /// surface app.css shows code on, in its theme.
    #[test]
    fn token_colours_meet_contrast() {
        let css = include_str!("../../ui/css/app.css");
        let (light, dark) = css.split_once(":root[data-theme=\"dark\"] {").unwrap();
        let token = |block: &str, name: &str| {
            let v = block.split(&format!("{name}: #")).nth(1).unwrap();
            let n = u32::from_str_radix(&v[..6], 16).unwrap();
            Color { r: (n >> 16) as u8, g: (n >> 8) as u8, b: n as u8, a: 0xff }
        };
        for (t, block) in [light, dark].into_iter().enumerate() {
            let surfaces = [token(block, "--code-bg"), token(block, "--bg-1")];
            assert!(surfaces.contains(&BACKGROUNDS[t]), "theme {t}: BACKGROUNDS is not one of app.css's code surfaces");
            for &(c, _) in &PALETTES[t].looks {
                for bg in surfaces {
                    let r = contrast(c, bg);
                    assert!(r >= 4.5, "theme {t}: #{:02x}{:02x}{:02x} on #{:02x}{:02x}{:02x} is {r:.2}:1", c.r, c.g, c.b, bg.r, bg.g, bg.b);
                }
            }
        }
        // colours that already read keep their value
        assert_eq!(readable(Color::BLACK, BACKGROUNDS[0]), Color::BLACK);
    }

    /// Stripped of its tags, the HTML is the code; each character's class
    /// gives the look of its whole scope stack in each theme (worked out
    /// anew at each character), and the theme CSS has a rule for it.
    #[test]
    fn spans_keep_the_text_and_the_looks() {
        let css = theme_css();
        let looks_of = |stack: &ScopeStack| {
            let s = stack.as_slice();
            PALETTES.each_ref().map(|p| (1..=s.len()).fold(p.looks[0], |parent, n| p.look(&s[..n], parent)))
        };
        for (lang, code) in SAMPLES.into_iter().chain([("rust", include_str!("markdown.rs"))]) {
            let html = highlight(code, lang);
            let chars = classed_chars(&html);
            assert_eq!(chars.iter().map(|(c, _)| c).collect::<String>(), code, "{lang}");
            let Some(syntax) = find_syntax(lang) else { continue };
            let (mut parse, mut stack, mut i) = (ParseState::new(syntax), ScopeStack::new(), 0);
            let mut looks = looks_of(&stack);
            for line in LinesWithEndings::from(code) {
                let ops = parse.parse_line(line, &SYNTAXES).unwrap();
                let mut ops = ops.iter().peekable();
                for (at, _) in line.char_indices() {
                    let mut changed = false;
                    while let Some((_, op)) = ops.next_if(|(p, _)| *p <= at) {
                        stack.apply(op).unwrap();
                        changed = true;
                    }
                    if changed {
                        looks = looks_of(&stack);
                    }
                    let class = chars[i].1;
                    for (t, p) in PALETTES.iter().enumerate() {
                        let slot = class.as_bytes().get(t).map_or(0, |b| usize::from(b - b'A'));
                        assert_eq!(slot, p.slot(looks[t]), "{lang}, theme {t}, character {i}");
                        if slot > 0 {
                            assert!(css.contains(&format!("{} .{class}", THEME_SCOPES[t])), "{class}");
                        }
                    }
                    i += 1;
                }
                for (_, op) in ops {
                    stack.apply(op).unwrap();
                }
                looks = looks_of(&stack);
            }
        }
    }

    #[test]
    fn growing_blocks_highlight_as_from_scratch() {
        for (lang, code) in SAMPLES {
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
        let (a, b) = (SAMPLES[0].1, SAMPLES[2].1);
        for end in 1..a.len().min(b.len()) {
            if a.is_char_boundary(end) && b.is_char_boundary(end) {
                assert_eq!(highlight_growing(&a[..end], "rust"), from_scratch(&a[..end], "rust"));
                assert_eq!(highlight_growing(&b[..end], "python"), from_scratch(&b[..end], "python"));
            }
        }
        let edited = "fn main() {\n    let changed = 1;\n";
        assert_eq!(highlight_growing(edited, "rust"), from_scratch(edited, "rust"));
    }

    /// HTML size and time of highlighting real files of this repo:
    /// `cargo test --release -p clauderust --lib bench_highlight -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_highlight() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
        for (file, lang) in [("src-tauri/src/engine.rs", "rust"), ("ui/js/chat.js", "js"), ("ui/css/app.css", "css"), ("dev/mock_api.py", "python")] {
            let code = std::fs::read_to_string(format!("{root}/{file}")).unwrap();
            let html = from_scratch(&code, lang);
            let n = 3;
            let t = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(from_scratch(&code, lang));
            }
            let total = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
            let syntax = find_syntax(lang).unwrap();
            let t = std::time::Instant::now();
            for _ in 0..n {
                let mut parse = ParseState::new(syntax);
                for line in LinesWithEndings::from(&code) {
                    std::hint::black_box(parse.parse_line(line, &SYNTAXES).unwrap());
                }
            }
            let parse = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
            eprintln!(
                "{file}: {} KB -> {} KB of HTML ({:.1}x, {} spans), {total:.1} ms ({parse:.1} ms of it parsing)",
                code.len() / 1024,
                html.len() / 1024,
                html.len() as f64 / code.len() as f64,
                html.matches("<span").count(),
            );
        }
    }
}

