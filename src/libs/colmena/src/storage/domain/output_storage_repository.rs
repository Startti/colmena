use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::pin::Pin;

use crate::storage::domain::storage_error::StorageError;

/// A request to persist a freshly generated artifact (image, audio file, etc.).
///
/// `session_id` and `agent_session_id` are forwarded to backends that derive
/// the storage path from the conversation scope (e.g. the ADP HTTP callback
/// uses them to build `chat-attachments/<userId>/<sessionId>/generated/...`).
/// Local / test adapters typically ignore them.
#[derive(Debug, Clone)]
pub struct StoreRequest {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub filename: String,
    pub session_id: Option<String>,
    pub agent_session_id: Option<String>,
}

/// Handle returned after a successful store. `storage_key` is the canonical,
/// stable reference (used by downstream tool calls to chain operations).
/// `read_url` is a (typically short-lived) URL the caller can hand to a UI or
/// to the LLM as part of the tool output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredOutput {
    pub storage_key: String,
    pub read_url: String,
    pub mime_type: String,
    pub filename: String,
    pub size_bytes: u64,
}

/// Bytes + minimal metadata returned by [`OutputStorageRepository::read`].
/// Used by cross-provider lazy upload (load_attachment with provider=Generated)
/// and by the `$attachment:<id>` placeholder resolver in http_request.
#[derive(Debug, Clone)]
pub struct StoredBytes {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub filename: String,
}

/// Streaming counterpart of [`StoredBytes`]. Used by `http_request` multipart
/// mode to forward bytes from storage to a downstream endpoint without
/// buffering the full payload in worker RAM.
///
/// `size_bytes` is mandatory — it lets the multipart sender announce
/// `Content-Length` per part, which downstream servers require.
pub struct StoredStream {
    pub stream: Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>>,
    pub size_bytes: u64,
    pub mime_type: String,
    pub filename: String,
}

impl std::fmt::Debug for StoredStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredStream")
            .field("size_bytes", &self.size_bytes)
            .field("mime_type", &self.mime_type)
            .field("filename", &self.filename)
            .field("stream", &"<async stream>")
            .finish()
    }
}

/// Most the default [`OutputStorageRepository::store_stream`] will buffer in
/// memory (64 MiB: one prepared part fits). A host that stores larger streams
/// overrides `store_stream`.
pub const DEFAULT_STREAM_BUFFER_MAX: u64 = 64 * 1024 * 1024;

/// Where a streamed output belongs. `Generated` is today's layout (the
/// conversation's `generated/` folder). `DerivedFrom` ties the blob to a source
/// file so it lives and dies with it (prepared tables of a large tabular
/// source): the host derives the path from the source's `storage_key` and the
/// `relative_path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorePlacement {
    Generated,
    DerivedFrom {
        source_storage_key: String,
        relative_path: String,
    },
}

/// A request to persist a payload that arrives as a stream, so a large file
/// never has to sit in memory as one `Vec` on the caller's side.
///
/// `size_hint` is the expected total when known; a host that uploads in
/// chunks may use it, and nothing may rely on it being exact.
pub struct StoreStreamRequest {
    pub stream: Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>>,
    pub size_hint: Option<u64>,
    pub mime_type: String,
    pub filename: String,
    pub session_id: Option<String>,
    pub agent_session_id: Option<String>,
    pub placement: StorePlacement,
}

impl std::fmt::Debug for StoreStreamRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreStreamRequest")
            .field("size_hint", &self.size_hint)
            .field("mime_type", &self.mime_type)
            .field("filename", &self.filename)
            .field("placement", &self.placement)
            .field("stream", &"<async stream>")
            .finish()
    }
}

