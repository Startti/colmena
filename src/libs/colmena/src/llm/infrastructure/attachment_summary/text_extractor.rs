//! Local text extraction from document bytes, plus a multi-byte-safe
//! char truncator. Used to feed the summary generator with at most
//! `summary_max_chars` of input text.

use thiserror::Error;

/// Error type for local text extraction.
///
/// `Ok(None)` (returned by the dispatcher) means the MIME is recognised
/// but not text-extractable (e.g. images, archives). `Err` is reserved
/// for malformed input of a supported MIME (corrupt PDF, invalid UTF-8).
#[derive(Debug, Error)]
pub enum ExtractError {
    #[error("pdf parse failure: {0}")]
    PdfParse(String),

    #[error("invalid UTF-8 text: {0}")]
    InvalidUtf8(String),
}

/// Dispatcher: given a MIME type and the file bytes, return either the
/// extracted text (`Ok(Some(...))`), an explicit "no text available for
/// this MIME" (`Ok(None)`), or an extraction error (`Err`).
///
/// Caller is responsible for char-truncating the returned text via
/// [`truncate_chars`]. This function returns the full extracted string.
pub fn extract_text(mime: &str, bytes: &[u8]) -> Result<Option<String>, ExtractError> {
    let mime_norm = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match mime_norm.as_str() {
        "application/pdf" => extract_pdf(bytes).map(Some),
        "text/plain" | "text/markdown" | "text/csv" | "text/html" | "text/x-markdown" => {
            extract_plaintext(bytes).map(Some)
        }
        _ => Ok(None),
    }
}

fn extract_pdf(bytes: &[u8]) -> Result<String, ExtractError> {
    pdf_extract::extract_text_from_mem(bytes).map_err(|e| ExtractError::PdfParse(e.to_string()))
}

fn extract_plaintext(bytes: &[u8]) -> Result<String, ExtractError> {
    std::str::from_utf8(bytes)
        .map(|s| s.to_string())
        .map_err(|e| ExtractError::InvalidUtf8(e.to_string()))
}

/// Truncate a string to at most `max_chars` Unicode characters
/// (not bytes). Safe across multi-byte UTF-8 sequences — never splits
/// a code point mid-way.
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((cut, _)) => s[..cut].to_string(),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- truncate_chars (unchanged from Task 2) ---------------------

    #[test]
    fn truncates_long_ascii_string() {
        assert_eq!(truncate_chars("abcdef", 3), "abc");
    }

    #[test]
    fn returns_full_string_when_under_cap() {
        assert_eq!(truncate_chars("abc", 10), "abc");
    }

    #[test]
    fn returns_empty_for_empty_input() {
        assert_eq!(truncate_chars("", 5), "");
    }

    #[test]
    fn handles_multi_byte_chars_without_panic() {
        let s = "🦀🦀🦀🦀🦀";
        let out = truncate_chars(s, 3);
        assert_eq!(out, "🦀🦀🦀");
        assert_eq!(out.chars().count(), 3);
    }

    #[test]
    fn cap_zero_returns_empty() {
        assert_eq!(truncate_chars("hello", 0), "");
    }

    // ---- extract_text ------------------------------------------------

    #[test]
    fn extract_plaintext_decodes_utf8() {
        let r = extract_text("text/plain", b"hello world").unwrap();
        assert_eq!(r.as_deref(), Some("hello world"));
    }

    #[test]
    fn extract_markdown_decodes_utf8() {
        let r = extract_text("text/markdown", b"# Title").unwrap();
        assert_eq!(r.as_deref(), Some("# Title"));
    }

    #[test]
    fn extract_csv_decodes_utf8() {
        let r = extract_text("text/csv", b"a,b\n1,2").unwrap();
        assert_eq!(r.as_deref(), Some("a,b\n1,2"));
    }

    #[test]
    fn extract_plaintext_invalid_utf8_errors() {
        let r = extract_text("text/plain", &[0xff, 0xfe, 0xfd]);
        assert!(matches!(r, Err(ExtractError::InvalidUtf8(_))));
    }

    #[test]
    fn extract_pdf_returns_text_for_valid_pdf() {
        let pdf = include_bytes!("../../../../tests/fixtures/hello.pdf");
        let r = extract_text("application/pdf", pdf).unwrap();
        let text = r.expect("extract_text returned None for valid PDF");
        assert!(
            text.to_lowercase().contains("hello"),
            "expected 'hello' in extracted text, got: {:?}",
            text
        );
    }

    #[test]
    fn extract_pdf_corrupt_bytes_errors() {
        let r = extract_text("application/pdf", b"not a pdf");
        assert!(matches!(r, Err(ExtractError::PdfParse(_))));
    }

    #[test]
    fn extract_unsupported_mime_returns_none() {
        let r = extract_text("application/zip", b"PK\x03\x04anything").unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn extract_image_returns_none_no_panic() {
        let r = extract_text("image/png", &[0x89, 0x50, 0x4e, 0x47]).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn extract_mime_is_case_insensitive() {
        let r = extract_text("TEXT/PLAIN", b"x").unwrap();
        assert_eq!(r.as_deref(), Some("x"));
    }

    #[test]
    fn extract_plaintext_with_charset_param_decodes_utf8() {
        let r = extract_text("text/plain; charset=utf-8", b"hello").unwrap();
        assert_eq!(r.as_deref(), Some("hello"));
    }

    #[test]
    fn extract_pdf_with_params_extracts() {
        let pdf = include_bytes!("../../../../tests/fixtures/hello.pdf");
        let r = extract_text("application/pdf; qs=0.001", pdf).unwrap();
        let text = r.expect("expected Some for application/pdf with params");
        assert!(text.to_lowercase().contains("hello"));
    }

    #[test]
    fn extract_text_handles_leading_whitespace_in_mime() {
        let r = extract_text("  text/plain  ; charset=utf-8", b"x").unwrap();
        assert_eq!(r.as_deref(), Some("x"));
    }
}
