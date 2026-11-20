//! Word documents (.docx) <-> Markdown.
//!
//! Reading extracts structure, not layout: headings, paragraphs, bold/italic/
//! strikethrough, inline code, links, bullet and numbered lists (with nesting)
//! and simple tables. Images, comments, headers/footers and page layout are
//! dropped. Writing produces a valid .docx package with real heading/list styles.
//! From Markdown you can reach HTML and plain text through the conversion graph.

use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, Read, Write},
};

use pulldown_cmark::{Event, HeadingLevel, Options as MdOptions, Parser, Tag, TagEnd};
use quick_xml::{events::Event as Xml, Reader};
use uberconv_core::{Context, Converter, Cost, Edge, Error, Format, Job, Result};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

pub struct DocxConverter;

impl Converter for DocxConverter {
    fn name(&self) -> &'static str {
        "docx"
    }

    fn edges(&self) -> Vec<Edge> {
        vec![
            // formatting, images and layout are dropped
            Edge::new(Format::Docx, Format::Markdown, Cost::lossy(15)),
            Edge::new(Format::Markdown, Format::Docx, Cost::lossless(15)),
        ]
    }

    fn convert(&self, job: &Job<'_>, ctx: &Context) -> Result<()> {
        ctx.report(0.1);
        match (job.from, job.to) {
            (Format::Docx, Format::Markdown) => {
                let md = docx_to_markdown(job.input)?;
                std::fs::write(job.output, md)?;
            }
            (Format::Markdown, Format::Docx) => {
                let md = std::fs::read_to_string(job.input)
                    .map_err(|e| Error::failed(format!("could not read the Markdown as UTF-8 text: {e}")))?;
                std::fs::write(job.output, markdown_to_docx(&md)?)?;
            }
            (f, t) => return Err(Error::failed(format!("docx converter can't do {f} -> {t}"))),
        }
        ctx.report(1.0);
        Ok(())
    }
}

// ═══════════════════════════ reading ═══════════════════════════

#[derive(Default, Clone, PartialEq)]
struct Run {
    text: String,
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: Option<String>,
}

enum Block {
    Heading(usize, Vec<Run>),
    Para(Vec<Run>),
    Item { ordered: bool, level: usize, runs: Vec<Run> },
    Quote(Vec<Run>),
    /// Consecutive code paragraphs are merged into one block.
    Code(String),
    Table(Vec<Vec<String>>),
}

fn read_part(zip: &mut ZipArchive<BufReader<File>>, name: &str) -> Option<String> {
    let mut file = zip.by_name(name).ok()?;
    let mut s = String::new();
    file.read_to_string(&mut s).ok()?;
    Some(s)
}

fn local(name: &[u8]) -> &[u8] {
    name.rsplit(|b| *b == b':').next().unwrap_or(name)
}

fn attr(e: &quick_xml::events::BytesStart<'_>, key: &str) -> Option<String> {
    e.attributes().flatten().find(|a| local(a.key.as_ref()) == key.as_bytes()).and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok().map(|v| v.into_owned()))
}

/// `<w:b/>` is on, `<w:b w:val="0"/>` / `"false"` is off.
fn toggle(e: &quick_xml::events::BytesStart<'_>) -> bool {
    !matches!(attr(e, "val").as_deref(), Some("0") | Some("false") | Some("off"))
}

fn entity_text(name: &str) -> Option<String> {
    Some(match name {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" => "'".into(),
        n => {
            let num = n.strip_prefix('#')?;
            let code = match num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)?.to_string()
        }
    })
}

/// style id -> lowercase style name
fn parse_styles(xml: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut reader = Reader::from_str(xml);
    let mut current: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Xml::Start(e)) | Ok(Xml::Empty(e)) => match local(e.name().as_ref()) {
                b"style" => current = attr(&e, "styleId"),
                b"name" => {
                    if let (Some(id), Some(n)) = (&current, attr(&e, "val")) {
                        map.insert(id.clone(), n.to_lowercase());
                    }
                }
                _ => {}
            },
            Ok(Xml::Eof) | Err(_) => break,
            _ => {}
        }
    }
    map
}

