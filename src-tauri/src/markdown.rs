//! Markdown -> safe HTML. Raw HTML in model output is shown as text, links
//! are restricted to safe schemes, code blocks are syntax highlighted and
//! math is emitted as placeholders that the UI renders with KaTeX.

use crate::highlight;
use crate::util::{escape_html, hash64};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use pulldown_cmark::{html, CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use std::collections::HashMap;

static URL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"https?://[^\s<>"'`\]\[]+"#).unwrap());
static CACHE: Lazy<Mutex<HashMap<u64, String>>> = Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Copy)]
pub struct Opts {
    pub math: bool,
    /// Render single newlines as line breaks, like Claude's chat bubbles
    /// (poems, addresses, step lists without markup).
    pub breaks: bool,
}

pub fn safe_url(url: &str) -> Option<String> {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || lower.starts_with('#')
    {
        Some(u.to_string())
    } else if !lower.contains(':') && !u.is_empty() {
        // relative links are meaningless in the app; render as plain anchors
        None
    } else {
        None
    }
}

/// Convert `\( … \)` and `\[ … \]` to `$…$` / `$$…$$` outside of code.
fn normalize_math_delims(md: &str) -> String {
    if !md.contains("\\(") && !md.contains("\\[") {
        return md.to_string();
    }
    let mut out = String::with_capacity(md.len());
    let mut in_fence = false;
    for line in md.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        let mut in_code = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '`' {
                in_code = !in_code;
                out.push(c);
                continue;
            }
            if c == '\\' && !in_code {
                match chars.peek() {
                    Some('(') | Some(')') => {
                        chars.next();
                        out.push('$');
                        continue;
                    }
                    Some('[') | Some(']') => {
                        chars.next();
                        out.push_str("$$");
                        continue;
                    }
                    _ => {}
                }
            }
            out.push(c);
        }
    }
    out
}

fn code_block_html(lang: &str, code: &str) -> String {
    let lang_clean = lang.split_whitespace().next().unwrap_or("").to_string();
    let label = highlight::language_label(&lang_clean);
    let body = highlight::highlight(code, &lang_clean);
    format!(
        "<div class=\"code-block\" data-lang=\"{l}\"><div class=\"code-header\"><span class=\"code-lang\">{label}</span><button class=\"code-copy\" type=\"button\" data-action=\"copy-code\"></button></div><pre><code class=\"hl-code\">{body}</code></pre></div>",
        l = escape_html(&lang_clean),
        label = escape_html(&label)
    )
}

fn linkify(text: &str) -> Vec<Event<'static>> {
    let mut out = vec![];
    let mut last = 0;
    for m in URL_RE.find_iter(text) {
        let mut url = m.as_str();
        // trim trailing punctuation and unbalanced closing parens
        loop {
            let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '*', '_']);
            let trimmed = if trimmed.ends_with(')') && trimmed.matches('(').count() < trimmed.matches(')').count() {
                &trimmed[..trimmed.len() - 1]
            } else {
                trimmed
            };
            if trimmed.len() == url.len() {
                break;
            }
            url = trimmed;
        }
        let start = m.start();
        let end = start + url.len();
        if start > last {
            out.push(Event::Text(CowStr::from(text[last..start].to_string())));
        }
        out.push(Event::Html(CowStr::from(format!(
            "<a href=\"{u}\">{t}</a>",
            u = escape_html(url),
            t = escape_html(url)
        ))));
        last = end;
    }
    if last < text.len() {
        out.push(Event::Text(CowStr::from(text[last..].to_string())));
    }
    out
}

/// Render markdown to HTML (cached by content hash).
pub fn render(md: &str, opts: Opts) -> String {
    let key = hash64(md) ^ (opts.math as u64) ^ ((opts.breaks as u64) << 1);
    if let Some(h) = CACHE.lock().get(&key) {
        return h.clone();
    }
    let html = render_uncached(md, opts);
    let mut c = CACHE.lock();
    if c.len() > 1500 {
        c.clear();
    }
    c.insert(key, html.clone());
    html
}

