//! DAG nodes for the documents feature: `document_create`, `document_edit`,
//! `document_read`.
//!
//! Each node lazily builds a `DocumentRuntime` from its config (via
//! `OnceCell`) so the same store / renderers / use cases are reused across
//! invocations — same pattern as `SqlNode`. All three configs are
//! `$DYNAMIC`-compatible and can be exposed as LLM tools via the existing
//! `dag_tool_executor` (spec §11.2, §11.3).

use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::documents::application::apply_patch::ApplyPatchInput;
use crate::documents::application::create_document::CreateDocumentInput;
use crate::documents::application::read_document::ReadDocumentInput;
use crate::documents::application::DocumentRuntime;
use crate::documents::domain::ids::{ArtifactId, ArtifactKind, SessionId, VersionId};
use crate::documents::domain::patch::{Patch, PatchOp, PatchSource};
use crate::documents::domain::DocumentError;
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::OnceCell;

#[derive(Debug, Error)]
enum DocNodeError {
    #[error("config error: {0}")]
    Config(String),
    #[error("missing required field: {0}")]
    MissingField(&'static str),
}

/// Resolves the session_id used to scope artifacts. Priority matches the
/// LLM node: input `__colmena_session_id` > input `session_id` > config
/// `session_id`. Defaults to "default" when the graph runs standalone.
fn resolve_session_id(inputs: &NodeInputs, config: &Value) -> SessionId {
    let id = inputs
        .get("__colmena_session_id")
        .and_then(|v| v.as_str())
        .or_else(|| inputs.get("session_id").and_then(|v| v.as_str()))
        .or_else(|| config.get("session_id").and_then(|v| v.as_str()))
        .unwrap_or("default")
        .to_string();
    SessionId::new(id)
}

fn build_runtime(
    cell: &OnceCell<Arc<DocumentRuntime>>,
    config: &Value,
) -> Result<Arc<DocumentRuntime>, Box<dyn StdError + Send + Sync>> {
    let rt = cell.get_or_try_init(|| {
        std::future::ready(DocumentRuntime::from_config(config).map_err(DocNodeError::Config))
    });
    // `OnceCell::get_or_try_init` returns a future; we need to block on it
    // synchronously since this helper is called from `async` contexts where
    // we already have a runtime. The future resolves immediately because
    // `from_config` is not async.
    let rt = futures::executor::block_on(rt)?;
    Ok(rt.clone())
}

fn document_error_to_value(e: DocumentError) -> Value {
    match &e {
        DocumentError::VersionConflict {
            current, conflicts, ..
        } => json!({
            "error": "VersionConflict",
            "current_version": current.0,
            "conflicts": conflicts,
        }),
        _ => json!({"error": e.to_string()}),
    }
}

/// `document_create` — creates a new artifact (Excel or Word).
///
/// Config:
///   - `kind` (required): "excel" | "word".
///   - `initial_ir` (optional): full IR object. Supports `$DYNAMIC`/`$ref`.
///   - `label` (optional): human-readable label.
///   - `retention_limit` (optional): u32.
///   - `storage_backend`, `storage_root`: see `DocumentRuntime::from_config`.
///   - `session_id` (optional): explicit override; usually injected by the
///     engine context.
///
/// Output:
///   `{ "output": { "artifact_id", "version_id", "label" } }`.
pub struct DocumentCreateNode {
    runtime: OnceCell<Arc<DocumentRuntime>>,
}

impl DocumentCreateNode {
    pub fn new() -> Self {
        Self {
            runtime: OnceCell::new(),
        }
    }
}

impl Default for DocumentCreateNode {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ExecutableNode for DocumentCreateNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let runtime = build_runtime(&self.runtime, config)?;