/// (numId, level) -> is it a bullet?
fn parse_numbering(xml: &str) -> HashMap<(String, usize), bool> {
    let mut num_to_abstract: HashMap<String, String> = HashMap::new();
    let mut abstract_levels: HashMap<(String, usize), bool> = HashMap::new();
    let (mut cur_num, mut cur_abs, mut cur_lvl): (Option<String>, Option<String>, usize) = (None, None, 0);
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(Xml::Start(e)) | Ok(Xml::Empty(e)) => match local(e.name().as_ref()) {
                b"num" => cur_num = attr(&e, "numId"),
                b"abstractNum" => cur_abs = attr(&e, "abstractNumId"),
                b"abstractNumId" => {
                    if let (Some(n), Some(a)) = (&cur_num, attr(&e, "val")) {
                        num_to_abstract.insert(n.clone(), a);
                    }
                }
                b"lvl" => cur_lvl = attr(&e, "ilvl").and_then(|v| v.parse().ok()).unwrap_or(0),
                b"numFmt" => {
                    if let (Some(a), Some(f)) = (&cur_abs, attr(&e, "val")) {
                        abstract_levels.insert((a.clone(), cur_lvl), f == "bullet" || f == "none");
                    }
                }
                _ => {}
            },
            Ok(Xml::End(e)) => match local(e.name().as_ref()) {
                b"num" => cur_num = None,
                b"abstractNum" => cur_abs = None,
                _ => {}
            },
            Ok(Xml::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let mut out = HashMap::new();
    for ((abs, lvl), bullet) in abstract_levels {
        for (num, a) in &num_to_abstract {
            if *a == abs {
                out.insert((num.clone(), lvl), bullet);
            }
        }
    }
    out
}

/// relationship id -> target (for hyperlinks)
fn parse_rels(xml: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(Xml::Start(e)) | Ok(Xml::Empty(e)) if local(e.name().as_ref()) == b"Relationship" => {
                if let (Some(id), Some(t)) = (attr(&e, "Id"), attr(&e, "Target")) {
                    map.insert(id, t);
                }
            }
            Ok(Xml::Eof) | Err(_) => break,
            _ => {}
        }
    }
    map
}

fn docx_to_markdown(path: &std::path::Path) -> Result<String> {
    let file = File::open(path)?;
    let mut zip = ZipArchive::new(BufReader::new(file))
        .map_err(|e| Error::failed(format!("not a valid .docx (zip) file: {e}")))?;
    let document = read_part(&mut zip, "word/document.xml")
        .ok_or_else(|| Error::failed("not a Word document: word/document.xml is missing"))?;
    let styles = read_part(&mut zip, "word/styles.xml").map(|s| parse_styles(&s)).unwrap_or_default();
    let numbering = read_part(&mut zip, "word/numbering.xml").map(|s| parse_numbering(&s)).unwrap_or_default();
    let rels = read_part(&mut zip, "word/_rels/document.xml.rels").map(|s| parse_rels(&s)).unwrap_or_default();

    let blocks = parse_document(&document, &styles, &numbering, &rels);
    Ok(render_markdown(&blocks))
}

