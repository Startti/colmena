//! Classifies an HTTP response body that is not JSON: is it a file the
//! `http_request` node should keep as a session attachment, and under which
//! MIME type and filename?
//!
//! The decision trusts the bytes before the headers. Some APIs send a list of
//! types in `Content-Type` (`application/json;charset=utf-8,application/pdf`),
//! so a PDF body can arrive labelled as JSON first. Magic bytes settle the
//! common formats; the header is the fallback for the rest (Office documents,
//! which are ZIP containers, or audio/video without a known signature).

/// The MIME type of a binary file body, or `None` when the body is not a
/// file the node should keep (text, HTML, an empty body, an unknown type).
pub fn file_mime(content_type: Option<&str>, bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    if let Some(mime) = sniff(bytes) {
        // A ZIP signature is also every Office document: let the header say which.
        if mime == "application/zip" {
            if let Some(declared) = declared_file_type(content_type) {
                return Some(declared);
            }
        }
        return Some(mime.to_string());
    }
    declared_file_type(content_type)
}

/// A filename for the stored file: the `Content-Disposition` filename when it
/// has one, else the last segment of the URL path, else `"download"`. The
/// extension for `mime` is appended when the name lacks one.
pub fn filename(content_disposition: Option<&str>, url: &str, mime: &str) -> String {
    let raw = content_disposition
        .and_then(disposition_filename)
        .or_else(|| url_basename(url))
        .unwrap_or_else(|| "download".to_string());
    let name: String = raw
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .take(120)
        .collect();
    let name = if name.trim().is_empty() {
        "download".to_string()
    } else {
        name
    };
    match extension(mime) {
        Some(ext) if !name.contains('.') => format!("{name}.{ext}"),
        _ => name,
    }
}

fn sniff(b: &[u8]) -> Option<&'static str> {
    let starts = |sig: &[u8]| b.starts_with(sig);
    if starts(b"%PDF-") {
        Some("application/pdf")
    } else if starts(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if starts(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WAVE" {
        Some("audio/wav")
    } else if starts(b"ID3") {
        Some("audio/mpeg")
    } else if starts(b"OggS") {
        Some("audio/ogg")
    } else if b.len() >= 8 && &b[4..8] == b"ftyp" {
        Some("video/mp4")
    } else if starts(b"PK\x03\x04") {
        Some("application/zip")
    } else {
        None
    }
}

/// The first type in a (possibly comma-separated) `Content-Type` that names a
/// file rather than text or JSON.
fn declared_file_type(content_type: Option<&str>) -> Option<String> {
    content_type?
        .split(',')
        .map(|part| {
            part.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .find(|t| is_file_type(t))
}

fn is_file_type(t: &str) -> bool {
    t == "application/pdf"
        || t == "application/octet-stream"
        || t == "application/zip"
        || t.starts_with("application/vnd.openxmlformats-officedocument.")
        || t == "application/msword"
        || t == "application/vnd.ms-excel"
        || t.starts_with("image/")
        || t.starts_with("audio/")
        || t.starts_with("video/")
}

fn disposition_filename(cd: &str) -> Option<String> {
    cd.split(';').find_map(|p| {
        let (k, v) = p.trim().split_once('=')?;
        if !k.trim().eq_ignore_ascii_case("filename") {
            return None;
        }
        let v = v.trim().trim_matches('"').trim();
        (!v.is_empty()).then(|| v.to_string())
    })
}

fn url_basename(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let path = path.split_once("://").map_or(path, |(_, rest)| rest);
    let (_, path) = path.split_once('/')?;
    let last = path.trim_end_matches('/').rsplit('/').next()?;
    (!last.is_empty()).then(|| last.to_string())
}

fn extension(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "application/pdf" => "pdf",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "audio/wav" => "wav",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "video/mp4" => "mp4",
        "application/zip" => "zip",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_body_wins_over_a_json_first_content_type() {
        let ct = "application/json;charset=utf-8,application/pdf;charset=utf-8";
        assert_eq!(
            file_mime(Some(ct), b"%PDF-1.4\n...").as_deref(),
            Some("application/pdf")
        );
    }

    #[test]
    fn text_and_html_are_not_files() {
        assert_eq!(file_mime(Some("text/html"), b"<html></html>"), None);
        assert_eq!(file_mime(Some("text/plain"), b"hola"), None);
        assert_eq!(file_mime(None, b"plain words"), None);
        assert_eq!(file_mime(Some("application/pdf"), b""), None);
    }

    #[test]
    fn header_names_the_type_when_bytes_have_no_signature() {
        assert_eq!(
            file_mime(Some("application/octet-stream"), b"\x00\x01\x02").as_deref(),
            Some("application/octet-stream")
        );
    }

    #[test]
    fn zip_signature_defers_to_a_declared_office_type() {
        let xlsx = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
        assert_eq!(
            file_mime(Some(xlsx), b"PK\x03\x04rest").as_deref(),
            Some(xlsx)
        );
        assert_eq!(
            file_mime(None, b"PK\x03\x04rest").as_deref(),
            Some("application/zip")
        );
    }

    #[test]
    fn filename_prefers_disposition_then_url_and_adds_extension() {
        assert_eq!(
            filename(
                Some(r#"inline; filename="voucher_123.pdf""#),
                "https://x/a",
                "application/pdf"
            ),
            "voucher_123.pdf"
        );
        assert_eq!(
            filename(
                None,
                "https://api.example.com/v3/reservations/123/voucher?x=1",
                "application/pdf"
            ),
            "voucher.pdf"
        );
        assert_eq!(
            filename(None, "https://api.example.com", "image/png"),
            "download.png"
        );
    }

    #[test]
    fn filename_drops_path_separators_from_the_header() {
        assert_eq!(
            filename(
                Some(r#"attachment; filename="../../etc/passwd""#),
                "https://x/y",
                "application/octet-stream"
            ),
            "....etcpasswd"
        );
    }
}
