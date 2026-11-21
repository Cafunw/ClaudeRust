//! Digest of large text attachments: logs, crash reports, long pasted
//! output. A file much bigger than the context window can't be sent whole,
//! so the model gets an overview, every distinct error and warning (grouped,
//! with counts and line numbers), the lines around the first occurrences
//! (including stack traces), and the beginning and end of the file.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

/// Log-like text above this size is digested (~100k tokens).
pub const LOG_DIGEST_OVER: usize = 400 * 1024;
/// Other text above this size is digested (~250k tokens).
pub const TEXT_DIGEST_OVER: usize = 1024 * 1024;
/// Target size of a digest.
const BUDGET: usize = 120 * 1024;
/// Characters shown per line.
const MAX_LINE: usize = 400;
/// Bytes of a line examined for its level.
const SCAN: usize = 2000;

static LEVEL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b(TRACE|DEBUG|DBG|INFO|INF|NOTICE|WARN|WARNING|WRN|ERROR|ERR|ERRO|EROR|FATAL|FTL|CRITICAL|CRIT|SEVERE|PANIC)\b|(?i:\blevel)[=:]\s*\x22?([A-Za-z]+)").unwrap()
});
static ERROR_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:fatal|critical|error|err|severe|panic|panicked|traceback|segmentation fault|segfault|core dumped|uncaught|unhandled|exception)\b|\b[A-Z][A-Za-z]*(?:Exception|Error)\b").unwrap()
});
static WARN_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:warn|warning|deprecated)\b").unwrap());
static TS_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?(?:Z|[+-]\d{2}:?\d{2})?|\b[A-Z][a-z]{2} +\d{1,2} \d{2}:\d{2}:\d{2}\b").unwrap()
});
static UUID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b").unwrap());
static HEX_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b0x[0-9a-f]+\b|\b[0-9a-f]{12,}\b").unwrap());
static NUM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\d+").unwrap());
static CONT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?:\s+(?:at |File |from |\.\.\.|#\d|\d+: |\|)|\s{2,}\S|\t|Caused by:|Stack backtrace:|stack backtrace:|During handling of|The above exception)").unwrap()
});

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sev {
    Error,
    Warn,
    Other,
}

fn head(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

/// Severity of one log line: an explicit level near the start decides;
/// otherwise keywords anywhere in the line.
pub fn severity(line: &str) -> Sev {
    if let Some(c) = LEVEL_RE.captures(head(line, 120)) {
        let lvl = c.get(1).or(c.get(2)).map(|m| m.as_str().to_ascii_uppercase()).unwrap_or_default();
        return match lvl.as_str() {
            "WARN" | "WARNING" | "WRN" => Sev::Warn,
            "ERROR" | "ERR" | "ERRO" | "EROR" | "FATAL" | "FTL" | "CRITICAL" | "CRIT" | "SEVERE" | "PANIC" => Sev::Error,
            _ => Sev::Other,
        };
    }
    let l = head(line, SCAN);
    if ERROR_RE.is_match(l) {
        Sev::Error
    } else if WARN_RE.is_match(l) {
        Sev::Warn
    } else {
        Sev::Other
    }
}

/// A line that continues the entry above it (stack frame, indented detail).
fn is_continuation(line: &str) -> bool {
    CONT_RE.is_match(head(line, 200))
}

/// What's left of a line once the parts that vary between occurrences
/// (timestamps, ids, addresses, numbers) are blanked out.
pub fn signature(line: &str) -> String {
    let l = head(line, 600);
    let s = TS_RE.replace_all(l, "");
    let s = UUID_RE.replace_all(&s, "<id>");
    let s = HEX_RE.replace_all(&s, "<hex>");
    let s = NUM_RE.replace_all(&s, "#");
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    head(&s, 240).to_string()
}

/// Whether text reads like a log: by extension, or when many of its first
/// lines carry a timestamp or a level.
pub fn looks_like_log(name: &str, text: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if matches!(ext.as_str(), "log" | "out" | "err" | "crash" | "ips" | "trace" | "stacktrace" | "dmp" | "logcat") {
        return true;
    }
    let sample: Vec<&str> = text.lines().take(200).filter(|l| !l.trim().is_empty()).collect();
    if sample.len() < 5 {
        return false;
    }
    let hits = sample.iter().filter(|l| TS_RE.is_match(head(l, 120)) || LEVEL_RE.is_match(head(l, 120))).count();
    hits * 10 >= sample.len() * 3
}

/// Whether text of this size should be digested rather than sent whole.
pub fn should_digest(name: &str, text: &str) -> bool {
    text.len() > TEXT_DIGEST_OVER || (text.len() > LOG_DIGEST_OVER && looks_like_log(name, text))
}

struct Group {
    sev: Sev,
    count: usize,
    first: usize,
    last: usize,
}

fn show(line: &str) -> String {
    let t = line.trim_end();
    if t.chars().count() > MAX_LINE {
        format!("{}…", t.chars().take(MAX_LINE).collect::<String>())
    } else {
        t.to_string()
    }
}

fn human_size(n: usize) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", n.div_ceil(1024))
    }
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Append `s` to `out` unless that would pass `cap`; returns false when full.
fn push_capped(out: &mut String, s: &str, cap: usize) -> bool {
    if out.len() + s.len() > cap {
        return false;
    }
    out.push_str(s);
    true
}