        let kind_raw = inputs
            .get("kind")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("kind").and_then(|v| v.as_str()))
            .ok_or(DocNodeError::MissingField("kind"))?;
        let kind = match kind_raw {
            "excel" => ArtifactKind::Excel,
            "word" => ArtifactKind::Word,
            other => {
                return Err(Box::new(DocNodeError::Config(format!(
                    "unknown kind `{other}` — expected `excel` or `word`"
                ))));
            }
        };

        let initial_ir = inputs
            .get("initial_ir")
            .or_else(|| config.get("initial_ir"))
            .filter(|v| !v.is_null())
            .cloned();

        let label = inputs
            .get("label")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("label").and_then(|v| v.as_str()))
            .map(|s| s.to_string());

        let retention_limit = inputs
            .get("retention_limit")
            .and_then(|v| v.as_u64())
            .or_else(|| config.get("retention_limit").and_then(|v| v.as_u64()))
            .map(|n| n as u32);

        let session_id = resolve_session_id(inputs, config);

        let input = CreateDocumentInput {
            kind,
            session_id,
            label,
            retention_limit,
            initial_ir,
            source: PatchSource::Agent,
        };

        match runtime.create.execute(input).await {
            Ok(out) => Ok(json!({
                "output": {
                    "artifact_id": out.artifact_id.0,
                    "version_id": out.version_id.0,
                    "label": out.label,
                }
            })),
            Err(e) => Ok(json!({ "output": document_error_to_value(e) })),
        }
    }

    fn default_output(&self) -> Option<&str> {
        Some("output")
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Create a new document artifact (Excel or Word) and return its \
             artifact_id + initial version. Use as a graph step or as an LLM tool.",
        )
    }

    fn schema(&self) -> Value {
        json!({
            "type": "document_create",
            "config": {
                "kind": "string (excel|word)",
                "initial_ir": "object (optional, $DYNAMIC compatible)",
                "label": "string (optional)",
                "retention_limit": "integer (optional)",
                "storage_backend": "string (optional, default localfs)",
                "storage_root": "string (optional, default ./.colmena/documents)",
                "session_id": "string (optional, usually from context)"
            },
            "outputs": {
                "output": "object {artifact_id, version_id, label}"
            }
        })
    }
}

/// `document_edit` — applies a patch (list of typed ops) to an existing
/// artifact.
///
/// Config:
///   - `artifact_id` (required).
///   - `base_version` (required): version this patch targets.
///   - `ops` (required): array of `PatchOp` JSON objects. Supports `$DYNAMIC`.
///   - storage and session fields as in `DocumentCreateNode`.
///
/// Output:
///   `{ "output": { "version_id", "diff_summary" } }` on success, or
///   `{ "output": { "error": "VersionConflict", "current_version", "conflicts" } }`.
pub struct DocumentEditNode {
    runtime: OnceCell<Arc<DocumentRuntime>>,
}

impl DocumentEditNode {
    pub fn new() -> Self {
        Self {
            runtime: OnceCell::new(),
        }
    }
}

impl Default for DocumentEditNode {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ExecutableNode for DocumentEditNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let runtime = build_runtime(&self.runtime, config)?;

