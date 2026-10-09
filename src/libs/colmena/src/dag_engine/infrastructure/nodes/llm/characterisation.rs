//! Characterisation of the `llm_call` attachment entry points that only this
//! module can reach (slice C0 of the large tabular files chain): how
//! `files[]` entries are parsed, and how `load_attachment` resolves a small
//! row. They pin TODAY's behaviour; the cross-module pins live in
//! `tests/small_file_characterisation.rs`.

use super::{parse_file_entries, AttachmentResolverImpl};
use crate::llm::application::LoadAttachmentResolver;
use crate::llm::domain::attachments::{
    AttachmentRegistry, AttachmentSource, UpsertAttachmentInput,
};
use crate::llm::domain::{FileSource, LlmError, ProviderKind};
use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;
use crate::storage::domain::{MockOutputStorageRepository, StoredBytes};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::sync::Arc;

const MIB: u64 = 1024 * 1024;

fn entries(v: Value) -> Vec<Value> {
    v.as_array().cloned().expect("an array of entries")
}

// ── C0.6: parse_file_entries ────────────────────────────────────────────

#[test]
fn an_entry_with_only_a_storage_key_is_skipped_not_rejected() {
    let arr = entries(json!([{
        "id": "doc-1",
        "mime_type": "text/csv",
        "filename": "big.csv",
        "size_bytes": 60 * MIB,
        "storage_key": "sk-1"
    }]));
    let (files, kept) = parse_file_entries(&arr, false).expect("no error today");
    assert!(
        files.is_empty(),
        "an entry with no data/url/path is dropped"
    );
    assert!(kept.is_empty());
}

#[test]
fn a_dropped_storage_key_entry_does_not_shift_the_kept_indices() {
    let data = STANDARD.encode(b"a,b\n1,2\n");
    let arr = entries(json!([
        {"id": "doc-1", "mime_type": "text/csv", "storage_key": "sk-1"},
        {"id": "doc-2", "mime_type": "text/csv", "filename": "s.csv", "data": data},
    ]));
    let (files, kept) = parse_file_entries(&arr, false).unwrap();
    assert_eq!(kept, [1]);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].document_id.as_deref(), Some("doc-2"));
}

#[test]
fn a_data_entry_parses_into_inline_bytes() {
    let arr = entries(json!([{
        "id": "doc-1",
        "mime_type": "text/csv",
        "filename": "s.csv",
        "size_bytes": 8,
        "data": STANDARD.encode(b"a,b\n1,2\n"),
    }]));
    let (files, kept) = parse_file_entries(&arr, false).unwrap();
    assert_eq!(kept, [0]);
    assert_eq!(files.len(), 1);
    let file = &files[0];
    assert_eq!(file.document_id.as_deref(), Some("doc-1"));
    assert_eq!(file.mime_type, "text/csv");
    assert_eq!(file.filename, "s.csv");
    assert_eq!(file.size_hint, Some(8));
    match &file.source {
        FileSource::InlineBytes { bytes } => assert_eq!(bytes, b"a,b\n1,2\n"),
        other => panic!("expected InlineBytes, got {other:?}"),
    }
}

#[test]
fn a_data_uri_prefix_is_stripped_and_missing_metadata_gets_defaults() {
    let uri = format!("data:text/csv;base64,{}", STANDARD.encode(b"x\n1\n"));
    let arr = entries(json!([{"data": uri}]));
    let (files, _) = parse_file_entries(&arr, false).unwrap();
    assert_eq!(files[0].mime_type, "application/octet-stream");
    assert_eq!(files[0].filename, "upload.file");
    assert_eq!(files[0].document_id, None);
    match &files[0].source {
        FileSource::InlineBytes { bytes } => assert_eq!(bytes, b"x\n1\n"),
        other => panic!("expected InlineBytes, got {other:?}"),
    }
}

#[test]
fn a_url_entry_parses_into_a_signed_url_and_needs_an_id() {
    let arr = entries(json!([{
        "id": "doc-1",
        "mime_type": "text/csv",
        "filename": "big.csv",
        "size_bytes": 40 * MIB,
        "url": "https://storage.example/big.csv?sig=x"
    }]));
    let (files, kept) = parse_file_entries(&arr, false).unwrap();
    assert_eq!(kept, [0]);
    assert_eq!(files[0].size_hint, Some(40 * MIB));
    match &files[0].source {
        FileSource::SignedUrl(url) => assert_eq!(url, "https://storage.example/big.csv?sig=x"),
        other => panic!("expected SignedUrl, got {other:?}"),
    }

    let no_id = entries(json!([{"url": "https://storage.example/x"}]));
    assert!(matches!(
        parse_file_entries(&no_id, false),
        Err(LlmError::UrlWithoutDocumentId)
    ));
}

