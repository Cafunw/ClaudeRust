//! Text of Excel workbooks (.xlsx: each sheet as CSV), PowerPoint
//! presentations (.pptx: the text of each slide), OpenDocument texts (.odt),
//! EPUB books (their chapters in reading order) and RTF documents. The zips
//! come from anywhere: the entry count, the bytes read of each part and the
//! text made are bounded, entry names are only looked up in the same zip,
//! nothing nested is opened, and no entity but XML's own is expanded.

use crate::attachments::refuse;
use crate::error::AppResult;
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Read;

const MAX_ENTRIES: usize = 10_000;
/// The most read of one part (a sheet, a slide): a larger one is cut there.
const MAX_PART_BYTES: u64 = 64 * 1024 * 1024;
/// The most read of all parts together: a workbook can list one part many times.
const MAX_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;
/// More shared strings than this are left out (their cells show empty): a
/// small part of `<si/>` would otherwise make millions of them.
const MAX_STRINGS: usize = 1 << 20;
/// Under the size where text is digested like a log, with room for the
/// note on what was left out.
const MAX_TEXT: usize = crate::logdigest::TEXT_DIGEST_OVER - 1024;
const MAX_ROWS: usize = 10_000;
const MAX_COLS: usize = 200;

/// The sheets of a workbook as CSV under their names.
pub fn xlsx_text(bytes: &[u8]) -> AppResult<String> {
    let mut p = Package::open(bytes, "Excel workbook", MAX_PART_BYTES)?;
    let strings = p.shared_strings()?;
    let mut out = Out::default();
    for (name, path) in p.parts("xl/workbook.xml", "sheet")? {
        let Some((xml, cut)) = p.part(&path)? else { continue };
        if !out.line(&format!("## Sheet: {name}")) {
            break;
        }
        sheet_csv(&xml, &strings, &mut out);
        if cut {
            out.line(&format!("[Only the first {} MB of this sheet were read.]", MAX_PART_BYTES >> 20));
        }
        out.line("");
    }
    out.finish(p.what, "sheets")
}

/// The text of each slide, a paragraph a line.
pub fn pptx_text(bytes: &[u8]) -> AppResult<String> {
    let mut p = Package::open(bytes, "PowerPoint presentation", MAX_PART_BYTES)?;
    let mut out = Out::default();
    for (n, (_, path)) in p.parts("ppt/presentation.xml", "sldId")?.into_iter().enumerate() {
        let Some((xml, _)) = p.part(&path)? else { continue };
        if !out.line(&format!("## Slide {}", n + 1)) {
            break;
        }
        let (mut lines, mut para, mut in_t) = (vec![], String::new(), false);
        walk(&xml, |t| match t {
            Tok::Open("t", _) => in_t = true,
            Tok::Close("t") => in_t = false,
            Tok::Text(s) if in_t => para.push_str(&unescape(s)),
            Tok::Open("br", _) => para.push('\n'),
            Tok::Close("p") => {
                if !para.trim().is_empty() {
                    lines.push(para.trim().to_string());
                }
                para.clear();
            }
            _ => {}
        });
        for l in lines {
            if !out.line(&l) {
                break;
            }
        }
        out.line("");
    }
    out.finish(p.what, "slides")
}

/// The most spaces one `<text:s text:c="n"/>` stands for: with more, a small
/// file could make text far larger than itself.
const MAX_SPACES: usize = 16;

/// The text of an OpenDocument text (content.xml): a paragraph or heading
/// a line, a table row's cells tab-separated; comments and the deleted text
/// of tracked changes are left out.
pub fn odt_text(bytes: &[u8]) -> AppResult<String> {
    let mut p = Package::open(bytes, "OpenDocument text", MAX_PART_BYTES)?;
    let (xml, cut) = p.part("content.xml")?.ok_or_else(|| refuse("office_invalid", "Not a valid OpenDocument text: content.xml is missing.".into()))?;
    let mut out = Out::default();
    let (mut para, mut row) = (String::new(), Vec::<String>::new());
    let (mut body, mut skip, mut cell, mut stop) = (false, 0usize, 0usize, false);
    walk(&xml, |t| {
        if stop {
            return;
        }
        match t {
            Tok::Open("body", _) => body = true,
            Tok::Close("body") => body = false,
            Tok::Open("annotation" | "tracked-changes", _) => skip += 1,
            Tok::Close("annotation" | "tracked-changes") => skip = skip.saturating_sub(1),
            _ if !body || skip > 0 => {}
            Tok::Text(s) => collapse_into(&mut para, &unescape(s)),
            Tok::Open("s", a) => {
                let n = attr(a, |k| k.ends_with(":c")).and_then(|c| c.trim().parse().ok()).unwrap_or(1usize);
                para.extend(std::iter::repeat_n(' ', n.min(MAX_SPACES)));
            }
            Tok::Open("tab", _) => para.push('\t'),
            Tok::Open("line-break", _) => para.push('\n'),
            Tok::Open("table-cell" | "covered-table-cell", _) => cell += 1,
            Tok::Close("p" | "h") if cell > 0 => para.push(' '),
            Tok::Close("p" | "h") => {
                stop = !out.line(para.trim_end());
                para.clear();
            }
            Tok::Close("table-cell" | "covered-table-cell") => {
                cell = cell.saturating_sub(1);
                row.push(para.trim().to_string());
                para.clear();
            }
            Tok::Close("table-row") => {
                while row.last().is_some_and(String::is_empty) {
                    row.pop();
                }
                stop = !out.line(&row.join("\t"));
                row.clear();
            }
            _ => {}
        }
    });
    if cut {
        out.line(&format!("[Only the first {} MB of this document were read.]", MAX_PART_BYTES >> 20));
    }
    out.finish(p.what, "text")
}

