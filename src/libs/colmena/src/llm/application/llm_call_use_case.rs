use crate::llm::domain::{
    BoxedByteStream, CachedFileEntry, FileCacheRepository, FileData, FileProviderRepository,
    FileSource, LlmConfig, LlmError, LlmMessage, LlmRepository, LlmRequest, LlmResponse,
    ProviderFileRef, ProviderKind,
};
use crate::llm::infrastructure::files::{FileProviderFactory, SignedUrlDownloader};
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;

pub struct LlmCallUseCase {
    repository: Arc<dyn LlmRepository>,
    file_cache: Option<Arc<dyn FileCacheRepository>>,
    downloader: Arc<SignedUrlDownloader>,
}

impl LlmCallUseCase {
    /// Constructs the use case with no file cache. Files using
    /// `FileSource::SignedUrl` will fail with `LlmError::InternalError`
    /// because they cannot be resolved without a cache.
    pub fn new(repository: Arc<dyn LlmRepository>) -> Self {
        Self {
            repository,
            file_cache: None,
            downloader: Arc::new(SignedUrlDownloader::new()),
        }
    }

    /// Adds a `FileCacheRepository` so that files arriving as `SignedUrl`
    /// can be downloaded and uploaded to the provider's Files API.
    pub fn with_file_cache(mut self, cache: Arc<dyn FileCacheRepository>) -> Self {
        self.file_cache = Some(cache);
        self
    }

    /// Override the downloader (mainly for tests).
    pub fn with_downloader(mut self, downloader: Arc<SignedUrlDownloader>) -> Self {
        self.downloader = downloader;
        self
    }

    pub async fn execute(
        &self,
        mut messages: Vec<LlmMessage>,
        config: LlmConfig,
    ) -> Result<LlmResponse, LlmError> {
        // 1. Validate input
        if messages.is_empty() {
            return Err(LlmError::EmptyMessages);
        }

        // 2. Resolve files if a cache is configured
        if let Some(cache) = &self.file_cache {
            let provider_kind = config.provider().kind().clone();
            let api_key = config.provider().api_key().to_string();
            let provider_files = FileProviderFactory::create(provider_kind.clone(), api_key)?;

            for msg in messages.iter_mut() {
                if let Some(files) = msg.files_mut() {
                    Self::resolve_files(
                        files,
                        provider_kind.clone(),
                        provider_files.clone(),
                        cache.clone(),
                        self.downloader.as_ref(),
                    )
                    .await?;
                }
            }
        }

        // 3. Create request
        let request = LlmRequest::new(messages, config, false)?;

        // 4. Execute call
        self.repository.call(request).await
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
        downloader: &SignedUrlDownloader,
    ) -> Result<(), LlmError> {
        if files.is_empty() {
            return Ok(());
        }

        let initial_count = files.len();
        let mut session_dedup: HashMap<String, ProviderFileRef> = HashMap::new();
        let mut errors_per_file = 0usize;
        let mut resolved: Vec<FileData> = Vec::with_capacity(files.len());

        let drained: Vec<FileData> = files.drain(..).collect();
        for file in drained {
            match Self::resolve_one(
                file,
                provider_kind.clone(),
                &provider,
                &cache,
                downloader,
                &mut session_dedup,
            )
            .await
            {
                Ok(f) => resolved.push(f),
                Err(e) => {
                    eprintln!("WARN: file resolution failed: {}", e);
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
        downloader: &SignedUrlDownloader,
        dedup: &mut HashMap<String, ProviderFileRef>,
    ) -> Result<FileData, LlmError> {
        match &file.source {
            FileSource::InlineBytes { .. } | FileSource::Uploaded(_) => Ok(file),
            FileSource::SignedUrl(url) => {
                let doc_id = file
                    .document_id
                    .as_deref()
                    .ok_or(LlmError::UrlWithoutDocumentId)?;

                // Intra-request dedup
                if let Some(r) = dedup.get(doc_id) {
                    file.source = FileSource::Uploaded(r.clone());
                    return Ok(file);
                }

                // Cache lookup
                if let Some(entry) = cache.lookup(doc_id, provider_kind.clone()).await? {
                    if entry.is_likely_alive(Utc::now()) {
                        let r = entry.into_ref();
                        dedup.insert(doc_id.to_string(), r.clone());
                        file.source = FileSource::Uploaded(r);
                        return Ok(file);
                    }
                    cache.invalidate(doc_id, provider_kind.clone()).await?;
                }

                // Download → upload
                let stream: BoxedByteStream = downloader.stream(url).await?;
                let r = provider
                    .upload_streaming(stream, &file.mime_type, &file.filename)
                    .await?;

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
        BoxedByteStream, CachedFileEntry, FileCacheRepository, FileData,
        FileProviderRepository, FileSource, LlmError, ProviderFileRef, ProviderKind,
    };
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use std::sync::{Arc, Mutex};

    struct StubCache {
        entries: Mutex<Vec<CachedFileEntry>>,
    }
    impl StubCache {
        fn new() -> Self {
            Self {
                entries: Mutex::new(Vec::new()),
            }
        }
        fn populate(&self, entry: CachedFileEntry) {
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
        async fn invalidate(
            &self,
            doc_id: &str,
            p: ProviderKind,
        ) -> Result<(), LlmError> {
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

    fn signed_url(id: &str) -> FileData {
        FileData {
            document_id: Some(id.into()),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_hint: Some(40_000_000),
            source: FileSource::SignedUrl("http://example.invalid/file?sig=x".into()),
        }
    }

    fn cached_entry(
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
    async fn inline_bytes_passes_through_untouched() {
        let cache = Arc::new(StubCache::new());
        let provider = Arc::new(StubProvider::new());
        let downloader = SignedUrlDownloader::new();

        let mut files = vec![FileData::inline(
            "application/pdf".into(),
            "x.pdf".into(),
            b"hello".to_vec(),
        )];

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
            FileSource::InlineBytes { .. } => {}
            _ => panic!("inline should pass through"),
        }
    }
}