/// Port for persisting generated output media. Two adapters ship with
/// Colmena: `LocalCacheStorageAdapter` (in-memory + base64 `data:` URLs for
/// CLI / tests) and `HttpCallbackStorageAdapter` (production: asks an external
/// API for a signed PUT URL and uploads to object storage). Consumers wire one
/// of them into `EngineConfig::storage`.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait OutputStorageRepository: Send + Sync {
    /// Persist `req.bytes` and return a stable handle (`storage_key`) plus a
    /// fetchable `read_url`.
    async fn store(&self, req: StoreRequest) -> Result<StoredOutput, StorageError>;

    /// Retrieve the bytes for a previously-stored output. Required by:
    /// - cross-provider lazy upload (e.g. image generated via OpenAI then
    ///   requested via Anthropic in a later turn — needs the bytes to upload
    ///   to Anthropic's Files API)
    /// - the `$attachment:<id>` placeholder in `http_request`
    ///
    /// Returns `StorageError::InvalidInput` for unknown `storage_key`.
    async fn read(&self, storage_key: &str) -> Result<StoredBytes, StorageError>;

    /// Streaming counterpart to [`read`]. Required by `http_request` multipart
    /// mode. Implementations must return a stream that yields the bytes of the
    /// stored object in order, plus accurate `size_bytes` metadata.
    ///
    /// Returns `StorageError::InvalidInput` for unknown `storage_key`.
    /// Returns `StorageError::BackendUnavailable` if the underlying source
    /// cannot be reached.
    async fn read_stream(&self, storage_key: &str) -> Result<StoredStream, StorageError>;

    /// Persist a payload that arrives as a stream and return the same handle
    /// as [`store`](Self::store). Additive: a host that predates this method
    /// compiles and runs unchanged.
    ///
    /// The default implementation **buffers the stream in memory** and calls
    /// [`store`](Self::store), ignoring `placement`. It refuses, with
    /// `InvalidInput("this host does not support streamed storage ...")`, a
    /// stream whose `size_hint`, or whose accumulated bytes, exceed
    /// [`DEFAULT_STREAM_BUFFER_MAX`]. A host that handles large files (ADP)
    /// must override it to upload in chunks and to honour `placement`. A
    /// stream error aborts before anything is stored.
    async fn store_stream(&self, req: StoreStreamRequest) -> Result<StoredOutput, StorageError> {
        let too_big = || {
            StorageError::InvalidInput(format!(
                "this host does not support streamed storage above {DEFAULT_STREAM_BUFFER_MAX} bytes"
            ))
        };
        if req.size_hint.is_some_and(|n| n > DEFAULT_STREAM_BUFFER_MAX) {
            return Err(too_big());
        }
        let mut stream = req.stream;
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if (bytes.len() + chunk.len()) as u64 > DEFAULT_STREAM_BUFFER_MAX {
                return Err(too_big());
            }
            bytes.extend_from_slice(&chunk);
        }
        self.store(StoreRequest {
            bytes,
            mime_type: req.mime_type,
            filename: req.filename,
            session_id: req.session_id,
            agent_session_id: req.agent_session_id,
        })
        .await
    }

    /// Prefix under which the blobs derived from `source_storage_key` (its
    /// prepared tables) live. A host that stores prepared tables MUST override
    /// it: the cleanup pass (`attachment_gc`) deletes only tracked keys it can
    /// contain, i.e. keys inside this root (compared on a path-segment
    /// boundary) and never the source itself, so with the default `None` it
    /// REFUSES to delete them (it leaves the row, logs and counts them) rather
    /// than trust keys it cannot check.
    fn derived_root(&self, source_storage_key: &str) -> Option<String> {
        let _ = source_storage_key;
        None
    }

    /// Delete everything derived from `source_storage_key`. `tracked_keys` are
    /// the blobs the registry knows about. The default deletes exactly those,
    /// in order, and stops at the first failure; it cannot remove a blob an
    /// attempt wrote before it died and never recorded. A host that can delete
    /// by prefix (ADP) overrides it to remove the whole derived prefix, which
    /// also covers those. Idempotent.
    async fn delete_derived(
        &self,
        source_storage_key: &str,
        tracked_keys: &[String],
    ) -> Result<(), StorageError> {
        let _ = source_storage_key;
        for key in tracked_keys {
            self.delete(key).await?;
        }
        Ok(())
    }

    /// Plan C: delete the blob associated with `storage_key`. Idempotent —
    /// returns `Ok(())` whether or not the blob existed. Backend failures
    /// (e.g., GCS unavailable) bubble up as `StorageError::BackendUnavailable`
    /// so the `attachment_gc` binary can retry on the next scheduled run.
    async fn delete(&self, storage_key: &str) -> Result<(), StorageError>;

    /// Feature C (additive, part 1 of 2): ask the host for a **read URL**
    /// for an existing `storage_key`, hinted to remain valid for roughly
    /// `ttl_seconds`. Distinct from `store`'s `read_url` field (issued at
    /// write time, for a brand-new object); this one is issued on demand for
    /// an object that may already be old.
    ///
    /// Default `Ok(None)` — an existing host that predates this method
    /// compiles and runs unchanged, and `None` means "this host does not
    /// provide read URLs". Part 2 of this feature (the `$attachment_url:`
    /// placeholder in `http_request`) turns that `None` into a clear tool
    /// error ("this host does not provide attachment URLs; use
    /// `$attachment:<id>` for the bytes") instead of falling back silently.
    ///
    /// **Signing never lives in this library.** A host that wants real URLs
    /// (e.g. ADP, which signs a GCS GET) overrides this method in its own
    /// `OutputStorageRepository` adapter and calls its own signing
    /// protocol; the library only defines the shape and forwards
    /// `ttl_seconds` unmodified — the host applies its own floor/cap (ADP:
    /// 24h) on top of whatever the caller asked for.
    ///
    /// `ttl_seconds` is a **hint**, not a guarantee: an adapter may return a
    /// URL valid for more or less time than requested. `LocalHttp`'s static
    /// file server never expires, so it accepts and ignores the hint.
    async fn read_url(
        &self,
        storage_key: &str,
        ttl_seconds: u64,
    ) -> Result<Option<String>, StorageError> {
        let _ = (storage_key, ttl_seconds);
        Ok(None)
    }

    /// Capability hint consumed by the `$attachment_url:` placeholder's
    /// teaching gate (part 2): the catalog/prelude mentions that
    /// placeholder to the model **only when this returns `true`**, so the
    /// model is never taught a form that fails. Default `false`, matching
    /// the default `read_url`. An adapter that overrides `read_url` to
    /// return real URLs overrides this alongside it — every shipping
    /// adapter keeps the two in sync, though nothing at the type level
    /// forces that.
    fn supports_read_url(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// Implements only the pre-Feature-C surface, leaning entirely on the
    /// trait's default `read_url` / `supports_read_url`. Stands in for "an
    /// existing host that doesn't implement it" — the additive-compatibility
    /// claim in the port's doc comment, exercised as a real trait object
    /// (not just a compile check) so a future regression that special-cases
    /// some concrete type would still be caught.
    struct LegacyAdapter;

    #[async_trait]
    impl OutputStorageRepository for LegacyAdapter {
        async fn store(&self, _req: StoreRequest) -> Result<StoredOutput, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn read(&self, _storage_key: &str) -> Result<StoredBytes, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn read_stream(&self, _storage_key: &str) -> Result<StoredStream, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn delete(&self, _storage_key: &str) -> Result<(), StorageError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn default_read_url_is_ok_none_for_a_host_that_does_not_implement_it() {
        let adapter: Arc<dyn OutputStorageRepository> = Arc::new(LegacyAdapter);
        let got = adapter.read_url("any-key", 900).await;
        assert!(matches!(got, Ok(None)), "expected Ok(None), got {got:?}");
    }

    #[test]
    fn default_supports_read_url_is_false() {
        let adapter = LegacyAdapter;
        assert!(!adapter.supports_read_url());
    }

    /// A minimal adapter that DOES override `read_url`, to prove the
    /// `ttl_seconds` argument the part-2 placeholder resolver will pass
    /// actually reaches an implementation unmodified — not swallowed or
    /// hardcoded somewhere between the caller and the port.
    struct TtlSpyAdapter {
        seen_ttl: AtomicU64,
    }

    #[async_trait]
    impl OutputStorageRepository for TtlSpyAdapter {
        async fn store(&self, _req: StoreRequest) -> Result<StoredOutput, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn read(&self, _storage_key: &str) -> Result<StoredBytes, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn read_stream(&self, _storage_key: &str) -> Result<StoredStream, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn delete(&self, _storage_key: &str) -> Result<(), StorageError> {
            Ok(())
        }
        async fn read_url(
            &self,
            _storage_key: &str,
            ttl_seconds: u64,
        ) -> Result<Option<String>, StorageError> {
            self.seen_ttl.store(ttl_seconds, Ordering::SeqCst);
            Ok(Some("https://spy.test/x".to_string()))
        }
        fn supports_read_url(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn ttl_argument_reaches_the_implementation() {
        let adapter = TtlSpyAdapter {
            seen_ttl: AtomicU64::new(0),
        };
        let got = adapter.read_url("k1", 3600).await.unwrap();
        assert_eq!(got, Some("https://spy.test/x".to_string()));
        assert_eq!(adapter.seen_ttl.load(Ordering::SeqCst), 3600);
        assert!(adapter.supports_read_url());
    }

    /// Repository that only implements the pre-`store_stream` surface and
    /// records what `store` receives, so the default `store_stream` can be
    /// seen to delegate to it.
    #[derive(Default)]
    struct RecordingStore {
        stored: std::sync::Mutex<Vec<StoreRequest>>,
    }

    #[async_trait]
    impl OutputStorageRepository for RecordingStore {
        async fn store(&self, req: StoreRequest) -> Result<StoredOutput, StorageError> {
            let out = StoredOutput {
                storage_key: format!("k/{}", req.filename),
                read_url: "https://store.test/x".to_string(),
                mime_type: req.mime_type.clone(),
                filename: req.filename.clone(),
                size_bytes: req.bytes.len() as u64,
            };
            self.stored.lock().unwrap().push(req);
            Ok(out)
        }
        async fn read(&self, _storage_key: &str) -> Result<StoredBytes, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn read_stream(&self, _storage_key: &str) -> Result<StoredStream, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn delete(&self, _storage_key: &str) -> Result<(), StorageError> {
            Ok(())
        }
    }

    fn stream_of(
        chunks: Vec<Result<&'static [u8], StorageError>>,
    ) -> Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>> {
        Box::pin(futures::stream::iter(
            chunks.into_iter().map(|c| c.map(Bytes::from_static)),
        ))
    }

    fn stream_request(
        stream: Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>>,
        placement: StorePlacement,
    ) -> StoreStreamRequest {
        StoreStreamRequest {
            stream,
            size_hint: Some(9),
            mime_type: "text/csv".to_string(),
            filename: "out.csv".to_string(),
            session_id: Some("s1".to_string()),
            agent_session_id: Some("a1".to_string()),
            placement,
        }
    }

    #[tokio::test]
    async fn output_storage_default_store_stream_buffers_and_calls_store() {
        let repo = RecordingStore::default();
        let req = stream_request(
            stream_of(vec![Ok(b"id,n\n"), Ok(b"1,"), Ok(b"2\n")]),
            StorePlacement::Generated,
        );
        let out = repo.store_stream(req).await.unwrap();
        let stored = repo.stored.lock().unwrap();
        assert_eq!(stored.len(), 1, "exactly one store call");
        assert_eq!(
            stored[0].bytes, b"id,n\n1,2\n",
            "output bytes equal input bytes"
        );
        assert_eq!(stored[0].mime_type, "text/csv");
        assert_eq!(stored[0].filename, "out.csv");
        assert_eq!(stored[0].session_id.as_deref(), Some("s1"));
        assert_eq!(stored[0].agent_session_id.as_deref(), Some("a1"));
        assert_eq!(out.storage_key, "k/out.csv");
        assert_eq!(out.size_bytes, 9);
    }

    #[tokio::test]
    async fn output_storage_default_store_stream_ignores_a_derived_placement() {
        let repo = RecordingStore::default();
        let req = stream_request(
            stream_of(vec![Ok(b"abc")]),
            StorePlacement::DerivedFrom {
                source_storage_key: "chat-attachments/u/s/src.csv".to_string(),
                relative_path: "prepared/t0/part-00000.parquet".to_string(),
            },
        );
        repo.store_stream(req).await.unwrap();
        let stored = repo.stored.lock().unwrap();
        assert_eq!(stored[0].bytes, b"abc");
        assert_eq!(
            stored[0].filename, "out.csv",
            "the default has no derived layout"
        );
    }

    #[tokio::test]
    async fn output_storage_default_store_stream_stops_on_a_stream_error_without_storing() {
        let repo = RecordingStore::default();
        let req = stream_request(
            stream_of(vec![
                Ok(b"abc"),
                Err(StorageError::BackendUnavailable("reset".to_string())),
                Ok(b"def"),
            ]),
            StorePlacement::Generated,
        );
        let err = repo.store_stream(req).await.unwrap_err();
        assert!(matches!(err, StorageError::BackendUnavailable(m) if m == "reset"));
        assert!(
            repo.stored.lock().unwrap().is_empty(),
            "nothing partial is stored"
        );
    }

    #[tokio::test]
    async fn output_storage_existing_store_is_untouched_by_the_new_method() {
        let repo = RecordingStore::default();
        let out = repo
            .store(StoreRequest {
                bytes: b"xy".to_vec(),
                mime_type: "text/plain".to_string(),
                filename: "a.txt".to_string(),
                session_id: None,
                agent_session_id: None,
            })
            .await
            .unwrap();
        assert_eq!(out.size_bytes, 2);
        assert_eq!(repo.stored.lock().unwrap()[0].bytes, b"xy");
    }

    /// Repository that records deletions, for the derived-blob defaults.
    #[derive(Default)]
    struct DeleteLog {
        deleted: std::sync::Mutex<Vec<String>>,
        refuse: Option<String>,
    }

    #[async_trait]
    impl OutputStorageRepository for DeleteLog {
        async fn store(&self, _r: StoreRequest) -> Result<StoredOutput, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn read(&self, _k: &str) -> Result<StoredBytes, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn read_stream(&self, _k: &str) -> Result<StoredStream, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn delete(&self, key: &str) -> Result<(), StorageError> {
            if self.refuse.as_deref() == Some(key) {
                return Err(StorageError::BackendUnavailable("refused".to_string()));
            }
            self.deleted.lock().unwrap().push(key.to_string());
            Ok(())
        }
    }

    #[test]
    fn output_storage_default_derived_root_is_unknown() {
        assert_eq!(
            DeleteLog::default().derived_root("chat-attachments/u/s/a.csv"),
            None
        );
    }

    #[tokio::test]
    async fn output_storage_default_delete_derived_deletes_each_tracked_key_in_order() {
        let repo = DeleteLog::default();
        let keys = vec![
            "p/1".to_string(),
            "p/2".to_string(),
            "p/manifest".to_string(),
        ];
        repo.delete_derived("src.csv", &keys).await.unwrap();
        assert_eq!(*repo.deleted.lock().unwrap(), keys);
        // Nothing tracked: nothing deleted and no error.
        let none = DeleteLog::default();
        none.delete_derived("src.csv", &[]).await.unwrap();
        assert!(none.deleted.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn output_storage_default_delete_derived_reports_a_refusal() {
        let repo = DeleteLog {
            refuse: Some("p/2".to_string()),
            ..Default::default()
        };
        let keys = vec!["p/1".to_string(), "p/2".to_string(), "p/3".to_string()];
        let err = repo.delete_derived("src.csv", &keys).await.unwrap_err();
        assert!(matches!(err, StorageError::BackendUnavailable(_)));
        assert_eq!(*repo.deleted.lock().unwrap(), vec!["p/1".to_string()]);
    }

    #[tokio::test]
    async fn output_storage_default_store_stream_refuses_a_declared_size_over_the_cap() {
        let repo = RecordingStore::default();
        let mut req = stream_request(stream_of(vec![Ok(b"abc")]), StorePlacement::Generated);
        req.size_hint = Some(DEFAULT_STREAM_BUFFER_MAX + 1);
        let err = repo.store_stream(req).await.unwrap_err();
        assert!(
            matches!(&err, StorageError::InvalidInput(m) if m.contains("does not support streamed storage")),
            "{err:?}"
        );
        assert!(repo.stored.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn output_storage_default_store_stream_refuses_a_stream_that_outgrows_the_cap() {
        static MIB: [u8; 1 << 20] = [0; 1 << 20];
        let repo = RecordingStore::default();
        let chunks = (0..=(DEFAULT_STREAM_BUFFER_MAX >> 20)).map(|_| Ok(Bytes::from_static(&MIB)));
        let mut req = stream_request(
            Box::pin(futures::stream::iter(chunks)),
            StorePlacement::Generated,
        );
        req.size_hint = None; // the hint lies or is missing
        let err = repo.store_stream(req).await.unwrap_err();
        assert!(matches!(err, StorageError::InvalidInput(_)), "{err:?}");
        assert!(repo.stored.lock().unwrap().is_empty());
    }
}