fn parse_document(
    xml: &str,
    styles: &HashMap<String, String>,
    numbering: &HashMap<(String, usize), bool>,
    rels: &HashMap<String, String>,
) -> Vec<Block> {
    let mut reader = Reader::from_str(xml);
    let mut blocks: Vec<Block> = Vec::new();

    // paragraph state
    let mut in_para = false;
    let mut para_style: Option<String> = None;
    let mut para_num: Option<(String, usize)> = None;
    let mut runs: Vec<Run> = Vec::new();
    // run state
    let mut run = Run::default();
    let mut in_run = false;
    let mut in_text = false;
    let mut link: Option<String> = None;
    // table state
    let mut table_depth = 0usize;
    let mut table: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut in_ppr = false;

    macro_rules! push_text {
        ($t:expr) => {
            run.text.push_str($t)
        };
    }

    loop {
        let ev = match reader.read_event() {
            Ok(ev) => ev,
            Err(_) => break,
        };
        match ev {
            Xml::Eof => break,
            Xml::Start(ref e) | Xml::Empty(ref e) => {
                let is_empty = matches!(ev, Xml::Empty(_));
                match local(e.name().as_ref()) {
                    b"p" => {
                        in_para = true;
                        para_style = None;
                        para_num = None;
                        runs.clear();
                    }
                    b"pPr" => in_ppr = true,
                    b"pStyle" if in_ppr => para_style = attr(e, "val"),
                    b"numId" if in_ppr => {
                        let id = attr(e, "val").unwrap_or_default();
                        if id != "0" {
                            let lvl = para_num.as_ref().map(|p| p.1).unwrap_or(0);
                            para_num = Some((id, lvl));
                        }
                    }
                    b"ilvl" if in_ppr => {
                        let lvl = attr(e, "val").and_then(|v| v.parse().ok()).unwrap_or(0);
                        let id = para_num.as_ref().map(|p| p.0.clone()).unwrap_or_default();
                        para_num = Some((id, lvl));
                    }
                    b"hyperlink" => {
                        link = attr(e, "id").and_then(|id| rels.get(&id).cloned());
                    }
                    b"r" if !is_empty => {
                        in_run = true;
                        run = Run { link: link.clone(), ..Run::default() };
                    }
                    b"b" if in_run && !in_ppr => run.bold = toggle(e),
                    b"i" if in_run && !in_ppr => run.italic = toggle(e),
                    b"strike" if in_run => run.strike = toggle(e),
                    b"rFonts" if in_run => {
                        let font = attr(e, "ascii").unwrap_or_default().to_lowercase();
                        if ["consolas", "courier new", "courier", "menlo", "monaco", "source code pro"].contains(&font.as_str()) {
                            run.code = true;
                        }
                    }
                    b"t" if in_run && !is_empty => in_text = true,
                    b"tab" if in_run => push_text!("\t"),
                    b"br" | b"cr" if in_run => push_text!("\n"),
                    b"tbl" => {
                        table_depth += 1;
                        if table_depth == 1 {
                            table.clear();
                        }
                    }
                    b"tr" if table_depth == 1 => row.clear(),
                    b"tc" if table_depth == 1 => cell.clear(),
                    _ => {}
                }
            }
            Xml::Text(t) if in_text => {
                if let Ok(s) = t.decode() {
                    push_text!(&s);
                }
            }
            Xml::GeneralRef(r) if in_text => {
                if let Ok(name) = r.decode() {
                    if let Some(s) = entity_text(&name) {
                        push_text!(&s);
                    }
                }
            }
            Xml::End(e) => match local(e.name().as_ref()) {
                b"t" => in_text = false,
                b"pPr" => in_ppr = false,
                b"r" => {
                    if in_run && !run.text.is_empty() {
                        runs.push(std::mem::take(&mut run));
                    }
                    in_run = false;
                }
                b"hyperlink" => link = None,
                b"p" => {
                    in_para = false;
                    if table_depth >= 1 {
                        let text: String = runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>().join("");
                        if table_depth == 1 {
                            if !cell.is_empty() && !text.is_empty() {
                                cell.push(' ');
                            }
                            cell.push_str(text.trim());
                        }
                    } else if runs.iter().any(|r| !r.text.trim().is_empty()) {
                        let name = para_style.as_ref().and_then(|s| styles.get(s)).cloned().unwrap_or_default();
                        let style_id = para_style.clone().unwrap_or_default().to_lowercase();
                        let heading = if name == "title" || style_id == "title" {
                            Some(1)
                        } else if name == "subtitle" {
                            Some(2)
                        } else if let Some(n) = name.strip_prefix("heading ").or_else(|| style_id.strip_prefix("heading")) {
                            n.trim().parse::<usize>().ok().map(|n| n.clamp(1, 6))
                        } else {
                            None
                        };
                        let taken = std::mem::take(&mut runs);
                        let is_code_style = name == "code block" || style_id == "codeblock";
                        if is_code_style {
                            let line: String = taken.iter().map(|r| r.text.as_str()).collect();
                            match blocks.last_mut() {
                                Some(Block::Code(existing)) => {
                                    existing.push('\n');
                                    existing.push_str(&line);
                                }
                                _ => blocks.push(Block::Code(line)),
                            }
                        } else if name == "quote" || name == "intense quote" || style_id == "quote" {
                            blocks.push(Block::Quote(taken));
                        } else if let Some(level) = heading {
                            blocks.push(Block::Heading(level, taken));
                        } else if let Some((id, level)) = &para_num {
                            let bullet = numbering.get(&(id.clone(), *level)).copied().unwrap_or(true);
                            blocks.push(Block::Item { ordered: !bullet, level: *level, runs: taken });
                        } else if name.starts_with("list bullet") || name.starts_with("list number") {
                            blocks.push(Block::Item { ordered: name.starts_with("list number"), level: 0, runs: taken });
                        } else {
                            blocks.push(Block::Para(taken));
                        }
                    }
                    runs.clear();
                }
                b"tc" if table_depth == 1 => row.push(std::mem::take(&mut cell)),
                b"tr" if table_depth == 1 => table.push(std::mem::take(&mut row)),
                b"tbl" => {
                    if table_depth == 1 && !table.is_empty() {
                        blocks.push(Block::Table(std::mem::take(&mut table)));
                    }
                    table_depth = table_depth.saturating_sub(1);
                }
                _ => {}
            },
            _ => {}
        }
        let _ = in_para;
    }
    blocks
}

