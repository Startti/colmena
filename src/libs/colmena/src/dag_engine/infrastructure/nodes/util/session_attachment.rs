//! Bytes of a session attachment, for consumers that cannot stream them (the
//! JSON body of `http_request` inlines them as a `data:` URI). An object the HOST
//! owns is never read this way: it would be held whole in memory.

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
    // This caller holds the whole object in memory: a host-owned object is refused.
    let stored = resolver
        .resolve_for_buffering(sid, document_id)
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

#[cfg(test)]
mod tests {
    //! The consumers that hold the whole attachment in memory (the JSON body of
    //! `http_request`, `image_edit`) never read an object the HOST owns.
    use super::*;
    use crate::llm::domain::attachments::{
        origin, AttachmentRegistry, AttachmentSource, UpsertAttachmentInput,
    };
    use crate::llm::domain::ProviderKind;
    use crate::llm::infrastructure::attachments::AttachmentStreamResolverImpl;
    use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;
    use crate::storage::domain::{MockOutputStorageRepository, StoredStream};
    use bytes::Bytes;
    use std::sync::Arc;

    async fn resolver(
        origin_value: &str,
        storage: MockOutputStorageRepository,
    ) -> AttachmentStreamResolverImpl {
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        reg.upsert(UpsertAttachmentInput {
            agent_session_id: "s".into(),
            document_id: "d".into(),
            provider: ProviderKind::OpenAi,
            provider_file_id: String::new(),
            mime_type: "text/csv".into(),
            filename: "big.csv".into(),
            size_bytes: Some(5),
            label: None,
            description: None,
            source: AttachmentSource::Path("hosts/k".into()),
            storage_key: Some("hosts/k".into()),
            origin: Some(origin_value.into()),
        })
        .await
        .unwrap();
        AttachmentStreamResolverImpl::new(Arc::new(reg), Arc::new(storage))
    }

    #[tokio::test]
    async fn a_host_reference_is_refused_and_an_engine_row_is_read() {
        // Host row: no storage expectation, so any read fails the test.
        let r = resolver(origin::HOST_STORAGE_REF, MockOutputStorageRepository::new()).await;
        let err = read_session_attachment(&r, Some("s"), "d", 1 << 20)
            .await
            .unwrap_err();
        assert!(
            err.contains(crate::llm::domain::large_tabular::refusal_text()),
            "{err}"
        );

        let mut storage = MockOutputStorageRepository::new();
        storage.expect_read_stream().times(1).returning(|_| {
            Ok(StoredStream {
                stream: Box::pin(futures::stream::once(async {
                    Ok::<_, crate::storage::domain::StorageError>(Bytes::from_static(b"hello"))
                })),
                size_bytes: 5,
                mime_type: "text/csv".into(),
                filename: "big.csv".into(),
            })
        });
        let r = resolver(origin::USER_UPLOAD, storage).await;
        let got = read_session_attachment(&r, Some("s"), "d", 1 << 20)
            .await
            .unwrap();
        assert_eq!(got.bytes, b"hello");
    }
}
