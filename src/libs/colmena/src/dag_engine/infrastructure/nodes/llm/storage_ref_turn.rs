//! A large tabular file that arrives as `storage_key` only: the turn registers
//! it in the attachment catalog under the host's key, and moves no byte. These
//! tests run the real `llm_call` node, as the engine's registry builds it, with
//! a real SQLite attachment registry and a real in-memory storage that counts.

use super::node_harness::{
    registry_with_storage, run_turn, run_turn_with_tools, CountingStorage, RecordingModel,
};
use super::{summary_target, SummaryTarget};
use crate::llm::domain::attachments::AttachmentSource;
use crate::llm::domain::large_tabular::refusal_text;
use crate::llm::domain::{AttachmentRegistry, FileData, FileSource, ProviderKind};
use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;
use crate::llm::infrastructure::ScriptedResponse;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::sync::Arc;

const MIB: u64 = 1024 * 1024;
const LIMIT: u64 = 50 * MIB;
const KEY: &str = "chat-attachments/u/s/prepared-source.csv";

fn entry(id: &str, size: u64) -> Value {
    json!({
        "id": id, "mime_type": "text/csv", "filename": "sales.csv",
        "size_bytes": size, "storage_key": KEY, "label": "Sales 2025",
    })
}

struct Turn {
    model: Arc<RecordingModel>,
    storage: Arc<CountingStorage>,
    attachments: SqliteAttachmentRegistry,
    _db: tempfile::NamedTempFile,
}

impl Turn {
    async fn run(switch: bool, files: Vec<Value>) -> Turn {
        let db = tempfile::NamedTempFile::new().unwrap();
        let url = format!("sqlite://{}", db.path().display());
        let storage = Arc::new(CountingStorage::default());
        let reg = registry_with_storage(Some(storage.clone()));
        reg.set_large_tabular(switch);
        let model = RecordingModel::new(4);
        run_turn(&reg, &url, files, &model).await.unwrap();
        Turn {
            model,
            storage,
            attachments: SqliteAttachmentRegistry::new(&url).await.unwrap(),
            _db: db,
        }
    }

    async fn row(&self, id: &str) -> Option<crate::llm::domain::ConversationAttachment> {
        self.attachments
            .lookup_by_document_id("agent_1", id)
            .await
            .unwrap()
    }
}

#[tokio::test]
#[serial_test::serial]
async fn a_large_key_only_file_is_registered_by_its_key_and_nothing_moves() {
    let turn = Turn::run(true, vec![entry("doc-big", LIMIT + 1)]).await;

    let row = turn.row("doc-big").await.expect("registered");
    assert_eq!(row.provider, ProviderKind::OpenAi);
    assert_eq!(row.provider_file_id, "", "never uploaded to a provider");
    assert_eq!(
        row.storage_key.as_deref(),
        Some(KEY),
        "the host's key as given"
    );
    assert_eq!(row.source, AttachmentSource::Path(KEY.to_string()));
    assert_eq!(row.size_bytes, Some(LIMIT + 1));
    assert_eq!(row.mime_type, "text/csv");
    assert_eq!(row.filename, "sales.csv");
    assert_eq!(row.label.as_deref(), Some("Sales 2025"));
    assert_eq!(
        row.origin.as_deref(),
        Some(crate::llm::domain::attachments::origin::HOST_STORAGE_REF),
        "marked as an object the host owns"
    );
    assert_eq!(row.description, None, "no auto-summary");

    assert_eq!(turn.storage.stores(), 0, "no stored copy");
    assert_eq!(turn.storage.reads(), 0, "no object read");
    assert_eq!(
        turn.model.files_seen(),
        0,
        "no provider adapter sees a file"
    );
    assert_eq!(
        turn.model.calls(),
        1,
        "the answer call only: no summary call"
    );
    let seen = turn.model.seen();
    assert!(seen.contains("doc-big"), "the model learns of it: {seen}");
    assert!(seen.contains("sales.csv"), "{seen}");
}

#[tokio::test]
#[serial_test::serial]
async fn the_boundary_and_the_switch_decide_whether_it_is_registered() {
    for (switch, size, registered) in [
        (true, LIMIT - 1, false),
        (true, LIMIT, false),
        (true, LIMIT + 1, true),
        (false, LIMIT + 1, false),
        (false, 400 * MIB, false),
    ] {
        let turn = Turn::run(switch, vec![entry("doc-1", size)]).await;
        assert_eq!(
            turn.row("doc-1").await.is_some(),
            registered,
            "switch {switch}, size {size}"
        );
        assert_eq!(turn.storage.reads() + turn.storage.stores(), 0);
    }
}

#[tokio::test]
#[serial_test::serial]
async fn a_small_file_beside_a_large_one_takes_the_small_path() {
    let small = json!({
        "id": "doc-small", "mime_type": "text/csv", "filename": "s.csv",
        "size_bytes": 8, "data": STANDARD.encode(b"a,b\n1,2\n"),
    });
    let turn = Turn::run(true, vec![entry("doc-big", LIMIT + 1), small]).await;

    assert_eq!(turn.storage.stores(), 1, "only the small file is copied");
    let small = turn.row("doc-small").await.expect("small registered");
    assert_ne!(small.storage_key.as_deref(), Some(KEY));
    assert!(small.storage_key.is_some(), "its stored copy");
    let big = turn.row("doc-big").await.expect("large registered");
    assert_eq!(big.storage_key.as_deref(), Some(KEY));
    assert_eq!(big.description, None);
}

