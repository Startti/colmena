//! `attachment_run_python` over a large file: the tool's answer for a call the
//! routing handed to the large path (dark behind `COLMENA_LARGE_TABULAR`).
//!
//! The answer carries `tables` (names, rows, column types) where the small path
//! carries `row_count` and `columns`. Every refusal is the typed error object of
//! [`RunRefusal`](crate::tabular_run::refusal::RunRefusal): a sentence and a code,
//! no key, no path, no adapter text.

use super::{err_envelope, truncate, AttachmentRunPythonArgs, OUTPUT_BYTE_CAP};
use crate::dag_engine::infrastructure::dag_tool_executor::LargeTarget;
use crate::llm::domain::ToolResult;
use crate::tabular_run::runtime::{LargeRunError, LargeRunRequest};
use serde::Serialize;

/// What the tool description says about large files, appended to the usual
/// description only while a runtime that can serve them is wired.
pub(super) const LARGE_FILES_TEXT: &str = "\n\nLarge files (over 50 MiB): `df` is NOT loaded. Use `tables`:\n\
- `tables.names`, `tables.schema(name)`: tables, columns, types, row counts.\n\
- `t = tables[name]` is a handle, not a DataFrame.\n\
- `t.read(columns=[...], filters=[...])`: load only the columns you need.\n\
- `for part in t.parts(columns=[...]):` up to 500,000 rows per part; aggregate\n  each part and combine. Use this for anything that touches every row.\n\
- `t.head()` to look at a few rows.\n\
A whole table cannot be loaded at once. Runs may take up to 5 minutes.\n\
To return a file, call `emit_table(df_or_parts, \"name\", \"csv\" | \"parquet\")` (up to 8 files; parquet takes one DataFrame).\n\
No charts or images: return aggregated numbers and build charts from them.\n\
The optional `tables` argument names the tables to make readable (default: all).";

/// The tool as the model sees it when large files are served: the usual
/// definition, the text above, and the `tables` argument.
pub(super) fn tool_definition() -> crate::llm::domain::tools::ToolDefinition {
    let mut def = super::build_attachment_run_python_tool_definition();
    def.description.push_str(LARGE_FILES_TEXT);
    if let Some(properties) = def
        .input_schema_override
        .as_mut()
        .and_then(|schema| schema.get_mut("properties"))
        .and_then(|p| p.as_object_mut())
    {
        properties.insert(
            "tables".to_string(),
            serde_json::json!({
                "type": "array",
                "items": {"type": "string"},
                "description": "Large files only: the tables to make readable (default: all)."
            }),
        );
    }
    def
}

