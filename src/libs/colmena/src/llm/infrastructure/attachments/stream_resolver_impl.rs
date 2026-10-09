//! Plan A: composite AttachmentStreamResolver impl.
//!
//! Resolution strategy:
//! 1. Look up `(agent_session_id, document_id)` in the registry; of several
//!    provider rows, a keyless lazy-upload row (no `origin`) loses to the rest.
//! 2. If found and `storage_key` is set, call `storage.read_stream(storage_key)`.
//!    Update `last_used_at` on success (best-effort, non-fatal).
//! 3. If lookup misses, return `NotFound`. The identifier is never read as a
//!    raw `storage_key`: callers pass model-written strings, and storage would
//!    serve any key, including another session's.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;

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
    /// Whether the large-file tool is served (see `large_tabular::tool_served`); the
    /// refusal of a host object points at it only then. Shared with the node registry.
    large_tool_served: Arc<std::sync::atomic::AtomicBool>,
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
        Self {
            registry,
            storage,
            large_tool_served: Arc::default(),
        }
    }

    /// Shares the flag that says whether the large-file tool is served.
    pub fn with_large_tool_flag(mut self, flag: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.large_tool_served = flag;
        self
    }
}

/// The error for a read of a host-owned object: the document and the file only,
/// as inert text (adapters put the key, or a local path, in their own text).
fn host_read_error(document_id: &str, filename: &str) -> crate::storage::domain::StorageError {
    use crate::llm::domain::large_tabular::inert_text;
    crate::storage::domain::StorageError::BackendUnavailable(format!(
        "the stored file could not be read: document_id={}, file={}",
        inert_text(document_id, 80),
        inert_text(filename, 80)
    ))
}

/// A storage error for a row the HOST owns, without the key: adapters put the key
/// (and a local one a filesystem path) in their text, and that text reaches the
/// model. The error names the document and the file only. Other rows keep their
/// error as it was.
fn hide_host_key(
    origin: &Option<String>,
    error: crate::storage::domain::StorageError,
    document_id: &str,
    filename: &str,
) -> AttachmentResolveError {
    if origin.as_deref() == Some(crate::llm::domain::attachments::origin::HOST_STORAGE_REF) {
        AttachmentResolveError::StorageError(host_read_error(document_id, filename))
    } else {
        AttachmentResolveError::StorageError(error)
    }
}

