//! `load_attachment` and a row that references an object the HOST owns
//! (`origin = host_storage_ref`).
//!
//! The check is made on the row `AttachmentResolverImpl` has ALREADY looked up for
//! the current provider, at the point it is about to use it, so the row judged is
//! by construction the row read or uploaded. There is no second lookup: a path
//! that touched the registry once before still touches it once, and every error
//! for any other row is the text it always was.

use super::node_harness::CountingStorage;
use super::AttachmentResolverImpl;
use crate::llm::application::LoadAttachmentResolver;
use crate::llm::domain::attachments::attachment_registry::MockAttachmentRegistry;
use crate::llm::domain::attachments::{
    origin, AttachmentError, AttachmentSource, ConversationAttachment, UpsertAttachmentInput,
};
use crate::llm::domain::large_tabular::refusal_text;
use crate::llm::domain::{AttachmentRegistry, FileSource, ProviderKind};
use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;
use crate::storage::domain::{OutputStorageRepository, StoreRequest};
use chrono::Utc;
use std::sync::Arc;

const MIB: u64 = 1024 * 1024;

fn input(
    key: &str,
    provider: ProviderKind,
    host: bool,
    mime: &str,
    size: Option<u64>,
) -> UpsertAttachmentInput {
    UpsertAttachmentInput {
        agent_session_id: "agent_1".to_string(),
        document_id: "doc-1".to_string(),
        provider,
        // Empty: a text-like row is served from its stored bytes, the read path.
        provider_file_id: String::new(),
        mime_type: mime.to_string(),
        filename: "big.csv".to_string(),
        size_bytes: size,
        label: None,
        description: None,
        source: AttachmentSource::Path(key.to_string()),
        storage_key: Some(key.to_string()),
        origin: Some(
            if host {
                origin::HOST_STORAGE_REF
            } else {
                origin::USER_UPLOAD
            }
            .to_string(),
        ),
    }
}

async fn store(storage: &CountingStorage, mime: &str) -> String {
    storage
        .store(StoreRequest {
            bytes: b"a,b\n1,2\n".to_vec(),
            mime_type: mime.to_string(),
            filename: "big.csv".to_string(),
            session_id: None,
            agent_session_id: Some("agent_1".to_string()),
        })
        .await
        .unwrap()
        .storage_key
}

fn resolver(
    registry: Arc<dyn AttachmentRegistry>,
    storage: &Arc<CountingStorage>,
    provider: ProviderKind,
) -> AttachmentResolverImpl {
    AttachmentResolverImpl {
        large_tool_served: false,
        registry,
        provider,
        api_key: "key".to_string(),
        storage: Some(storage.clone()),
    }
}

async fn sqlite() -> Arc<SqliteAttachmentRegistry> {
    Arc::new(
        SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap(),
    )
}

#[tokio::test]
async fn a_host_row_for_the_current_provider_is_refused_before_any_read() {
    for (mime, size) in [
        ("text/csv", Some(60 * MIB)),
        ("text/csv", Some(8)),
        ("text/csv", None),
        ("application/pdf", Some(90 * MIB)),
    ] {
        let storage = Arc::new(CountingStorage::default());
        let key = store(&storage, mime).await;
        let reg = sqlite().await;
        reg.upsert(input(&key, ProviderKind::OpenAi, true, mime, size))
            .await
            .unwrap();
        let err = resolver(reg, &storage, ProviderKind::OpenAi)
            .resolve("agent_1", "doc-1")
            .await
            .unwrap_err();
        assert_eq!(err, refusal_text(), "{mime} {size:?}");
        assert_eq!(storage.reads(), 0, "{mime} {size:?}");
    }
}

/// The two-provider state, in both insertion orders: the host row under one
/// provider and an ordinary row under the other. The row judged is the row read.
#[tokio::test]
async fn the_row_judged_is_the_row_read_with_two_providers_in_either_order() {
    for host_first in [true, false] {
        let storage = Arc::new(CountingStorage::default());
        let host_key = store(&storage, "text/csv").await;
        let copy_key = store(&storage, "text/csv").await;
        let reg = sqlite().await;
        let host_row = input(
            &host_key,
            ProviderKind::OpenAi,
            true,
            "text/csv",
            Some(60 * MIB),
        );
        let copy_row = input(
            &copy_key,
            ProviderKind::Anthropic,
            false,
            "text/csv",
            Some(8),
        );
        if host_first {
            reg.upsert(host_row).await.unwrap();
            reg.upsert(copy_row).await.unwrap();
        } else {
            reg.upsert(copy_row).await.unwrap();
            reg.upsert(host_row).await.unwrap();
        }

        // A turn on the host row's provider: refused, nothing read.
        let on_host = resolver(reg.clone(), &storage, ProviderKind::OpenAi);
        assert_eq!(
            on_host.resolve("agent_1", "doc-1").await.unwrap_err(),
            refusal_text(),
            "host_first={host_first}"
        );
        assert_eq!(storage.reads(), 0, "host_first={host_first}");

        // A turn on the other provider reads ITS row (the engine's copy).
        let on_copy = resolver(reg.clone(), &storage, ProviderKind::Anthropic);
        let file = on_copy.resolve("agent_1", "doc-1").await.unwrap().unwrap();
        assert!(matches!(file.source, FileSource::InlineBytes { .. }));
        assert_eq!(storage.reads(), 1, "host_first={host_first}");
    }
}