/// Lines of `text`, with very long ones (minified code, progress bars
/// redrawn with `\r`) cut into pieces so each can be classified and shown.
/// Returns whether anything was cut.
fn pieces(text: &str) -> (Vec<&str>, bool) {
    const LONG: usize = 4000;
    const PIECE: usize = 1000;
    let mut out = Vec::new();
    let mut split = false;
    for l in text.lines() {
        if l.len() <= LONG {
            out.push(l);
            continue;
        }
        split = true;
        let mut rest = l;
        while !rest.is_empty() {
            let cut = head(rest, PIECE).len().max(rest.chars().next().map(char::len_utf8).unwrap_or(1));
            out.push(&rest[..cut]);
            rest = &rest[cut..];
        }
    }
    (out, split)
}

pub struct Digest {
    pub text: String,
    pub lines: usize,
    pub errors: usize,
    pub warnings: usize,
}

/// Build the digest. `path`, when known, is mentioned so an agent can read
/// other parts of the file itself.
pub fn digest(name: &str, text: &str, path: Option<&std::path::Path>) -> Digest {
    let (lines, split) = pieces(text);
    let n = lines.len();
    let mut groups: HashMap<String, Group> = HashMap::new();
    let (mut errors, mut warnings) = (0usize, 0usize);
    let mut first_error: Option<usize> = None;
    let mut last_error: Option<usize> = None;
    for (i, l) in lines.iter().enumerate() {
        if is_continuation(l) {
            continue;
        }
        let sev = severity(l);
        if sev == Sev::Other {
            continue;
        }
        if sev == Sev::Error {
            errors += 1;
            first_error.get_or_insert(i);
            last_error = Some(i);
        } else {
            warnings += 1;
        }
        let sig = signature(l);
        if let Some(g) = groups.get_mut(&sig) {
            g.count += 1;
            g.last = i;
        } else if groups.len() < 20_000 {
            groups.insert(sig, Group { sev, count: 1, first: i, last: i });
        }
    }
    let ts_first = lines.iter().take(5000).find_map(|l| TS_RE.find(head(l, 200)).map(|m| m.as_str().to_string()));
    let ts_last = lines.iter().rev().take(5000).find_map(|l| TS_RE.find(head(l, 200)).map(|m| m.as_str().to_string()));

    let mut out = String::new();
    out.push_str(&format!(
        "[Digest of {name} ({}, {} lines): the file is too large to include in full. Below: an overview, each distinct error and warning with how often it occurs, the lines around the first occurrences (with stack traces), and the beginning and end of the file. Line numbers refer to the original file.{}{}]\n\n",
        human_size(text.len()),
        thousands(n),
        if split { " Very long lines were cut into 1,000-byte pieces; line numbers count those pieces." } else { "" },
        path.map(|p| format!(" The full file is at {}.", p.display())).unwrap_or_default()
    ));
    out.push_str("## Overview\n");
    out.push_str(&format!("- Lines: {} · errors: {} · warnings: {} · distinct messages: {}\n", thousands(n), thousands(errors), thousands(warnings), thousands(groups.len())));
    if let (Some(a), Some(b)) = (&ts_first, &ts_last) {
        out.push_str(&format!("- Time range: {a} → {b}\n"));
    }
    if let Some(f) = first_error {
        out.push_str(&format!("- First error: line {}: {}\n", thousands(f + 1), show(head(lines[f], 300))));
    }
    if let (Some(l), Some(f)) = (last_error, first_error) {
        if l != f {
            out.push_str(&format!("- Last error: line {}: {}\n", thousands(l + 1), show(head(lines[l], 300))));
        }
    }
    out.push('\n');

    let mut sorted: Vec<&Group> = groups.values().collect();
    sorted.sort_by(|a, b| b.count.cmp(&a.count).then(a.first.cmp(&b.first)));
    let list = |sev: Sev, title: &str, cap: usize, max: usize| -> String {
        let mut s = String::new();
        let items: Vec<&&Group> = sorted.iter().filter(|g| g.sev == sev).collect();
        if items.is_empty() {
            return s;
        }
        s.push_str(&format!("## {title} ({} distinct, most frequent first)\n", thousands(items.len())));
        let mut shown = 0;
        for g in items.iter().take(max) {
            let at = if g.count > 1 {
                format!("lines {}…{}", thousands(g.first + 1), thousands(g.last + 1))
            } else {
                format!("line {}", thousands(g.first + 1))
            };
            if !push_capped(&mut s, &format!("- ×{} · {at}: {}\n", thousands(g.count), show(head(lines[g.first], 300))), cap) {
                break;
            }
            shown += 1;
        }
        if items.len() > shown {
            s.push_str(&format!("- … and {} more\n", thousands(items.len() - shown)));
        }
        s.push('\n');
        s
    };
    out.push_str(&list(Sev::Error, "Errors", 24 * 1024, 80));
    out.push_str(&list(Sev::Warn, "Warnings", 8 * 1024, 30));

    // Beginning and end: more of them when there's little else to show.
    let quiet = errors == 0 && warnings == 0;
    let (head_cap, tail_cap) = if quiet { (60 * 1024, 40 * 1024) } else { (6 * 1024, 30 * 1024) };
    // bytes a line takes in the digest (as shown, with its number)
    let cost = |l: &str| show(l).len() + 11;
    let head_end = {
        let mut s = 0usize;
        let mut k = 0;
        while k < n && k < if quiet { 2000 } else { 40 } && s + cost(lines[k]) < head_cap {
            s += cost(lines[k]);
            k += 1;
        }
        k
    };
    let tail_start = {
        let mut s = 0usize;
        let mut k = n;
        while k > head_end && n - k < if quiet { 2000 } else { 150 } && s + cost(lines[k - 1]) < tail_cap {
            s += cost(lines[k - 1]);
            k -= 1;
        }
        k
    };
    let numbered = |from: usize, to: usize| -> String {
        let mut s = String::from("```\n");
        for (i, l) in lines.iter().enumerate().take(to).skip(from) {
            s.push_str(&format!("{:>7} | {}\n", i + 1, show(l)));
        }
        s.push_str("```\n\n");
        s
    };
    let head_txt = if head_end > 0 { format!("## Beginning (lines 1–{})\n{}", thousands(head_end), numbered(0, head_end)) } else { String::new() };
    let tail_txt = if tail_start < n { format!("## End (lines {}–{})\n{}", thousands(tail_start + 1), thousands(n), numbered(tail_start, n)) } else { String::new() };

    // Context around first occurrences: the first error (often the cause),
    // then the most frequent groups, errors before warnings.
    let mut picks: Vec<usize> = Vec::new();
    if let Some(f) = first_error {
        picks.push(f);
    }
    for g in sorted.iter().filter(|g| g.sev == Sev::Error).take(30).chain(sorted.iter().filter(|g| g.sev == Sev::Warn).take(8)) {
        if !picks.contains(&g.first) {
            picks.push(g.first);
        }
    }
    if let Some(l) = last_error {
        if !picks.contains(&l) {
            picks.push(l);
        }
    }
    let ctx_cap = BUDGET.saturating_sub(out.len() + head_txt.len() + tail_txt.len());
    let mut shown = vec![false; n];
    let mut used = 0usize;
    for &i in &picks {
        if i >= tail_start {
            continue; // already in the end
        }
        let mut to = i + 1;
        while to < n && to - i <= 150 && is_continuation(lines[to]) {
            to += 1;
        }
        // the closing "SomeError: message" line of a Python traceback
        if to > i + 1 && to < n && !lines[to].starts_with(char::is_whitespace) && ERROR_RE.is_match(head(lines[to], 200)) {
            to += 1;
        }
        // an error in the beginning still gets the rest of its stack trace
        let from = i.saturating_sub(3).max(head_end);
        let to = (to + 2).min(tail_start).min(n);
        if from >= to {
            continue;
        }
        let added: usize = (from..to).filter(|&k| !shown[k]).map(|k| cost(lines[k])).sum();
        if used + added > ctx_cap {
            continue;
        }
        used += added;
        shown[from..to].iter_mut().for_each(|x| *x = true);
    }
    let mut ctx = String::new();
    let mut k = 0;
    while k < n {
        if !shown[k] {
            k += 1;
            continue;
        }
        let a = k;
        while k < n && shown[k] {
            k += 1;
        }
        if ctx.is_empty() {
            ctx.push_str("## Around the first occurrences\n");
        }
        ctx.push_str(&numbered(a, k));
    }

    out.push_str(&head_txt);
    out.push_str(&ctx);
    out.push_str(&tail_txt);
    Digest { text: out, lines: n, errors, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_log() -> String {
        let mut s = String::new();
        s.push_str("2026-11-20 01:00:00 INFO server starting on :8080\n");
        for i in 0..20_000 {
            s.push_str(&format!("2026-11-20 01:{:02}:{:02} INFO request id={i} path=/api/items took {}ms\n", (i / 60) % 60, i % 60, i % 97));
            if i % 1000 == 500 {
                s.push_str(&format!("2026-11-20 01:{:02}:{:02} ERROR db: connection reset by peer (attempt {})\n", (i / 60) % 60, i % 60, i % 7));
            }
            if i == 7777 {
                s.push_str("2026-11-20 02:09:37 ERROR handler panicked\n");
                s.push_str("Traceback (most recent call last):\n");
                s.push_str("  File \"app.py\", line 42, in handle\n");
                s.push_str("    items = load(user_id)\n");
                s.push_str("ValueError: user 0x7f3a not found\n");
            }
            if i % 4000 == 0 {
                s.push_str(&format!("2026-11-20 01:00:{:02} WARN cache miss ratio high: {}%\n", i % 60, 40 + i % 9));
            }
        }
        s.push_str("2026-11-20 03:12:44 INFO shutting down\n");
        s
    }

    #[test]
    fn levels_and_signatures() {
        assert_eq!(severity("2026-11-20 01:00:00 INFO retrying after error"), Sev::Other);
        assert_eq!(severity("level=error msg=\"boom\""), Sev::Error);
        assert_eq!(severity("[WARN] slow"), Sev::Warn);
        assert_eq!(severity("thread 'main' panicked at src/main.rs:3:5"), Sev::Error);
        assert_eq!(severity("java.lang.NullPointerException: x is null"), Sev::Error);
        assert_eq!(severity("all 3 errors fixed"), Sev::Other);
        assert_eq!(severity("ok"), Sev::Other);
        assert_eq!(
            signature("2026-11-20 01:02:03 ERROR db: reset (attempt 3) id=0xdeadbeef"),
            signature("2026-11-21 11:12:13 ERROR db: reset (attempt 5) id=0x1234")
        );
        assert!(is_continuation("    at com.x.Foo.bar(Foo.java:10)"));
        assert!(is_continuation("  File \"a.py\", line 1"));
        assert!(!is_continuation("2026-11-20 ERROR x"));
    }

    #[test]
    fn digest_of_a_large_log() {
        let log = sample_log();
        assert!(log.len() > LOG_DIGEST_OVER);
        assert!(looks_like_log("server.txt", &log));
        assert!(should_digest("server.txt", &log));
        let d = digest("server.log", &log, None);
        assert!(d.text.len() <= BUDGET + 8 * 1024, "digest is {} bytes", d.text.len());
        assert_eq!(d.lines, log.lines().count());
        // the repeated error is one group with its count
        assert!(d.text.contains("×20 ·"), "{}", &d.text[..2000]);
        assert!(d.text.contains("connection reset by peer"));
        // the one-off panic is shown with its traceback
        assert!(d.text.contains("items = load(user_id)"));
        assert!(d.text.contains("ValueError: user 0x7f3a not found"));
        assert!(d.text.contains("Time range: 2026-11-20 01:00:00 → 2026-11-20 03:12:44"));
        assert!(d.text.contains("## Warnings"));
        // beginning and end of the file
        assert!(d.text.contains("server starting on :8080"));
        assert!(d.text.contains("shutting down"));
        assert_eq!(d.warnings, 5);
    }

    #[test]
    fn early_error_keeps_its_stack_trace() {
        let mut s = String::new();
        for i in 0..20 {
            s.push_str(&format!("2026-11-20 01:00:{i:02} INFO boot step {i}\n"));
        }
        s.push_str("2026-11-20 01:00:20 ERROR Startup failed\n");
        for f in 0..60 {
            s.push_str(&format!("\tat com.acme.Boot.step{f}(Boot.java:{f})\n"));
        }
        s.push_str("Caused by: java.io.IOException: disk full\n");
        for i in 0..30_000 {
            s.push_str(&format!("2026-11-20 01:01:00 INFO heartbeat {i}\n"));
        }
        let d = digest("app.log", &s, None);
        assert!(d.text.contains("Boot.step55"), "trace beyond the beginning section is kept");
        assert!(d.text.contains("Caused by: java.io.IOException: disk full"));
    }

    #[test]
    fn wide_characters_and_long_lines_stay_in_budget() {
        let cjk: String = (0..12_000).map(|i| format!("{i} 日本語のテキスト行です。これは長い行のサンプルで、制限を確認するためのものです。日本語のテキスト行です。これは長い行のサンプル\n")).collect();
        assert!(cjk.len() > TEXT_DIGEST_OVER);
        let d = digest("notes.txt", &cjk, None);
        assert!(d.text.len() <= BUDGET + 8 * 1024, "digest is {} bytes", d.text.len());
        // one 2 MB line with an error in the middle
        let mut one = "x".repeat(1_000_000);
        one.push_str(" ERROR: checksum mismatch ");
        one.push_str(&"y".repeat(1_000_000));
        let d = digest("min.js", &one, None);
        assert!(d.errors >= 1);
        assert!(d.text.contains("checksum mismatch"));
        assert!(d.text.contains("cut into 1,000-byte pieces"));
        assert!(d.text.len() <= BUDGET + 8 * 1024);
    }

    #[test]
    fn quiet_text_keeps_head_and_tail() {
        let text: String = (0..60_000).map(|i| format!("row {i}, some value, another\n")).collect();
        assert!(!looks_like_log("data.csv", &text));
        assert!(should_digest("data.csv", &text));
        let d = digest("data.csv", &text, Some(std::path::Path::new("/tmp/data.csv")));
        assert!(d.text.contains("The full file is at /tmp/data.csv"));
        assert!(d.text.contains("row 0,"));
        assert!(d.text.contains("row 59999,"));
        assert!(d.text.len() <= BUDGET + 8 * 1024);
        assert_eq!(d.errors, 0);
        // small text is left alone
        assert!(!should_digest("a.log", "2026-11-20 01:00:00 INFO x\n"));
    }
}
