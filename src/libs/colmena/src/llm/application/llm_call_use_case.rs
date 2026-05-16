use crate::llm::domain::{
    BoxedByteStream, CachedFileEntry, FileCacheRepository, FileData, FileProviderFactoryPort,
    FileProviderRepository, FileSource, LlmConfig, LlmError, LlmMessage, LlmRepository, LlmRequest,
    LlmResponse, ProviderFileRef, ProviderKind, SignedUrlFetcher,
};
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;

/// Orquesta una llamada al LLM, resolviendo `FileSource::SignedUrl` →
/// `FileSource::Uploaded` antes de llegar al adapter.
///
/// Sigue arquitectura hexagonal estricta: solo depende de puertos del
/// `domain/`. Los adapters concretos (`SignedUrlDownloader`,
/// `FileProviderFactory`, `PostgresFileCache`) se inyectan vía builders.
pub struct LlmCallUseCase {
    repository: Arc<dyn LlmRepository>,
    file_cache: Option<Arc<dyn FileCacheRepository>>,
    file_provider_factory: Option<Arc<dyn FileProviderFactoryPort>>,
    signed_url_fetcher: Option<Arc<dyn SignedUrlFetcher>>,
}

impl LlmCallUseCase {
    /// Construye un use case sin capacidad de resolver `SignedUrl`.
    /// Para activar la resolución hay que inyectar las 3 dependencias:
    /// `with_file_cache`, `with_file_provider_factory`, `with_signed_url_fetcher`.
    pub fn new(repository: Arc<dyn LlmRepository>) -> Self {
        Self {
            repository,
            file_cache: None,
            file_provider_factory: None,
            signed_url_fetcher: None,
        }
    }

    /// Inyecta el cache de Files API. Necesario (junto con factory + fetcher)
    /// para que la resolución de `SignedUrl` funcione.
    pub fn with_file_cache(mut self, cache: Arc<dyn FileCacheRepository>) -> Self {
        self.file_cache = Some(cache);
        self
    }

    /// Inyecta la factory que construye `FileProviderRepository` por provider.
    pub fn with_file_provider_factory(mut self, factory: Arc<dyn FileProviderFactoryPort>) -> Self {
        self.file_provider_factory = Some(factory);
        self
    }

    /// Inyecta el fetcher que descarga el contenido de signed URLs.
    pub fn with_signed_url_fetcher(mut self, fetcher: Arc<dyn SignedUrlFetcher>) -> Self {
        self.signed_url_fetcher = Some(fetcher);
        self
    }

    pub async fn execute(
        &self,
        mut messages: Vec<LlmMessage>,
        config: LlmConfig,
    ) -> Result<LlmResponse, LlmError> {
        if messages.is_empty() {
            return Err(LlmError::EmptyMessages);
        }

        let provider_kind = config.provider().kind().clone();
        let api_key = config.provider().api_key().to_string();

        // Build the file provider once (reused across retry). Solo si las
        // 3 dependencias de resolución están inyectadas.
        let provider_files = match (&self.file_cache, &self.file_provider_factory) {
            (Some(_), Some(factory)) => {
                Some(factory.build(provider_kind.clone(), api_key.clone())?)
            }
            _ => None,
        };

        // Snapshot the original SignedUrl for each document_id BEFORE resolve_files
        // mutates the message into FileSource::Uploaded. Needed to recover the URL
        // on a ProviderFileNotFound retry.
        let url_snapshot = Self::snapshot_signed_urls(&messages);

        // First resolution.
        self.resolve_files_in_messages(
            &mut messages,
            provider_kind.clone(),
            provider_files.clone(),
        )
        .await?;

        // First call attempt.
        let request = LlmRequest::new(messages.clone(), config.clone(), false)?;
        match self.repository.call(request).await {
            Ok(r) => Ok(r),
            Err(LlmError::ProviderFileNotFound { provider_file_id }) => {
                // Invalidate cache entries with that file id, then re-resolve and retry.
                if let Some(cache) = &self.file_cache {
                    self.invalidate_provider_file_id(
                        cache.as_ref(),
                        provider_kind.clone(),
                        &provider_file_id,
                        &messages,
                    )
                    .await;
                }

                // Reset Uploaded → SignedUrl for files matching the bad id, so resolve_files re-uploads.
                Self::reset_uploaded_files_with_id(&mut messages, &provider_file_id, &url_snapshot);

                // Re-resolve.
                self.resolve_files_in_messages(
                    &mut messages,
                    provider_kind.clone(),
                    provider_files.clone(),
                )
                .await?;

                // Single retry.
                let request = LlmRequest::new(messages, config, false)?;
                self.repository.call(request).await
            }
            Err(e) => Err(e),
        }
    }