fn storage_ref(key: &str) -> FileData {
    FileData {
        document_id: Some("doc-1".into()),
        mime_type: "text/csv".into(),
        filename: "big.csv".into(),
        size_hint: Some(LIMIT + 1),
        source: FileSource::StorageRef(key.into()),
        retained_inline_bytes: None,
    }
}

/// The summary reads its source back (`acquire_bytes` reads a `Path` source from
/// the local disk in local mode), so a storage reference must never become a
/// summary target, whatever its key names.
#[test]
fn a_storage_ref_is_never_a_summary_target_even_if_its_key_names_a_local_file() {
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("secret.csv");
    std::fs::write(&local, b"LOCAL-FILE-CONTENT").unwrap();
    let key = local.to_str().unwrap();
    let file = storage_ref(key);
    let source = AttachmentSource::Path(key.to_string());
    assert!(summary_target(&file, "doc-1", source).is_none());
}

#[test]
fn the_other_sources_still_become_summary_targets() {
    let mut signed = storage_ref("k");
    signed.source = FileSource::SignedUrl("https://example.invalid/x".into());
    let target: Option<SummaryTarget> = summary_target(
        &signed,
        "doc-1",
        AttachmentSource::SignedUrl("https://example.invalid/x".into()),
    );
    assert!(target.is_some());

    let mut inline = storage_ref("k");
    inline.source = FileSource::InlineBytes {
        bytes: b"a".to_vec(),
    };
    inline.retained_inline_bytes = Some(b"a".to_vec());
    assert!(summary_target(&inline, "doc-1", AttachmentSource::Inline).is_some());
    inline.retained_inline_bytes = None;
    assert!(
        summary_target(&inline, "doc-1", AttachmentSource::Inline).is_none(),
        "inline without retained bytes: nothing to read, as before"
    );
}

/// K2, end to end: a file registered while the switch was on is still refused,
/// by `load_attachment` and by a whole-object tool, after the switch is turned
/// OFF (a rollback); and nothing is read.
#[tokio::test]
#[serial_test::serial]
async fn a_registered_file_is_still_refused_after_the_switch_is_turned_off() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let url = format!("sqlite://{}", db.path().display());
    let storage = Arc::new(CountingStorage::default());
    let reg = registry_with_storage(Some(storage.clone()));

    reg.set_large_tabular(true);
    run_turn(
        &reg,
        &url,
        vec![entry("doc-big", LIMIT + 1)],
        &RecordingModel::new(2),
    )
    .await
    .unwrap();
    assert_eq!(storage.reads() + storage.stores(), 0);

    reg.set_large_tabular(false);
    let model = RecordingModel::scripted(vec![
        ScriptedResponse::ToolCall {
            id: "c1".into(),
            tool_name: "load_attachment".into(),
            arguments: json!({"document_id": "doc-big"}),
        },
        ScriptedResponse::ToolCall {
            id: "c2".into(),
            tool_name: "attachment_run_python".into(),
            arguments: json!({"attachment_id": "doc-big", "code": "print(len(df))"}),
        },
        ScriptedResponse::Text("ok".into()),
    ]);
    run_turn_with_tools(
        &reg,
        &url,
        vec![],
        json!({"attachment_run_python": {"node_type": "attachment_run_python"}}),
        &model,
    )
    .await
    .unwrap();

    let seen = model.seen();
    assert!(seen.contains(refusal_text()), "{seen}");
    assert!(
        seen.contains("large_tabular_file"),
        "own error code: {seen}"
    );
    assert!(!seen.contains("expired"), "{seen}");
    assert_eq!(storage.reads(), 0, "the host's object was never read");
    assert_eq!(model.files_seen(), 0, "and never sent to the provider");
}

/// K6: a storage reference in a turn that cannot register it (no agent session,
/// so no catalog) is reported, not dropped silently.
#[tokio::test]
#[serial_test::serial]
async fn a_storage_ref_that_cannot_be_registered_is_reported() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let url = format!("sqlite://{}", db.path().display());
    let storage = Arc::new(CountingStorage::default());
    let reg = registry_with_storage(Some(storage.clone()));
    reg.set_large_tabular(true);
    let model = RecordingModel::new(2);
    super::node_harness::run_turn_without_session(
        &reg,
        &url,
        vec![entry("doc-big", LIMIT + 1)],
        &model,
    )
    .await
    .unwrap();
    let seen = model.seen();
    assert!(seen.contains("[file: sales.csv]"), "{seen}");
    assert!(seen.contains("not delivered"), "{seen}");
    assert!(seen.contains("no attachment catalog"), "the reason: {seen}");
    assert!(!seen.contains(KEY), "no key: {seen}");
    assert_eq!(storage.reads() + storage.stores(), 0);
}
