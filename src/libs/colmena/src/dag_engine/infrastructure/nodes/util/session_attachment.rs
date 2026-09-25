//! Bytes of a session attachment, for consumers that cannot stream them (the
//! JSON body of `http_request` inlines them as a `data:` URI).

use futures::StreamExt;

use crate::llm::domain::attachments::AttachmentStreamResolver;
use crate::storage::domain::StoredBytes;

/// Resolve `document_id` in `agent_session_id` through the session's resolver
/// (a raw storage_key is NotFound, never read) and read at most `max_bytes`.
/// Errors start with `AttachmentResolveError:` or `FileTooLarge:`, like the
/// multipart path of `http_request`.
pub(crate) async fn read_session_attachment(
    resolver: &dyn AttachmentStreamResolver,
    agent_session_id: Option<&str>,
    document_id: &str,
    max_bytes: u64,
) -> Result<StoredBytes, String> {
    let sid = agent_session_id.ok_or_else(|| {
        format!("AttachmentResolveError: '$attachment:{document_id}' needs an agent_session_id")
    })?;
    let stored = resolver
        .resolve(sid, document_id)
        .await
        .map_err(|e| format!("AttachmentResolveError: {e}"))?;
    let too_large = |n: u64| {
        format!("FileTooLarge: attachment '{document_id}' is {n} bytes, max is {max_bytes}")
    };
    if stored.size_bytes > max_bytes {
        return Err(too_large(stored.size_bytes));
    }
    let mut bytes = Vec::with_capacity(stored.size_bytes as usize);
    let mut stream = stored.stream;
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk.map_err(|e| format!("AttachmentResolveError: {e}"))?);
        if bytes.len() as u64 > max_bytes {
            return Err(too_large(bytes.len() as u64));
        }
    }
    let (mime_type, filename) = (stored.mime_type, stored.filename);
    Ok(StoredBytes {
        bytes,
        mime_type,
        filename,
    })
}