/// The text of an EPUB book: the XHTML documents its spine lists, in that
/// order, a block (paragraph, heading, list item, table row) a line.
pub fn epub_text(bytes: &[u8]) -> AppResult<String> {
    let mut p = Package::open(bytes, "EPUB book", MAX_PART_BYTES)?;
    let invalid = |why: &str| refuse("office_invalid", format!("Not a valid EPUB book: {why}."));
    let (container, _) = p.part("META-INF/container.xml")?.ok_or_else(|| invalid("META-INF/container.xml is missing"))?;
    let mut opf = None;
    walk(&container, |t| {
        if let Tok::Open("rootfile", a) = t {
            opf = opf.take().or_else(|| attr(a, |k| k == "full-path").map(|v| unescape(v).into_owned()));
        }
    });
    let opf = opf.ok_or_else(|| invalid("its package document isn't named"))?;
    let (package, _) = p.part(&opf)?.ok_or_else(|| invalid("its package document is missing"))?;
    let (mut items, mut spine) = (HashMap::new(), vec![]);
    walk(&package, |t| match t {
        Tok::Open("item", a) => {
            if let (Some(id), Some(href)) = (attr(a, |k| k == "id"), attr(a, |k| k == "href")) {
                items.insert(id.to_string(), unescape(href).into_owned());
            }
        }
        Tok::Open("itemref", a) if spine.len() < MAX_ENTRIES => spine.extend(attr(a, |k| k == "idref").map(str::to_string)),
        _ => {}
    });
    let dir = opf.rsplit_once('/').map_or("", |(d, _)| d);
    let mut out = Out::default();
    for id in spine {
        let Some(href) = items.get(&id) else { continue };
        let Some((xhtml, cut)) = p.part(&zip_path(dir, href))? else { continue };
        if !xhtml_text(&xhtml, &mut out) || (cut && !out.line(&format!("[Only the first {} MB of this chapter were read.]", MAX_PART_BYTES >> 20))) {
            break;
        }
    }
    out.finish(p.what, "chapters")
}

/// Elements that start and end a line of their own.
const BLOCKS: &[&str] = &[
    "p", "div", "h1", "h2", "h3", "h4", "h5", "h6", "li", "dt", "dd", "tr", "table", "ul", "ol", "blockquote", "pre", "section", "article",
    "header", "footer", "aside", "nav", "figure", "figcaption", "hr", "body",
];

/// Adds the text of an XHTML document to `out`; false once `out` is full.
fn xhtml_text(xml: &str, out: &mut Out) -> bool {
    let (mut para, mut skip, mut pre, mut ok) = (String::new(), 0usize, 0usize, true);
    let flush = |para: &mut String, out: &mut Out| {
        let line = para.trim();
        let added = line.is_empty() || out.line(line);
        para.clear();
        added
    };
    walk(xml, |t| {
        if !ok {
            return;
        }
        let (name, open) = match t {
            Tok::Open(n, _) => (n, true),
            Tok::Close(n) => (n, false),
            Tok::Text(_) if skip > 0 => return,
            Tok::Text(s) if pre > 0 => return para.push_str(&unescape(s)),
            Tok::Text(s) => return collapse_into(&mut para, &unescape(s)),
        };
        match name {
            "head" | "script" | "style" => skip = if open { skip + 1 } else { skip.saturating_sub(1) },
            _ if skip > 0 => {}
            "br" if open => para.push('\n'),
            "td" | "th" if !open => para.push('\t'),
            _ if BLOCKS.contains(&name) => {
                if name == "pre" {
                    pre = if open { pre + 1 } else { pre.saturating_sub(1) };
                }
                ok = flush(&mut para, &mut *out);
            }
            _ => {}
        }
    });
    ok && flush(&mut para, out)
}

/// Text added as HTML and ODF show it: a run of whitespace is one space, none at a line's start.
fn collapse_into(para: &mut String, s: &str) {
    for c in s.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\r') {
            if !para.is_empty() && !para.ends_with([' ', '\t', '\n']) {
                para.push(' ');
            }
        } else {
            para.push(c);
        }
    }
}

