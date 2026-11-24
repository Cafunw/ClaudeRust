//! A Doc artifact (markdown) as a Word document: headings, paragraphs with
//! bold, italic, struck and code runs, lists, quotes, code blocks, tables and
//! rules, in styles of the document's own (word/styles.xml).

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::io::Write;

/// `s` as XML text: escaped, without the control characters XML 1.0 forbids.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(' '),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

#[derive(Default)]
struct Writer {
    body: String,
    /// the paragraph being written: its properties and runs
    para: Option<(String, String)>,
    bold: u32,
    italic: u32,
    strike: u32,
    link: u32,
    quote: u32,
    /// one entry per open list: the next number of an ordered one
    lists: Vec<Option<u64>>,
    code: bool,
    /// rows of the table being written (cells' runs), and whether the head is open
    table: Option<Vec<Vec<String>>>,
    cell: Option<String>,
    head: bool,
}

impl Writer {
    fn open(&mut self, style: &str, indent: u32) {
        if self.para.as_ref().is_some_and(|(_, runs)| runs.is_empty()) {
            return;
        }
        self.close();
        let style = if self.quote > 0 && style == "Normal" { "Quote" } else { style };
        let ind = if indent > 0 { format!("<w:ind w:left=\"{indent}\"/>") } else { String::new() };
        self.para = Some((format!("<w:pStyle w:val=\"{style}\"/>{ind}"), String::new()));
    }

    fn close(&mut self) {
        if let Some((props, runs)) = self.para.take() {
            self.body.push_str(&format!("<w:p><w:pPr>{props}</w:pPr>{runs}</w:p>"));
        }
    }

    fn run(&mut self, text: &str, code: bool) {
        let mut props = String::new();
        if self.bold > 0 || self.head {
            props.push_str("<w:b/>");
        }
        if self.italic > 0 {
            props.push_str("<w:i/>");
        }
        if self.strike > 0 {
            props.push_str("<w:strike/>");
        }
        if code {
            props.push_str("<w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\" w:cs=\"Consolas\"/>");
        }
        if self.link > 0 {
            props.push_str("<w:color w:val=\"1F5FBF\"/><w:u w:val=\"single\"/>");
        }
        let r = format!("<w:r><w:rPr>{props}</w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>", esc(text));
        self.push(&r);
    }

    fn push(&mut self, xml: &str) {
        if let Some(cell) = &mut self.cell {
            cell.push_str(xml);
            return;
        }
        if self.para.is_none() {
            self.open("Normal", 0);
        }
        if let Some((_, runs)) = &mut self.para {
            runs.push_str(xml);
        }
    }

    fn event(&mut self, e: Event) {
        match e {
            Event::Start(Tag::Paragraph) => self.open("Normal", 0),
            Event::Start(Tag::Heading { level, .. }) => {
                let n = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    _ => 3,
                };
                self.open(&format!("Heading{n}"), 0);
            }
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item) => self.close(),
            Event::Start(Tag::BlockQuote(_)) => self.quote += 1,
            Event::End(TagEnd::BlockQuote(_)) => {
                self.close();
                self.quote = self.quote.saturating_sub(1);
            }
            Event::Start(Tag::List(start)) => {
                self.close();
                self.lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                self.close();
                self.lists.pop();
            }
            Event::Start(Tag::Item) => {
                self.close();
                let depth = self.lists.len() as u32;
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}. ", *n - 1)
                    }
                    _ => "• ".to_string(),
                };
                self.open("ListParagraph", 360 + 360 * depth);
                self.run(&marker, false);
            }
            Event::TaskListMarker(done) => self.run(if done { "☑ " } else { "☐ " }, false),
            Event::Start(Tag::CodeBlock(_)) => {
                self.close();
                self.code = true;
            }
            Event::End(TagEnd::CodeBlock) => self.code = false,
            Event::Start(Tag::Table(_)) => {
                self.close();
                self.table = Some(vec![]);
            }
            Event::Start(Tag::TableHead | Tag::TableRow) => {
                self.head = matches!(e, Event::Start(Tag::TableHead));
                if let Some(t) = &mut self.table {
                    t.push(vec![]);
                }
            }
            Event::End(TagEnd::TableHead) => self.head = false,
            Event::Start(Tag::TableCell) => self.cell = Some(String::new()),
            Event::End(TagEnd::TableCell) => {
                let cell = self.cell.take().unwrap_or_default();
                if let Some(row) = self.table.as_mut().and_then(|t| t.last_mut()) {
                    row.push(cell);
                }
            }
            Event::End(TagEnd::Table) => {
                let rows = self.table.take().unwrap_or_default();
                let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
                let border = |side: &str| format!("<w:{side} w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"A6A6A6\"/>");
                let borders: String = ["top", "left", "bottom", "right", "insideH", "insideV"].iter().map(|s| border(s)).collect();
                self.body.push_str(&format!("<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>{borders}</w:tblBorders></w:tblPr><w:tblGrid>{}</w:tblGrid>", "<w:gridCol/>".repeat(cols)));
                for row in rows {
                    self.body.push_str("<w:tr>");
                    for i in 0..cols {
                        let runs = row.get(i).map(String::as_str).unwrap_or("");
                        self.body.push_str(&format!("<w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr><w:p>{runs}</w:p></w:tc>"));
                    }
                    self.body.push_str("</w:tr>");
                }
                self.body.push_str("</w:tbl>");
            }
            Event::Rule => {
                self.close();
                self.body.push_str("<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"A6A6A6\"/></w:pBdr></w:pPr></w:p>");
            }
            Event::Start(Tag::Strong) => self.bold += 1,
            Event::End(TagEnd::Strong) => self.bold = self.bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => self.italic += 1,
            Event::End(TagEnd::Emphasis) => self.italic = self.italic.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => self.strike += 1,
            Event::End(TagEnd::Strikethrough) => self.strike = self.strike.saturating_sub(1),
            Event::Start(Tag::Link { .. }) => self.link += 1,
            Event::End(TagEnd::Link) => self.link = self.link.saturating_sub(1),
            Event::Text(t) if self.code => {
                for line in t.trim_end_matches('\n').split('\n') {
                    self.open("Code", 0);
                    self.run(line, true);
                    self.close();
                }
            }
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => self.run(&t, false),
            Event::Code(t) => self.run(&t, true),
            Event::SoftBreak => self.run(" ", false),
            Event::HardBreak => self.push("<w:r><w:br/></w:r>"),
            _ => {}
        }
    }
}