fn escape_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Escape characters that would start a block at the beginning of a line.
fn escape_line_start(s: String) -> String {
    let starts_block = s.starts_with('#')
        || s.starts_with('+')
        || s.starts_with("- ")
        || s.starts_with("\\>")
        || s.split_once(". ").is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    if starts_block {
        format!("\\{s}")
    } else {
        s
    }
}

fn runs_to_md(runs: &[Run]) -> String {
    // merge neighbours that share formatting so we don't emit `**a****b**`
    let mut merged: Vec<Run> = Vec::new();
    for r in runs {
        match merged.last_mut() {
            Some(last)
                if last.bold == r.bold && last.italic == r.italic && last.strike == r.strike && last.code == r.code && last.link == r.link =>
            {
                last.text.push_str(&r.text)
            }
            _ => merged.push(r.clone()),
        }
    }
    let mut out = String::new();
    for r in merged {
        // emphasis markers must hug the text, so whitespace goes outside them
        let trimmed = r.text.trim();
        if trimmed.is_empty() {
            out.push_str(&r.text.replace('\n', " "));
            continue;
        }
        let lead = &r.text[..r.text.len() - r.text.trim_start().len()];
        let trail = &r.text[r.text.trim_end().len()..];
        let mut body = if r.code { format!("`{}`", trimmed.replace('`', "'")) } else { escape_md(trimmed).replace('\n', "  \n") };
        if !r.code {
            if r.strike {
                body = format!("~~{body}~~");
            }
            match (r.bold, r.italic) {
                (true, true) => body = format!("***{body}***"),
                (true, false) => body = format!("**{body}**"),
                (false, true) => body = format!("*{body}*"),
                _ => {}
            }
        }
        if let Some(url) = &r.link {
            body = format!("[{body}]({url})");
        }
        out.push_str(lead);
        out.push_str(&body);
        out.push_str(trail);
    }
    out.trim().to_string()
}

fn render_markdown(blocks: &[Block]) -> String {
    let mut out = String::new();
    let mut prev_was_item = false;
    for block in blocks {
        let is_item = matches!(block, Block::Item { .. });
        if !out.is_empty() {
            out.push_str(if is_item && prev_was_item { "\n" } else { "\n\n" });
        }
        match block {
            Block::Heading(level, runs) => {
                out.push_str(&"#".repeat(*level));
                out.push(' ');
                out.push_str(&runs_to_md(runs));
            }
            Block::Para(runs) => out.push_str(&escape_line_start(runs_to_md(runs))),
            Block::Quote(runs) => {
                out.push_str("> ");
                out.push_str(&runs_to_md(runs));
            }
            Block::Code(text) => {
                out.push_str("```\n");
                out.push_str(text);
                out.push_str("\n```");
            }
            Block::Item { ordered, level, runs } => {
                out.push_str(&"  ".repeat(*level));
                out.push_str(if *ordered { "1. " } else { "- " });
                out.push_str(&runs_to_md(runs));
            }
            Block::Table(rows) => {
                let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0).max(1);
                let cell = |s: &str| escape_md(s).replace('|', "\\|").replace('\n', " ");
                for (i, r) in rows.iter().enumerate() {
                    let cells: Vec<String> = (0..cols).map(|c| cell(r.get(c).map(String::as_str).unwrap_or(""))).collect();
                    out.push_str(&format!("| {} |\n", cells.join(" | ")));
                    if i == 0 {
                        out.push_str(&format!("|{}|\n", " --- |".repeat(cols)));
                    }
                }
                while out.ends_with('\n') {
                    out.pop();
                }
            }
        }
        prev_was_item = is_item;
    }
    out.push('\n');
    out
}