impl AttachmentStreamResolverImpl {
    /// The one lookup behind `resolve` and `resolve_for_buffering`. With
    /// `buffering`, a row the HOST owns is refused before storage is asked: the
    /// caller would hold the whole object in memory.
    async fn resolve_row(
        &self,
        agent_session_id: &str,
        document_id: &str,
        buffering: bool,
    ) -> Result<StoredStream, AttachmentResolveError> {
        // Path 1: document_id lookup in registry.
        if let Some(row) = self
            .registry
            .lookup_by_document_id(agent_session_id, document_id)
            .await?
        {
            if buffering && row.is_host_storage_ref() {
                return Err(AttachmentResolveError::HostObject {
                    // The registry sets this flag to false and leaves it so: this resolver
                    // is shared by every node (`http_request` among them) and cannot know
                    // which tools the calling node offers, so it names none. The LLM node's
                    // own paths, which do know, name the tool the node has.
                    large_tool: self
                        .large_tool_served
                        .load(std::sync::atomic::Ordering::Relaxed)
                        .then_some(
                            crate::llm::domain::large_tabular::LargeTool::AttachmentRunPython,
                        ),
                });
            }
            let key = row.storage_key.clone().ok_or_else(|| {
                AttachmentResolveError::StorageKeyMissing {
                    document_id: document_id.to_string(),
                }
            })?;

            let mut stream = self
                .storage
                .read_stream(&key)
                .await
                .map_err(|e| hide_host_key(&row.origin, e, document_id, &row.filename))?;
            if row.is_host_storage_ref() {
                // An error in the middle of the stream carries storage's text too.
                let (doc, name) = (document_id.to_string(), row.filename.clone());
                stream.stream = Box::pin(
                    stream
                        .stream
                        .map(move |chunk| chunk.map_err(|_| host_read_error(&doc, &name))),
                );
            }
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

#[async_trait]
impl AttachmentStreamResolver for AttachmentStreamResolverImpl {
    async fn resolve(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<StoredStream, AttachmentResolveError> {
        self.resolve_row(agent_session_id, document_id, false).await
    }

    async fn resolve_for_buffering(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<StoredStream, AttachmentResolveError> {
        self.resolve_row(agent_session_id, document_id, true).await
    }

    async fn resolve_url(
        &self,
        agent_session_id: &str,
        document_id: &str,
        ttl_seconds: u64,
    ) -> Result<Option<String>, AttachmentResolveError> {
        // Same lookup as `resolve`: an id the session's registry does not
        // know is NotFound, and storage is not asked for a URL.
        let row = self
            .registry
            .lookup_by_document_id(agent_session_id, document_id)
            .await?
            .ok_or_else(|| AttachmentResolveError::NotFound {
                document_id: document_id.to_string(),
            })?;
        let key = row
            .storage_key
            .ok_or_else(|| AttachmentResolveError::StorageKeyMissing {
                document_id: document_id.to_string(),
            })?;
        let url = self
            .storage
            .read_url(&key, ttl_seconds)
            .await
            .map_err(|e| hide_host_key(&row.origin, e, document_id, &row.filename))?;
        // A URL handed out is a use: the GC (days since the last use) keeps
        // the object well past the URL's life. Best-effort, like `resolve`.
        if url.is_some() {
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
        }
        Ok(url)
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

    #[tokio::test]
    async fn resolve_url_asks_storage_for_the_rows_key_with_the_ttl() {
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        reg.upsert(base_upsert("agent_x", "doc-1", Some("sk-1".to_string())))
            .await
            .unwrap();
        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_read_url()
            .withf(|key: &str, ttl: &u64| key == "sk-1" && *ttl == 3600)
            .times(1)
            .returning(|_, _| Ok(Some("https://files.test/sk-1?sig=x".to_string())));
        let reg: Arc<dyn AttachmentRegistry> = Arc::new(reg);
        let resolver = AttachmentStreamResolverImpl::new(reg.clone(), Arc::new(storage));

        let url = resolver
            .resolve_url("agent_x", "doc-1", 3600)
            .await
            .unwrap();
        assert_eq!(url.as_deref(), Some("https://files.test/sk-1?sig=x"));
        // A URL handed out is a use: the GC counts days since the last one.
        let row = reg.lookup_by_document_id("agent_x", "doc-1").await.unwrap();
        assert!(row.expect("row").last_used_at.is_some());
    }

    #[tokio::test]
    async fn resolve_url_outside_the_session_is_not_found_and_storage_is_not_asked() {
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        reg.upsert(base_upsert("agent_x", "doc-1", Some("sk-1".to_string())))
            .await
            .unwrap();
        // No expectation: any call to storage panics.
        let storage = MockOutputStorageRepository::new();
        let resolver = AttachmentStreamResolverImpl::new(Arc::new(reg), Arc::new(storage));

        for (session, id) in [("agent_y", "doc-1"), ("agent_x", "sk-1")] {
            let err = resolver.resolve_url(session, id, 900).await.unwrap_err();
            assert!(
                matches!(err, AttachmentResolveError::NotFound { .. }),
                "{session}/{id}: {err:?}"
            );
        }
    }

    /// Storage errors put the key (and, for a local adapter, a filesystem path)
    /// in their text. For a row the HOST owns that text must not reach the model:
    /// the error names the document and the file only.
    #[tokio::test]
    async fn a_host_reference_storage_error_never_carries_the_key() {
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        let mut input = base_upsert("agent_x", "doc-1", Some("hosts/secret/key.csv".to_string()));
        input.origin = Some(crate::llm::domain::attachments::origin::HOST_STORAGE_REF.to_string());
        input.filename = "sales.csv".to_string();
        reg.upsert(input).await.unwrap();

        let mut storage = MockOutputStorageRepository::new();
        storage.expect_read_stream().returning(|k| {
            Err(StorageError::InvalidInput(format!(
                "storage_key '{k}' not found at /var/data/{k}"
            )))
        });
        storage
            .expect_read_url()
            .returning(|k, _| Err(StorageError::BackendUnavailable(format!("cannot sign {k}"))));
        let resolver = AttachmentStreamResolverImpl::new(Arc::new(reg), Arc::new(storage));

        let stream_err = resolver.resolve("agent_x", "doc-1").await.unwrap_err();
        let url_err = resolver
            .resolve_url("agent_x", "doc-1", 60)
            .await
            .unwrap_err();
        for err in [stream_err.to_string(), url_err.to_string()] {
            assert!(err.contains("doc-1") && err.contains("sales.csv"), "{err}");
            assert!(
                !err.contains("secret") && !err.contains("/var/data"),
                "no key: {err}"
            );
        }
    }

    /// Rows the engine stored keep their error text as before.
    #[tokio::test]
    async fn an_engine_stored_row_keeps_its_storage_error() {
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        reg.upsert(base_upsert("agent_x", "doc-1", Some("sk-1".to_string())))
            .await
            .unwrap();
        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_read_stream()
            .returning(|k| Err(StorageError::InvalidInput(format!("missing {k}"))));
        let resolver = AttachmentStreamResolverImpl::new(Arc::new(reg), Arc::new(storage));
        let err = resolver.resolve("agent_x", "doc-1").await.unwrap_err();
        assert!(err.to_string().contains("missing sk-1"), "{err}");
    }

    fn host_row() -> UpsertAttachmentInput {
        let mut input = base_upsert("agent_x", "doc-1", Some("hosts/secret/key.csv".to_string()));
        input.origin = Some(crate::llm::domain::attachments::origin::HOST_STORAGE_REF.to_string());
        input.filename = "sales.csv".to_string();
        input
    }

    async fn registry_with(input: UpsertAttachmentInput) -> Arc<dyn AttachmentRegistry> {
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        reg.upsert(input).await.unwrap();
        Arc::new(reg)
    }

    /// A host row is never held whole in memory: refused before storage is asked.
    #[tokio::test]
    async fn buffering_a_host_reference_is_refused_without_touching_storage() {
        // No expectations: any storage call fails the test.
        let resolver = AttachmentStreamResolverImpl::new(
            registry_with(host_row()).await,
            Arc::new(MockOutputStorageRepository::new()),
        );
        let err = resolver
            .resolve_for_buffering("agent_x", "doc-1")
            .await
            .unwrap_err();
        assert!(
            matches!(err, AttachmentResolveError::HostObject { .. }),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            crate::llm::domain::large_tabular::refusal_text()
        );
    }

    /// Every other row, and every streamed use of a host row, behaves as before.
    #[tokio::test]
    async fn buffering_any_other_row_and_streaming_a_host_row_are_unchanged() {
        for (input, via_buffering) in [
            (base_upsert("agent_x", "doc-1", Some("sk-1".into())), true),
            (host_row(), false),
        ] {
            let mut storage = MockOutputStorageRepository::new();
            storage
                .expect_read_stream()
                .times(1)
                .returning(|_| Ok(make_stream(b"hello", "text/csv", "a.csv")));
            let resolver =
                AttachmentStreamResolverImpl::new(registry_with(input).await, Arc::new(storage));
            let got = if via_buffering {
                resolver.resolve_for_buffering("agent_x", "doc-1").await
            } else {
                resolver.resolve("agent_x", "doc-1").await
            };
            assert_eq!(got.unwrap().size_bytes, 5);
        }
    }

    /// An error in the middle of the stream also carries storage's text; for a
    /// host row it names the document and the file only.
    #[tokio::test]
    async fn a_mid_stream_error_of_a_host_reference_hides_the_key() {
        use futures::StreamExt;
        let mut storage = MockOutputStorageRepository::new();
        storage.expect_read_stream().returning(|k| {
            let k = k.to_string();
            let s: Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>> =
                Box::pin(stream::iter(vec![
                    Ok(Bytes::from_static(b"ok")),
                    Err(StorageError::InvalidInput(format!(
                        "lost {k} at /var/data/{k}"
                    ))),
                ]));
            Ok(StoredStream {
                stream: s,
                size_bytes: 10,
                mime_type: "text/csv".into(),
                filename: "x".into(),
            })
        });
        let resolver =
            AttachmentStreamResolverImpl::new(registry_with(host_row()).await, Arc::new(storage));
        let mut got = resolver.resolve("agent_x", "doc-1").await.unwrap().stream;
        assert!(got.next().await.unwrap().is_ok());
        let err = got.next().await.unwrap().unwrap_err().to_string();
        assert!(err.contains("doc-1") && err.contains("sales.csv"), "{err}");
        assert!(
            !err.contains("secret") && !err.contains("/var/data"),
            "{err}"
        );
    }

    /// The refusal follows the shared flag: pointed at the tool only while it is served.
    #[tokio::test]
    async fn the_host_object_refusal_follows_whether_the_tool_is_served() {
        use crate::llm::domain::large_tabular::refusal_text_for;
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let resolver = AttachmentStreamResolverImpl::new(
            registry_with(host_row()).await,
            Arc::new(MockOutputStorageRepository::new()),
        )
        .with_large_tool_flag(flag.clone());
        let err = resolver
            .resolve_for_buffering("agent_x", "doc-1")
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), refusal_text_for(false));
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
        let err = resolver
            .resolve_for_buffering("agent_x", "doc-1")
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), refusal_text_for(true));
    }
}
