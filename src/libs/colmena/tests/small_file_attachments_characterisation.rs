//! Characterisation of today's small-file attachment size behaviour (slice C0b
//! of the large tabular files chain): how `resolve_files` treats a CSV around
//! 50 MiB by size hint, and where the 50 MiB byte cap of the tabular readers,
//! the summary builder and the SQL bulk insert applies.
//!
//! Every test pins what the code does NOW. Nothing here changes behaviour.

use async_trait::async_trait;
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::sql_bulk_tools::{
    build_tabular_summary, execute_bulk_insert_csv_postgres_copy, parse_attachment_to_records,
    parse_inspect_bytes_with_filename, BulkInsertArgs, InspectArgs,
};
use colmena::llm::application::LlmCallUseCase;
use colmena::llm::domain::{
    BoxedByteStream, CachedFileEntry, FileCacheRepository, FileData, FileProviderRepository,
    FileSource, LlmError, ProviderFileRef, ProviderKind, SignedUrlFetcher,
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MIB: usize = 1024 * 1024;
const CSV_MIME: &str = "text/csv";

/// One header cell and one data cell padded so the file is exactly `total` bytes.
fn csv_of_exact_size(total: usize) -> Vec<u8> {
    let mut csv = b"a\n".to_vec();
    csv.resize(total, b'x');
    csv
}

// ── C0.5: resolve_files has no size threshold ───────────────────────────

struct CountingProvider {
    uploads: Mutex<usize>,
}

#[async_trait]
impl FileProviderRepository for CountingProvider {
    async fn upload_streaming(
        &self,
        _stream: BoxedByteStream,
        mime_type: &str,
        filename: &str,
    ) -> Result<ProviderFileRef, LlmError> {
        let mut n = self.uploads.lock().unwrap();
        *n += 1;
        Ok(ProviderFileRef {
            provider: ProviderKind::Anthropic,
            provider_file_id: format!("uploaded-{n}"),
            mime_type: mime_type.to_string(),
            filename: filename.to_string(),
            expires_at: None,
        })
    }

    fn ttl(&self) -> Option<Duration> {
        None
    }

    fn provider(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }
}

#[derive(Default)]
struct RecordingCache {
    entries: Mutex<Vec<CachedFileEntry>>,
}

#[async_trait]
impl FileCacheRepository for RecordingCache {
    async fn lookup(
        &self,
        _document_id: &str,
        _provider: ProviderKind,
    ) -> Result<Option<CachedFileEntry>, LlmError> {
        Ok(None)
    }

    async fn upsert(&self, entry: &CachedFileEntry) -> Result<(), LlmError> {
        self.entries.lock().unwrap().push(entry.clone());
        Ok(())
    }

    async fn invalidate(&self, _id: &str, _provider: ProviderKind) -> Result<(), LlmError> {
        Ok(())
    }
}

/// Serves a few bytes whatever the size hint claims: only hints are in play.
struct TinyFetcher;

#[async_trait]
impl SignedUrlFetcher for TinyFetcher {
    async fn stream(&self, _url: &str) -> Result<BoxedByteStream, LlmError> {
        Ok(Box::pin(futures::stream::once(async {
            Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from_static(b"a,b\n1,2\n"))
        })))
    }
}