// ═══════════════════════════ writing ═══════════════════════════

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[derive(Default)]
struct Writer {
    body: String,
    para_props: String,
    para_content: String,
    para_open: bool,
    // inline formatting depth
    bold: u32,
    italic: u32,
    strike: u32,
    // lists: (ordered, numId)
    lists: Vec<(bool, u32)>,
    next_num_id: u32,
    extra_nums: String,
    quote_depth: u32,
    in_code_block: bool,
    // tables
    in_table: bool,
    table_rows: String,
    row_cells: String,
    cell_content: String,
    in_header_row: bool,
    // links
    links: Vec<String>,
    // image alt capture
    image_alt: Option<String>,
}

impl Writer {
    fn run(&self, text: &str, code: bool) -> String {
        let mut props = String::new();
        if code {
            props.push_str("<w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\" w:cs=\"Consolas\"/><w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"EEEEEE\"/>");
        }
        if self.bold > 0 || self.in_header_row {
            props.push_str("<w:b/>");
        }
        if self.italic > 0 {
            props.push_str("<w:i/>");
        }
        if self.strike > 0 {
            props.push_str("<w:strike/>");
        }
        let rpr = if props.is_empty() { String::new() } else { format!("<w:rPr>{props}</w:rPr>") };
        format!("<w:r>{rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r>", xml_escape(text))
    }

    fn add_text(&mut self, text: &str, code: bool) {
        if let Some(alt) = &mut self.image_alt {
            alt.push_str(text);
            return;
        }
        let r = self.run(text, code);
        self.push_content(&r);
    }

    fn push_content(&mut self, xml: &str) {
        if self.in_table {
            self.cell_content.push_str(xml);
        } else {
            if !self.para_open {
                self.begin_para("");
            }
            self.para_content.push_str(xml);
        }
    }

    fn begin_para(&mut self, props: &str) {
        self.flush_para();
        self.para_open = true;
        self.para_props = props.to_string();
    }

    fn flush_para(&mut self) {
        if self.para_open {
            let ppr = if self.para_props.is_empty() { String::new() } else { format!("<w:pPr>{}</w:pPr>", self.para_props) };
            self.body.push_str(&format!("<w:p>{ppr}{}</w:p>", self.para_content));
            self.para_open = false;
            self.para_content.clear();
            self.para_props.clear();
        }
    }

    /// Paragraph properties for plain text in the current context (quote/list).
    fn context_props(&self) -> String {
        let mut p = String::new();
        if let Some(&(ordered, num_id)) = self.lists.last() {
            let level = self.lists.len() - 1;
            let _ = ordered;
            p.push_str(&format!(
                "<w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"{}\"/><w:numId w:val=\"{num_id}\"/></w:numPr>",
                level.min(5)
            ));
        } else if self.quote_depth > 0 {
            p.push_str("<w:pStyle w:val=\"Quote\"/>");
        }
        p
    }
}