/// The entry `href` (percent-encoded, maybe with a #fragment) names,
/// relative to the zip directory `dir`; `..` never leaves the zip's root.
fn zip_path(dir: &str, href: &str) -> String {
    let href = percent_decode(href.split('#').next().unwrap_or(""));
    let mut parts: Vec<&str> = if href.starts_with('/') { vec![] } else { dir.split('/').filter(|s| !s.is_empty()).collect() };
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    let (b, mut out, mut i) = (s.as_bytes(), Vec::with_capacity(s.len()), 0);
    while i < b.len() {
        match s.get(i + 1..i + 3).filter(|_| b[i] == b'%').and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(v) => {
                out.push(v);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Groups nested deeper than this keep the state of the one at this depth
/// (a file of braces would otherwise make a stack as large as itself).
const MAX_RTF_DEPTH: usize = 1024;

/// Destinations whose text isn't the document's: tables, metadata,
/// headers and footers, pictures and embedded objects, list numbering.
const RTF_SKIPPED: &[&[u8]] = &[
    b"fonttbl", b"colortbl", b"stylesheet", b"info", b"pict", b"object", b"objdata", b"header", b"headerl", b"headerr", b"headerf",
    b"footer", b"footerl", b"footerr", b"footerf", b"listtable", b"listoverridetable", b"revtbl", b"filetbl", b"listtext", b"pntext",
    b"pntxta", b"pntxtb", b"ftnsep", b"ftnsepc", b"aftnsep", b"aftnsepc", b"fldinst", b"themedata", b"datastore", b"latentstyles",
];

/// Text made from an RTF document so far.
struct Rtf {
    out: String,
    /// bytes in the document's code page not decoded yet (a double-byte
    /// character comes as two `\'hh`)
    bytes: Vec<u8>,
    code_page: &'static encoding_rs::Encoding,
    /// the high half of a `\uN` surrogate pair
    high: Option<u32>,
}

impl Rtf {
    fn flush(&mut self) {
        if !self.bytes.is_empty() {
            self.out.push_str(&self.code_page.decode_without_bom_handling(&self.bytes).0);
            self.bytes.clear();
        }
    }

    fn push(&mut self, c: char) {
        self.flush();
        if self.high.take().is_some() {
            self.out.push(char::REPLACEMENT_CHARACTER);
        }
        self.out.push(c);
    }

    /// The UTF-16 unit `\uN` writes (negative above 32767).
    fn unit(&mut self, n: i64) {
        let u = (if n < 0 { n + 65536 } else { n }) as u32;
        match (self.high, u) {
            (_, 0xD800..=0xDBFF) => {
                self.flush();
                self.high = Some(u);
            }
            (Some(h), 0xDC00..=0xDFFF) => {
                self.high = None;
                self.push(char::from_u32(0x10000 + ((h - 0xD800) << 10) + (u - 0xDC00)).unwrap_or(char::REPLACEMENT_CHARACTER));
            }
            _ => self.push(char::from_u32(u).unwrap_or(char::REPLACEMENT_CHARACTER)),
        }
    }
}

/// The Windows code page `\ansicpgN` names.
fn code_page(n: i64) -> Option<&'static encoding_rs::Encoding> {
    let label = match n {
        874 | 1250..=1258 => format!("windows-{n}"),
        932 => "shift_jis".into(),
        936 => "gbk".into(),
        949 => "euc-kr".into(),
        950 => "big5".into(),
        10000 => "macintosh".into(),
        65001 => "utf-8".into(),
        _ => return None,
    };
    encoding_rs::Encoding::for_label(label.as_bytes())
}

/// The plain text of an RTF document: control words and symbols, `\uN`
/// and `\'hh` (in the `\ansicpg` code page) are read; the destinations in
/// `RTF_SKIPPED`, `\*` destinations and `\bin` data are skipped.
// shortcut: `\'hh` is read in the document's code page, not in its font's `\fcharset`; follow the font if mixed-script files come out wrong.
pub fn rtf_text(bytes: &[u8]) -> AppResult<String> {
    let b = bytes;
    let mut r = Rtf { out: String::new(), bytes: vec![], code_page: encoding_rs::WINDOWS_1252, high: None };
    // a group's state: skipped, and how many characters stand in for a \uN
    let (mut skip, mut uc) = (false, 1usize);
    let (mut stack, mut deeper) = (Vec::<(bool, usize)>::new(), 0usize);
    // the stand-ins of the last \uN still to drop
    let mut drop = 0usize;
    let mut i = 0;
    while i < b.len() && r.out.len() <= MAX_TEXT {
        let c = b[i];
        i += 1;
        let text = match c {
            b'{' | b'}' => {
                r.flush();
                drop = 0;
                if c == b'}' && deeper > 0 {
                    deeper -= 1;
                } else if c == b'}' {
                    (skip, uc) = stack.pop().unwrap_or((skip, uc));
                } else if stack.len() < MAX_RTF_DEPTH {
                    stack.push((skip, uc));
                } else {
                    deeper += 1;
                }
                continue;
            }
            b'\r' | b'\n' => continue,
            b'\\' if b.get(i).is_some_and(u8::is_ascii_alphabetic) => {
                let word = i;
                while b.get(i).is_some_and(u8::is_ascii_alphabetic) {
                    i += 1;
                }
                let word = &b[word..i];
                let neg = b.get(i) == Some(&b'-') && b.get(i + 1).is_some_and(u8::is_ascii_digit);
                i += usize::from(neg);
                let digits = i;
                while b.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
                let n = std::str::from_utf8(&b[digits..i]).ok().and_then(|d| d.parse::<i64>().ok()).map(|n| if neg { -n } else { n });
                i += usize::from(b.get(i) == Some(&b' '));
                match (word, n) {
                    (b"bin", n) => i = i.saturating_add(n.unwrap_or(0).max(0) as usize).min(b.len()),
                    (b"uc", Some(n)) => uc = n.clamp(0, 16) as usize,
                    (w, _) if RTF_SKIPPED.contains(&w) => skip = true,
                    _ if skip => {}
                    (b"ansicpg", Some(n)) => {
                        r.flush();
                        r.code_page = code_page(n).unwrap_or(r.code_page);
                    }
                    (b"u", Some(n)) if (-32768..=65535).contains(&n) => {
                        r.unit(n);
                        drop = uc;
                    }
                    (b"par" | b"line" | b"sect" | b"page" | b"row" | b"nestrow", _) => r.push('\n'),
                    (b"tab" | b"cell" | b"nestcell", _) => r.push('\t'),
                    (b"emdash", _) => r.push('—'),
                    (b"endash", _) => r.push('–'),
                    (b"bullet", _) => r.push('•'),
                    (b"lquote", _) => r.push('‘'),
                    (b"rquote", _) => r.push('’'),
                    (b"ldblquote", _) => r.push('“'),
                    (b"rdblquote", _) => r.push('”'),
                    (b"emspace" | b"enspace" | b"qmspace", _) => r.push(' '),
                    _ => {}
                }
                continue;
            }
            b'\\' => {
                let Some(&s) = b.get(i) else { break };
                i += 1;
                match s {
                    b'\'' => match b.get(i..i + 2).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                        Some(v) => {
                            i += 2;
                            v
                        }
                        None => continue,
                    },
                    b'\\' | b'{' | b'}' => s,
                    b'*' => {
                        skip = true;
                        continue;
                    }
                    b'~' if !skip => {
                        r.push('\u{a0}');
                        continue;
                    }
                    b'_' if !skip => {
                        r.push('-');
                        continue;
                    }
                    b'\r' | b'\n' if !skip => {
                        r.push('\n');
                        continue;
                    }
                    _ => continue,
                }
            }
            c => c,
        };
        if drop > 0 {
            drop -= 1;
        } else if !skip {
            if r.high.take().is_some() {
                r.out.push(char::REPLACEMENT_CHARACTER);
            }
            r.bytes.push(text);
        }
    }
    r.flush();
    let full = r.out.len() > MAX_TEXT;
    if full {
        let mut end = MAX_TEXT;
        while !r.out.is_char_boundary(end) {
            end -= 1;
        }
        r.out.truncate(end);
    }
    let mut out = Out::default();
    out.text = r.out.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim_matches('\n').to_string();
    out.full = full;
    out.finish("RTF document", "text")
}

struct Package<'a> {
    zip: zip::ZipArchive<std::io::Cursor<&'a [u8]>>,
    what: &'static str,
    limit: u64,
    /// what may still be read, of all parts
    left: u64,
}

