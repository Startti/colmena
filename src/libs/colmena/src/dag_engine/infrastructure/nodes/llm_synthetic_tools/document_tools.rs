//! Synthetic LLM tools for document artifacts.
//!
//! Each tool is a thin adapter: it builds a schemars-derived JSON Schema for
//! the LLM, parses the LLM-provided arguments, injects `session_id` from
//! context (never from the LLM), and dispatches to the matching use case.

use crate::documents::application::apply_patch::{ApplyPatchInput, ApplyPatchUseCase};
use crate::documents::application::create_document::{
    CreateDocumentInput, CreateDocumentUseCase,
};
use crate::documents::application::read_document::{ReadDocumentInput, ReadDocumentUseCase};
use crate::documents::domain::ids::{ArtifactId, ArtifactKind, SessionId, VersionId};
use crate::documents::domain::patch::{Patch, PatchSource};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

pub const DOCUMENT_CREATE_TOOL: &str = "document_create";
pub const DOCUMENT_APPLY_PATCH_TOOL: &str = "document_apply_patch";
pub const DOCUMENT_READ_TOOL: &str = "document_read";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DocumentCreateArgs {
    /// "excel" or "word". Determines the IR structure and render target.
    pub kind: String,
    /// Optional initial IR. If omitted, creates an empty document.
    #[serde(default)]
    pub initial_ir: Option<serde_json::Value>,
    /// Optional label. If omitted, auto-generated as "Untitled {Kind} {timestamp}".
    #[serde(default)]
    pub label: Option<String>,
    /// Max number of versions retained. Default: server config (typically 20).
    #[serde(default)]
    pub retention_limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DocumentApplyPatchArgs {
    /// Target artifact ID.
    pub artifact_id: String,
    /// Version the patch is based on. If server's HEAD is newer and ops don't
    /// conflict, the server auto-rebases.
    pub base_version: String,
    /// Ordered operations to apply atomically.
    pub ops: Vec<crate::documents::domain::patch::PatchOp>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DocumentReadArgs {
    pub artifact_id: String,
    /// Specific version, or omitted for current.
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

pub fn build_document_create_tool() -> ToolDefinition {
    let schema = schemars::schema_for!(DocumentCreateArgs);
    ToolDefinition {
        name: DOCUMENT_CREATE_TOOL.into(),
        description: "Create a new document artifact (Excel or Word). Returns the \
                     artifact_id and initial version. Use for any new document task."
            .into(),
        input_schema: serde_json::to_value(schema).unwrap(),
    }
}

pub fn build_document_apply_patch_tool() -> ToolDefinition {
    let schema = schemars::schema_for!(DocumentApplyPatchArgs);
    ToolDefinition {
        name: DOCUMENT_APPLY_PATCH_TOOL.into(),
        description: "Apply a patch (list of ops) to an existing document atomically. \
                     If the base_version is stale, the server auto-rebases when ops \
                     don't conflict. On conflict, returns a VersionConflict with \
                     structured details."
            .into(),
        input_schema: serde_json::to_value(schema).unwrap(),
    }
}

pub fn build_document_read_tool() -> ToolDefinition {
    let schema = schemars::schema_for!(DocumentReadArgs);
    ToolDefinition {
        name: DOCUMENT_READ_TOOL.into(),
        description: "Read the full IR of a document at a given version (or current).".into(),
        input_schema: serde_json::to_value(schema).unwrap(),
    }
}

pub struct DocumentToolsContext {
    pub create: Arc<CreateDocumentUseCase>,
    pub apply: Arc<ApplyPatchUseCase>,
    pub read: Arc<ReadDocumentUseCase>,
    pub session_id: SessionId,
}

pub async fn dispatch_document_create(
    ctx: &DocumentToolsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    let parsed: DocumentCreateArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return json!({"error": format!("invalid args: {e}")}),
    };
    let kind = match parsed.kind.as_str() {
        "excel" => ArtifactKind::Excel,
        "word" => ArtifactKind::Word,
        other => return json!({"error": format!("unknown kind: {other}")}),
    };
    let input = CreateDocumentInput {
        kind,
        session_id: ctx.session_id.clone(),
        label: parsed.label,
        retention_limit: parsed.retention_limit,
        initial_ir: parsed.initial_ir,
        source: PatchSource::Agent,
    };
    match ctx.create.execute(input).await {
        Ok(out) => json!({
            "artifact_id": out.artifact_id.0,
            "version_id": out.version_id.0,
            "label": out.label,
        }),
        Err(e) => json!({"error": e.to_string()}),
    }
}

pub async fn dispatch_document_apply_patch(
    ctx: &DocumentToolsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    let parsed: DocumentApplyPatchArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return json!({"error": format!("invalid args: {e}")}),
    };
    let patch = Patch {
        artifact_id: parsed.artifact_id,
        base_version: parsed.base_version,
        source: PatchSource::Agent,
        ops: parsed.ops,
    };
    match ctx.apply.execute(ApplyPatchInput { patch }).await {
        Ok(out) => json!({
            "version_id": out.version_id.0,
            "diff_summary": out.summary.natural_language,
        }),
        Err(e) => match &e {
            crate::documents::domain::DocumentError::VersionConflict {
                current,
                conflicts,
                ..
            } => json!({
                "error": "VersionConflict",
                "current_version": current.0,
                "conflicts": conflicts,
            }),
            _ => json!({"error": e.to_string()}),
        },
    }
}

pub async fn dispatch_document_read(
    ctx: &DocumentToolsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    let parsed: DocumentReadArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return json!({"error": format!("invalid args: {e}")}),
    };
    let input = ReadDocumentInput {
        artifact_id: ArtifactId::new(parsed.artifact_id),
        version: parsed.version.map(VersionId::new),
    };
    match ctx.read.execute(input).await {
        Ok(out) => json!({
            "ir": out.ir,
            "version_id": out.version.0,
        }),
        Err(e) => json!({"error": e.to_string()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_create_schema_mentions_kind() {
        let t = build_document_create_tool();
        let s = t.input_schema.to_string();
        assert!(s.contains("kind"));
        assert!(s.contains("initial_ir"));
    }

    #[test]
    fn apply_patch_schema_includes_ops_enum() {
        let t = build_document_apply_patch_tool();
        let s = t.input_schema.to_string();
        assert!(s.contains("set_cell"));
        assert!(s.contains("A1-style"));
    }
}