        let artifact_id = inputs
            .get("artifact_id")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("artifact_id").and_then(|v| v.as_str()))
            .ok_or(DocNodeError::MissingField("artifact_id"))?
            .to_string();

        let base_version = inputs
            .get("base_version")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("base_version").and_then(|v| v.as_str()))
            .ok_or(DocNodeError::MissingField("base_version"))?
            .to_string();

        let ops_raw = inputs
            .get("ops")
            .or_else(|| config.get("ops"))
            .ok_or(DocNodeError::MissingField("ops"))?
            .clone();
        let ops: Vec<PatchOp> = serde_json::from_value(ops_raw).map_err(|e| {
            Box::new(DocNodeError::Config(format!("invalid ops array: {e}")))
                as Box<dyn StdError + Send + Sync>
        })?;

        let patch = Patch {
            artifact_id,
            base_version,
            source: PatchSource::Agent,
            ops,
        };

        match runtime.apply.execute(ApplyPatchInput { patch }).await {
            Ok(out) => Ok(json!({
                "output": {
                    "version_id": out.version_id.0,
                    "diff_summary": out.summary.natural_language,
                }
            })),
            Err(e) => Ok(json!({ "output": document_error_to_value(e) })),
        }
    }

    fn default_output(&self) -> Option<&str> {
        Some("output")
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Apply a patch (list of ops) to an existing document artifact \
             atomically. Returns the new version_id and a diff summary, or a \
             structured VersionConflict error.",
        )
    }

    fn schema(&self) -> Value {
        json!({
            "type": "document_edit",
            "config": {
                "artifact_id": "string (required)",
                "base_version": "string (required)",
                "ops": "array of PatchOp (required, $DYNAMIC compatible)",
                "storage_backend": "string (optional)",
                "storage_root": "string (optional)"
            },
            "outputs": {
                "output": "object {version_id, diff_summary} | {error: VersionConflict, ...}"
            }
        })
    }
}

/// `document_read` — reads the current (or a specific) IR of an artifact.
///
/// Config:
///   - `artifact_id` (required).
///   - `version` (optional): specific version, defaults to current HEAD.
///   - storage fields as in `DocumentCreateNode`.
///
/// Output:
///   `{ "output": { "ir", "version_id" } }`.
pub struct DocumentReadNode {
    runtime: OnceCell<Arc<DocumentRuntime>>,
}

impl DocumentReadNode {
    pub fn new() -> Self {
        Self {
            runtime: OnceCell::new(),
        }
    }
}

impl Default for DocumentReadNode {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ExecutableNode for DocumentReadNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let runtime = build_runtime(&self.runtime, config)?;

