use std::fs;

use pulldown_cmark::{html, Event, Options as MdOptions, Parser, Tag, TagEnd};
use uberconv_core::{Context, Converter, Cost, Edge, Error, Format, Job, Result};

pub struct TextConverter;

impl Converter for TextConverter {
    fn name(&self) -> &'static str {
        "text"
    }

    fn edges(&self) -> Vec<Edge> {
        use Format::*;
        vec![
            Edge::new(Markdown, Html, Cost::lossless(5)),
            Edge::new(Markdown, Text, Cost::lossy(5)),
            Edge::new(Text, Markdown, Cost::lossless(2)),
            Edge::new(Text, Html, Cost::lossless(5)),
            Edge::new(Html, Text, Cost::lossy(5)),
        ]
    }

    fn convert(&self, job: &Job<'_>, ctx: &Context) -> Result<()> {
        let input = fs::read_to_string(job.input)
            .map_err(|e| Error::failed(format!("could not read {} as UTF-8 text: {e}", job.input.display())))?;
        ctx.report(0.4);
        let out = match (job.from, job.to) {
            (Format::Markdown, Format::Html) => wrap_html(&markdown_to_html(&input), job),
            (Format::Markdown, Format::Text) => markdown_to_text(&input),
            (Format::Text, Format::Markdown) => input,
            (Format::Text, Format::Html) => wrap_html(&format!("<pre>{}</pre>", escape(&input)), job),
            (Format::Html, Format::Text) => html_to_text(&input),
            (f, t) => return Err(Error::failed(format!("text converter can't do {f} -> {t}"))),
        };
        fs::write(job.output, out)?;
        ctx.report(1.0);
        Ok(())
    }
}

fn md_options() -> MdOptions {
    MdOptions::ENABLE_TABLES
        | MdOptions::ENABLE_STRIKETHROUGH
        | MdOptions::ENABLE_TASKLISTS
        | MdOptions::ENABLE_FOOTNOTES
}

fn markdown_to_html(md: &str) -> String {
    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(md, md_options()));
    out
}

fn wrap_html(body: &str, job: &Job<'_>) -> String {
    let title = job
        .input
        .file_stem()
        .and_then(|s| s.to_str())
        .map(escape)
        .unwrap_or_default();
    format!(
        "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{title}</title>\n<style>body{{font-family:system-ui,sans-serif;max-width:46rem;margin:2rem auto;padding:0 1rem;line-height:1.6}}pre,code{{background:#f4f4f4;border-radius:4px}}pre{{padding:.75rem;overflow:auto}}</style>\n</head>\n<body>\n{body}</body>\n</html>\n"
    )
}

fn markdown_to_text(md: &str) -> String {
    let mut out = String::new();
    for ev in Parser::new_ext(md, md_options()) {
        match ev {
            Event::Text(t) | Event::Code(t) => out.push_str(&t),
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            Event::Start(Tag::Item) => out.push_str("- "),
            Event::End(TagEnd::Paragraph) | Event::End(TagEnd::Heading(_)) | Event::End(TagEnd::CodeBlock) => {
                out.push_str("\n\n")
            }
            Event::End(TagEnd::Item) | Event::End(TagEnd::TableRow) => out.push('\n'),
            Event::End(TagEnd::TableCell) => out.push('\t'),
            Event::Rule => out.push_str("----\n\n"),
            _ => {}
        }
    }
    out.trim_end().to_string() + "\n"
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Small tag stripper: drops <script>/<style>, turns block tags into line
/// breaks and decodes the common entities. Not a layout engine.
fn html_to_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let Some(end_rel) = html[i..].find('>') else { break };
            let tag = lower[i + 1..i + end_rel].trim_start_matches('/').to_string();
            let name: String = tag.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            let skipped = i + end_rel + 1;
            if (name == "script" || name == "style") && !lower[i + 1..].starts_with('/') {
                let close = format!("</{name}");
                i = lower[skipped..].find(&close).map(|p| skipped + p).unwrap_or(bytes.len());
                if let Some(g) = html[i.min(bytes.len())..].find('>') {
                    i += g + 1;
                }
                continue;
            }
            if matches!(name.as_str(), "p" | "div" | "br" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            i = skipped;
        } else {
            let next = html[i..].find('<').map(|p| i + p).unwrap_or(bytes.len());
            out.push_str(&html[i..next]);
            i = next;
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let mut lines: Vec<&str> = decoded.lines().map(|l| l.trim()).collect();
    lines.dedup_by(|a, b| a.is_empty() && b.is_empty());
    lines.join("\n").trim().to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_variants() {
        assert!(markdown_to_html("# Hi\n\n*x*").contains("<h1>Hi</h1>"));
        let t = markdown_to_text("# Title\n\nsome *text*\n\n- a\n- b\n");
        assert!(t.contains("Title\n\nsome text"));
        assert!(t.contains("- a\n- b"));
    }

    #[test]
    fn html_strip() {
        let t = html_to_text("<style>p{}</style><h1>A &amp; B</h1><p>one</p><script>x()</script><p>two</p>");
        assert_eq!(t, "A & B\none\ntwo\n");
    }
}