#[test]
fn a_path_entry_needs_local_mode_and_reads_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("s.csv");
    std::fs::write(&file, b"a\n1\n").unwrap();
    let arr = entries(json!([{"id": "doc-1", "path": file.to_str().unwrap()}]));

    assert!(matches!(
        parse_file_entries(&arr, false),
        Err(LlmError::PathFieldNotAllowed)
    ));
    let (files, kept) = parse_file_entries(&arr, true).unwrap();
    assert_eq!(kept, [0]);
    match &files[0].source {
        FileSource::InlineBytes { bytes } => assert_eq!(bytes, b"a\n1\n"),
        other => panic!("expected InlineBytes, got {other:?}"),
    }
}

#[test]
fn data_wins_over_url_and_the_inline_limit_is_30_mib() {
    let both = entries(json!([{
        "id": "doc-1",
        "data": STANDARD.encode(b"a\n"),
        "url": "https://storage.example/x"
    }]));
    let (files, _) = parse_file_entries(&both, false).unwrap();
    assert!(matches!(files[0].source, FileSource::InlineBytes { .. }));

    let over_hint = entries(json!([{"data": STANDARD.encode(b"a\n"), "size_bytes": 30 * MIB + 1}]));
    assert!(matches!(
        parse_file_entries(&over_hint, false),
        Err(LlmError::DataFieldTooLarge { size }) if size == 30 * MIB + 1
    ));
    let at_hint = entries(json!([{"data": STANDARD.encode(b"a\n"), "size_bytes": 30 * MIB}]));
    assert!(parse_file_entries(&at_hint, false).is_ok());
}

// ── C0.7: load_attachment on a small row ────────────────────────────────

struct Fixture {
    resolver: AttachmentResolverImpl,
}

async fn fixture(
    row_provider_file_id: &str,
    storage_key: Option<&str>,
    storage: MockOutputStorageRepository,
) -> Fixture {
    let registry = Arc::new(
        SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap(),
    );
    registry
        .upsert(UpsertAttachmentInput {
            agent_session_id: "agent_1".to_string(),
            document_id: "doc-1".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: row_provider_file_id.to_string(),
            mime_type: "text/csv".to_string(),
            filename: "small.csv".to_string(),
            size_bytes: Some(8),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            storage_key: storage_key.map(str::to_string),
            origin: Some("user_upload".to_string()),
        })
        .await
        .unwrap();
    Fixture {
        resolver: AttachmentResolverImpl {
            large_tool_served: false,
            registry,
            provider: ProviderKind::OpenAi,
            api_key: "key".to_string(),
            storage: Some(Arc::new(storage)),
        },
    }
}

#[tokio::test]
async fn a_text_row_without_a_provider_file_is_served_inline_from_storage() {
    let mut storage = MockOutputStorageRepository::new();
    storage
        .expect_read()
        .withf(|key| key == "sk-1")
        .times(1)
        .returning(|_| {
            Ok(StoredBytes {
                bytes: b"a,b\n1,2\n".to_vec(),
                mime_type: "application/octet-stream".to_string(),
                filename: "stored-name".to_string(),
            })
        });
    let f = fixture("", Some("sk-1"), storage).await;

    let file = f
        .resolver
        .resolve("agent_1", "doc-1")
        .await
        .unwrap()
        .expect("the row resolves");

    // Mime, filename and size come from the catalog row, bytes from storage.
    assert_eq!(file.document_id.as_deref(), Some("doc-1"));
    assert_eq!(file.mime_type, "text/csv");
    assert_eq!(file.filename, "small.csv");
    assert_eq!(file.size_hint, Some(8));
    match file.source {
        FileSource::InlineBytes { bytes } => assert_eq!(bytes, b"a,b\n1,2\n"),
        other => panic!("expected InlineBytes, got {other:?}"),
    }
    assert!(file.retained_inline_bytes.is_none());
}

#[tokio::test]
async fn a_row_with_a_provider_file_is_returned_as_uploaded_without_reading_storage() {
    // No expectation on `read`: a call would fail the test.
    let f = fixture("pf-1", Some("sk-1"), MockOutputStorageRepository::new()).await;

    let file = f
        .resolver
        .resolve("agent_1", "doc-1")
        .await
        .unwrap()
        .expect("the row resolves");

    match file.source {
        FileSource::Uploaded(r) => {
            assert_eq!(r.provider_file_id, "pf-1");
            assert_eq!(r.provider, ProviderKind::OpenAi);
            assert_eq!(r.mime_type, "text/csv");
            assert_eq!(r.filename, "small.csv");
        }
        other => panic!("expected Uploaded, got {other:?}"),
    }
    assert_eq!(file.size_hint, Some(8));
}

#[tokio::test]
async fn an_unknown_document_resolves_to_nothing() {
    let f = fixture("pf-1", Some("sk-1"), MockOutputStorageRepository::new()).await;
    let missing = f.resolver.resolve("agent_1", "doc-missing").await.unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn a_text_row_with_no_storage_key_is_a_tool_error() {
    let f = fixture("", None, MockOutputStorageRepository::new()).await;
    let err = f.resolver.resolve("agent_1", "doc-1").await.unwrap_err();
    assert_eq!(
        err,
        "load_attachment: attachment 'doc-1' has no provider_file_id and no storage_key \
         — cannot resolve bytes (text attachment was not persisted)"
    );
}