        let artifact_id = inputs
            .get("artifact_id")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("artifact_id").and_then(|v| v.as_str()))
            .ok_or(DocNodeError::MissingField("artifact_id"))?
            .to_string();

        let version = inputs
            .get("version")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("version").and_then(|v| v.as_str()))
            .map(|s| VersionId::new(s.to_string()));

        let input = ReadDocumentInput {
            artifact_id: ArtifactId::new(artifact_id),
            version,
        };

        match runtime.read.execute(input).await {
            Ok(out) => Ok(json!({
                "output": {
                    "ir": out.ir,
                    "version_id": out.version.0,
                }
            })),
            Err(e) => Ok(json!({ "output": document_error_to_value(e) })),
        }
    }

    fn default_output(&self) -> Option<&str> {
        Some("output")
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Read the IR of a document artifact at a given version (or current \
             HEAD). Returns the parsed IR and the version_id read.",
        )
    }

    fn schema(&self) -> Value {
        json!({
            "type": "document_read",
            "config": {
                "artifact_id": "string (required)",
                "version": "string (optional, defaults to current HEAD)",
                "storage_backend": "string (optional)",
                "storage_root": "string (optional)"
            },
            "outputs": {
                "output": "object {ir, version_id}"
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use tempfile::tempdir;

    fn make_inputs() -> NodeInputs {
        HashMap::new()
    }

    #[tokio::test]
    async fn create_node_returns_artifact_id_and_version() {
        let tmp = tempdir().unwrap();
        let node = DocumentCreateNode::new();
        let inputs = make_inputs();
        let config = json!({
            "kind": "excel",
            "storage_root": tmp.path().to_str().unwrap()
        });
        let mut state = json!({});
        let res = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .unwrap();
        let out = &res["output"];
        assert!(out["artifact_id"].as_str().unwrap().starts_with("art_"));
        assert_eq!(out["version_id"], "v1");
    }

    #[tokio::test]
    async fn create_then_read_roundtrip() {
        let tmp = tempdir().unwrap();
        let storage_root = tmp.path().to_str().unwrap().to_string();

        let create_node = DocumentCreateNode::new();
        let create_cfg = json!({
            "kind": "excel",
            "storage_root": &storage_root,
            "initial_ir": {
                "kind": "excel",
                "artifact_id": "x", "version_id": "v1",
                "schema_version": "1.0.0",
                "workbook": {"sheets": [{"id":"s1","name":"H","order":0,"columns":[],"cells":{},"tables":[]}], "named_styles": {}}
            }
        });
        let mut state = json!({});
        let created = create_node
            .execute(&make_inputs(), &create_cfg, &mut state, None)
            .await
            .unwrap();
        let artifact_id = created["output"]["artifact_id"].as_str().unwrap().to_string();

        let read_node = DocumentReadNode::new();
        let read_cfg = json!({
            "artifact_id": artifact_id,
            "storage_root": &storage_root
        });
        let read = read_node
            .execute(&make_inputs(), &read_cfg, &mut state, None)
            .await
            .unwrap();
        assert_eq!(read["output"]["version_id"], "v1");
        assert_eq!(read["output"]["ir"]["workbook"]["sheets"][0]["id"], "s1");
    }

    #[tokio::test]
    async fn edit_node_applies_patch_and_advances_version() {
        let tmp = tempdir().unwrap();
        let storage_root = tmp.path().to_str().unwrap().to_string();

        let create_cfg = json!({
            "kind": "excel",
            "storage_root": &storage_root,
            "initial_ir": {
                "kind": "excel",
                "artifact_id": "x", "version_id": "v1",
                "schema_version": "1.0.0",
                "workbook": {"sheets":[{"id":"s1","name":"H","order":0,"columns":[],"cells":{},"tables":[]}], "named_styles":{}}
            }
        });
        let mut state = json!({});
        let create_node = DocumentCreateNode::new();
        let created = create_node
            .execute(&make_inputs(), &create_cfg, &mut state, None)
            .await
            .unwrap();
        let artifact_id = created["output"]["artifact_id"].as_str().unwrap().to_string();

        let edit_node = DocumentEditNode::new();
        let edit_cfg = json!({
            "artifact_id": artifact_id,
            "base_version": "v1",
            "storage_root": &storage_root,
            "ops": [
                {"op": "set_cell", "sheet_id": "s1", "address": "A1", "value": "Hello"}
            ]
        });
        let edited = edit_node
            .execute(&make_inputs(), &edit_cfg, &mut state, None)
            .await
            .unwrap();
        assert_eq!(edited["output"]["version_id"], "v2");
    }

    #[tokio::test]
    async fn edit_node_reports_version_conflict_on_stale_base() {
        let tmp = tempdir().unwrap();
        let storage_root = tmp.path().to_str().unwrap().to_string();

        let create_cfg = json!({
            "kind": "excel",
            "storage_root": &storage_root,
            "initial_ir": {
                "kind":"excel","artifact_id":"x","version_id":"v1","schema_version":"1.0.0",
                "workbook":{"sheets":[{"id":"s1","name":"H","order":0,"columns":[],"cells":{},"tables":[]}],"named_styles":{}}
            }
        });
        let mut state = json!({});
        let created = DocumentCreateNode::new()
            .execute(&make_inputs(), &create_cfg, &mut state, None)
            .await
            .unwrap();
        let artifact_id = created["output"]["artifact_id"].as_str().unwrap().to_string();

        let edit_cfg = json!({
            "artifact_id": artifact_id,
            "base_version": "v0",
            "storage_root": &storage_root,
            "ops": []
        });
        let res = DocumentEditNode::new()
            .execute(&make_inputs(), &edit_cfg, &mut state, None)
            .await
            .unwrap();
        assert_eq!(res["output"]["error"], "VersionConflict");
    }

    #[test]
    fn nodes_have_descriptions() {
        assert!(DocumentCreateNode::new().description().is_some());
        assert!(DocumentEditNode::new().description().is_some());
        assert!(DocumentReadNode::new().description().is_some());
    }
}
