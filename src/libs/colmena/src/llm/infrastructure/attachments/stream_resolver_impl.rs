//! Plan A: composite AttachmentStreamResolver impl.
//!
//! Resolution strategy:
//! 1. Look up `(agent_session_id, document_id)` in the registry; of several
//!    provider rows, one that has a `storage_key` wins.
//! 2. If found and `storage_key` is set, call `storage.read_stream(storage_key)`.
//!    Update `last_used_at` on success (best-effort, non-fatal).
//! 3. If lookup misses, return `NotFound`. The identifier is never read as a
//!    raw `storage_key`: callers pass model-written strings, and storage would
//!    serve any key, including another session's.

use std::sync::Arc;

use async_trait::async_trait;

use crate::llm::domain::attachments::{
    AttachmentRegistry, AttachmentResolveError, AttachmentStreamResolver,
};
use crate::storage::domain::{OutputStorageRepository, StoredStream};

/// Production [`AttachmentStreamResolver`] composing an
/// [`AttachmentRegistry`] (catalog of `(agent_session_id, document_id) →
/// storage_key`) and an [`OutputStorageRepository`] (`storage_key → bytes`).
///
/// Wire one of these into the engine at startup; consumers receive it as
/// `Arc<dyn AttachmentStreamResolver>` so the registry/storage choice
/// (Postgres + GCS in prod, SQLite + LocalCache in tests) is invisible.
pub struct AttachmentStreamResolverImpl {
    registry: Arc<dyn AttachmentRegistry>,
    storage: Arc<dyn OutputStorageRepository>,
}

impl AttachmentStreamResolverImpl {
    /// Construct a resolver from already-wired registry + storage adapters.
    ///
    /// Both arguments are `Arc<dyn _>` because the resolver is normally
    /// shared across nodes (LLM, http_request, image_generation, …) and
    /// across concurrent DAG runs.
    pub fn new(
        registry: Arc<dyn AttachmentRegistry>,
        storage: Arc<dyn OutputStorageRepository>,
    ) -> Self {
        Self { registry, storage }
    }
}

#[async_trait]
impl AttachmentStreamResolver for AttachmentStreamResolverImpl {
    async fn resolve(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<StoredStream, AttachmentResolveError> {
        // Path 1: document_id lookup in registry.
        if let Some(row) = self
            .registry
            .lookup_by_document_id(agent_session_id, document_id)
            .await?
        {
            let key = row
                .storage_key
                .ok_or_else(|| AttachmentResolveError::StorageKeyMissing {
                    document_id: document_id.to_string(),
                })?;

            let stream = self.storage.read_stream(&key).await?;
            // Best-effort: touch_last_used failure is non-fatal.
            if let Err(e) = self
                .registry
                .touch_last_used(agent_session_id, document_id)
                .await
            {
                tracing::warn!(
                    target: "colmena::attachment",
                    error = %e,
                    document_id = %document_id,
                    "touch_last_used failed (non-fatal)"
                );
            }
            return Ok(stream);
        }

        // Not a document_id of this session (a raw storage_key, another
        // session's id, a made-up id): NotFound, and storage is not asked.
        Err(AttachmentResolveError::NotFound {
            document_id: document_id.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bytes::Bytes;
    use futures::{stream, Stream};
    use std::pin::Pin;

    use crate::llm::domain::attachments::{AttachmentSource, UpsertAttachmentInput};
    use crate::llm::domain::ProviderKind;
    use crate::llm::infrastructure::persistence::sqlite_attachment_registry::SqliteAttachmentRegistry;
    use crate::storage::domain::{MockOutputStorageRepository, StorageError};

    fn make_stream(body: &'static [u8], mime: &str, filename: &str) -> StoredStream {
        let s: Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>> =
            Box::pin(stream::iter(vec![Ok(Bytes::from_static(body))]));
        StoredStream {
            stream: s,
            size_bytes: body.len() as u64,
            mime_type: mime.to_string(),
            filename: filename.to_string(),
        }
    }

    fn base_upsert(sid: &str, doc_id: &str, storage_key: Option<String>) -> UpsertAttachmentInput {
        UpsertAttachmentInput {
            agent_session_id: sid.to_string(),
            document_id: doc_id.to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-1".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "a.pdf".to_string(),
            size_bytes: Some(10),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            storage_key,
            origin: Some("user_upload".to_string()),
        }
    }

    #[tokio::test]
    async fn resolve_via_document_id_uses_storage_key_from_registry() {
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        reg.upsert(base_upsert("agent_x", "doc-1", Some("sk-1".to_string())))
            .await
            .unwrap();

        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_read_stream()
            .withf(|k| k == "sk-1")
            .times(1)
            .returning(|_| Ok(make_stream(b"hello", "application/pdf", "a.pdf")));

        let reg_arc: Arc<dyn AttachmentRegistry> = Arc::new(reg);
        let resolver = AttachmentStreamResolverImpl::new(reg_arc.clone(), Arc::new(storage));

        let out = resolver.resolve("agent_x", "doc-1").await.unwrap();
        assert_eq!(out.size_bytes, 5);
        assert_eq!(out.mime_type, "application/pdf");

        // touch_last_used side effect: row should now have last_used_at set.
        let row = reg_arc
            .lookup_by_document_id("agent_x", "doc-1")
            .await
            .unwrap()
            .expect("row should exist");
        assert!(
            row.last_used_at.is_some(),
            "touch_last_used should have populated last_used_at"
        );
    }

    #[tokio::test]
    async fn resolve_never_reads_an_id_the_session_registry_does_not_know() {
        // `agent_y` owns doc-1 → sk-1. For `agent_x`, a raw storage_key (known
        // to storage or not), another session's document_id and an unknown id
        // are all NotFound, and storage is never asked.
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        reg.upsert(base_upsert("agent_y", "doc-1", Some("sk-1".to_string())))
            .await
            .unwrap();
        let mut storage = MockOutputStorageRepository::new();
        storage.expect_read_stream().never();
        let resolver = AttachmentStreamResolverImpl::new(Arc::new(reg), Arc::new(storage));

        for id in ["sk-1", "doc-1", "sk-raw", "missing-id"] {
            let err = resolver.resolve("agent_x", id).await.unwrap_err();
            assert!(
                matches!(err, AttachmentResolveError::NotFound { ref document_id } if document_id == id),
                "{id}: expected NotFound, got {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn resolve_returns_storage_key_missing_when_row_has_no_storage_key() {
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        // Row exists but storage_key is None (legacy/pre-Plan-A row).
        reg.upsert(base_upsert("agent_x", "doc-legacy", None))
            .await
            .unwrap();

        // Storage MUST NOT be touched on this path.
        let storage = MockOutputStorageRepository::new();

        let resolver = AttachmentStreamResolverImpl::new(Arc::new(reg), Arc::new(storage));

        let err = resolver.resolve("agent_x", "doc-legacy").await.unwrap_err();
        assert!(
            matches!(
                err,
                AttachmentResolveError::StorageKeyMissing { ref document_id }
                    if document_id == "doc-legacy"
            ),
            "expected StorageKeyMissing, got {:?}",
            err
        );
    }
}