/// The tool's response for a call that ran over a large file.
#[derive(Debug, Serialize)]
struct LargeResponse {
    stdout: String,
    result: serde_json::Value,
    duration_ms: u64,
    /// What the code could read: `[{name, rows, columns: [{name, type}]}]`.
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    tables: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn answer(call_id: &str, response: &LargeResponse) -> ToolResult {
    let v = serde_json::to_value(response).unwrap_or(serde_json::Value::Null);
    ToolResult::success(call_id.to_string(), v.to_string())
}

pub(super) async fn dispatch(
    call_id: &str,
    args: &AttachmentRunPythonArgs,
    target: LargeTarget,
) -> ToolResult {
    let started = std::time::Instant::now();
    let outcome = target
        .runtime
        .run(LargeRunRequest {
            source_key: target.source_key,
            mime_type: target.mime_type,
            filename: target.filename,
            size_bytes: target.size_bytes,
            code: args.code.clone(),
            tables: args.tables.clone(),
        })
        .await;
    let duration_ms = started.elapsed().as_millis() as u64;
    match outcome {
        Ok(out) => answer(
            call_id,
            &LargeResponse {
                stdout: truncate(&out.stdout, OUTPUT_BYTE_CAP),
                result: out.result,
                duration_ms,
                tables: out.tables,
                error: None,
            },
        ),
        Err(LargeRunError::Refused(refusal)) => {
            ToolResult::success(call_id.to_string(), refusal.to_tool_error().to_string())
        }
        Err(LargeRunError::Python(text)) => answer(
            call_id,
            &LargeResponse {
                stdout: String::new(),
                result: serde_json::Value::Null,
                duration_ms,
                tables: serde_json::Value::Null,
                error: Some(truncate(&text, OUTPUT_BYTE_CAP)),
            },
        ),
        Err(LargeRunError::Timeout { secs }) => {
            err_envelope(call_id, format!("code execution exceeded {secs}s timeout"))
        }
        Err(LargeRunError::Internal(text)) => err_envelope(call_id, text),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        build_attachment_run_python_tool_definition, dispatch_attachment_run_python_via_executor,
    };
    use crate::dag_engine::application::ports::NodeRegistryPort;
    use crate::dag_engine::domain::node::ExecutableNode;
    use crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor;
    use crate::llm::domain::attachments::{origin, AttachmentSource};
    use crate::llm::domain::{ConversationAttachment, FunctionCall, ProviderKind, ToolCall};
    use crate::storage::domain::{MockOutputStorageRepository, StoredBytes};
    use crate::tabular_run::mounted::MountedError;
    use crate::tabular_run::refusal::Budget;
    use crate::tabular_run::refusal::RunRefusal;
    use crate::tabular_run::runtime::LargeTabularRuntime;
    use crate::tabular_run::testkit::*;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::sync::Arc;

    struct NoNodes;
    impl NodeRegistryPort for NoNodes {
        fn get_node(&self, _: &str) -> Option<Arc<dyn ExecutableNode>> {
            None
        }
        fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
            HashMap::new()
        }
    }

    const MIB: u64 = 1024 * 1024;

    fn row(host: bool) -> ConversationAttachment {
        ConversationAttachment {
            agent_session_id: "agent_1".into(),
            document_id: "doc-1".into(),
            provider: ProviderKind::OpenAi,
            provider_file_id: if host { String::new() } else { "pf-1".into() },
            mime_type: "text/csv".into(),
            filename: "sales.csv".into(),
            size_bytes: Some(60 * MIB),
            label: None,
            description: None,
            source: AttachmentSource::Path(SOURCE.into()),
            registered_at: chrono::Utc::now(),
            refreshed_at: chrono::Utc::now(),
            storage_key: Some(SOURCE.into()),
            origin: Some(
                if host {
                    origin::HOST_STORAGE_REF
                } else {
                    origin::USER_UPLOAD
                }
                .into(),
            ),
            last_used_at: None,
        }
    }