/// The `<w:body>` content of markdown `md`.
fn body_xml(md: &str) -> String {
    let mut w = Writer::default();
    for e in Parser::new_ext(md, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS) {
        w.event(e);
    }
    w.close();
    // Word wants the body to end in a paragraph (a table may be last)
    w.body.push_str("<w:p/>");
    w.body
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>"#;
const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#;
const DOC_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;
const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:cs="Calibri" w:eastAsia="Calibri"/><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="360" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="36"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="280" w:after="100"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="28"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="200" w:after="80"/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/><w:sz w:val="24"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Quote"><w:name w:val="Quote"/><w:basedOn w:val="Normal"/><w:pPr><w:ind w:left="720"/></w:pPr><w:rPr><w:i/><w:color w:val="595959"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="60"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Code"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/><w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/></w:pPr><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:sz w:val="20"/></w:rPr></w:style></w:styles>"#;

/// Markdown `md` as the bytes of a .docx titled `title`.
pub fn docx(md: &str, title: &str) -> Result<Vec<u8>, String> {
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#,
        body_xml(md)
    );
    let core = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{}</dc:title></cp:coreProperties>"#,
        esc(title)
    );
    let parts = [("[Content_Types].xml", CONTENT_TYPES), ("_rels/.rels", RELS), ("word/_rels/document.xml.rels", DOC_RELS), ("word/document.xml", &doc), ("word/styles.xml", STYLES), ("docProps/core.xml", &core)];
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, xml) in parts {
        z.start_file(name, o).map_err(|e| e.to_string())?;
        z.write_all(xml.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(z.finish().map_err(|e| e.to_string())?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn part(bytes: &[u8], name: &str) -> String {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut s = String::new();
        z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn a_doc_becomes_a_word_document_in_its_styles() {
        let md = "# Plan\n\nSome **bold** and *italic* & `code`.\n\n- one\n- two\n\n1. first\n2. second\n\n> said\n\n| A | B |\n|---|---|\n| 1 | <b>2</b> |\n\n```\nlet x = 1;\n```\n\n---\n";
        let bytes = docx(md, "Q3 <plan>").unwrap();
        let doc = part(&bytes, "word/document.xml");
        for want in [
            "<w:pStyle w:val=\"Heading1\"/>",
            "<w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">bold</w:t>",
            "<w:rPr><w:i/></w:rPr><w:t xml:space=\"preserve\">italic</w:t>",
            " &amp; ",
            "• ",
            "1. ",
            "2. ",
            "<w:pStyle w:val=\"Quote\"/>",
            "<w:tbl>",
            ">&lt;b&gt;</w:t>",
            ">&lt;/b&gt;</w:t>",
            "<w:pStyle w:val=\"Code\"/>",
            "let x = 1;",
            "<w:pBdr>",
        ] {
            assert!(doc.contains(want), "{want} missing in {doc}");
        }
        // header cells are bold, every cell holds a paragraph
        assert!(doc.contains("<w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr><w:p><w:r><w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">A</w:t>"));
        assert!(doc.ends_with("<w:p/><w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>"));
        assert!(part(&bytes, "docProps/core.xml").contains("<dc:title>Q3 &lt;plan&gt;</dc:title>"));
        for name in ["[Content_Types].xml", "_rels/.rels", "word/_rels/document.xml.rels", "word/styles.xml"] {
            assert!(part(&bytes, name).starts_with("<?xml"), "{name}");
        }
    }

    #[test]
    fn control_characters_never_reach_the_xml() {
        let doc = body_xml("a\u{1}b\u{7}c");
        assert!(doc.contains(">abc<"), "{doc}");
    }
}
