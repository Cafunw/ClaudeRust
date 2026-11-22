//! Text of Excel workbooks (.xlsx: each sheet as CSV) and PowerPoint
//! presentations (.pptx: the text of each slide). They are zips from
//! anywhere: the entry count, the bytes read of each part and the text made
//! are bounded, entry names are only looked up in the same zip, nothing
//! nested is opened, and no entity but XML's own is expanded.

use crate::error::{AppError, AppResult};
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

struct Package<'a> {
    zip: zip::ZipArchive<std::io::Cursor<&'a [u8]>>,
    what: &'static str,
    limit: u64,
    /// what may still be read, of all parts
    left: u64,
}

impl<'a> Package<'a> {
    fn open(bytes: &'a [u8], what: &'static str, limit: u64) -> AppResult<Self> {
        let zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| AppError::msg(format!("Not a valid {what}: {e}")))?;
        if zip.len() > MAX_ENTRIES {
            return Err(AppError::msg(format!("This {what} has too many parts to read ({}).", zip.len())));
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
            Err(e) => return Err(AppError::msg(format!("Not a valid {what}: {e}"))),
        };
        let mut buf = Vec::new();
        // the header's size is the file's word: only what inflates counts
        entry.take(self.limit.min(self.left) + 1).read_to_end(&mut buf).map_err(|e| AppError::msg(format!("This {what} is damaged: {e}")))?;
        if buf.len() as u64 > self.left {
            return Err(AppError::msg(format!("This {what} is too large to read.")));
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
        let (xml, _) = self.part(main)?.ok_or_else(|| AppError::msg(format!("Not a valid {}: {main} is missing.", self.what)))?;
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
            return Err(AppError::msg(format!("This {what} has no {parts} to read.")));
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

#[cfg(test)]
mod tests {
    use super::*;

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

