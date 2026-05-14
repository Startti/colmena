use crate::llm::domain::attachments::AttachmentSource;
use sha2::{Digest, Sha256};

/// Deterministically compute a stable id `att_<hex16>` for a file based on
/// `(filename, mime_type, size, source-specific-discriminator)`.
///
/// - SignedUrl → discriminator = the URL string.
/// - Path      → discriminator = the absolute path.
/// - Inline    → discriminator = the SHA-256 of the raw bytes (provided by caller).
///
/// `size_bytes` may be `None`; we hash the byte string `"?"` in that case.
///
/// For `Inline`, callers must hash the bytes upstream and pass the hex digest
/// via `inline_bytes_digest`. (We do not take the bytes themselves here to
/// avoid copying potentially-large buffers into this function.)
pub fn generate_attachment_id(
    filename: &str,
    mime_type: &str,
    size_bytes: Option<u64>,
    source: &AttachmentSource,
    inline_bytes_digest: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(filename.as_bytes());
    hasher.update(b"|");
    hasher.update(mime_type.as_bytes());
    hasher.update(b"|");
    match size_bytes {
        Some(n) => hasher.update(n.to_string().as_bytes()),
        None => hasher.update(b"?"),
    }
    hasher.update(b"|");
    match source {
        AttachmentSource::SignedUrl(url) => hasher.update(url.as_bytes()),
        AttachmentSource::Path(p) => hasher.update(p.as_bytes()),
        AttachmentSource::Inline => {
            let d = inline_bytes_digest.unwrap_or("");
            hasher.update(d.as_bytes());
        }
    }
    let digest = hasher.finalize();
    let hex: String = digest
        .iter()
        .take(8)
        .map(|b| format!("{:02x}", b))
        .collect();
    format!("att_{}", hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_inputs_produce_same_id() {
        let a = generate_attachment_id(
            "Q3.pdf",
            "application/pdf",
            Some(1024),
            &AttachmentSource::SignedUrl("https://x?sig=y".to_string()),
            None,
        );
        let b = generate_attachment_id(
            "Q3.pdf",
            "application/pdf",
            Some(1024),
            &AttachmentSource::SignedUrl("https://x?sig=y".to_string()),
            None,
        );
        assert_eq!(a, b);
        assert!(a.starts_with("att_"));
        assert_eq!(a.len(), 4 + 16);
    }

    #[test]
    fn different_urls_produce_different_ids() {
        let a = generate_attachment_id(
            "x.pdf",
            "application/pdf",
            Some(1),
            &AttachmentSource::SignedUrl("u1".into()),
            None,
        );
        let b = generate_attachment_id(
            "x.pdf",
            "application/pdf",
            Some(1),
            &AttachmentSource::SignedUrl("u2".into()),
            None,
        );
        assert_ne!(a, b);
    }

    #[test]
    fn different_filenames_produce_different_ids() {
        let a = generate_attachment_id(
            "a.pdf",
            "application/pdf",
            Some(1),
            &AttachmentSource::Path("/p".into()),
            None,
        );
        let b = generate_attachment_id(
            "b.pdf",
            "application/pdf",
            Some(1),
            &AttachmentSource::Path("/p".into()),
            None,
        );
        assert_ne!(a, b);
    }

    #[test]
    fn inline_uses_bytes_digest() {
        let a = generate_attachment_id(
            "x.bin",
            "application/octet-stream",
            Some(3),
            &AttachmentSource::Inline,
            Some("aaaa"),
        );
        let b = generate_attachment_id(
            "x.bin",
            "application/octet-stream",
            Some(3),
            &AttachmentSource::Inline,
            Some("bbbb"),
        );
        assert_ne!(a, b);
    }
}