fn render_uncached(md: &str, opts: Opts) -> String {
    let src = if opts.math { normalize_math_delims(md) } else { md.to_string() };
    let mut options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM;
    if opts.math {
        options |= Options::ENABLE_MATH;
    }
    let parser = Parser::new_ext(&src, options);

    let mut events: Vec<Event> = Vec::new();
    let mut code: Option<(String, String)> = None; // (lang, buffer)
    let mut link_depth = 0usize;
    let mut skip_link_end: Vec<bool> = vec![];

    for ev in parser {
        if let Some((_, buf)) = code.as_mut() {
            match ev {
                Event::Text(t) => {
                    buf.push_str(&t);
                    continue;
                }
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, buf) = code.take().unwrap();
                    events.push(Event::Html(CowStr::from(code_block_html(&lang, &buf))));
                    continue;
                }
                _ => continue,
            }
        }
        match ev {
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::Html(s) | Event::InlineHtml(s) => {
                events.push(Event::Text(s));
            }
            Event::Start(Tag::HtmlBlock) => {
                events.push(Event::Html(CowStr::from("<p class=\"raw-html\">")));
            }
            Event::End(TagEnd::HtmlBlock) => {
                events.push(Event::Html(CowStr::from("</p>")));
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => match safe_url(&dest_url) {
                Some(u) => {
                    link_depth += 1;
                    skip_link_end.push(false);
                    events.push(Event::Start(Tag::Link {
                        link_type,
                        dest_url: CowStr::from(u),
                        title,
                        id,
                    }));
                }
                None => {
                    skip_link_end.push(true);
                }
            },
            Event::End(TagEnd::Link) => {
                if skip_link_end.pop() == Some(false) {
                    link_depth = link_depth.saturating_sub(1);
                    events.push(Event::End(TagEnd::Link));
                }
            }
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => match safe_url(&dest_url) {
                Some(u) => {
                    link_depth += 1;
                    skip_link_end.push(false);
                    events.push(Event::Start(Tag::Link {
                        link_type,
                        dest_url: CowStr::from(u),
                        title,
                        id,
                    }));
                }
                None => skip_link_end.push(true),
            },
            Event::End(TagEnd::Image) => {
                if skip_link_end.pop() == Some(false) {
                    link_depth = link_depth.saturating_sub(1);
                    events.push(Event::End(TagEnd::Link));
                }
            }
            Event::Start(Tag::Table(a)) => {
                events.push(Event::Html(CowStr::from("<div class=\"table-wrap\">")));
                events.push(Event::Start(Tag::Table(a)));
            }
            Event::End(TagEnd::Table) => {
                events.push(Event::End(TagEnd::Table));
                events.push(Event::Html(CowStr::from("</div>")));
            }
            Event::InlineMath(t) => {
                let e = escape_html(&t);
                events.push(Event::Html(CowStr::from(format!(
                    "<span class=\"math math-inline\" data-tex=\"{e}\">{e}</span>"
                ))));
            }
            Event::DisplayMath(t) => {
                let e = escape_html(&t);
                events.push(Event::Html(CowStr::from(format!(
                    "<span class=\"math math-display\" data-tex=\"{e}\">{e}</span>"
                ))));
            }
            Event::SoftBreak if opts.breaks => events.push(Event::HardBreak),
            other => events.push(other),
        }
    }
    // Unterminated code block (still streaming)
    if let Some((lang, buf)) = code.take() {
        events.push(Event::Html(CowStr::from(code_block_html(&lang, &buf))));
    }
    // Second pass: merge adjacent text events and linkify bare URLs.
    let mut final_events: Vec<Event> = Vec::with_capacity(events.len());
    let mut text_buf = String::new();
    let mut depth = 0usize;
    let flush = |buf: &mut String, depth: usize, out: &mut Vec<Event>| {
        if buf.is_empty() {
            return;
        }
        let t = std::mem::take(buf);
        if depth == 0 && (t.contains("http://") || t.contains("https://")) {
            out.extend(linkify(&t));
        } else {
            out.push(Event::Text(CowStr::from(t)));
        }
    };
    for ev in events {
        match ev {
            Event::Text(t) => text_buf.push_str(&t),
            other => {
                flush(&mut text_buf, depth, &mut final_events);
                match &other {
                    Event::Start(Tag::Link { .. }) => depth += 1,
                    Event::End(TagEnd::Link) => depth = depth.saturating_sub(1),
                    _ => {}
                }
                final_events.push(other);
            }
        }
    }
    flush(&mut text_buf, depth, &mut final_events);
    let _ = link_depth;
    let mut out = String::with_capacity(src.len() * 3 / 2);
    html::push_html(&mut out, final_events.into_iter());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    const O: Opts = Opts { math: true, breaks: false };

    #[test]
    fn escapes_raw_html_and_bad_links() {
        let h = render("<script>alert(1)</script>\n\n[x](javascript:alert(1)) <img src=x onerror=y>", O);
        assert!(!h.contains("<script>"), "{h}");
        assert!(!h.contains("javascript:"), "{h}");
        assert!(!h.contains("<img"), "{h}");
    }

    #[test]
    fn code_and_math_and_links() {
        let h = render("Hi https://example.com/a_(b). and $x^2$\n\n```rust\nfn a() {}\n```\n", O);
        assert!(h.contains("href=\"https://example.com/a_(b)\""), "{h}");
        assert!(h.contains("data-tex=\"x^2\""), "{h}");
        assert!(h.contains("code-block"), "{h}");
        let h = render("\\[ a+b \\]", O);
        assert!(h.contains("math-display"), "{h}");
    }

    #[test]
    fn streaming_unclosed_fence() {
        let h = render("```py\nprint(1)", O);
        assert!(h.contains("code-block"));
    }

    #[test]
    fn soft_breaks_only_in_chat() {
        let chat = render("Waves fold\nsalt wind\n\nnext", Opts { math: false, breaks: true });
        assert!(chat.contains("Waves fold<br />"), "{chat}");
        let doc = render("Waves fold\nsalt wind", O);
        assert!(!doc.contains("<br"), "{doc}");
    }

    #[test]
    fn tables() {
        let h = render("|a|b|\n|-|-|\n|1|2|\n", O);
        assert!(h.contains("table-wrap") && h.contains("<table>"));
    }
}
