//! Pure MIME parsing: raw RFC822 bytes -> structured email (headers, text body,
//! attachment metadata + bytes). Wraps `mail-parser`. No network.

use mail_parser::{MessageParser, MimeHeaders};

/// Metadata for one attachment. `bytes` carries the decoded content so the node
/// can register it as a Colmena attachment when download is requested.
#[derive(Debug, Clone, PartialEq)]
pub struct AttachmentInfo {
    pub filename: String,
    pub mime: String,
    pub size: usize,
    pub bytes: Vec<u8>,
}

/// Structured result of parsing one email.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEmail {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub date: String,
    pub body_text: String,
    pub body_truncated: bool,
    pub attachments: Vec<AttachmentInfo>,
}

/// Parse raw RFC822 bytes. Prefers text/plain; falls back to HTML converted to
/// text. Truncates the body to `body_max_bytes` (UTF-8 safe), setting
/// `body_truncated`. Returns Err on unparseable input.
pub fn parse_email(raw: &[u8], body_max_bytes: usize) -> Result<ParsedEmail, String> {
    let msg = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| "imap_read: failed to parse message".to_string())?;

    let from = msg
        .from()
        .and_then(|a| a.first())
        .and_then(|addr| addr.address())
        .unwrap_or("")
        .to_string();
    let to = msg
        .to()
        .and_then(|a| a.first())
        .and_then(|addr| addr.address())
        .unwrap_or("")
        .to_string();
    let subject = msg.subject().unwrap_or("").to_string();
    let date = msg.date().map(|d| d.to_rfc3339()).unwrap_or_default();

    let raw_body: String = match msg.body_text(0) {
        Some(t) => t.into_owned(),
        None => match msg.body_html(0) {
            Some(h) => strip_html(&h),
            None => String::new(),
        },
    };
    let (body_text, body_truncated) = truncate_utf8(&raw_body, body_max_bytes);

    let mut attachments = Vec::new();
    for part in msg.attachments() {
        let bytes = part.contents().to_vec();
        let filename = part.attachment_name().unwrap_or("attachment").to_string();
        let mime = part
            .content_type()
            .map(|ct| match ct.subtype() {
                Some(st) => format!("{}/{}", ct.ctype(), st),
                None => ct.ctype().to_string(),
            })
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let size = bytes.len();
        attachments.push(AttachmentInfo {
            filename,
            mime,
            size,
            bytes,
        });
    }

    Ok(ParsedEmail {
        from,
        to,
        subject,
        date,
        body_text,
        body_truncated,
        attachments,
    })
}

/// Truncate to at most `max` bytes without splitting a UTF-8 char boundary.
fn truncate_utf8(s: &str, max: usize) -> (String, bool) {
    if s.len() <= max {
        return (s.to_string(), false);
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

/// Minimal HTML-to-text: drop tags, collapse whitespace.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: &[u8] = b"From: alice@example.com\r\nTo: bob@example.com\r\nSubject: Hello\r\nDate: Mon, 01 Jun 2026 10:00:00 +0000\r\nContent-Type: text/plain\r\n\r\nThis is the body.";

    #[test]
    fn parses_plain_text_email() {
        let p = parse_email(PLAIN, 5120).unwrap();
        assert_eq!(p.from, "alice@example.com");
        assert_eq!(p.to, "bob@example.com");
        assert_eq!(p.subject, "Hello");
        assert!(p.body_text.contains("This is the body."));
        assert!(!p.body_truncated);
        assert!(p.attachments.is_empty());
    }

    #[test]
    fn truncates_long_body() {
        let mut raw = b"Subject: x\r\nContent-Type: text/plain\r\n\r\n".to_vec();
        raw.extend(std::iter::repeat(b'a').take(10_000));
        let p = parse_email(&raw, 100).unwrap();
        assert!(p.body_truncated);
        assert!(p.body_text.len() <= 100);
    }

    #[test]
    fn extracts_attachment_metadata_and_bytes() {
        let raw = b"Subject: with attach\r\nContent-Type: multipart/mixed; boundary=BB\r\n\r\n--BB\r\nContent-Type: text/plain\r\n\r\nbody here\r\n--BB\r\nContent-Type: text/plain; name=\"note.txt\"\r\nContent-Disposition: attachment; filename=\"note.txt\"\r\n\r\nFILEDATA\r\n--BB--\r\n";
        let p = parse_email(raw, 5120).unwrap();
        assert!(p.body_text.contains("body here"));
        assert_eq!(p.attachments.len(), 1);
        assert_eq!(p.attachments[0].filename, "note.txt");
        assert_eq!(p.attachments[0].bytes, b"FILEDATA");
        assert_eq!(p.attachments[0].size, 8);
    }

    #[test]
    fn html_only_is_stripped_to_text() {
        let raw = b"Subject: h\r\nContent-Type: text/html\r\n\r\n<html><body><p>Hello <b>world</b></p></body></html>";
        let p = parse_email(raw, 5120).unwrap();
        assert!(p.body_text.contains("Hello"));
        assert!(p.body_text.contains("world"));
        assert!(!p.body_text.contains("<"));
    }

    #[test]
    fn strip_html_collapses_whitespace() {
        assert_eq!(strip_html("<p>a</p>\n  <p>b</p>"), "a b");
    }
}
