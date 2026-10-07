//! A large tabular file that arrives as `FileSource::StorageRef`: resolution
//! leaves it alone and the catalog registers it by its key. Parsing an entry
//! into one is covered where the parser is wired.

use super::file_registrations;
use crate::llm::application::LlmCallUseCase;
use crate::llm::domain::attachments::AttachmentSource;
use crate::llm::domain::{
    BoxedByteStream, CachedFileEntry, FileCacheRepository, FileData, FileProviderRepository,
    FileSource, LlmError, ProviderFileRef, ProviderKind, SignedUrlFetcher,
};
use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, Mutex};

const MIB: u64 = 1024 * 1024;
const LIMIT: u64 = 50 * MIB;
const CSV: &str = "text/csv";

// ── resolution: a storage reference is never downloaded or uploaded ─────

/// Counts uploads; a storage reference must never reach it.
#[derive(Default)]
struct CountingProvider(Mutex<Vec<String>>);

#[async_trait]
impl FileProviderRepository for CountingProvider {
    async fn upload_streaming(
        &self,
        _stream: BoxedByteStream,
        mime_type: &str,
        filename: &str,
    ) -> Result<ProviderFileRef, LlmError> {
        self.0.lock().unwrap().push(filename.to_string());
        Ok(ProviderFileRef {
            provider: ProviderKind::Anthropic,
            provider_file_id: "uploaded-1".to_string(),
            mime_type: mime_type.to_string(),
            filename: filename.to_string(),
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

/// Counts cache calls.
#[derive(Default)]
struct CountingCache(Mutex<usize>);

#[async_trait]
impl FileCacheRepository for CountingCache {
    async fn lookup(
        &self,
        _document_id: &str,
        _provider: ProviderKind,
    ) -> Result<Option<CachedFileEntry>, LlmError> {
        *self.0.lock().unwrap() += 1;
        Ok(None)
    }
    async fn upsert(&self, _entry: &CachedFileEntry) -> Result<(), LlmError> {
        *self.0.lock().unwrap() += 1;
        Ok(())
    }
    async fn invalidate(&self, _id: &str, _provider: ProviderKind) -> Result<(), LlmError> {
        *self.0.lock().unwrap() += 1;
        Ok(())
    }
}

/// Counts downloads.
#[derive(Default)]
struct CountingFetcher(Mutex<usize>);

#[async_trait]
impl SignedUrlFetcher for CountingFetcher {
    async fn stream(&self, _url: &str) -> Result<BoxedByteStream, LlmError> {
        *self.0.lock().unwrap() += 1;
        Ok(Box::pin(futures::stream::once(async {
            Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from_static(b"a\n1\n"))
        })))
    }
}

fn storage_ref_file(id: &str) -> FileData {
    FileData {
        document_id: Some(id.to_string()),
        mime_type: CSV.to_string(),
        filename: "big.csv".to_string(),
        size_hint: Some(LIMIT + 1),
        source: FileSource::StorageRef(format!("chat-attachments/u/s/{id}")),
        retained_inline_bytes: None,
    }
}

#[tokio::test]
async fn resolve_files_passes_a_storage_ref_through_without_download_upload_or_cache() {
    let provider = Arc::new(CountingProvider::default());
    let cache = Arc::new(CountingCache::default());
    let fetcher = CountingFetcher::default();
    let mut files = vec![storage_ref_file("doc-1")];

    LlmCallUseCase::resolve_files(
        &mut files,
        ProviderKind::Anthropic,
        provider.clone(),
        cache.clone(),
        &fetcher,
    )
    .await
    .expect("a storage reference is not an error, and not AllFilesFailedToResolve");

    assert_eq!(files, [storage_ref_file("doc-1")], "left exactly as it was");
    assert!(provider.0.lock().unwrap().is_empty(), "no provider upload");
    assert_eq!(*cache.0.lock().unwrap(), 0, "no cache call");
    assert_eq!(*fetcher.0.lock().unwrap(), 0, "no download");
}

#[tokio::test]
async fn resolve_files_still_resolves_the_other_files_around_a_storage_ref() {
    let provider = Arc::new(CountingProvider::default());
    let signed = FileData {
        document_id: Some("doc-small".to_string()),
        mime_type: "application/pdf".to_string(),
        filename: "small.pdf".to_string(),
        size_hint: Some(1024),
        source: FileSource::SignedUrl("https://example.invalid/small.pdf?sig=x".to_string()),
        retained_inline_bytes: None,
    };
    let mut files = vec![storage_ref_file("doc-1"), signed];

    LlmCallUseCase::resolve_files(
        &mut files,
        ProviderKind::Anthropic,
        provider.clone(),
        Arc::new(CountingCache::default()),
        &CountingFetcher::default(),
    )
    .await
    .unwrap();

    assert_eq!(files.len(), 2, "order and count kept");
    assert_eq!(files[0], storage_ref_file("doc-1"));
    assert!(matches!(files[1].source, FileSource::Uploaded(_)));
    assert_eq!(*provider.0.lock().unwrap(), ["small.pdf"]);
}

/// Fails every download, like a signed URL that answers 404.
struct FailingFetcher;

#[async_trait]
impl SignedUrlFetcher for FailingFetcher {
    async fn stream(&self, _url: &str) -> Result<BoxedByteStream, LlmError> {
        Err(LlmError::RequestFailed {
            message: "404".to_string(),
        })
    }
}

fn signed_url_file(id: &str) -> FileData {
    FileData {
        document_id: Some(id.to_string()),
        mime_type: "application/pdf".to_string(),
        filename: "small.pdf".to_string(),
        size_hint: Some(1024),
        source: FileSource::SignedUrl("https://example.invalid/small.pdf?sig=x".to_string()),
        retained_inline_bytes: None,
    }
}

/// A storage reference needs no resolution, so it must not hide that every file
/// that did need it failed.
#[tokio::test]
async fn resolve_files_still_reports_all_failed_when_a_storage_ref_rides_along() {
    let mut files = vec![storage_ref_file("doc-1"), signed_url_file("doc-2")];
    let err = LlmCallUseCase::resolve_files(
        &mut files,
        ProviderKind::Anthropic,
        Arc::new(CountingProvider::default()),
        Arc::new(CountingCache::default()),
        &FailingFetcher,
    )
    .await
    .expect_err("the only file that needed resolving failed");
    assert!(matches!(err, LlmError::AllFilesFailedToResolve), "{err:?}");
}

#[tokio::test]
async fn resolve_files_keeps_a_storage_ref_when_only_some_other_files_fail() {
    let ok = FileData {
        document_id: Some("doc-4".into()),
        mime_type: "application/pdf".into(),
        filename: "ok.pdf".into(),
        size_hint: Some(1),
        source: FileSource::InlineBytes {
            bytes: b"x".to_vec(),
        },
        retained_inline_bytes: None,
    };
    let mut files = vec![
        storage_ref_file("doc-1"),
        signed_url_file("doc-2"),
        ok,
        storage_ref_file("doc-3"),
    ];
    LlmCallUseCase::resolve_files(
        &mut files,
        ProviderKind::Anthropic,
        Arc::new(CountingProvider::default()),
        Arc::new(CountingCache::default()),
        &FailingFetcher,
    )
    .await
    .expect("one resolvable file succeeded");
    let ids: Vec<_> = files
        .iter()
        .filter_map(|f| f.document_id.as_deref())
        .collect();
    assert_eq!(
        ids,
        ["doc-1", "doc-4", "doc-3"],
        "failed one dropped, order kept"
    );
}

#[test]
fn a_storage_ref_is_registered_with_its_key_as_the_source() {
    let file = storage_ref_file("doc-1");
    let entries = [json!({
        "id": "doc-1",
        "mime_type": CSV,
        "filename": "big.csv",
        "storage_key": "chat-attachments/u/s/doc-1",
        "label": "Sales 2025",
    })];
    let registrations = file_registrations(std::slice::from_ref(&file), &entries, &[0]);
    assert_eq!(registrations.len(), 1);
    assert_eq!(registrations[0].document_id, "doc-1");
    assert_eq!(registrations[0].label.as_deref(), Some("Sales 2025"));
    assert_eq!(
        registrations[0].source,
        AttachmentSource::Path("chat-attachments/u/s/doc-1".to_string())
    );
}

#[test]
fn a_storage_ref_without_an_id_gets_a_generated_one_that_depends_on_its_key() {
    let mut a = storage_ref_file("x");
    a.document_id = None;
    let mut b = a.clone();
    b.source = FileSource::StorageRef("chat-attachments/u/s/other".to_string());
    let ids: Vec<String> = [a, b]
        .iter()
        .map(|f| {
            file_registrations(std::slice::from_ref(f), &[], &[])[0]
                .document_id
                .clone()
        })
        .collect();
    assert_ne!(ids[0], ids[1]);
}