/// The Generated fallback and the lazy cross-provider upload read the object
/// whole: a host row reaching them is refused before the read.
#[tokio::test]
async fn a_host_row_reaching_the_generated_fallback_is_refused_before_the_lazy_upload() {
    let storage = Arc::new(CountingStorage::default());
    let key = store(&storage, "text/csv").await;
    let reg = sqlite().await;
    let mut row = input(
        &key,
        ProviderKind::Generated,
        true,
        "text/csv",
        Some(60 * MIB),
    );
    row.provider_file_id = key.clone();
    reg.upsert(row).await.unwrap();
    let err = resolver(reg, &storage, ProviderKind::Anthropic)
        .resolve("agent_1", "doc-1")
        .await
        .unwrap_err();
    assert_eq!(err, refusal_text());
    assert_eq!(storage.reads(), 0);
}

/// Engine rows resolve exactly as before, at any size.
#[tokio::test]
async fn an_engine_stored_row_resolves_as_it_always_did() {
    for size in [Some(8), Some(60 * MIB), Some(400 * MIB), None] {
        let storage = Arc::new(CountingStorage::default());
        let key = store(&storage, "text/csv").await;
        let reg = sqlite().await;
        reg.upsert(input(&key, ProviderKind::OpenAi, false, "text/csv", size))
            .await
            .unwrap();
        let file = resolver(reg, &storage, ProviderKind::OpenAi)
            .resolve("agent_1", "doc-1")
            .await
            .unwrap_or_else(|e| panic!("{size:?}: {e}"))
            .expect("the row resolves");
        assert!(matches!(file.source, FileSource::InlineBytes { .. }));
        assert_eq!(storage.reads(), 1, "{size:?}");
    }
}

fn row_for_mock(provider_file_id: &str, key: &str) -> ConversationAttachment {
    ConversationAttachment {
        agent_session_id: "agent_1".into(),
        document_id: "doc-1".into(),
        provider: ProviderKind::OpenAi,
        provider_file_id: provider_file_id.into(),
        mime_type: "text/csv".into(),
        filename: "big.csv".into(),
        size_bytes: Some(8),
        label: None,
        description: None,
        source: AttachmentSource::Inline,
        registered_at: Utc::now(),
        refreshed_at: Utc::now(),
        storage_key: Some(key.into()),
        origin: Some(origin::USER_UPLOAD.into()),
        last_used_at: None,
    }
}

/// Query count and sibling-row failure (M3): a load of an ordinary row asks the
/// registry exactly what it asked before (one lookup for the current provider,
/// one touch) and NEVER the cross-provider lookup; a registry whose cross-provider
/// lookup fails (a decode error on a sibling provider's row) still loads the file.
#[tokio::test]
async fn a_load_makes_the_same_registry_calls_as_before_and_ignores_sibling_rows() {
    let storage = Arc::new(CountingStorage::default());
    let key = store(&storage, "text/csv").await;
    let mut registry = MockAttachmentRegistry::new();
    let row = row_for_mock("", &key);
    registry
        .expect_lookup()
        .times(1)
        .returning(move |_, _, _| Ok(Some(row.clone())));
    registry
        .expect_touch_last_used()
        .times(1)
        .returning(|_, _| Ok(()));
    registry
        .expect_lookup_by_document_id()
        .times(0)
        .returning(|_, _| {
            Err(AttachmentError::RepositoryFailed(
                "sibling row undecodable".into(),
            ))
        });
    let file = resolver(Arc::new(registry), &storage, ProviderKind::OpenAi)
        .resolve("agent_1", "doc-1")
        .await
        .expect("the load succeeds")
        .expect("the row resolves");
    assert!(matches!(file.source, FileSource::InlineBytes { .. }));
    // Mock expectations (times) are verified when the registry drops.
}

/// A registry failure on the lookup the loader always made keeps its text.
#[tokio::test]
async fn a_registry_failure_keeps_the_text_it_always_had() {
    let storage = Arc::new(CountingStorage::default());
    let mut registry = MockAttachmentRegistry::new();
    registry
        .expect_lookup()
        .times(1)
        .returning(|_, _, _| Err(AttachmentError::RepositoryFailed("db down".into())));
    let err = resolver(Arc::new(registry), &storage, ProviderKind::OpenAi)
        .resolve("agent_1", "doc-1")
        .await
        .unwrap_err();
    assert_eq!(
        err,
        AttachmentError::RepositoryFailed("db down".into()).to_string()
    );
    assert_eq!(storage.reads(), 0);
}