fn markdown_to_docx(md: &str) -> Result<Vec<u8>> {
    let options = MdOptions::ENABLE_TABLES | MdOptions::ENABLE_STRIKETHROUGH | MdOptions::ENABLE_TASKLISTS;
    let mut w = Writer { next_num_id: 2, ..Writer::default() };
    let mut item_pending = false; // a list item just started; its first paragraph carries the bullet

    for ev in Parser::new_ext(md, options) {
        match ev {
            Event::Start(Tag::Paragraph) => {
                if item_pending {
                    item_pending = false; // paragraph already opened by the item
                } else if !w.in_table {
                    let props = w.context_props();
                    w.begin_para(&props);
                }
            }
            Event::End(TagEnd::Paragraph) => {
                if !w.in_table && w.lists.is_empty() {
                    w.flush_para();
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let n = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                w.begin_para(&format!("<w:pStyle w:val=\"Heading{n}\"/>"));
            }
            Event::End(TagEnd::Heading(_)) => w.flush_para(),
            Event::Start(Tag::BlockQuote(_)) => {
                w.flush_para();
                w.quote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                w.flush_para();
                w.quote_depth = w.quote_depth.saturating_sub(1);
            }
            Event::Start(Tag::List(first)) => {
                w.flush_para();
                let ordered = first.is_some();
                let num_id = if ordered {
                    let id = w.next_num_id;
                    w.next_num_id += 1;
                    w.extra_nums.push_str(&format!(
                        "<w:num w:numId=\"{id}\"><w:abstractNumId w:val=\"1\"/><w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"{}\"/></w:lvlOverride></w:num>",
                        first.unwrap_or(1)
                    ));
                    id
                } else {
                    1
                };
                w.lists.push((ordered, num_id));
            }
            Event::End(TagEnd::List(_)) => {
                w.flush_para();
                w.lists.pop();
            }
            Event::Start(Tag::Item) => {
                let props = w.context_props();
                w.begin_para(&props);
                item_pending = true;
            }
            Event::End(TagEnd::Item) => {
                w.flush_para();
                item_pending = false;
            }
            Event::TaskListMarker(done) => w.add_text(if done { "☑ " } else { "☐ " }, false),
            Event::Start(Tag::CodeBlock(_)) => {
                w.flush_para();
                w.in_code_block = true;
            }
            Event::End(TagEnd::CodeBlock) => w.in_code_block = false,
            Event::Start(Tag::Emphasis) => w.italic += 1,
            Event::End(TagEnd::Emphasis) => w.italic = w.italic.saturating_sub(1),
            Event::Start(Tag::Strong) => w.bold += 1,
            Event::End(TagEnd::Strong) => w.bold = w.bold.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => w.strike += 1,
            Event::End(TagEnd::Strikethrough) => w.strike = w.strike.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) => {
                w.links.push(dest_url.to_string());
                let id = w.links.len();
                w.push_content(&format!("<w:hyperlink r:id=\"rIdL{id}\" w:history=\"1\">"));
            }
            Event::End(TagEnd::Link) => w.push_content("</w:hyperlink>"),
            Event::Start(Tag::Image { .. }) => w.image_alt = Some(String::new()),
            Event::End(TagEnd::Image) => {
                if let Some(alt) = w.image_alt.take() {
                    w.add_text(&format!("[image: {alt}]"), false);
                }
            }
            Event::Start(Tag::Table(_)) => {
                w.flush_para();
                w.in_table = true;
                w.table_rows.clear();
            }
            Event::End(TagEnd::Table) => {
                w.in_table = false;
                let tbl = format!(
                    "<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr>{}</w:tbl><w:p/>",
                    w.table_rows
                );
                w.body.push_str(&tbl);
            }
            Event::Start(Tag::TableHead) => {
                w.in_header_row = true;
                w.row_cells.clear();
            }
            Event::End(TagEnd::TableHead) => {
                w.table_rows.push_str(&format!("<w:tr>{}</w:tr>", w.row_cells));
                w.in_header_row = false;
            }
            Event::Start(Tag::TableRow) => w.row_cells.clear(),
            Event::End(TagEnd::TableRow) => w.table_rows.push_str(&format!("<w:tr>{}</w:tr>", w.row_cells)),
            Event::Start(Tag::TableCell) => w.cell_content.clear(),
            Event::End(TagEnd::TableCell) => {
                let cell = format!("<w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr><w:p>{}</w:p></w:tc>", w.cell_content);
                w.row_cells.push_str(&cell);
            }
            Event::Text(t) => {
                if w.in_code_block {
                    for line in t.trim_end_matches('\n').split('\n') {
                        let r = w.run(line, true);
                        w.body.push_str(&format!("<w:p><w:pPr><w:pStyle w:val=\"CodeBlock\"/></w:pPr>{r}</w:p>"));
                    }
                } else {
                    w.add_text(&t, false);
                }
            }
            Event::Code(t) => w.add_text(&t, true),
            Event::Html(t) | Event::InlineHtml(t) => w.add_text(&t, false),
            Event::SoftBreak => w.add_text(" ", false),
            Event::HardBreak => w.push_content("<w:r><w:br/></w:r>"),
            Event::Rule => {
                w.flush_para();
                w.body.push_str("<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"999999\"/></w:pBdr></w:pPr></w:p>");
            }
            _ => {}
        }
    }
    w.flush_para();

    package(&w)
}

const NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";

