//! The guard at the lowest shared point of tool reads: every tool that reads an
//! attachment whole (`sql_inspect_attachment`, `sql_bulk_insert_from_attachment`,
//! `attachment_run_python`, `data_run_python`, the gsheets and gdocs importers)
//! goes through `DagToolExecutor::fetch_attachment_bytes`. A row that references
//! an object the HOST owns is refused there BEFORE the storage is touched,
//! whatever the large tabular switch says now and whatever the row's size or mime
//! say: a refusal with a code, not an out-of-memory worker. Rows the engine
//! stored itself keep today's behaviour at every size.

use super::DagToolExecutor;
use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::domain::node::ExecutableNode;
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_run_python::dispatch_attachment_run_python_via_executor;
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::gdocs_tools::dispatch_create_from_docx_via_executor;
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::gsheets_tools::dispatch_create_from_xlsx_via_executor;
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::sql_bulk_tools::{
    dispatch_sql_bulk_insert_from_attachment_via_executor,
    dispatch_sql_inspect_attachment_via_executor,
};
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::tabular_bindings::{
    resolve_bindings, AttachmentFetcher, DataBinding,
};
use crate::llm::domain::attachments::attachment_registry::MockAttachmentRegistry;
use crate::llm::domain::attachments::{origin, AttachmentError, AttachmentSource};
use crate::llm::domain::large_tabular::{refusal_text, LARGE_TABULAR_ERROR_CODE};
use crate::llm::domain::{ConversationAttachment, FunctionCall, ProviderKind, ToolCall};
use crate::storage::domain::{
    MockOutputStorageRepository, StorageError, StoredBytes, StoredStream,
};
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;

const MIB: u64 = 1024 * 1024;

struct NoNodes;
impl NodeRegistryPort for NoNodes {
    fn get_node(&self, _: &str) -> Option<Arc<dyn ExecutableNode>> {
        None
    }
    fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
        HashMap::new()
    }
}

/// A row the engine registered for a storage reference: the host's key, no
/// provider file, and the marker that says the object is not ours.
fn host_ref(mime: &str, size: Option<u64>) -> ConversationAttachment {
    ConversationAttachment {
        agent_session_id: "agent_1".into(),
        document_id: "doc-1".into(),
        provider: ProviderKind::OpenAi,
        provider_file_id: String::new(),
        mime_type: mime.into(),
        filename: "big.csv".into(),
        size_bytes: size,
        label: None,
        description: None,
        source: AttachmentSource::Path("host-key".into()),
        registered_at: Utc::now(),
        refreshed_at: Utc::now(),
        storage_key: Some("host-key".into()),
        origin: Some(origin::HOST_STORAGE_REF.into()),
        last_used_at: None,
    }
}

/// A row for a copy the engine stored itself (uploaded to a provider too).
fn engine_copy(mime: &str, size: Option<u64>) -> ConversationAttachment {
    ConversationAttachment {
        provider_file_id: "pf-1".into(),
        source: AttachmentSource::SignedUrl("https://example.invalid/x".into()),
        storage_key: Some("copy-key".into()),
        origin: Some(origin::USER_UPLOAD.into()),
        ..host_ref(mime, size)
    }
}

fn storage_that_reads(times: usize) -> MockOutputStorageRepository {
    let mut storage = MockOutputStorageRepository::new();
    storage.expect_read().times(times).returning(|_| {
        Ok(StoredBytes {
            bytes: b"a,b\n1,2\n".to_vec(),
            mime_type: "text/csv".into(),
            filename: "big.csv".into(),
        })
    });
    storage
}

fn executor(
    catalog_row: ConversationAttachment,
    storage: MockOutputStorageRepository,
) -> DagToolExecutor {
    DagToolExecutor::new(Arc::new(NoNodes), Default::default())
        .with_attachments(vec![catalog_row])
        .with_attachment_storage(Arc::new(storage))
}

/// The marker decides, not the size or the mime: a host reference is refused
/// with a size over, at, below the old threshold, unknown, or a non-tabular mime.
/// The tool has no switch to consult, so "the switch is off now" is covered too.
#[tokio::test]
async fn a_host_reference_is_refused_whatever_its_size_or_mime_before_storage_is_touched() {
    for (mime, size) in [
        ("text/csv", Some(60 * MIB)),
        ("text/csv", Some(8)),
        ("text/csv", None),
        ("application/pdf", Some(90 * MIB)),
        ("application/octet-stream", None),
    ] {
        // No `read` expectation: reaching storage would panic the test.
        let ex = executor(host_ref(mime, size), MockOutputStorageRepository::new());
        let err = ex.fetch_attachment_bytes("doc-1").await.unwrap_err();
        assert_eq!(err, refusal_text(), "{mime} {size:?}");
    }
}