    fn call(args: &str) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            call_type: "function".into(),
            function: FunctionCall::new("attachment_run_python".into(), args.into()),
            response: None,
            provider_signature: None,
            scope_index: None,
        }
    }

    async fn body(ex: &DagToolExecutor, args: &str) -> Value {
        // The small path runs the code in this process.
        pyo3::Python::initialize();
        let result = dispatch_attachment_run_python_via_executor(ex, &call(args))
            .await
            .unwrap();
        serde_json::from_str(&result.output).unwrap()
    }

    fn executor(host: bool, runtime: Option<Arc<LargeTabularRuntime>>) -> DagToolExecutor {
        let mut storage = MockOutputStorageRepository::new();
        storage.expect_read().returning(|_| {
            Ok(StoredBytes {
                bytes: b"a,b\n1,2\n".to_vec(),
                mime_type: "text/csv".into(),
                filename: "sales.csv".into(),
            })
        });
        let ex = DagToolExecutor::new(Arc::new(NoNodes), Default::default())
            .with_attachments(vec![row(host)])
            .with_attachment_storage(Arc::new(storage));
        match runtime {
            Some(rt) => ex.with_large_tabular(rt),
            None => ex,
        }
    }

    #[tokio::test]
    async fn a_host_owned_file_runs_over_its_tables_and_answers_with_them() {
        let p = prepared(&[("sales", 2), ("stores", 1)], 4).await;
        let exec = Recorder::ok(json!({"total": 7}));
        let ex = executor(true, Some(Arc::new(runtime(&p, exec.clone(), true))));
        let out = body(
            &ex,
            r#"{"attachment_id":"doc-1","code":"result = 1","tables":["stores"]}"#,
        )
        .await;
        assert_eq!(out["result"], json!({"total": 7}));
        assert_eq!(out["stdout"], "hi\n");
        assert_eq!(out["tables"][0]["name"], "stores");
        assert!(
            out.get("row_count").is_none() && out.get("columns").is_none(),
            "{out}"
        );
        assert!(out.get("error").is_none(), "{out}");
        let seen = exec.seen.lock().unwrap();
        assert_eq!(seen[0].1, vec![1], "the `tables` argument was honoured");
        assert!(seen[0].0.code.contains("result = 1"));
    }

    /// Not wired (the switch is off, or the host gave the engine nothing): a
    /// host-owned file keeps the refusal it has always had, and a small file its
    /// usual path.
    #[tokio::test]
    async fn without_a_runtime_nothing_is_routed() {
        let out = body(
            &executor(true, None),
            r#"{"attachment_id":"doc-1","code":"print(1)"}"#,
        )
        .await;
        assert_eq!(out["code"], "large_tabular_file", "{out}");
        let out = body(
            &executor(false, None),
            r#"{"attachment_id":"doc-1","code":"print(1)"}"#,
        )
        .await;
        assert!(
            out.get("row_count").is_some(),
            "the small path answered: {out}"
        );
    }

    /// A small file, with the large path wired: it is not routed, the mounted
    /// executor is never asked, and the answer is the small path's.
    #[tokio::test]
    async fn a_small_file_keeps_its_path_even_with_the_runtime_wired() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok(Value::Null);
        let ex = executor(false, Some(Arc::new(runtime(&p, exec.clone(), true))));
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"print(1)"}"#).await;
        assert!(out.get("row_count").is_some(), "{out}");
        assert_eq!(exec.calls(), 0);
        assert!(p.storage.reads.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_refusal_is_the_typed_error_object_and_nothing_else() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::answering(Err(MountedError::Refused(RunRefusal::OverBudget(
            Budget::Volumes,
        ))));
        let ex = executor(true, Some(Arc::new(runtime(&p, exec, true))));
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["code"], "large_tabular_over_budget");
        assert_eq!(out["source"], "execution");
        assert_eq!(out.as_object().unwrap().len(), 3, "{out}");
        assert!(!out.to_string().contains(SOURCE));
    }

    #[tokio::test]
    async fn a_switch_that_is_off_in_the_runtime_refuses_and_runs_nothing() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok(Value::Null);
        let ex = executor(true, Some(Arc::new(runtime(&p, exec.clone(), false))));
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["code"], "large_tabular_disabled");
        assert_eq!(exec.calls(), 0);
    }

    /// The schema the model sees is the one it always saw: the `tables`
    /// argument is not in it.
    #[test]
    fn the_tool_schema_is_unchanged_by_the_tables_argument() {
        let def = serde_json::to_string(&build_attachment_run_python_tool_definition()).unwrap();
        assert!(!def.contains("tables"), "{def}");
    }

    /// With large files served the tool gains the text and the argument, and
    /// keeps everything else it had.
    #[test]
    fn the_large_file_definition_adds_the_text_and_the_argument_only() {
        let usual = build_attachment_run_python_tool_definition();
        let large = super::tool_definition();
        assert_eq!(large.name, usual.name);
        assert_eq!(large.summary, usual.summary);
        assert_eq!(
            large.description,
            format!("{}{}", usual.description, super::LARGE_FILES_TEXT)
        );
        let schema = |d: &crate::llm::domain::tools::ToolDefinition| {
            d.input_schema_override.clone().unwrap()
        };
        let (mut a, b) = (schema(&usual), schema(&large));
        assert!(b["properties"]["tables"]["items"]["type"] == "string");
        a["properties"]["tables"] = b["properties"]["tables"].clone();
        assert_eq!(a, b, "nothing else in the schema changed");
        assert!(super::LARGE_FILES_TEXT.contains("`df` is NOT loaded"));
        assert!(super::LARGE_FILES_TEXT.contains("up to 5 minutes"));
    }
}
