//! Pure-Rust JSON syntax highlighter.
//!
//! Takes an already-pretty-printed JSON string and wraps its tokens
//! in classed `<span>`s so a stylesheet can colour them. We tokenize
//! the existing string (rather than re-emit from a `serde_json::Value`)
//! so indentation and trailing whitespace from
//! `serde_json::to_string_pretty` survive unchanged. Strings that
//! aren't valid JSON pass through HTML-escaped with no highlighting.
//!
//! Token classes mirror Prism's defaults so authors with custom CSS
//! can drop in a Prism-compatible JSON theme without translation:
//!   .tok-string   — string literal values
//!   .tok-key      — string literals that occupy the key position
//!   .tok-number   — numeric literals
//!   .tok-keyword  — `true` / `false` / `null`
//!   .tok-punct    — structural punctuation (`{}[],:`)

/// Highlight `src` (a JSON document) as classed HTML spans. Returns
/// HTML safe for `{{ … | safe }}` insertion — the caller has
/// pre-promised that `src` came from our own JSON serializer.
pub fn highlight_json(src: &str) -> String {
    if !is_likely_json(src) {
        return html_escape(src);
    }
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len() * 2);
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'"' => {
                let (text, end) = scan_string(bytes, i);
                let is_key = peek_is_colon(bytes, end);
                let cls = if is_key { "tok-key" } else { "tok-string" };
                push_span(&mut out, cls, &html_escape(text));
                i = end;
            }
            b't' if starts_with(bytes, i, b"true") => {
                push_span(&mut out, "tok-keyword", "true");
                i += 4;
            }
            b'f' if starts_with(bytes, i, b"false") => {
                push_span(&mut out, "tok-keyword", "false");
                i += 5;
            }
            b'n' if starts_with(bytes, i, b"null") => {
                push_span(&mut out, "tok-keyword", "null");
                i += 4;
            }
            b'-' | b'0'..=b'9' => {
                let end = scan_number(bytes, i);
                push_span(&mut out, "tok-number", &src[i..end]);
                i = end;
            }
            b'{' | b'}' | b'[' | b']' | b',' | b':' => {
                push_span(&mut out, "tok-punct", &(b as char).to_string());
                i += 1;
            }
            b'<' | b'>' | b'&' => {
                // Shouldn't happen inside well-formed JSON outside of
                // strings, but stay defensive.
                out.push_str(&html_escape_char(b as char));
                i += 1;
            }
            _ => {
                // Whitespace and anything else: emit verbatim.
                out.push(b as char);
                i += 1;
            }
        }
    }
    out
}

/// Cheap check that the text *might* be JSON. We don't want to wrap
/// arbitrary content (e.g. a urlencoded `serializedValue` from the
/// spec) in token spans that would mis-render it.
fn is_likely_json(src: &str) -> bool {
    let t = src.trim_start();
    t.starts_with('{') || t.starts_with('[') || t.starts_with('"') || {
        let head: String = t.chars().take(8).collect();
        head.starts_with("true")
            || head.starts_with("false")
            || head.starts_with("null")
            || head
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit() || c == '-')
    }
}

fn scan_string(bytes: &[u8], start: usize) -> (&str, usize) {
    // start points at the opening quote.
    let mut j = start + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => {
                j += 2; // skip the escaped char too
            }
            b'"' => {
                j += 1;
                break;
            }
            _ => j += 1,
        }
    }
    // SAFETY: the scan only steps over ASCII delimiters; any
    // multi-byte UTF-8 inside the string stays intact. We slice on
    // byte indices that align with the boundaries we walked past.
    let text = std::str::from_utf8(&bytes[start..j]).unwrap_or("");
    (text, j)
}

fn scan_number(bytes: &[u8], start: usize) -> usize {
    let mut j = start + 1;
    while j < bytes.len() {
        let c = bytes[j];
        if c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-') {
            j += 1;
        } else {
            break;
        }
    }
    j
}

fn peek_is_colon(bytes: &[u8], from: usize) -> bool {
    let mut k = from;
    while k < bytes.len()
        && (bytes[k] == b' ' || bytes[k] == b'\t' || bytes[k] == b'\n' || bytes[k] == b'\r')
    {
        k += 1;
    }
    k < bytes.len() && bytes[k] == b':'
}

fn starts_with(bytes: &[u8], i: usize, needle: &[u8]) -> bool {
    bytes[i..].starts_with(needle)
}

fn push_span(out: &mut String, class: &str, text: &str) {
    out.push_str("<span class=\"");
    out.push_str(class);
    out.push_str("\">");
    out.push_str(text);
    out.push_str("</span>");
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            _ => out.push(c),
        }
    }
    out
}

fn html_escape_char(c: char) -> String {
    match c {
        '<' => "&lt;".into(),
        '>' => "&gt;".into(),
        '&' => "&amp;".into(),
        _ => c.to_string(),
    }
}