impl<'a> Package<'a> {
    fn open(bytes: &'a [u8], what: &'static str, limit: u64) -> AppResult<Self> {
        let zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| refuse("office_invalid", format!("Not a valid {what}: {e}")))?;
        if zip.len() > MAX_ENTRIES {
            return Err(refuse("office_too_many_parts", format!("This {what} has too many parts to read ({}).", zip.len())));
        }
        Ok(Package { zip, what, limit, left: MAX_PACKAGE_BYTES })
    }

    /// The text of the part `name` (None when there's none) and whether it
    /// was cut at the limit.
    fn part(&mut self, name: &str) -> AppResult<Option<(String, bool)>> {
        let what = self.what;
        let entry = match self.zip.by_name(name) {
            Ok(e) => e,
            Err(zip::result::ZipError::FileNotFound) => return Ok(None),
            Err(e) => return Err(refuse("office_invalid", format!("Not a valid {what}: {e}"))),
        };
        let mut buf = Vec::new();
        // the header's size is the file's word: only what inflates counts
        entry.take(self.limit.min(self.left) + 1).read_to_end(&mut buf).map_err(|e| refuse("office_damaged", format!("This {what} is damaged: {e}")))?;
        if buf.len() as u64 > self.left {
            return Err(refuse("office_too_large", format!("This {what} is too large to read.")));
        }
        let cut = buf.len() as u64 > self.limit;
        buf.truncate(self.limit as usize);
        self.left -= buf.len() as u64;
        Ok(Some((String::from_utf8(buf).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()), cut)))
    }

    /// What `main` lists, in its order (a workbook's sheets, a
    /// presentation's slides): each `tag` element's name and the path of
    /// the part its relationship points at.
    fn parts(&mut self, main: &str, tag: &str) -> AppResult<Vec<(String, String)>> {
        let (xml, _) = self.part(main)?.ok_or_else(|| refuse("office_invalid", format!("Not a valid {}: {main} is missing.", self.what)))?;
        let mut listed = vec![];
        walk(&xml, |t| {
            if let Tok::Open(n, a) = t {
                if n == tag {
                    let name = attr(a, |k| k == "name").map(|v| unescape(v).into_owned()).unwrap_or_default();
                    listed.push((name, attr(a, |k| k.ends_with(":id")).unwrap_or("").to_string()));
                }
            }
        });
        let (dir, file) = main.rsplit_once('/').unwrap_or(("", main));
        let rels = self.part(&format!("{dir}/_rels/{file}.rels"))?.map(|(x, _)| x).unwrap_or_default();
        let mut targets = HashMap::new();
        walk(&rels, |t| {
            if let Tok::Open("Relationship", a) = t {
                if let (Some(id), Some(target)) = (attr(a, |k| k == "Id"), attr(a, |k| k == "Target")) {
                    targets.insert(id.to_string(), unescape(target).into_owned());
                }
            }
        });
        Ok(listed
            .into_iter()
            .filter_map(|(name, id)| {
                let t = targets.get(&id)?;
                Some((name, t.strip_prefix('/').map_or_else(|| format!("{dir}/{t}"), str::to_string)))
            })
            .collect())
    }

    fn shared_strings(&mut self) -> AppResult<Vec<String>> {
        let Some((xml, _)) = self.part("xl/sharedStrings.xml")? else { return Ok(vec![]) };
        let (mut strings, mut cur, mut in_t, mut in_phonetic) = (vec![], String::new(), false, false);
        walk(&xml, |t| match t {
            Tok::Open("t", _) => in_t = true,
            Tok::Close("t") => in_t = false,
            Tok::Open("rPh", _) => in_phonetic = true,
            Tok::Close("rPh") => in_phonetic = false,
            Tok::Text(s) if in_t && !in_phonetic => cur.push_str(&unescape(s)),
            Tok::Close("si") if strings.len() < MAX_STRINGS => strings.push(std::mem::take(&mut cur)),
            Tok::Close("si") => cur.clear(),
            _ => {}
        });
        Ok(strings)
    }
}

/// A sheet's rows with a value as CSV lines, cells at their columns.
// shortcut: dates show as Excel's day numbers (their format is in styles.xml); read the number formats if users need dates.
fn sheet_csv(xml: &str, strings: &[String], out: &mut Out) {
    let (mut row, mut value, mut kind) = (Vec::<String>::new(), String::new(), String::new());
    let (mut col, mut next_col, mut rows) = (0, 0, 0);
    let (mut in_v, mut in_t, mut wide, mut stop) = (false, false, false, false);
    walk(xml, |t| {
        if stop {
            return;
        }
        match t {
            Tok::Open("row", _) => {
                row.clear();
                next_col = 0;
            }
            Tok::Open("c", a) => {
                kind = attr(a, |k| k == "t").unwrap_or("n").to_string();
                col = attr(a, |k| k == "r").and_then(column).unwrap_or(next_col);
                next_col = col + 1;
                value.clear();
            }
            Tok::Open("v", _) => in_v = true,
            Tok::Close("v") => in_v = false,
            Tok::Open("t", _) => in_t = true,
            Tok::Close("t") => in_t = false,
            Tok::Text(s) if in_v || in_t => value.push_str(&unescape(s)),
            Tok::Close("c") => {
                let shown = match kind.as_str() {
                    "s" => value.trim().parse::<usize>().ok().and_then(|i| strings.get(i)).cloned().unwrap_or_default(),
                    "b" => (if value.trim() == "1" { "TRUE" } else { "FALSE" }).to_string(),
                    _ => std::mem::take(&mut value),
                };
                if col >= MAX_COLS {
                    wide = true;
                } else if !shown.is_empty() {
                    if row.len() <= col {
                        row.resize(col + 1, String::new());
                    }
                    row[col] = shown;
                }
            }
            Tok::Close("row") if !row.is_empty() => {
                if rows == MAX_ROWS {
                    out.line(&format!("[Only the first {MAX_ROWS} rows with values are included.]"));
                    stop = true;
                    return;
                }
                rows += 1;
                stop = !out.line(&row.iter().map(|c| csv(c)).collect::<Vec<_>>().join(","));
            }
            _ => {}
        }
    });
    if wide {
        out.line(&format!("[Only the first {MAX_COLS} columns are included.]"));
    }
}

/// The 0-based column of a cell reference such as "B7".
fn column(r: &str) -> Option<usize> {
    let letters = r.bytes().take_while(u8::is_ascii_uppercase).count();
    (1..=3).contains(&letters).then(|| r.bytes().take(letters).fold(0, |n, c| n * 26 + usize::from(c - b'A' + 1)) - 1)
}

fn csv(cell: &str) -> Cow<'_, str> {
    if cell.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", cell.replace('"', "\"\"")).into()
    } else {
        cell.into()
    }
}

/// Text made within `MAX_TEXT`.
#[derive(Default)]
struct Out {
    text: String,
    full: bool,
}

impl Out {
    /// Adds a line; false, adding nothing, once the text is full.
    fn line(&mut self, s: &str) -> bool {
        if self.full || self.text.len() + s.len() + 1 > MAX_TEXT {
            self.full = true;
            return false;
        }
        self.text.push_str(s);
        self.text.push('\n');
        true
    }