fn package(w: &Writer) -> Result<Vec<u8>> {
    let zip_err = |e: zip::result::ZipError| Error::failed(format!("could not build the .docx: {e}"));
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut add = |name: &str, content: &str| -> Result<()> {
        zip.start_file(name, opts).map_err(zip_err)?;
        zip.write_all(content.as_bytes())?;
        Ok(())
    };

    add(
        "[Content_Types].xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/><Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/><Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/></Types>",
    )?;
    add(
        "_rels/.rels",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>",
    )?;

    let mut rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rIdS\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/><Relationship Id=\"rIdN\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/>",
    );
    for (i, url) in w.links.iter().enumerate() {
        rels.push_str(&format!(
            "<Relationship Id=\"rIdL{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"{}\" TargetMode=\"External\"/>",
            i + 1,
            xml_escape(url)
        ));
    }
    rels.push_str("</Relationships>");
    add("word/_rels/document.xml.rels", &rels)?;

    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:document {NS}><w:body>{}<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>",
        w.body
    );
    add("word/document.xml", &document)?;
    add("word/styles.xml", &styles_xml())?;
    add("word/numbering.xml", &numbering_xml(&w.extra_nums))?;

    let cursor = zip.finish().map_err(zip_err)?;
    Ok(cursor.into_inner())
}

fn styles_xml() -> String {
    let heading = |n: usize, size: u32| {
        format!(
            "<w:style w:type=\"paragraph\" w:styleId=\"Heading{n}\"><w:name w:val=\"heading {n}\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before=\"{}\" w:after=\"120\"/><w:outlineLvl w:val=\"{}\"/></w:pPr><w:rPr><w:b/><w:color w:val=\"1F2A44\"/><w:sz w:val=\"{size}\"/><w:szCs w:val=\"{size}\"/></w:rPr></w:style>",
            if n == 1 { 360 } else { 240 },
            n - 1
        )
    };
    let mut s = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles {NS}><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"Calibri\" w:hAnsi=\"Calibri\" w:eastAsia=\"Calibri\" w:cs=\"Calibri\"/><w:sz w:val=\"22\"/><w:szCs w:val=\"22\"/><w:lang w:val=\"en-US\"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after=\"120\" w:line=\"276\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>"
    );
    for (n, size) in [(1, 36), (2, 30), (3, 26), (4, 24), (5, 22), (6, 22)] {
        s.push_str(&heading(n, size));
    }
    s.push_str(
        "<w:style w:type=\"paragraph\" w:styleId=\"ListParagraph\"><w:name w:val=\"List Paragraph\"/><w:basedOn w:val=\"Normal\"/><w:qFormat/><w:pPr><w:spacing w:after=\"40\"/><w:contextualSpacing/></w:pPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Quote\"><w:name w:val=\"Quote\"/><w:basedOn w:val=\"Normal\"/><w:qFormat/><w:pPr><w:pBdr><w:left w:val=\"single\" w:sz=\"18\" w:space=\"8\" w:color=\"BBBBBB\"/></w:pBdr><w:ind w:left=\"360\"/></w:pPr><w:rPr><w:i/><w:color w:val=\"555555\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"CodeBlock\"><w:name w:val=\"Code Block\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F3F3F3\"/><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr><w:rPr><w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\" w:cs=\"Consolas\"/><w:sz w:val=\"20\"/></w:rPr></w:style>\
<w:style w:type=\"table\" w:styleId=\"TableGrid\"><w:name w:val=\"Table Grid\"/><w:tblPr><w:tblBorders><w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/><w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/><w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/><w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/><w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/><w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/></w:tblBorders><w:tblCellMar><w:left w:w=\"100\" w:type=\"dxa\"/><w:right w:w=\"100\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr></w:style>\
</w:styles>",
    );
    s
}

fn numbering_xml(extra_nums: &str) -> String {
    let levels = |bullet: bool| -> String {
        (0..6)
            .map(|i| {
                let (fmt, text) = if bullet {
                    ("bullet".to_string(), ["•", "◦", "▪"][i % 3].to_string())
                } else {
                    (["decimal", "lowerLetter", "lowerRoman"][i % 3].to_string(), format!("%{}.", i + 1))
                };
                format!(
                    "<w:lvl w:ilvl=\"{i}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{fmt}\"/><w:lvlText w:val=\"{text}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{}\" w:hanging=\"360\"/></w:pPr></w:lvl>",
                    720 * (i + 1)
                )
            })
            .collect()
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:numbering {NS}><w:abstractNum w:abstractNumId=\"0\"><w:multiLevelType w:val=\"hybridMultilevel\"/>{}</w:abstractNum><w:abstractNum w:abstractNumId=\"1\"><w:multiLevelType w:val=\"hybridMultilevel\"/>{}</w:abstractNum><w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>{extra_nums}</w:numbering>",
        levels(true),
        levels(false)
    )
}