    async fn resolve_files_in_messages(
        &self,
        messages: &mut [LlmMessage],
        provider_kind: ProviderKind,
        provider_files: Option<Arc<dyn FileProviderRepository>>,
    ) -> Result<(), LlmError> {
        let (Some(cache), Some(provider_files), Some(fetcher)) =
            (&self.file_cache, provider_files, &self.signed_url_fetcher)
        else {
            return Ok(()); // No file-resolution deps configured: nothing to resolve.
        };

        for msg in messages.iter_mut() {
            if let Some(files) = msg.files_mut() {
                Self::resolve_files(
                    files,
                    provider_kind.clone(),
                    provider_files.clone(),
                    cache.clone(),
                    fetcher.as_ref(),
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Walks all messages and invalidates cache rows for any file whose
    /// current Uploaded ref carries the bad provider_file_id.
    async fn invalidate_provider_file_id(
        &self,
        cache: &dyn FileCacheRepository,
        provider: ProviderKind,
        provider_file_id: &str,
        messages: &[LlmMessage],
    ) {
        for msg in messages {
            if let Some(files) = msg.files() {
                for file in files {
                    if let FileSource::Uploaded(r) = &file.source {
                        if r.provider_file_id == provider_file_id {
                            if let Some(doc_id) = &file.document_id {
                                // Best-effort invalidate; ignore errors.
                                let _ = cache.invalidate(doc_id, provider.clone()).await;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Captures `(document_id → original signed URL)` for every file that
    /// arrives as `FileSource::SignedUrl`. The map is consulted on a
    /// `ProviderFileNotFound` retry to revert an `Uploaded` ref back to its
    /// originating URL, so `resolve_files` re-downloads and re-uploads.
    ///
    /// Files that arrived already as `Uploaded` (caller-provided file_id) or
    /// as `InlineBytes` are not in the snapshot — there is no source URL to
    /// recover for them, and the retry will not help.
    fn snapshot_signed_urls(messages: &[LlmMessage]) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for msg in messages {
            if let Some(files) = msg.files() {
                for file in files {
                    if let FileSource::SignedUrl(url) = &file.source {
                        if let Some(doc_id) = &file.document_id {
                            map.insert(doc_id.clone(), url.clone());
                        }
                    }
                }
            }
        }
        map
    }

    /// Replaces `FileSource::Uploaded` entries whose `provider_file_id`
    /// matches the bad id with the original `SignedUrl` from the snapshot,
    /// so the next `resolve_files` call sees the URL again and re-uploads.
    ///
    /// Files without a snapshot entry stay as `Uploaded` — they were either
    /// caller-provided as `Uploaded` (no source URL exists) or arrived as
    /// `InlineBytes` (impossible to be Uploaded with that bad id, but
    /// guarded anyway). The retry is best-effort in those cases.
    fn reset_uploaded_files_with_id(
        messages: &mut [LlmMessage],
        provider_file_id: &str,
        snapshot: &HashMap<String, String>,
    ) {
        for msg in messages.iter_mut() {
            let Some(files) = msg.files_mut() else {
                continue;
            };
            for file in files.iter_mut() {
                let matches = match &file.source {
                    FileSource::Uploaded(r) => r.provider_file_id == provider_file_id,
                    _ => false,
                };
                if !matches {
                    continue;
                }
                let Some(doc_id) = file.document_id.as_deref() else {
                    continue;
                };
                if let Some(url) = snapshot.get(doc_id) {
                    file.source = FileSource::SignedUrl(url.clone());
                }
            }
        }
    }

    /// Resolves `FileSource::SignedUrl` entries into `FileSource::Uploaded`
    /// by consulting the cache and falling back to download+upload on miss.
    /// Per-file resilient: errors on individual files log and skip; the
    /// vector is rebuilt to contain only successfully-resolved files.
    /// Returns `Err(AllFilesFailedToResolve)` if every file errored.
    pub async fn resolve_files(
        files: &mut Vec<FileData>,
        provider_kind: ProviderKind,
        provider: Arc<dyn FileProviderRepository>,
        cache: Arc<dyn FileCacheRepository>,
        fetcher: &dyn SignedUrlFetcher,
    ) -> Result<(), LlmError> {
        if files.is_empty() {
            return Ok(());
        }

        crate::colmena_log!(
            "[file-resolve] resolving {} file(s) for provider {}",
            files.len(),
            provider_kind
        );

        let initial_count = files.len();
        let mut session_dedup: HashMap<String, ProviderFileRef> = HashMap::new();
        let mut errors_per_file = 0usize;
        let mut resolved: Vec<FileData> = Vec::with_capacity(files.len());

        let drained: Vec<FileData> = std::mem::take(files);
        for file in drained {
            match Self::resolve_one(
                file,
                provider_kind.clone(),
                &provider,
                &cache,
                fetcher,
                &mut session_dedup,
            )
            .await
            {
                Ok(f) => resolved.push(f),
                Err(e) => {
                    tracing::warn!(error = %e, "file resolution failed; skipping file");
                    errors_per_file += 1;
                }
            }
        }

        if initial_count > 0 && errors_per_file == initial_count {
            return Err(LlmError::AllFilesFailedToResolve);
        }

        *files = resolved;
        Ok(())
    }

    async fn resolve_one(
        mut file: FileData,
        provider_kind: ProviderKind,
        provider: &Arc<dyn FileProviderRepository>,
        cache: &Arc<dyn FileCacheRepository>,
        fetcher: &dyn SignedUrlFetcher,
        dedup: &mut HashMap<String, ProviderFileRef>,
    ) -> Result<FileData, LlmError> {
        match &file.source {
            FileSource::InlineBytes { bytes } => {
                let bytes_owned = bytes.clone();
                crate::colmena_log!(
                    "[file-resolve] '{}' is inline bytes ({}, {} B), uploading to {} Files API",
                    file.filename,
                    file.mime_type,
                    bytes_owned.len(),
                    provider_kind
                );

                // Intra-request dedup when the caller supplied a document_id.
                if let Some(doc_id) = file.document_id.as_deref() {
                    if let Some(r) = dedup.get(doc_id) {
                        crate::colmena_log!(
                            "[file-resolve] '{}' (id={}) inline-bytes intra-request dedup HIT — reusing file_id {}",
                            file.filename,
                            doc_id,
                            r.provider_file_id
                        );
                        file.source = FileSource::Uploaded(r.clone());
                        return Ok(file);
                    }
                }

                // Cross-request cache is intentionally NOT consulted for InlineBytes:
                // the cache key is (document_id, provider) and does not include a
                // content hash, so a stale entry could hand out a file_id pointing
                // at outdated content. The conversation_attachments registry covers
                // cross-turn reuse via load_attachment.

                let stream: BoxedByteStream = Box::pin(futures::stream::once(async move {
                    Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from(bytes_owned))
                }));
                let provider_ref = provider
                    .upload_streaming(stream, &file.mime_type, &file.filename)
                    .await?;

                crate::colmena_log!(
                    "[file-resolve] '{}' inline-bytes upload complete (file_id={})",
                    file.filename,
                    provider_ref.provider_file_id
                );

                if let Some(doc_id) = file.document_id.as_deref() {
                    dedup.insert(doc_id.to_string(), provider_ref.clone());
                }
                file.source = FileSource::Uploaded(provider_ref);
                Ok(file)
            }
            FileSource::Uploaded(r) => {
                crate::colmena_log!(
                    "[file-resolve] '{}' already Uploaded(provider={}, file_id={}), passing through",
                    file.filename,
                    r.provider,
                    r.provider_file_id
                );
                Ok(file)
            }
            FileSource::SignedUrl(url) => {
                let doc_id = match file.document_id.as_deref() {
                    Some(id) => id,
                    None => {
                        crate::colmena_log!(
                            "[file-resolve] '{}' has SignedUrl but no document_id; emitting UrlWithoutDocumentId",
                            file.filename
                        );
                        return Err(LlmError::UrlWithoutDocumentId);
                    }
                };

                // Anthropic does not accept file_id for image content, and OpenAI
                // chat-completions image_url requires `url` (file_id only works in
                // the Responses API). Pass the signed URL through to the adapter.
                // Skips download + upload entirely. No cache (URL is per-request).
                let is_url_passthrough_provider = matches!(
                    provider_kind,
                    ProviderKind::Anthropic | ProviderKind::OpenAi
                );
                if is_url_passthrough_provider && file.mime_type.starts_with("image/") {
                    crate::colmena_log!(
                        "[file-resolve] '{}' (id={}) image + {} — passing signed URL directly to adapter (no upload)",
                        file.filename,
                        doc_id,
                        provider_kind
                    );
                    return Ok(file);
                }

                // Intra-request dedup
                if let Some(r) = dedup.get(doc_id) {
                    crate::colmena_log!(
                        "[file-resolve] '{}' (id={}) intra-request dedup HIT — reusing file_id {}",
                        file.filename,
                        doc_id,
                        r.provider_file_id
                    );
                    file.source = FileSource::Uploaded(r.clone());
                    return Ok(file);
                }

                // Cache lookup
                crate::colmena_log!(
                    "[file-resolve] '{}' (id={}) looking up cache for provider {}",
                    file.filename,
                    doc_id,
                    provider_kind
                );
                if let Some(entry) = cache.lookup(doc_id, provider_kind.clone()).await? {
                    if entry.is_likely_alive(Utc::now()) {
                        crate::colmena_log!(
                            "[file-resolve] '{}' (id={}) cache HIT alive (file_id={}, expires_at={:?}) — skipping download/upload",
                            file.filename,
                            doc_id,
                            entry.provider_file_id,
                            entry.expires_at
                        );
                        let r = entry.into_ref();
                        dedup.insert(doc_id.to_string(), r.clone());
                        file.source = FileSource::Uploaded(r);
                        return Ok(file);
                    }
                    crate::colmena_log!(
                        "[file-resolve] '{}' (id={}) cache HIT but EXPIRED (expires_at={:?} < now+5min) — invalidating and re-uploading",
                        file.filename,
                        doc_id,
                        entry.expires_at
                    );
                    cache.invalidate(doc_id, provider_kind.clone()).await?;
                } else {
                    crate::colmena_log!(
                        "[file-resolve] '{}' (id={}) cache MISS — will download + upload",
                        file.filename,
                        doc_id
                    );
                }

                // Download → upload
                crate::colmena_log!(
                    "[file-resolve] '{}' (id={}) opening signed-URL stream from GCS",
                    file.filename,
                    doc_id
                );
                let stream: BoxedByteStream = fetcher.stream(url).await?;

                crate::colmena_log!(
                    "[file-resolve] '{}' (id={}) piping stream → {} Files API upload",
                    file.filename,
                    doc_id,
                    provider_kind
                );
                let r = provider
                    .upload_streaming(stream, &file.mime_type, &file.filename)
                    .await?;

                crate::colmena_log!(
                    "[file-resolve] '{}' (id={}) upload complete: provider_file_id={}",
                    file.filename,
                    doc_id,
                    r.provider_file_id
                );

                // Persist
                let now = Utc::now();
                let expires_at = provider
                    .ttl()
                    .and_then(|d| chrono::Duration::from_std(d).ok().map(|cd| now + cd));
                cache
                    .upsert(&CachedFileEntry {
                        document_id: doc_id.to_string(),
                        provider: provider_kind,
                        provider_file_id: r.provider_file_id.clone(),
                        mime_type: r.mime_type.clone(),
                        filename: r.filename.clone(),
                        size_bytes: file.size_hint.map(|n| n as i64),
                        uploaded_at: now,
                        expires_at,
                        last_used_at: now,
                    })
                    .await?;
                crate::colmena_log!(
                    "[file-resolve] '{}' (id={}) cache upserted (expires_at={:?})",
                    file.filename,
                    doc_id,
                    expires_at
                );

                dedup.insert(doc_id.to_string(), r.clone());
                file.source = FileSource::Uploaded(r);
                Ok(file)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{LlmProvider, MockLlmRepository, ProviderKind};
    use std::sync::Arc;

    fn create_test_config() -> LlmConfig {
        let provider = LlmProvider::new(
            ProviderKind::OpenAi,
            "test_key".into(),
            Some("gpt-4".into()),
        )
        .unwrap();
        LlmConfig::new(provider)
    }

    #[tokio::test]
    async fn test_execute_success() {
        let mut mock_repo = MockLlmRepository::new();
        let config = create_test_config();
        let messages = vec![LlmMessage::user("hello".to_string()).unwrap()];

        // 1. Setup mock expectation
        mock_repo.expect_call().times(1).returning(|req| {
            LlmResponse::new(
                req.id().clone(),
                "response".into(),
                req.config().provider().clone(),
            )
        });

        // 2. Create use case and execute
        let use_case = LlmCallUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute(messages, config).await;

        // 3. Assert success
        assert!(result.is_ok());
        assert_eq!(result.unwrap().content(), "response");
    }

    #[tokio::test]
    async fn test_execute_validation_error_empty_messages() {
        let mock_repo = MockLlmRepository::new(); // No expectations, should not be called
        let config = create_test_config();

        let use_case = LlmCallUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute(vec![], config).await; // Empty messages

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), LlmError::EmptyMessages);
    }

    #[tokio::test]
    async fn test_execute_repository_error() {
        let mut mock_repo = MockLlmRepository::new();
        let config = create_test_config();
        let messages = vec![LlmMessage::user("hello".to_string()).unwrap()];

        // 1. Setup mock expectation to return an error
        mock_repo.expect_call().times(1).returning(|_| {
            Err(LlmError::NetworkError {
                message: "Connection timed out".to_string(),
            })
        });

        // 2. Create use case and execute
        let use_case = LlmCallUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute(messages, config).await;

        // 3. Assert error
        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::NetworkError { message } => assert_eq!(message, "Connection timed out"),
            _ => panic!("Expected NetworkError"),
        }
    }
}

#[cfg(test)]
mod resolve_files_tests {
    use super::*;
    use crate::llm::domain::{
        BoxedByteStream, CachedFileEntry, FileCacheRepository, FileData, FileProviderRepository,
        FileSource, LlmError, ProviderFileRef, ProviderKind,
    };
    // Tests pueden importar adapters concretos para fixtures: la regla "domain
    // sin infrastructure" aplica al código de producción, no a tests.
    use crate::llm::infrastructure::files::SignedUrlDownloader;
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use std::sync::{Arc, Mutex};

    pub struct StubCache {
        pub entries: Mutex<Vec<CachedFileEntry>>,
    }
    impl StubCache {
        pub fn new() -> Self {
            Self {
                entries: Mutex::new(Vec::new()),
            }
        }
        pub fn populate(&self, entry: CachedFileEntry) {
            self.entries.lock().unwrap().push(entry);
        }
    }
    #[async_trait]
    impl FileCacheRepository for StubCache {
        async fn lookup(
            &self,
            doc_id: &str,
            p: ProviderKind,
        ) -> Result<Option<CachedFileEntry>, LlmError> {
            Ok(self
                .entries
                .lock()
                .unwrap()
                .iter()
                .find(|e| e.document_id == doc_id && e.provider == p)
                .cloned())
        }
        async fn upsert(&self, e: &CachedFileEntry) -> Result<(), LlmError> {
            let mut v = self.entries.lock().unwrap();
            v.retain(|x| !(x.document_id == e.document_id && x.provider == e.provider));
            v.push(e.clone());
            Ok(())
        }
        async fn invalidate(&self, doc_id: &str, p: ProviderKind) -> Result<(), LlmError> {
            self.entries
                .lock()
                .unwrap()
                .retain(|x| !(x.document_id == doc_id && x.provider == p));
            Ok(())
        }
    }

    struct StubProvider {
        upload_count: Mutex<usize>,
    }
    impl StubProvider {
        fn new() -> Self {
            Self {
                upload_count: Mutex::new(0),
            }
        }
    }
    #[async_trait]
    impl FileProviderRepository for StubProvider {
        async fn upload_streaming(
            &self,
            _stream: BoxedByteStream,
            mime: &str,
            name: &str,
        ) -> Result<ProviderFileRef, LlmError> {
            let mut c = self.upload_count.lock().unwrap();
            *c += 1;
            Ok(ProviderFileRef {
                provider: ProviderKind::Anthropic,
                provider_file_id: format!("uploaded-{}", *c),
                mime_type: mime.into(),
                filename: name.into(),
                expires_at: None,
            })
        }
        fn ttl(&self) -> Option<std::time::Duration> {
            None
        }
        fn provider(&self) -> ProviderKind {
            ProviderKind::Anthropic
        }
    }

    pub fn signed_url(id: &str) -> FileData {
        FileData {
            document_id: Some(id.into()),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_hint: Some(40_000_000),
            source: FileSource::SignedUrl("http://example.invalid/file?sig=x".into()),
        }
    }

    pub fn cached_entry(
        doc_id: &str,
        file_id: &str,
        expires_at: Option<DateTime<Utc>>,
    ) -> CachedFileEntry {
        let now = Utc::now();
        CachedFileEntry {
            document_id: doc_id.into(),
            provider: ProviderKind::Anthropic,
            provider_file_id: file_id.into(),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_bytes: None,
            uploaded_at: now,
            expires_at,
            last_used_at: now,
        }
    }

    #[tokio::test]
    async fn cache_hit_alive_skips_upload() {
        let cache = Arc::new(StubCache::new());
        let provider = Arc::new(StubProvider::new());
        let downloader = SignedUrlDownloader::new();

        cache.populate(cached_entry("doc-1", "cached-id", None));

        let mut files = vec![signed_url("doc-1")];
        LlmCallUseCase::resolve_files(
            &mut files,
            ProviderKind::Anthropic,
            provider.clone(),
            cache.clone(),
            &downloader,
        )
        .await
        .unwrap();

        assert_eq!(*provider.upload_count.lock().unwrap(), 0);
        match &files[0].source {
            FileSource::Uploaded(r) => assert_eq!(r.provider_file_id, "cached-id"),
            _ => panic!("expected Uploaded"),
        }
    }

    #[tokio::test]
    async fn dedup_within_request_uploads_once_with_cache_hit() {
        let cache = Arc::new(StubCache::new());
        let provider = Arc::new(StubProvider::new());
        let downloader = SignedUrlDownloader::new();

        cache.populate(cached_entry("doc-x", "pre-uploaded", None));

        let mut files = vec![signed_url("doc-x"), signed_url("doc-x")];
        LlmCallUseCase::resolve_files(
            &mut files,
            ProviderKind::Anthropic,
            provider.clone(),
            cache.clone(),
            &downloader,
        )
        .await
        .unwrap();

        assert_eq!(*provider.upload_count.lock().unwrap(), 0);
        for f in &files {
            assert!(matches!(f.source, FileSource::Uploaded(_)));
        }
    }

    #[tokio::test]
    async fn url_without_id_errors() {
        let cache = Arc::new(StubCache::new());
        let provider = Arc::new(StubProvider::new());
        let downloader = SignedUrlDownloader::new();

        let mut files = vec![FileData {
            document_id: None,
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_hint: None,
            source: FileSource::SignedUrl("http://x".into()),
        }];

        // Per-file resilience: error logged and dropped. With only one file
        // and 100% failure, AllFilesFailedToResolve is raised.
        let r = LlmCallUseCase::resolve_files(
            &mut files,
            ProviderKind::Anthropic,
            provider,
            cache,
            &downloader,
        )
        .await;
        assert!(matches!(r, Err(LlmError::AllFilesFailedToResolve)));
    }

    #[tokio::test]
    async fn anthropic_image_signed_url_skips_upload() {
        let cache = Arc::new(StubCache::new());
        let provider = Arc::new(StubProvider::new());
        let downloader = SignedUrlDownloader::new();

        let mut files = vec![FileData {
            document_id: Some("img-1".into()),
            mime_type: "image/jpeg".into(),
            filename: "x.jpeg".into(),
            size_hint: None,
            source: FileSource::SignedUrl("https://example/img?sig=y".into()),
        }];

        LlmCallUseCase::resolve_files(
            &mut files,
            ProviderKind::Anthropic,
            provider.clone(),
            cache.clone(),
            &downloader,
        )
        .await
        .unwrap();

        // No upload happened.
        assert_eq!(*provider.upload_count.lock().unwrap(), 0);
        // Source stays as SignedUrl (no Uploaded conversion).
        match &files[0].source {
            FileSource::SignedUrl(_) => {}
            _ => panic!("expected SignedUrl to be preserved"),
        }
    }

    #[tokio::test]
    async fn openai_image_signed_url_skips_upload() {
        let cache = Arc::new(StubCache::new());
        let provider = Arc::new(StubProvider::new());
        let downloader = SignedUrlDownloader::new();

        let mut files = vec![FileData {
            document_id: Some("img-2".into()),
            mime_type: "image/png".into(),
            filename: "x.png".into(),
            size_hint: None,
            source: FileSource::SignedUrl("https://example/img?sig=y".into()),
        }];

        LlmCallUseCase::resolve_files(
            &mut files,
            ProviderKind::OpenAi,
            provider.clone(),
            cache.clone(),
            &downloader,
        )
        .await
        .unwrap();

        assert_eq!(*provider.upload_count.lock().unwrap(), 0);
        match &files[0].source {
            FileSource::SignedUrl(_) => {}
            _ => panic!("expected SignedUrl preserved for OpenAI image"),
        }
    }

    #[tokio::test]
    async fn resolve_files_uploads_inline_bytes_and_marks_uploaded() {
        // GIVEN a single InlineBytes file (e.g. from path: or data: input)
        let bytes = b"%PDF-1.4 hello world".to_vec();
        let mut files = vec![FileData {
            document_id: Some("doc-inline-1".to_string()),
            mime_type: "application/pdf".to_string(),
            filename: "hello.pdf".to_string(),
            size_hint: Some(bytes.len() as u64),
            source: FileSource::InlineBytes { bytes },
        }];

        // WHEN we run resolve_files with stub provider + stub cache
        let provider: Arc<dyn FileProviderRepository> = Arc::new(StubProvider::new());
        let cache: Arc<dyn FileCacheRepository> = Arc::new(StubCache::new());
        let fetcher = SignedUrlDownloader::new();

        LlmCallUseCase::resolve_files(
            &mut files,
            ProviderKind::Anthropic,
            provider.clone(),
            cache,
            &fetcher,
        )
        .await
        .expect("resolve_files should succeed");

        // THEN the file's source should be Uploaded with the stub-provided id
        assert_eq!(files.len(), 1);
        match &files[0].source {
            FileSource::Uploaded(r) => {
                assert_eq!(r.provider_file_id, "uploaded-1");
                assert_eq!(r.mime_type, "application/pdf");
                assert_eq!(r.filename, "hello.pdf");
            }
            other => panic!("expected Uploaded, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn resolve_files_inline_bytes_intra_request_dedup() {
        // GIVEN two InlineBytes entries with the SAME document_id
        let bytes = b"same content".to_vec();
        let mut files = vec![
            FileData {
                document_id: Some("doc-dedup".to_string()),
                mime_type: "application/pdf".to_string(),
                filename: "a.pdf".to_string(),
                size_hint: Some(bytes.len() as u64),
                source: FileSource::InlineBytes {
                    bytes: bytes.clone(),
                },
            },
            FileData {
                document_id: Some("doc-dedup".to_string()),
                mime_type: "application/pdf".to_string(),
                filename: "b.pdf".to_string(),
                size_hint: Some(bytes.len() as u64),
                source: FileSource::InlineBytes { bytes },
            },
        ];

        let stub_provider = Arc::new(StubProvider::new());
        let provider: Arc<dyn FileProviderRepository> = stub_provider.clone();
        let cache: Arc<dyn FileCacheRepository> = Arc::new(StubCache::new());
        let fetcher = SignedUrlDownloader::new();

        LlmCallUseCase::resolve_files(
            &mut files,
            ProviderKind::Anthropic,
            provider,
            cache,
            &fetcher,
        )
        .await
        .unwrap();

        // THEN only ONE upload was issued (second was deduped)
        let upload_count = *stub_provider.upload_count.lock().unwrap();
        assert_eq!(
            upload_count, 1,
            "expected exactly 1 upload for two files with same document_id, got {}",
            upload_count
        );

        // Both entries should be Uploaded with the same provider_file_id
        match (&files[0].source, &files[1].source) {
            (FileSource::Uploaded(a), FileSource::Uploaded(b)) => {
                assert_eq!(a.provider_file_id, b.provider_file_id);
            }
            _ => panic!("expected both Uploaded"),
        }
    }
}

#[cfg(test)]
mod snapshot_and_reset_tests {
    use super::*;
    use crate::llm::domain::{FileData, FileSource, LlmMessage, ProviderFileRef, ProviderKind};

    fn signed_file(doc_id: &str, url: &str) -> FileData {
        FileData {
            document_id: Some(doc_id.into()),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_hint: None,
            source: FileSource::SignedUrl(url.into()),
        }
    }

    fn uploaded_file(doc_id: Option<&str>, file_id: &str) -> FileData {
        FileData {
            document_id: doc_id.map(|s| s.to_string()),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_hint: None,
            source: FileSource::Uploaded(ProviderFileRef {
                provider: ProviderKind::Anthropic,
                provider_file_id: file_id.into(),
                mime_type: "application/pdf".into(),
                filename: "x.pdf".into(),
                expires_at: None,
            }),
        }
    }

    #[test]
    fn snapshot_collects_signed_url_per_document_id() {
        let messages = vec![
            LlmMessage::user_with_files(
                "first".into(),
                vec![
                    signed_file("doc-1", "https://gcs/one"),
                    signed_file("doc-2", "https://gcs/two"),
                ],
            )
            .unwrap(),
            LlmMessage::user_with_files(
                "second".into(),
                vec![FileData::inline(
                    "application/pdf".into(),
                    "y.pdf".into(),
                    b"x".to_vec(),
                )],
            )
            .unwrap(),
        ];

        let snapshot = LlmCallUseCase::snapshot_signed_urls(&messages);
        assert_eq!(
            snapshot.get("doc-1").map(|s| s.as_str()),
            Some("https://gcs/one")
        );
        assert_eq!(
            snapshot.get("doc-2").map(|s| s.as_str()),
            Some("https://gcs/two")
        );
        assert_eq!(snapshot.len(), 2);
    }

    #[test]
    fn snapshot_skips_uploaded_and_inline() {
        let messages = vec![LlmMessage::user_with_files(
            "x".into(),
            vec![
                uploaded_file(Some("doc-1"), "file-abc"),
                FileData::inline("application/pdf".into(), "y.pdf".into(), b"x".to_vec()),
            ],
        )
        .unwrap()];

        let snapshot = LlmCallUseCase::snapshot_signed_urls(&messages);
        assert!(snapshot.is_empty());
    }

    #[test]
    fn reset_replaces_uploaded_with_signed_url_from_snapshot() {
        let mut messages = vec![LlmMessage::user_with_files(
            "x".into(),
            vec![uploaded_file(Some("doc-1"), "lost")],
        )
        .unwrap()];

        let mut snapshot = HashMap::new();
        snapshot.insert("doc-1".to_string(), "https://gcs/orig".to_string());

        LlmCallUseCase::reset_uploaded_files_with_id(&mut messages, "lost", &snapshot);

        let files = messages[0].files().unwrap();
        match &files[0].source {
            FileSource::SignedUrl(url) => assert_eq!(url, "https://gcs/orig"),
            other => panic!("expected SignedUrl, got {:?}", other),
        }
    }

    #[test]
    fn reset_leaves_uploaded_intact_when_id_does_not_match() {
        let mut messages = vec![LlmMessage::user_with_files(
            "x".into(),
            vec![uploaded_file(Some("doc-1"), "good")],
        )
        .unwrap()];

        let mut snapshot = HashMap::new();
        snapshot.insert("doc-1".to_string(), "https://gcs/orig".to_string());

        LlmCallUseCase::reset_uploaded_files_with_id(&mut messages, "lost", &snapshot);

        match &messages[0].files().unwrap()[0].source {
            FileSource::Uploaded(r) => assert_eq!(r.provider_file_id, "good"),
            other => panic!("expected Uploaded preserved, got {:?}", other),
        }
    }

    #[test]
    fn reset_no_op_when_snapshot_has_no_entry_for_doc_id() {
        // Caller-provided Uploaded with no SignedUrl history: nothing to recover.
        let mut messages = vec![LlmMessage::user_with_files(
            "x".into(),
            vec![uploaded_file(Some("doc-1"), "lost")],
        )
        .unwrap()];

        let snapshot = HashMap::new();
        LlmCallUseCase::reset_uploaded_files_with_id(&mut messages, "lost", &snapshot);

        match &messages[0].files().unwrap()[0].source {
            FileSource::Uploaded(r) => assert_eq!(r.provider_file_id, "lost"),
            other => panic!("expected Uploaded preserved, got {:?}", other),
        }
    }

    #[test]
    fn reset_no_op_when_file_has_no_document_id() {
        let mut messages =
            vec![
                LlmMessage::user_with_files("x".into(), vec![uploaded_file(None, "lost")]).unwrap(),
            ];

        let mut snapshot = HashMap::new();
        snapshot.insert("doc-1".to_string(), "https://gcs/orig".to_string());

        LlmCallUseCase::reset_uploaded_files_with_id(&mut messages, "lost", &snapshot);

        match &messages[0].files().unwrap()[0].source {
            FileSource::Uploaded(r) => assert_eq!(r.provider_file_id, "lost"),
            other => panic!("expected Uploaded preserved, got {:?}", other),
        }
    }
}

#[cfg(test)]
mod retry_tests {
    use super::resolve_files_tests::*;
    use super::*;
    use crate::llm::domain::{
        FileData, FileSource, LlmConfig, LlmError, LlmMessage, LlmProvider, LlmResponse,
        MockLlmRepository, ProviderKind,
    };
    // Tests pueden usar adapters reales como fixture cuando el escenario es
    // E2E-like (wiremock + cache real + provider real con key inválida).
    use crate::llm::infrastructure::files::{FileProviderFactory, SignedUrlDownloader};
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn config_for(provider: ProviderKind) -> LlmConfig {
        let p = LlmProvider::new(provider, "k".into(), Some("model".into())).unwrap();
        LlmConfig::new(p)
    }

    /// After a `ProviderFileNotFound`, `execute` must:
    ///   1. Invalidate the cached row for the bad `provider_file_id`.
    ///   2. Revert `Uploaded(bad_id)` to the original `SignedUrl` (snapshot).
    ///   3. Re-run `resolve_files` so the URL is hit again.
    ///
    /// We assert (3) by counting GETs on a wiremock-served URL: with the fix
    /// it must be exactly 1 (cache HIT shorts the first resolve, retry hits
    /// it after the reset). Before the fix it was 0 (the no-op `reset` left
    /// the file as `Uploaded`, so resolve short-circuited a second time).
    #[tokio::test]
    async fn retries_redownload_after_provider_file_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/doc.pdf"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"fake-pdf"))
            .mount(&server)
            .await;
        let url = format!("{}/doc.pdf?sig=x", server.uri());

        let mut mock_repo = MockLlmRepository::new();
        mock_repo.expect_call().times(1).returning(|_req| {
            Err(LlmError::ProviderFileNotFound {
                provider_file_id: "lost".to_string(),
            })
        });
        // Second LLM call is unreachable: after the reset, resolve_files
        // re-downloads (good) but then tries to upload to the real Anthropic
        // Files API with a fake key, which fails. resolve_files returns
        // AllFilesFailedToResolve before any second LLM call. We don't
        // expect_call() a second time.

        let cache = Arc::new(StubCache::new());
        cache.populate(cached_entry("doc-1", "lost", None));

        let use_case = LlmCallUseCase::new(Arc::new(mock_repo))
            .with_file_cache(cache.clone())
            .with_file_provider_factory(Arc::new(FileProviderFactory::new()))
            .with_signed_url_fetcher(Arc::new(SignedUrlDownloader::new()));

        let file = FileData {
            document_id: Some("doc-1".into()),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_hint: Some(40_000_000),
            source: FileSource::SignedUrl(url.clone()),
        };
        let msg = LlmMessage::user_with_files("describe".into(), vec![file]).unwrap();

        let _ = use_case
            .execute(vec![msg], config_for(ProviderKind::Anthropic))
            .await;

        let received = server.received_requests().await.unwrap();
        let hits = received
            .iter()
            .filter(|r| r.url.path() == "/doc.pdf")
            .count();
        assert_eq!(
            hits, 1,
            "expected exactly 1 GET during retry redownload, got {}",
            hits
        );
    }

    #[tokio::test]
    async fn no_retry_on_other_errors() {
        let mut mock_repo = MockLlmRepository::new();
        mock_repo
            .expect_call()
            .times(1) // exactly one call, no retry
            .returning(|_req| {
                Err(LlmError::NetworkError {
                    message: "boom".into(),
                })
            });

        let use_case = LlmCallUseCase::new(Arc::new(mock_repo));
        let msg = LlmMessage::user("hi".into()).unwrap();
        let result = use_case
            .execute(vec![msg], config_for(ProviderKind::OpenAi))
            .await;

        assert!(matches!(result, Err(LlmError::NetworkError { .. })));
    }

    #[tokio::test]
    async fn no_retry_on_success() {
        let mut mock_repo = MockLlmRepository::new();
        mock_repo.expect_call().times(1).returning(|req| {
            LlmResponse::new(
                req.id().clone(),
                "ok".into(),
                req.config().provider().clone(),
            )
        });

        let use_case = LlmCallUseCase::new(Arc::new(mock_repo));
        let msg = LlmMessage::user("hi".into()).unwrap();
        let result = use_case
            .execute(vec![msg], config_for(ProviderKind::OpenAi))
            .await;

        assert!(result.is_ok());
    }
}