/// K3: a copy the engine stored and read whole today keeps being read, at any
/// size; the tools' own 50 MiB cap on actual bytes still answers for it.
#[tokio::test]
async fn an_engine_copy_is_read_as_it_always_was_at_every_size() {
    for size in [Some(8), Some(50 * MIB + 1), Some(90 * MIB), None] {
        let ex = executor(engine_copy("text/csv", size), storage_that_reads(1));
        ex.fetch_attachment_bytes("doc-1")
            .await
            .unwrap_or_else(|e| panic!("{size:?}: {e}"));
    }
}

/// A row known only to the live registry is judged on the row that same lookup
/// returned: ONE `lookup_by_document_id` (what develop makes on a catalog miss, no
/// more) and the `touch_last_used` it always did.
#[tokio::test]
async fn a_row_known_only_to_the_live_registry_is_judged_by_the_one_lookup() {
    let mut registry = MockAttachmentRegistry::new();
    registry
        .expect_lookup_by_document_id()
        .times(1)
        .returning(|_, _| Ok(Some(host_ref("text/csv", Some(60 * MIB)))));
    registry
        .expect_touch_last_used()
        .times(1)
        .returning(|_, _| Ok(()));
    let ex = DagToolExecutor::new(Arc::new(NoNodes), Default::default())
        .with_attachment_registry(Arc::new(registry))
        .with_agent_session_id(Some("agent_1".to_string()))
        .with_attachment_storage(Arc::new(MockOutputStorageRepository::new()));
    assert_eq!(
        ex.fetch_attachment_bytes("doc-1").await.unwrap_err(),
        refusal_text()
    );

    // An engine row on the same path: one lookup, one touch, one read.
    let mut registry = MockAttachmentRegistry::new();
    registry
        .expect_lookup_by_document_id()
        .times(1)
        .returning(|_, _| Ok(Some(engine_copy("text/csv", Some(90 * MIB)))));
    registry
        .expect_touch_last_used()
        .times(1)
        .returning(|_, _| Ok(()));
    let ex = DagToolExecutor::new(Arc::new(NoNodes), Default::default())
        .with_attachment_registry(Arc::new(registry))
        .with_agent_session_id(Some("agent_1".to_string()))
        .with_attachment_storage(Arc::new(storage_that_reads(1)));
    ex.fetch_attachment_bytes("doc-1").await.unwrap();
}

/// A registry failure keeps the text it always had (no new prefix, no second
/// lookup), and nothing is read.
#[tokio::test]
async fn a_registry_failure_keeps_its_text_and_reads_nothing() {
    let mut registry = MockAttachmentRegistry::new();
    registry
        .expect_lookup_by_document_id()
        .times(1)
        .returning(|_, _| Err(AttachmentError::RepositoryFailed("db down".into())));
    let ex = DagToolExecutor::new(Arc::new(NoNodes), Default::default())
        .with_attachment_registry(Arc::new(registry))
        .with_agent_session_id(Some("agent_1".to_string()))
        .with_attachment_storage(Arc::new(MockOutputStorageRepository::new()));
    let err = ex.fetch_attachment_bytes("doc-1").await.unwrap_err();
    assert_eq!(
        err,
        format!(
            "attachment registry lookup failed: {}",
            AttachmentError::RepositoryFailed("db down".into())
        )
    );
}

fn call(tool: &str, args: &str) -> ToolCall {
    ToolCall {
        id: "c1".into(),
        call_type: "function".into(),
        function: FunctionCall::new(tool.into(), args.into()),
        response: None,
        provider_signature: None,
        scope_index: None,
    }
}

fn refused_executor() -> DagToolExecutor {
    executor(
        host_ref("text/csv", Some(60 * MIB)),
        MockOutputStorageRepository::new(),
    )
}

fn assert_coded(body: &serde_json::Value, label: &str) {
    assert_eq!(body["code"], LARGE_TABULAR_ERROR_CODE, "{label}: {body}");
    assert!(body.to_string().contains(refusal_text()), "{label}: {body}");
}

