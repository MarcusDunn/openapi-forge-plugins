//! CommonMark -> safe HTML, used everywhere the spec exposes a
//! CommonMark slot (operation/parameter/schema/response descriptions,
//! info.description, tag.description, etc).

use pulldown_cmark::{html, Options, Parser};

pub fn render(src: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_SMART_PUNCTUATION);
    let parser = Parser::new_ext(src, opts);
    let mut out = String::with_capacity(src.len() + src.len() / 4);
    html::push_html(&mut out, parser);
    out
}

pub fn render_opt(src: Option<&str>) -> Option<String> {
    src.map(render)
}

/// Plain-text first paragraph, used for `<meta name="description">`.
pub fn first_paragraph_text(src: &str) -> String {
    use pulldown_cmark::Event;
    let parser = Parser::new(src);
    let mut buf = String::new();
    let mut in_para = false;
    for ev in parser {
        match ev {
            Event::Start(pulldown_cmark::Tag::Paragraph) => in_para = true,
            Event::End(pulldown_cmark::TagEnd::Paragraph) => {
                if !buf.is_empty() {
                    break;
                }
                in_para = false;
            }
            Event::Text(t) if in_para => buf.push_str(&t),
            Event::Code(t) if in_para => buf.push_str(&t),
            Event::SoftBreak | Event::HardBreak if in_para => buf.push(' '),
            _ => {}
        }
    }
    buf
}