    /// The text made from a `what`'s `parts`; an error when none was found.
    fn finish(mut self, what: &str, parts: &str) -> AppResult<String> {
        if self.text.is_empty() {
            return Err(refuse("office_empty", format!("This {what} has no {parts} to read.")));
        }
        if self.full {
            self.text.push_str(&format!("[The rest of this file was left out: its text is over {} KB.]", MAX_TEXT / 1024));
        }
        Ok(self.text.trim_end().to_string())
    }
}

enum Tok<'a> {
    /// A start tag (an empty-element tag is also followed by its Close):
    /// the name and the attributes as written.
    Open(&'a str, &'a str),
    Close(&'a str),
    Text(&'a str),
}

/// Walks the tags and text of `xml`, element names without their namespace
/// prefix; declarations, comments and doctypes are skipped, not read.
fn walk<'a>(xml: &'a str, mut f: impl FnMut(Tok<'a>)) {
    let local = |name: &'a str| name.rsplit_once(':').map_or(name, |(_, l)| l);
    let mut rest = xml;
    while !rest.is_empty() {
        if let Some(tag) = rest.strip_prefix('<') {
            let Some(end) = tag.find('>') else { break };
            let body = &tag[..end];
            rest = &tag[end + 1..];
            if let Some(name) = body.strip_prefix('/') {
                f(Tok::Close(local(name.trim_end())));
            } else if !body.starts_with(['?', '!']) {
                let empty = body.ends_with('/');
                let body = body.strip_suffix('/').unwrap_or(body);
                let (name, attrs) = body.split_once(|c: char| c.is_ascii_whitespace()).unwrap_or((body, ""));
                f(Tok::Open(local(name), attrs));
                if empty {
                    f(Tok::Close(local(name)));
                }
            }
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            f(Tok::Text(&rest[..end]));
            rest = &rest[end..];
        }
    }
}

/// The value, as written, of the first attribute whose name `want` takes.
fn attr(attrs: &str, want: impl Fn(&str) -> bool) -> Option<&str> {
    let mut rest = attrs;
    loop {
        let (name, after) = rest.split_once('=')?;
        let after = after.trim_start();
        let quote = after.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let (value, next) = after[1..].split_once(quote)?;
        if want(name.trim()) {
            return Some(value);
        }
        rest = next;
    }
}

/// XML text with its character references and built-in entities decoded;
/// any other entity stays as written (nothing a DTD defines is expanded).
fn unescape(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return s.into();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let name = rest[1..].find(';').filter(|&n| n <= 10).map(|n| &rest[1..=n]);
        let ch = name.and_then(|n| match n {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            // XHTML's, in EPUB books that declare its DTD
            "nbsp" => Some('\u{a0}'),
            _ => {
                let num = n.strip_prefix('#')?;
                let code = match num.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => num.parse().ok()?,
                };
                char::from_u32(code)
            }
        });
        match (name, ch) {
            (Some(n), Some(c)) => {
                out.push(c);
                rest = &rest[n.len() + 2..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out.into()
}

/// A zip of `files` (name, content), deflated.
#[cfg(test)]
pub(crate) fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
    use std::io::Write;
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data) in files {
        z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(data.as_bytes()).unwrap();
    }
    z.finish().unwrap().into_inner()
}

/// Make every size field of a one-entry zip claim `size` bytes.
#[cfg(test)]
pub(crate) fn lie_about_size(mut zip: Vec<u8>, size: u32) -> Vec<u8> {
    let patch = |z: &mut Vec<u8>, sig: [u8; 4], off: usize| {
        let at = z.windows(4).position(|w| w == sig).unwrap();
        z[at + off..at + off + 4].copy_from_slice(&size.to_le_bytes());
    };
    patch(&mut zip, [0x50, 0x4b, 0x03, 0x04], 22); // local header
    patch(&mut zip, [0x50, 0x4b, 0x01, 0x02], 24); // central directory
    zip
}

#[cfg(test)]
fn zip_owned(files: &[(String, String)]) -> Vec<u8> {
    zip_of(&files.iter().map(|(n, d)| (n.as_str(), d.as_str())).collect::<Vec<_>>())
}

/// A workbook whose sheets are `(name, sheetData xml)`, listed in that
/// order but stored under reversed part names, with `strings` shared.
#[cfg(test)]
pub(crate) fn xlsx_of(sheets: &[(&str, &str)], strings: &[&str]) -> Vec<u8> {
    let n = sheets.len();
    let list: String = sheets.iter().enumerate().map(|(i, (name, _))| format!(r#"<sheet name="{name}" sheetId="{}" r:id="rId{i}"/>"#, i + 1)).collect();
    let rels: String = (0..n).map(|i| format!(r#"<Relationship Id="rId{i}" Type="x" Target="worksheets/sheet{}.xml"/>"#, n - i)).collect();
    let sst: String = strings.iter().map(|s| format!("<si><t>{s}</t></si>")).collect();
    let mut files = vec![
        ("xl/workbook.xml".to_string(), format!(r#"<?xml version="1.0"?><workbook xmlns:r="r"><sheets>{list}</sheets></workbook>"#)),
        ("xl/_rels/workbook.xml.rels".to_string(), format!("<Relationships>{rels}</Relationships>")),
        ("xl/sharedStrings.xml".to_string(), format!("<sst>{sst}</sst>")),
    ];
    for (i, (_, data)) in sheets.iter().enumerate() {
        files.push((format!("xl/worksheets/sheet{}.xml", n - i), format!("<worksheet><sheetData>{data}</sheetData></worksheet>")));
    }
    zip_owned(&files)
}

/// A presentation whose slides are `slides` (each its paragraphs), listed
/// in that order but stored under reversed part names.
#[cfg(test)]
pub(crate) fn pptx_of(slides: &[&[&str]]) -> Vec<u8> {
    let n = slides.len();
    let list: String = (0..n).map(|i| format!(r#"<p:sldId id="{}" r:id="rId{i}"/>"#, 256 + i)).collect();
    let rels: String = (0..n).map(|i| format!(r#"<Relationship Id="rId{i}" Target="/ppt/slides/slide{}.xml"/>"#, n - i)).collect();
    let mut files = vec![
        ("ppt/presentation.xml".to_string(), format!("<p:presentation><p:sldIdLst>{list}</p:sldIdLst></p:presentation>")),
        ("ppt/_rels/presentation.xml.rels".to_string(), format!("<Relationships>{rels}</Relationships>")),
    ];
    for (i, paras) in slides.iter().enumerate() {
        let body: String = paras.iter().map(|p| format!(r#"<a:p><a:r><a:rPr lang="en"/><a:t>{p}</a:t></a:r></a:p>"#)).collect();
        files.push((format!("ppt/slides/slide{}.xml", n - i), format!("<p:sld><p:cSld><p:spTree><p:sp><p:txBody>{body}<a:p/></p:txBody></p:sp></p:spTree></p:cSld></p:sld>")));
    }
    zip_owned(&files)
}

/// An EPUB book whose spine lists `chapters` (path under OEBPS, XHTML body)
/// in that order, its manifest in the reverse order.
#[cfg(test)]
pub(crate) fn epub_of(chapters: &[(&str, &str)]) -> Vec<u8> {
    let items: String = chapters.iter().enumerate().rev().map(|(i, (href, _))| format!(r#"<item id="c{i}" href="{href}" media-type="application/xhtml+xml"/>"#)).collect();
    let spine: String = (0..chapters.len()).map(|i| format!(r#"<itemref idref="c{i}"/>"#)).collect();
    let mut files = vec![
        ("mimetype".to_string(), "application/epub+zip".to_string()),
        ("META-INF/container.xml".to_string(), r#"<container><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#.to_string()),
        ("OEBPS/content.opf".to_string(), format!("<package><manifest>{items}</manifest><spine>{spine}</spine></package>")),
    ];
    for (href, body) in chapters {
        files.push((zip_path("OEBPS", href), format!("<html><head><title>T</title><style>p{{}}</style></head><body>{body}</body></html>")));
    }
    zip_owned(&files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odt_paragraphs_tables_and_spaces() {
        let content = r#"<?xml version="1.0"?><office:document-content><office:automatic-styles><style:style style:name="P1">ignored</style:style></office:automatic-styles>
            <office:body><office:text>
              <text:h text:outline-level="1">Title &amp; more</text:h>
              <text:p>One  <text:span>two</text:span>
                three<text:s text:c="3"/>x<text:tab/>y<text:line-break/>z</text:p>
              <text:p/>
              <text:p>Note<office:annotation><text:p>a comment</text:p></office:annotation> kept</text:p>
              <text:tracked-changes><text:changed-region><text:deletion><text:p>deleted</text:p></text:deletion></text:changed-region></text:tracked-changes>
              <table:table><table:table-row><table:table-cell><text:p>a</text:p><text:p>b</text:p></table:table-cell><table:covered-table-cell/><table:table-cell><text:p>c</text:p></table:table-cell><table:table-cell/></table:table-row>
              <table:table-row><table:table-cell><text:p>1</text:p></table:table-cell></table:table-row></table:table>
            </office:text></office:body></office:document-content>"#;
        let text = odt_text(&zip_of(&[("mimetype", "application/vnd.oasis.opendocument.text"), ("content.xml", content)])).unwrap();
        assert_eq!(text, "Title & more\nOne two three   x\ty\nz\n\nNote kept\na b\t\tc\n1");
    }

    #[test]
    fn hostile_odt_is_bounded() {
        assert!(odt_text(b"PK\x03\x04 nope").unwrap_err().to_string().contains("Not a valid OpenDocument text"));
        assert_eq!(odt_text(&zip_of(&[("styles.xml", "<x/>")])).unwrap_err().to_string(), "Not a valid OpenDocument text: content.xml is missing.");
        // a space count can't make the text larger than the file
        let spaces = format!("<office:body><text:p>a{}b</text:p></office:body>", r#"<text:s text:c="99999999999999"/>"#.repeat(3));
        assert_eq!(odt_text(&zip_of(&[("content.xml", &spaces)])).unwrap(), format!("a{}b", " ".repeat(3 * MAX_SPACES)));
        // text outside the body (styles, settings) isn't the document's
        assert!(odt_text(&zip_of(&[("content.xml", "<office:styles><text:p>x</text:p></office:styles>")])).unwrap_err().to_string().contains("no text"));
        // a part that inflates without end is cut at the part limit; the text made stays under MAX_TEXT
        let big = format!("<office:body>{}</office:body>", "<text:p>words and more words</text:p>".repeat(60_000));
        let text = odt_text(&zip_of(&[("content.xml", &big)])).unwrap();
        assert!(text.len() < crate::logdigest::TEXT_DIGEST_OVER && text.ends_with("KB.]"), "{}", text.len());
    }

    #[test]
    fn epub_chapters_in_spine_order() {
        let book = epub_of(&[
            ("text/ch%201.xhtml", "<h1>Chapter&#160;1</h1><p>It  was\n  a <em>dark</em> night.</p><script>alert(1)</script><p>Line<br/>break &nbsp;x</p>"),
            ("../extra/two.xhtml#start", "<ul><li>one</li><li>two</li></ul><table><tr><td>a</td><td>b</td></tr></table><pre>  keep\n    this</pre>"),
        ]);
        assert_eq!(epub_text(&book).unwrap(), "Chapter\u{a0}1\nIt was a dark night.\nLine\nbreak \u{a0}x\none\ntwo\na\tb\nkeep\n    this");
    }

    #[test]
    fn hostile_epubs_are_bounded() {
        assert!(epub_text(&zip_of(&[("a.txt", "x")])).unwrap_err().to_string().contains("container.xml is missing"));
        let no_opf = zip_of(&[("META-INF/container.xml", "<container/>")]);
        assert!(epub_text(&no_opf).unwrap_err().to_string().contains("isn't named"));
        // a spine of missing or outside items reads nothing; `..` stays inside the zip
        let opf = r#"<package><manifest><item id="a" href="../../../../etc/passwd"/></manifest><spine><itemref idref="a"/><itemref idref="nope"/></spine></package>"#;
        let book = zip_of(&[("META-INF/container.xml", r#"<rootfile full-path="OEBPS/p.opf"/>"#), ("OEBPS/p.opf", opf)]);
        assert_eq!(epub_text(&book).unwrap_err().to_string(), "This EPUB book has no chapters to read.");
        assert_eq!(zip_path("OEBPS/text", "../../../../etc/passwd"), "etc/passwd");
        assert_eq!(zip_path("OEBPS", "/abs/x.xhtml"), "abs/x.xhtml");
        // one chapter listed over and over stops once the text is full
        let chapter = format!("<p>{}</p>", "x".repeat(10_000));
        let spine: String = r#"<itemref idref="c"/>"#.repeat(MAX_ENTRIES * 2);
        let opf = format!(r#"<package><manifest><item id="c" href="c.xhtml"/></manifest><spine>{spine}</spine></package>"#);
        let book = zip_of(&[("META-INF/container.xml", r#"<rootfile full-path="p.opf"/>"#), ("p.opf", &opf), ("c.xhtml", &chapter)]);
        let text = epub_text(&book).unwrap();
        assert!(text.len() < crate::logdigest::TEXT_DIGEST_OVER && text.ends_with("KB.]"));
        assert_eq!(percent_decode("a%20b%zz%4"), "a b%zz%4");
    }

    #[test]
    fn rtf_text_and_escapes() {
        let doc = br#"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fswiss Arial;}{\f1 Courier;}}{\colortbl;\red255\green0\blue0;}
{\stylesheet{\s0 Normal;}}{\info{\title Secret title}{\author Me}}{\*\generator Riched20 10.0;}\viewkind4\uc1
\pard\f0\fs24 Caf\'e9 \b bold\b0  \{braces\} \\ back\par
Tab\tab here\line next\par
{\field{\*\fldinst HYPERLINK "https://x.test"}{\fldrslt link text}}\par
\u351?ey \uc0\u287 x \uc1\u-10179?\u-8704? \emdash  \ldblquote q\rdblquote\par
{\pict\pngblip 89504e470d0a}{\object{\*\objdata 0105}}\bin4 {}}}after\par
{\header head}{\footer foot}End}"#;
        assert_eq!(rtf_text(doc).unwrap(), "Café bold {braces} \\ back\nTab\there\nnext\nlink text\nşey ğx 😀 — “q”\nafter\nEnd");
        // the document's code page: Turkish, and a double-byte one
        assert_eq!(rtf_text(br"{\rtf1\ansi\ansicpg1254 \'fd\'fe\'f0\'dd}").unwrap(), "ışğİ");
        assert_eq!(rtf_text(br"{\rtf1\ansi\ansicpg932 \'82\'a0}").unwrap(), "あ");
        assert_eq!(rtf_text(br"{\rtf1\ansicpg99999 \'e9}").unwrap(), "é", "an unknown code page keeps the default");
    }

    #[test]
    fn hostile_rtf_is_bounded() {
        // deep nesting, unbalanced braces, cut escapes, huge parameters
        let deep = [b"{\\rtf1 a".as_slice(), &b"{".repeat(1_000_000), b"b", &b"}".repeat(2_000_000), b"c"].concat();
        assert_eq!(rtf_text(&deep).unwrap(), "abc");
        assert_eq!(rtf_text(br"{\rtf1 x\'").unwrap(), "x");
        assert_eq!(rtf_text(br"{\rtf1 x\'4").unwrap(), "x4");
        assert_eq!(rtf_text(br"{\rtf1 x\").unwrap(), "x");
        assert_eq!(rtf_text(br"{\rtf1 x\u99999999999999? y\u-99999 z}").unwrap(), "x? yz");
        assert_eq!(rtf_text(br"{\rtf1 x\bin99999999999999 secret}").unwrap(), "x");
        assert_eq!(rtf_text(br"{\rtf1 x\bin-5 y}").unwrap(), "xy");
        assert_eq!(rtf_text(br"{\rtf1 \uc99999999999\u65 abc}").unwrap(), "A");
        // a lone or unpaired surrogate is a replacement character
        assert_eq!(rtf_text(br"{\rtf1 \u-10179?x\u-8704?}").unwrap(), "\u{fffd}x\u{fffd}");
        let long_word = [b"{\\rtf1 ".as_slice(), &b"a".repeat(100), b"\\", &b"z".repeat(100_000), b" end}"].concat();
        assert_eq!(rtf_text(&long_word).unwrap(), "a".repeat(100) + "end");
        // the text made is bounded
        let huge = [b"{\\rtf1 ".as_slice(), &b"word ".repeat(400_000), b"}"].concat();
        let text = rtf_text(&huge).unwrap();
        assert!(text.len() < crate::logdigest::TEXT_DIGEST_OVER && text.ends_with("KB.]"));
        assert!(rtf_text(br"{\rtf1{\*\generator x}{\fonttbl{\f0 A;}}}").unwrap_err().to_string().contains("no text"));
    }

    #[test]
    fn sheets_become_csv_in_workbook_order() {
        let data = r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="C1" t="s"><v>1</v></c></row>
            <row r="2"><c r="A2"><v>3.5</v></c><c r="B2" t="b"><v>1</v></c><c r="C2" t="inlineStr"><is><t>a, "b"</t></is></c></row>
            <row r="3"><c r="A3" t="str"><f>A2*2</f><v>7</v></c><c t="e"><v>#DIV/0!</v></c><c r="D3" t="b"><v>0</v></c></row>
            <row r="4"><c r="A4" s="1"/></row>"#;
        let x = xlsx_of(&[("Data &amp; more", data), ("Empty", "")], &["Name", "Tom &amp; Jerry"]);
        assert_eq!(
            xlsx_text(&x).unwrap(),
            "## Sheet: Data & more\nName,,Tom & Jerry\n3.5,TRUE,\"a, \"\"b\"\"\"\n7,#DIV/0!,,FALSE\n\n## Sheet: Empty"
        );
    }

    #[test]
    fn shared_strings_join_runs_and_skip_phonetics() {
        let x = zip_of(&[
            ("xl/workbook.xml", r#"<workbook><sheets><x:sheet name="S" r:id="a"/></sheets></workbook>"#),
            ("xl/_rels/workbook.xml.rels", r#"<Relationships><Relationship Target="worksheets/s.xml" Id="a"/></Relationships>"#),
            ("xl/sharedStrings.xml", r#"<sst><si><r><t>Bold</t></r><r><t xml:space="preserve"> plain</t></r><rPh><t>ruby</t></rPh></si></sst>"#),
            ("xl/worksheets/s.xml", r#"<worksheet><sheetData><row><c t="s"><v>0</v></c><c t="s"><v>9</v></c></row></sheetData></worksheet>"#),
        ]);
        // an index past the shared strings is an empty cell
        assert_eq!(xlsx_text(&x).unwrap(), "## Sheet: S\nBold plain");
    }

    #[test]
    fn big_sheets_are_cut_and_say_so() {
        let rows: String = (1..=MAX_ROWS + 5).map(|i| format!("<row><c><v>{i}</v></c></row>")).collect();
        let text = xlsx_text(&xlsx_of(&[("Big", &rows), ("Next", "<row><c><v>1</v></c></row>")], &[])).unwrap();
        assert!(text.contains(&format!("\n{MAX_ROWS}\n[Only the first {MAX_ROWS} rows")));
        assert!(text.ends_with("## Sheet: Next\n1"));
        let wide = r#"<row><c r="A1"><v>1</v></c><c r="ZZ1"><v>2</v></c></row>"#;
        assert_eq!(xlsx_text(&xlsx_of(&[("W", wide)], &[])).unwrap(), "## Sheet: W\n1\n[Only the first 200 columns are included.]");
        let long = "x".repeat(1000);
        let rows: String = (0..2000).map(|_| format!(r#"<row><c t="inlineStr"><is><t>{long}</t></is></c></row>"#)).collect();
        let text = xlsx_text(&xlsx_of(&[("Long", &rows), ("Never", "")], &[])).unwrap();
        assert!(text.len() < crate::logdigest::TEXT_DIGEST_OVER);
        assert!(text.ends_with("[The rest of this file was left out: its text is over 1023 KB.]"));
        assert!(!text.contains("Never"));
    }

    #[test]
    fn slides_in_presentation_order() {
        let p = pptx_of(&[&["Title", "Sub &lt;title&gt; &#169; &#x263A;"], &[], &["Last"]]);
        assert_eq!(pptx_text(&p).unwrap(), "## Slide 1\nTitle\nSub <title> © ☺\n\n## Slide 2\n\n## Slide 3\nLast");
    }

    #[test]
    fn hostile_zips_are_bounded() {
        // not a zip, a cut zip, a damaged entry, no workbook
        assert!(xlsx_text(b"PK\x03\x04 nope").unwrap_err().to_string().contains("Not a valid Excel workbook"));
        let good = xlsx_of(&[("S", "<row><c><v>1</v></c></row>")], &[]);
        assert!(xlsx_text(&good[..good.len() / 2]).is_err());
        let slides: String = (0..500).map(|i| format!(r#"<p:sldId id="{i}"/>"#)).collect();
        let mut damaged = zip_of(&[("ppt/presentation.xml", &slides)]);
        let le = |z: &[u8], at: usize| usize::from(u16::from_le_bytes([z[at], z[at + 1]]));
        let data = 30 + le(&damaged, 26) + le(&damaged, 28);
        damaged[data..data + 8].fill(0xFF);
        assert!(pptx_text(&damaged).unwrap_err().to_string().contains("presentation is damaged"));
        assert!(xlsx_text(&zip_of(&[("a.txt", "hi")])).unwrap_err().to_string().contains("xl/workbook.xml is missing"));
        // too many entries
        let names: Vec<String> = (0..=MAX_ENTRIES).map(|i| format!("f{i}")).collect();
        let many = zip_of(&names.iter().map(|n| (n.as_str(), "")).collect::<Vec<_>>());
        assert!(xlsx_text(&many).unwrap_err().to_string().contains("too many parts"));
        // a part that inflates past the limit is cut there, whatever its header claims
        let bomb = zip_of(&[("x.xml", &"<".repeat(1 << 20))]);
        let (text, cut) = Package::open(&bomb, "Excel workbook", 4096).unwrap().part("x.xml").unwrap().unwrap();
        assert!(cut && text.len() == 4096);
        // and all parts read together are bounded: a workbook can list one part over and over
        let mut p = Package::open(&bomb, "Excel workbook", 1 << 20).unwrap();
        p.left = (5 << 20) / 2;
        assert!(p.part("x.xml").is_ok() && p.part("x.xml").is_ok());
        assert!(p.part("x.xml").unwrap_err().to_string().contains("too large to read"));
        // so are the shared strings a small part of empty ones makes
        let sst = format!("<sst>{}</sst>", "<si/>".repeat(MAX_STRINGS + 5));
        assert_eq!(Package::open(&zip_of(&[("xl/sharedStrings.xml", &sst)]), "Excel workbook", MAX_PART_BYTES).unwrap().shared_strings().unwrap().len(), MAX_STRINGS);
        let liar = lie_about_size(bomb, 10);
        let read = Package::open(&liar, "Excel workbook", 4096).unwrap().part("x.xml");
        assert!(read.map_or(true, |r| r.unwrap().0.len() <= 4096));
        // entities a DTD defines are not expanded
        let laughs = r#"<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;&lol;">]><sst><si><t>&lol2;</t></si></sst>"#;
        let x = zip_of(&[
            ("xl/workbook.xml", r#"<workbook><sheets><sheet name="S" r:id="a"/></sheets></workbook>"#),
            ("xl/_rels/workbook.xml.rels", r#"<Relationships><Relationship Id="a" Target="s.xml"/></Relationships>"#),
            ("xl/sharedStrings.xml", laughs),
            ("xl/s.xml", r#"<sheetData><row><c t="s"><v>0</v></c></row></sheetData>"#),
        ]);
        assert_eq!(xlsx_text(&x).unwrap(), "## Sheet: S\n&lol2;");
        // a relationship target only names an entry of the same zip
        let x = zip_of(&[
            ("xl/workbook.xml", r#"<workbook><sheets><sheet name="S" r:id="a"/></sheets></workbook>"#),
            ("xl/_rels/workbook.xml.rels", r#"<Relationships><Relationship Id="a" Target="../../../../etc/passwd"/></Relationships>"#),
        ]);
        assert_eq!(xlsx_text(&x).unwrap_err().to_string(), "This Excel workbook has no sheets to read.");
        // nothing listed
        assert!(xlsx_text(&zip_of(&[("xl/workbook.xml", "<workbook/>")])).is_err());
        assert_eq!(pptx_text(&zip_of(&[("ppt/presentation.xml", "<p/>")])).unwrap_err().to_string(), "This PowerPoint presentation has no slides to read.");
    }

    #[test]
    fn xml_helpers() {
        assert_eq!(column("A1"), Some(0));
        assert_eq!(column("AA10"), Some(26));
        assert_eq!(column("XFD1"), Some(16383));
        assert_eq!(column("ABCD1"), None);
        assert_eq!(column("12"), None);
        assert_eq!(attr(r#"id="256" r:id='rId2'"#, |k| k.ends_with(":id")), Some("rId2"));
        assert_eq!(attr(r#"id="256" r:id='rId2'"#, |k| k == "id"), Some("256"));
        assert_eq!(attr("broken", |_| true), None);
        assert_eq!(unescape("a &amp;amp; &unknown; &#65; &#xZZ; & b"), "a &amp; &unknown; A &#xZZ; & b");
    }
}