/// Every tool that reads an attachment whole answers with the same refusal text
/// and the same structured code, before any fetch.
#[tokio::test]
async fn every_whole_object_tool_returns_the_refusal_with_its_code() {
    let ex = refused_executor();
    let fixed: HashMap<String, serde_json::Value> = HashMap::from([
        (
            "connection_url".to_string(),
            serde_json::json!("postgres://u:p@127.0.0.1:1/x"),
        ),
        (
            "permissions".to_string(),
            serde_json::json!({"allowed_schemas": ["public"]}),
        ),
    ]);
    let inspect = dispatch_sql_inspect_attachment_via_executor(
        &ex,
        &call("sql_inspect_attachment", r#"{"attachment_id":"doc-1"}"#),
        &fixed,
    )
    .await
    .unwrap();
    let insert = dispatch_sql_bulk_insert_from_attachment_via_executor(
        &ex,
        &call(
            "sql_bulk_insert_from_attachment",
            r#"{"attachment_id":"doc-1","table":"public.t","column_mapping":{}}"#,
        ),
        &fixed,
    )
    .await
    .unwrap();
    let run_python = dispatch_attachment_run_python_via_executor(
        &ex,
        &call(
            "attachment_run_python",
            r#"{"attachment_id":"doc-1","code":"print(1)"}"#,
        ),
    )
    .await
    .unwrap();
    for (label, result) in [
        ("inspect", inspect),
        ("insert", insert),
        ("run_python", run_python),
    ] {
        assert_coded(&serde_json::from_str(&result.output).unwrap(), label);
    }

    let sheets = dispatch_create_from_xlsx_via_executor(
        &ex,
        serde_json::json!({"attachment_id": "doc-1", "title": "t"}),
    )
    .await;
    assert_coded(&sheets, "gsheets create_from_xlsx");
    let docs = dispatch_create_from_docx_via_executor(
        &ex,
        serde_json::json!({"attachment_id": "doc-1", "title": "t"}),
        "s1",
    )
    .await;
    assert_coded(&docs, "gdocs create_from_docx");
}

/// `data_run_python` binds attachments through a fetcher over the same executor
/// call; its binding error carries the code too.
#[tokio::test]
async fn a_data_run_python_binding_on_a_refused_file_carries_the_code() {
    let ex = Arc::new(refused_executor());
    let fetch: AttachmentFetcher = Box::new(move |id: String| {
        let ex = ex.clone();
        Box::pin(async move {
            let sb = ex.fetch_attachment_bytes(&id).await?;
            Ok((sb.bytes, sb.mime_type))
        })
    });
    let binding: DataBinding =
        serde_json::from_value(serde_json::json!({"var": "t", "attachment_id": "doc-1"})).unwrap();
    let err = resolve_bindings(&[binding], None, None, &fetch)
        .await
        .unwrap_err();
    assert_eq!(err["code"], LARGE_TABULAR_ERROR_CODE, "{err}");
}

/// `fetch_attachment_stream` is allowed for a host row (it never holds the object
/// whole) but its errors, at the start and in the middle of the stream, must not
/// carry storage's text (the key, or a local path): they name the document and the
/// file only. Other rows keep the error they always had.
#[tokio::test]
async fn a_host_rows_stream_errors_hide_the_key_and_other_rows_keep_theirs() {
    use futures::StreamExt;
    let mut storage = MockOutputStorageRepository::new();
    storage.expect_read_stream().returning(|k| {
        if k == "host-key" {
            Err(StorageError::InvalidInput(format!(
                "no {k} at /var/data/{k}"
            )))
        } else {
            Err(StorageError::InvalidInput(format!("no {k}")))
        }
    });
    let ex = executor(host_ref("text/csv", Some(60 * MIB)), storage);
    let err = ex.fetch_attachment_stream("doc-1").await.unwrap_err();
    assert!(err.contains("doc-1") && err.contains("big.csv"), "{err}");
    assert!(
        !err.contains("host-key") && !err.contains("/var/data"),
        "{err}"
    );

    let mut storage = MockOutputStorageRepository::new();
    storage
        .expect_read_stream()
        .returning(|k| Err(StorageError::InvalidInput(format!("no {k}"))));
    let ex = executor(engine_copy("text/csv", Some(8)), storage);
    assert_eq!(
        ex.fetch_attachment_stream("doc-1").await.unwrap_err(),
        "attachment_storage.read_stream failed for 'doc-1': invalid storage input: no copy-key"
    );

    // A failure in the middle of the stream of a host row.
    let mut storage = MockOutputStorageRepository::new();
    storage.expect_read_stream().returning(|k| {
        let k = k.to_string();
        Ok(StoredStream {
            stream: Box::pin(futures::stream::iter(vec![Err(
                StorageError::InvalidInput(format!("lost {k} at /var/data/{k}")),
            )])),
            size_bytes: 1,
            mime_type: "text/csv".into(),
            filename: "x".into(),
        })
    });
    let ex = executor(host_ref("text/csv", Some(60 * MIB)), storage);
    let mut got = ex.fetch_attachment_stream("doc-1").await.unwrap().stream;
    let err = got.next().await.unwrap().unwrap_err().to_string();
    assert!(err.contains("doc-1") && err.contains("big.csv"), "{err}");
    assert!(
        !err.contains("host-key") && !err.contains("/var/data"),
        "{err}"
    );
}
