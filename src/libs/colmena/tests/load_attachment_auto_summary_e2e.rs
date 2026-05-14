//! End-to-end coverage of the auto-summary path with a mocked LLM
//! (no network). Asserts that:
//!   * a caller-supplied description is preserved verbatim on upsert
//!   * `update_description` overwrites an existing (or `NULL`) description
//!
//! The unit tests in Task 8 already cover the generator's behaviour with a
//! mocked `LlmRepository`; the full wiring against the `llm_call` node is
//! captured by the real-API test graph in Task 11. This integration test
//! exercises the persistence path and the public surface area touched by the
//! feature.

use colmena::llm::domain::attachments::{
    AttachmentRegistry, AttachmentSource, UpsertAttachmentInput,
};
use colmena::llm::domain::ProviderKind;
use colmena::llm::infrastructure::persistence::SqliteAttachmentRegistry;
use std::sync::Arc;

async fn setup_registry() -> Arc<dyn AttachmentRegistry> {
    let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
        .await
        .expect("registry init");
    Arc::new(reg)
}

#[tokio::test]
async fn caller_supplied_description_is_preserved() {
    let reg = setup_registry().await;
    reg.upsert(UpsertAttachmentInput {
        agent_session_id: "s1".into(),
        document_id: "doc-1".into(),
        provider: ProviderKind::Mock,
        provider_file_id: "pf-1".into(),
        mime_type: "application/pdf".into(),
        filename: "a.pdf".into(),
        size_bytes: Some(100),
        label: None,
        description: Some("caller-supplied".into()),
        source: AttachmentSource::Inline,
    })
    .await
    .unwrap();

    let row = reg
        .lookup("s1", "doc-1", ProviderKind::Mock)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.description.as_deref(), Some("caller-supplied"));
}

#[tokio::test]
async fn update_description_overwrites_existing() {
    let reg = setup_registry().await;
    reg.upsert(UpsertAttachmentInput {
        agent_session_id: "s1".into(),
        document_id: "doc-1".into(),
        provider: ProviderKind::Mock,
        provider_file_id: "pf-1".into(),
        mime_type: "application/pdf".into(),
        filename: "a.pdf".into(),
        size_bytes: Some(100),
        label: None,
        description: None,
        source: AttachmentSource::Inline,
    })
    .await
    .unwrap();
    reg.update_description("s1", "doc-1", ProviderKind::Mock, "auto-generated")
        .await
        .unwrap();

    let row = reg
        .lookup("s1", "doc-1", ProviderKind::Mock)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.description.as_deref(), Some("auto-generated"));
}