#[tokio::test]
async fn resolve_files_treats_a_csv_the_same_on_both_sides_of_50_mib() {
    for hint in [50 * MIB as u64 - 1, 50 * MIB as u64, 50 * MIB as u64 + 1] {
        let mut files = vec![FileData {
            document_id: Some("doc-csv".to_string()),
            mime_type: CSV_MIME.to_string(),
            filename: "big.csv".to_string(),
            size_hint: Some(hint),
            source: FileSource::SignedUrl("https://example.invalid/big.csv?sig=x".to_string()),
            retained_inline_bytes: None,
        }];
        let provider = Arc::new(CountingProvider {
            uploads: Mutex::new(0),
        });
        let cache = Arc::new(RecordingCache::default());
        LlmCallUseCase::resolve_files(
            &mut files,
            ProviderKind::Anthropic,
            provider.clone(),
            cache.clone(),
            &TinyFetcher,
        )
        .await
        .unwrap_or_else(|e| panic!("hint {hint}: {e}"));

        assert_eq!(
            *provider.uploads.lock().unwrap(),
            1,
            "hint {hint}: uploaded"
        );
        assert_eq!(files.len(), 1, "hint {hint}");
        match &files[0].source {
            FileSource::Uploaded(r) => {
                assert_eq!(r.provider_file_id, "uploaded-1", "hint {hint}");
                assert_eq!(r.mime_type, CSV_MIME, "hint {hint}");
            }
            other => panic!("hint {hint}: expected Uploaded, got {other:?}"),
        }
        assert_eq!(files[0].size_hint, Some(hint), "hint {hint}: hint kept");
        let cached = cache.entries.lock().unwrap();
        assert_eq!(cached.len(), 1, "hint {hint}: one cache row");
        assert_eq!(cached[0].size_bytes, Some(hint as i64), "hint {hint}");
    }
}

// ── C0.7: the 50 MiB byte cap of the SQL bulk tools and the summary ─────

#[test]
fn tabular_readers_accept_exactly_50_mib_and_refuse_one_byte_more() {
    let at_cap = csv_of_exact_size(50 * MIB);
    let over_cap = csv_of_exact_size(50 * MIB + 1);

    let (columns, records) =
        parse_attachment_to_records(&at_cap, CSV_MIME, "f.csv", None, None, None, 100_000)
            .expect("50 MiB loads");
    assert_eq!((columns, records.len()), (vec!["a".to_string()], 1));
    let err = parse_attachment_to_records(&over_cap, CSV_MIME, "f.csv", None, None, None, 100_000)
        .unwrap_err();
    assert_eq!(err, "attachment too large: 52428801 bytes > limit 50 MB");

    let args = InspectArgs {
        attachment_id: String::new(),
        sample_rows: Some(3),
        delimiter: None,
        sheet_name: None,
        header_row: None,
        target_table: None,
    };
    let inspected = parse_inspect_bytes_with_filename(&at_cap, CSV_MIME, "f.csv", &args)
        .expect("50 MiB inspects");
    assert_eq!(inspected.total_rows, 1);
    let err = parse_inspect_bytes_with_filename(&over_cap, CSV_MIME, "f.csv", &args).unwrap_err();
    assert_eq!(
        err,
        "attachment too large: 52428801 bytes > limit 52428800 bytes (50 MB)"
    );

    let summary = build_tabular_summary(CSV_MIME, "f.csv", &at_cap).expect("summary at the cap");
    assert!(
        summary.starts_with("CSV, 1 cols × 1 rows"),
        "got: {summary}"
    );
    assert_eq!(build_tabular_summary(CSV_MIME, "f.csv", &over_cap), None);
}

#[tokio::test]
async fn sql_bulk_insert_refuses_over_50_mib_before_touching_the_database() {
    let args: BulkInsertArgs = serde_json::from_value(json!({
        "attachment_id": "doc-1",
        "table": "public.products",
        "column_mapping": {"a": "a"},
    }))
    .unwrap();
    let allowed = vec!["reporting".to_string()];

    let err = execute_bulk_insert_csv_postgres_copy(
        "postgres://invalid.invalid/db",
        &args,
        &allowed,
        &csv_of_exact_size(50 * MIB + 1),
    )
    .await
    .unwrap_err();
    assert_eq!(err, "attachment too large: 52428801 bytes > limit 50 MB");

    // At the cap the size check passes and the next check (the schema
    // allow-list) answers instead, still before any connection is made.
    let err = execute_bulk_insert_csv_postgres_copy(
        "postgres://invalid.invalid/db",
        &args,
        &allowed,
        &csv_of_exact_size(50 * MIB),
    )
    .await
    .unwrap_err();
    assert!(!err.contains("attachment too large"), "got: {err}");
}
