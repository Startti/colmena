//! Synthetic LLM tools for document artifacts.
//!
//! Each tool is a thin adapter: it builds a schemars-derived JSON Schema for
//! the LLM, parses the LLM-provided arguments, injects `session_id` from the
//! caller's context (never from the LLM), and dispatches to the matching use
//! case in `documents::application`.
//!
//! Security rule (spec §11.1): `session_id` MUST NOT appear in any tool's
//! input schema. The LLM never sets it; the server resolves it from execution
//! context and passes it down. If a malicious model includes `session_id` in
//! the JSON, it is silently ignored by the typed structs below.

use crate::documents::application::apply_patch::{ApplyPatchInput, ApplyPatchUseCase};
use crate::documents::application::create_document::{CreateDocumentInput, CreateDocumentUseCase};
use crate::documents::application::get_head::{GetHeadInput, GetHeadUseCase};
use crate::documents::application::list_versions::ListVersionsUseCase;
use crate::documents::application::read_document::{ReadDocumentInput, ReadDocumentUseCase};
use crate::documents::application::rollback::{RollbackInput, RollbackUseCase};
use crate::documents::domain::ids::{ArtifactId, ArtifactKind, SessionId, VersionId};
use crate::documents::domain::patch::{Patch, PatchSource};
use crate::documents::domain::SessionArtifactIndex;
use crate::llm::domain::tools::{ToolDefinition, ToolParameters};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub const DOCUMENT_CREATE_TOOL: &str = "document_create";
pub const DOCUMENT_APPLY_PATCH_TOOL: &str = "document_apply_patch";
pub const DOCUMENT_READ_TOOL: &str = "document_read";
pub const DOCUMENT_GET_HEAD_TOOL: &str = "document_get_head";
pub const DOCUMENT_LIST_VERSIONS_TOOL: &str = "document_list_versions";
pub const DOCUMENT_ROLLBACK_TOOL: &str = "document_rollback";
pub const DOCUMENT_LIST_MY_ARTIFACTS_TOOL: &str = "document_list_my_artifacts";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DocumentCreateArgs {
    /// Document type. Determines the IR shape and the rendered binary
    /// (xlsx for "excel", docx for "word").
    pub kind: String,
    /// Optional initial IR. If omitted, an empty document is created. The
    /// shape must match the schema for the chosen `kind`.
    #[serde(default)]
    pub initial_ir: Option<serde_json::Value>,
    /// Optional human-readable label. If omitted, auto-generated as
    /// "Untitled {Kind} {YYYY-MM-DD HH:MM}".
    #[serde(default)]
    pub label: Option<String>,
    /// Maximum number of versions retained. Older versions beyond this
    /// window are pruned (initial v1 is always pinned). Defaults to the
    /// server's configured retention.
    #[serde(default)]
    pub retention_limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DocumentApplyPatchArgs {
    /// Target artifact ID (e.g. "art_abc123").
    pub artifact_id: String,
    /// Version this patch is based on (e.g. "v3"). The server auto-rebases
    /// when the current HEAD is newer and ops don't conflict; otherwise it
    /// returns a VersionConflict with structured details.
    pub base_version: String,
    /// Ordered operations applied atomically. All ops succeed or none do.
    pub ops: Vec<crate::documents::domain::patch::PatchOp>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DocumentReadArgs {
    /// Artifact ID to read.
    pub artifact_id: String,
    /// Specific version to read (e.g. "v3"). If omitted, returns the
    /// current HEAD.
    #[serde(default)]
    pub version: Option<String>,
    /// Optional slice to retrieve only part of the document. When set, the
    /// returned IR contains only the requested sheets / blocks / cell ranges.
    #[serde(default)]
    pub slice: Option<DocumentReadSlice>,
}

/// Selects a portion of the IR to return. Used by `document_read.slice`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DocumentReadSlice {
    /// (Excel) Restrict to these sheet IDs.
    #[serde(default)]
    pub sheets: Option<Vec<String>>,
    /// (Word) Restrict to these block IDs.
    #[serde(default)]
    pub block_ids: Option<Vec<String>>,
    /// (Excel) Restrict to specific cell ranges per sheet.
    #[serde(default)]
    pub cell_ranges: Option<Vec<CellRangeFilter>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CellRangeFilter {
    pub sheet_id: String,
    /// A1-style range (e.g. "A1:C20").
    pub range: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DocumentGetHeadArgs {
    /// Artifact ID to inspect.
    pub artifact_id: String,
    /// Optional baseline. When provided, the response includes a narration
    /// of every user edit between this version and the current HEAD — the
    /// pull-explicit mechanism for the agent to catch up on user changes.
    #[serde(default)]
    pub since_version: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DocumentListVersionsArgs {
    /// Artifact ID whose versions to list.
    pub artifact_id: String,
    /// Maximum entries to return (most recent first). Defaults to all
    /// retained versions when omitted.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DocumentRollbackArgs {
    /// Artifact ID to roll back.
    pub artifact_id: String,
    /// Target version. A new HEAD is written whose IR equals the target
    /// version's IR; full history is preserved.
    pub to_version: String,
}

/// Empty argument struct: this tool takes no parameters. The current
/// session is resolved server-side from execution context.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct DocumentListMyArtifactsArgs {}

/// Generate a `crate::llm::domain::tools::ToolDefinition` whose JSON Schema is
/// the schemars-derived schema (carried via `input_schema_override`). The
/// structured `parameters` field is left empty because LLM providers consume
/// the override verbatim when present.
fn build_synthetic_tool<T: JsonSchema>(name: &str, description: &str) -> ToolDefinition {
    let schema = schemars::schema_for!(T);
    let mut schema_json =
        serde_json::to_value(schema).expect("schemars schema must serialize to JSON Value");
    sanitize_schema_for_llm_providers(&mut schema_json);
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        parameters: ToolParameters::new(),
        input_schema_override: Some(schema_json),
    }
}

/// Walk a JSON Schema and normalize shapes that some LLM providers (notably
/// OpenAI) reject:
///
/// 1. Boolean schemas at `items` / `additionalProperties` positions are
///    replaced with `{}` (schemars emits `true` for opaque types like
///    `serde_json::Value`, but OpenAI requires an object schema there).
/// 2. Any `{"type": "object"}` without a `properties` field gets an empty
///    `properties: {}` injected (OpenAI requires `properties` to be present
///    even on schemas that take no parameters).
fn sanitize_schema_for_llm_providers(value: &mut serde_json::Value) {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            for key in ["items", "additionalProperties"] {
                if let Some(v) = map.get_mut(key) {
                    if v.is_boolean() {
                        *v = Value::Object(serde_json::Map::new());
                    }
                }
            }
            let is_object_schema = map
                .get("type")
                .and_then(|t| t.as_str())
                .map(|t| t == "object")
                .unwrap_or(false);
            if is_object_schema && !map.contains_key("properties") {
                map.insert(
                    "properties".to_string(),
                    Value::Object(serde_json::Map::new()),
                );
            }
            for (_, v) in map.iter_mut() {
                sanitize_schema_for_llm_providers(v);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                sanitize_schema_for_llm_providers(v);
            }
        }
        _ => {}
    }
}

pub fn build_document_create_tool() -> ToolDefinition {
    build_synthetic_tool::<DocumentCreateArgs>(
        DOCUMENT_CREATE_TOOL,
        "Create a new document artifact (Excel or Word). Returns the \
         artifact_id and initial version. Use for any new document task.",
    )
}

pub fn build_document_apply_patch_tool() -> ToolDefinition {
    build_synthetic_tool::<DocumentApplyPatchArgs>(
        DOCUMENT_APPLY_PATCH_TOOL,
        "Apply a patch (list of ops) to an existing document atomically. \
         If the base_version is stale, the server auto-rebases when ops \
         don't conflict. On conflict, returns a VersionConflict with \
         structured details.",
    )
}

pub fn build_document_read_tool() -> ToolDefinition {
    build_synthetic_tool::<DocumentReadArgs>(
        DOCUMENT_READ_TOOL,
        "Read the IR of a document at a given version (or current). \
         Use `slice` to fetch only specific sheets, blocks or cell ranges \
         when the document is large.",
    )
}

pub fn build_document_get_head_tool() -> ToolDefinition {
    build_synthetic_tool::<DocumentGetHeadArgs>(
        DOCUMENT_GET_HEAD_TOOL,
        "Get the current HEAD of an artifact. Optionally pass \
         `since_version` to receive a natural-language narration of \
         every user edit between that version and HEAD — useful before \
         applying a new patch to ensure you operate on fresh state.",
    )
}

pub fn build_document_list_versions_tool() -> ToolDefinition {
    build_synthetic_tool::<DocumentListVersionsArgs>(
        DOCUMENT_LIST_VERSIONS_TOOL,
        "List the versions retained for an artifact, most recent \
         first, with timestamps, source (agent/user) and per-version \
         summary. Use as a precursor to `document_rollback`.",
    )
}

pub fn build_document_rollback_tool() -> ToolDefinition {
    build_synthetic_tool::<DocumentRollbackArgs>(
        DOCUMENT_ROLLBACK_TOOL,
        "Roll back an artifact to a previous version. The target's \
         IR is copied to a new HEAD; full history is preserved (this \
         is not a destructive operation).",
    )
}

pub fn build_document_list_my_artifacts_tool() -> ToolDefinition {
    build_synthetic_tool::<DocumentListMyArtifactsArgs>(
        DOCUMENT_LIST_MY_ARTIFACTS_TOOL,
        "List every artifact that belongs to the current session. \
         Returns id, kind, label, current version and last update for \
         each. Takes no parameters: the session is resolved server-side.",
    )
}

/// All seven tool definitions, in the order the spec lists them.
pub fn build_all_document_tools() -> Vec<ToolDefinition> {
    vec![
        build_document_create_tool(),
        build_document_apply_patch_tool(),
        build_document_read_tool(),
        build_document_get_head_tool(),
        build_document_list_versions_tool(),
        build_document_rollback_tool(),
        build_document_list_my_artifacts_tool(),
    ]
}

/// Manual injected into the LLM's system prompt whenever the host node is
/// configured with a `documents` block. It teaches the model how to use the
/// seven `document_*` tools end-to-end so the user prompt only needs to
/// describe the request, not the mechanics. Kept terse on purpose: schemars
/// already provides per-parameter docs in each tool definition; this prelude
/// covers concepts and workflows that span multiple tools.
pub const DOCUMENTS_SYSTEM_PRELUDE: &str = r##"## Document Artifacts

You can author and edit document artifacts (Excel workbooks and Word
documents) through the `document_*` tools below. Each artifact is versioned
and immutable: every change produces a new `version_id`. Always use these
tools — never invent file content yourself.

### Tools at a glance
- `document_create` — create a new artifact. `kind` is "excel" or "word".
  Omit `initial_ir` to start from an empty workbook/document; the server
  fills the envelope automatically. Returns `artifact_id` + `version_id`
  (always "v1").
- `document_apply_patch` — atomically apply a list of typed `ops` to an
  existing artifact. Requires `base_version` (the version you based your
  patch on). Returns the new `version_id` plus a `diff_summary` (a list of
  natural-language strings, one per op).
- `document_read` — read the IR at a version (or HEAD). Pass `slice` to
  fetch only the parts you need on large documents.
- `document_get_head` — fetch HEAD + a narration of every user edit since
  `since_version`. Use this BEFORE applying a patch when the user has been
  editing concurrently.
- `document_list_versions` — list retained versions, newest first.
- `document_rollback` — non-destructive rollback to an earlier version.
- `document_list_my_artifacts` — list every artifact in the current
  session.

### Standard workflow
1. **Create** with `document_create`. For most requests, omit `initial_ir`
   and shape the document via patches in step 2 — it is simpler and avoids
   schema mistakes.
2. **Patch** with `document_apply_patch`. Pass `base_version` = the latest
   `version_id` you know. Each patch is a list of typed ops applied
   atomically; if any op fails, none are applied.
3. **Read or report** the result. Use `document_read` to verify state, then
   tell the user the `artifact_id` and a brief description of what changed.

### Versioning and conflicts
- `base_version` is required on every patch. Use the most recent
  `version_id` you have seen.
- If the server's HEAD has moved AND your ops conflict with intervening
  user edits, the tool returns `{"error":"VersionConflict","current_version":"...","conflicts":[...]}`.
  When this happens: call `document_get_head(artifact_id, since_version=YOUR_BASE)`
  to read the user-edit narration, reformulate your ops on top of the new
  HEAD, and retry.
- Non-conflicting changes are auto-rebased server-side; you do not need to
  retry in that case.

### Excel ops (most common)
All Excel ops require a stable `sheet_id` (NOT the display name). When you
add a new sheet, the server generates the id and returns it inside
`diff_summary` (e.g. `"Added sheet 'Ventas' (id: sheet_01...)"`).

**Important — never use a newly-added sheet in the same patch.** Apply
`add_sheet` first, read the generated id from `diff_summary`, then apply
`set_cell` (or other cell-level ops) in a follow-up patch using that id.
Do not invent sheet ids like "sheet_01" — they will fail with "sheet not
found".
- `set_cell { sheet_id, address, value }` — A1-style address. `value` is
  any JSON scalar. Prefix `=` for formulas. Optional `value_type`,
  `format`, `style_ref`.
- `set_range { sheet_id, range, values }` — bulk write a rectangular
  region; `values` is row-major 2D array.
- `clear_range`, `insert_row`, `delete_row`, `insert_column`,
  `delete_column`.
- `add_sheet { name, at_index? }`, `rename_sheet`, `delete_sheet`,
  `reorder_sheets { order: [sheet_id, ...] }`.
- `create_table`, `resize_table`, `delete_table` for named tables.
- `set_column_width`, `define_style` for presentation.

### Word ops (most common)
Blocks and runs are addressed by stable IDs. The server assigns IDs for
new content; check `diff_summary` after each patch.
- `insert_block { before? | after? | (omit both to append), block }` —
  `block` is a tagged JSON object (paragraph, heading, list, table,
  image, page_break).
- `replace_block`, `delete_block`, `move_block`, `set_heading_level`.
- Runs (text spans inside paragraphs/headings):
  `replace_run_text { block_id, run_id, new_text }`,
  `set_run_style { block_id, run_id, style_patch }`,
  `insert_run`, `delete_run`.
- Lists: `insert_list_item`, `replace_list_item`, `delete_list_item`.
- Tables: `insert_table_row`, `delete_table_row`, `update_table_cell`.

### Rollback workflow
1. `document_list_versions(artifact_id)` to find the version to revert to.
2. `document_rollback(artifact_id, to_version)` — copies the target IR to
   a new HEAD. History is preserved; this is not destructive.

### Practical rules
- Prefer narrow patches over rewriting the whole document. Patch only the
  cells/blocks the user asked to change.
- Always call a `document_read` (or trust the freshly returned `version_id`)
  before reporting "done" to the user.
- Report the `artifact_id` so the user can reference the artifact in
  follow-up requests."##;

pub struct DocumentToolsContext {
    pub create: Arc<CreateDocumentUseCase>,
    pub apply: Arc<ApplyPatchUseCase>,
    pub read: Arc<ReadDocumentUseCase>,
    pub get_head: Arc<GetHeadUseCase>,
    pub list_versions: Arc<ListVersionsUseCase>,
    pub rollback: Arc<RollbackUseCase>,
    /// Optional session index. When `None`, `document_list_my_artifacts`
    /// returns a structured error. The other tools work without it.
    pub session_index: Option<Arc<dyn SessionArtifactIndex>>,
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
        Ok(out) => {
            if let Some(index) = &ctx.session_index {
                let _ = index
                    .register(&ctx.session_id, &out.artifact_id, &out.meta)
                    .await;
            }
            json!({
                "artifact_id": out.artifact_id.0,
                "version_id": out.version_id.0,
                "label": out.label,
            })
        }
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
    let artifact_id = ArtifactId::new(parsed.artifact_id.clone());
    let patch = Patch {
        artifact_id: parsed.artifact_id,
        base_version: parsed.base_version,
        source: PatchSource::Agent,
        ops: parsed.ops,
    };
    match ctx.apply.execute(ApplyPatchInput { patch }).await {
        Ok(out) => {
            if let Some(index) = &ctx.session_index {
                let _ = index
                    .update_head(&artifact_id, &out.version_id, chrono::Utc::now())
                    .await;
            }
            json!({
                "version_id": out.version_id.0,
                "diff_summary": out.summary.natural_language,
            })
        }
        Err(e) => match &e {
            crate::documents::domain::DocumentError::VersionConflict {
                current, conflicts, ..
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
    let slice = parsed.slice;
    let input = ReadDocumentInput {
        artifact_id: ArtifactId::new(parsed.artifact_id),
        version: parsed.version.map(VersionId::new),
    };
    match ctx.read.execute(input).await {
        Ok(out) => {
            let ir = match slice {
                Some(s) => apply_slice(out.ir, &s),
                None => out.ir,
            };
            json!({
                "ir": ir,
                "version_id": out.version.0,
            })
        }
        Err(e) => json!({"error": e.to_string()}),
    }
}

pub async fn dispatch_document_get_head(
    ctx: &DocumentToolsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    let parsed: DocumentGetHeadArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return json!({"error": format!("invalid args: {e}")}),
    };
    let input = GetHeadInput {
        artifact_id: ArtifactId::new(parsed.artifact_id),
        since_version: parsed.since_version.map(VersionId::new),
    };
    match ctx.get_head.execute(input).await {
        Ok(out) => json!({
            "artifact_id": out.artifact_id.0,
            "version_id": out.current_version.0,
            "updated_at": out.updated_at,
            "last_source": out.last_source,
            "summary_since": out.summary_since,
            "versions_in_window": out.versions_in_window
                .iter()
                .map(|v| v.0.clone())
                .collect::<Vec<_>>(),
        }),
        Err(e) => json!({"error": e.to_string()}),
    }
}

pub async fn dispatch_document_list_versions(
    ctx: &DocumentToolsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    let parsed: DocumentListVersionsArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return json!({"error": format!("invalid args: {e}")}),
    };
    let id = ArtifactId::new(parsed.artifact_id);
    match ctx.list_versions.execute(&id, parsed.limit).await {
        Ok(entries) => json!({
            "versions": entries
                .into_iter()
                .map(|e| json!({
                    "version_id": e.version_id.0,
                    "applied_at": e.applied_at,
                    "source": e.source,
                    "summary": e.summary,
                }))
                .collect::<Vec<_>>(),
        }),
        Err(e) => json!({"error": e.to_string()}),
    }
}

pub async fn dispatch_document_rollback(
    ctx: &DocumentToolsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    let parsed: DocumentRollbackArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return json!({"error": format!("invalid args: {e}")}),
    };
    let input = RollbackInput {
        artifact_id: ArtifactId::new(parsed.artifact_id),
        to_version: VersionId::new(parsed.to_version),
    };
    match ctx.rollback.execute(input).await {
        Ok(out) => json!({
            "new_version_id": out.new_version_id.0,
            "copied_from": out.copied_from.0,
        }),
        Err(e) => json!({"error": e.to_string()}),
    }
}

pub async fn dispatch_document_list_my_artifacts(
    ctx: &DocumentToolsContext,
    _args: serde_json::Value,
) -> serde_json::Value {
    let Some(index) = &ctx.session_index else {
        return json!({
            "error": "session_index_not_configured",
            "detail": "document_list_my_artifacts requires a SessionArtifactIndex \
                       (Postgres or in-memory). It is not wired in this runtime.",
        });
    };
    match index.list_by_session(&ctx.session_id).await {
        Ok(items) => json!({
            "artifacts": items
                .into_iter()
                .map(|s| json!({
                    "artifact_id": s.artifact_id.0,
                    "session_id": s.session_id.0,
                    "kind": s.kind,
                    "label": s.label,
                    "current_version": s.current_version.0,
                    "updated_at": s.updated_at,
                }))
                .collect::<Vec<_>>(),
        }),
        Err(e) => json!({"error": e.to_string()}),
    }
}

/// Apply a `DocumentReadSlice` to a full IR value. Best-effort: if the IR
/// shape doesn't match the requested slice (e.g. asking for sheets on a
/// Word doc), unrecognised filters are ignored.
fn apply_slice(ir: serde_json::Value, slice: &DocumentReadSlice) -> serde_json::Value {
    let mut out = ir;

    if let (Some(workbook), Some(sheet_filter)) = (
        out.get_mut("workbook").and_then(|w| w.as_object_mut()),
        slice.sheets.as_ref(),
    ) {
        if let Some(sheets) = workbook.get_mut("sheets").and_then(|s| s.as_array_mut()) {
            sheets.retain(|s| {
                s.get("id")
                    .and_then(|i| i.as_str())
                    .map(|id| sheet_filter.iter().any(|f| f == id))
                    .unwrap_or(false)
            });
        }
    }

    if let (Some(workbook), Some(range_filter)) = (
        out.get_mut("workbook").and_then(|w| w.as_object_mut()),
        slice.cell_ranges.as_ref(),
    ) {
        if let Some(sheets) = workbook.get_mut("sheets").and_then(|s| s.as_array_mut()) {
            for sheet in sheets.iter_mut() {
                let sheet_id = sheet
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_string();
                let allowed_ranges: Vec<&str> = range_filter
                    .iter()
                    .filter(|r| r.sheet_id == sheet_id)
                    .map(|r| r.range.as_str())
                    .collect();
                if allowed_ranges.is_empty() {
                    continue;
                }
                if let Some(cells) = sheet.get_mut("cells").and_then(|c| c.as_object_mut()) {
                    cells.retain(|addr, _| {
                        allowed_ranges
                            .iter()
                            .any(|range| address_in_range(addr, range))
                    });
                }
            }
        }
    }

    if let (Some(document), Some(block_filter)) = (
        out.get_mut("document").and_then(|d| d.as_object_mut()),
        slice.block_ids.as_ref(),
    ) {
        if let Some(blocks) = document.get_mut("blocks").and_then(|b| b.as_array_mut()) {
            blocks.retain(|b| {
                b.get("id")
                    .and_then(|i| i.as_str())
                    .map(|id| block_filter.iter().any(|f| f == id))
                    .unwrap_or(false)
            });
        }
    }

    out
}

/// Best-effort A1-range membership check. Handles "B5", "A1:C10", "A:A".
fn address_in_range(addr: &str, range: &str) -> bool {
    let (start, end) = match range.split_once(':') {
        Some((a, b)) => (a, b),
        None => (range, range),
    };
    let (sc, sr) = match split_a1(start) {
        Some(p) => p,
        None => return false,
    };
    let (ec, er) = match split_a1(end) {
        Some(p) => p,
        None => return false,
    };
    let (ac, ar) = match split_a1(addr) {
        Some(p) => p,
        None => return false,
    };
    let row_ok = match (sr, er, ar) {
        (Some(s), Some(e), Some(a)) => a >= s.min(e) && a <= s.max(e),
        (None, None, _) => true,
        _ => false,
    };
    let col_ok = ac >= sc.min(ec) && ac <= sc.max(ec);
    row_ok && col_ok
}

fn split_a1(s: &str) -> Option<(u32, Option<u32>)> {
    let s = s.trim().to_ascii_uppercase();
    let mut col = 0u32;
    let mut idx = 0usize;
    for ch in s.chars() {
        if ch.is_ascii_alphabetic() {
            col = col * 26 + (ch as u32 - 'A' as u32 + 1);
            idx += ch.len_utf8();
        } else {
            break;
        }
    }
    if col == 0 {
        return None;
    }
    let row_part = &s[idx..];
    if row_part.is_empty() {
        return Some((col, None));
    }
    row_part.parse::<u32>().ok().map(|r| (col, Some(r)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_string(t: &ToolDefinition) -> String {
        t.input_schema_override
            .as_ref()
            .expect("synthetic tool must carry an input_schema_override")
            .to_string()
    }

    #[test]
    fn document_create_schema_mentions_kind() {
        let t = build_document_create_tool();
        let s = schema_string(&t);
        assert!(s.contains("kind"));
        assert!(s.contains("initial_ir"));
    }

    #[test]
    fn apply_patch_schema_includes_ops_enum() {
        let t = build_document_apply_patch_tool();
        let s = schema_string(&t);
        assert!(s.contains("set_cell"));
        assert!(s.contains("A1-style"));
    }

    #[test]
    fn read_schema_includes_slice() {
        let t = build_document_read_tool();
        let s = schema_string(&t);
        assert!(s.contains("slice"));
        assert!(s.contains("block_ids"));
        assert!(s.contains("cell_ranges"));
    }

    #[test]
    fn build_all_returns_seven_tools() {
        let all = build_all_document_tools();
        assert_eq!(all.len(), 7);
        let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&DOCUMENT_CREATE_TOOL));
        assert!(names.contains(&DOCUMENT_APPLY_PATCH_TOOL));
        assert!(names.contains(&DOCUMENT_READ_TOOL));
        assert!(names.contains(&DOCUMENT_GET_HEAD_TOOL));
        assert!(names.contains(&DOCUMENT_LIST_VERSIONS_TOOL));
        assert!(names.contains(&DOCUMENT_ROLLBACK_TOOL));
        assert!(names.contains(&DOCUMENT_LIST_MY_ARTIFACTS_TOOL));
    }

    #[test]
    fn no_tool_schema_exposes_session_id() {
        for t in build_all_document_tools() {
            let s = schema_string(&t);
            assert!(
                !s.contains("\"session_id\""),
                "tool `{}` leaks session_id in its input schema:\n{}",
                t.name,
                s
            );
        }
    }

    #[test]
    fn list_my_artifacts_takes_no_visible_params() {
        let t = build_document_list_my_artifacts_tool();
        let s = schema_string(&t);
        assert!(!s.contains("artifact_id"));
        assert!(!s.contains("session_id"));
    }

    #[test]
    fn address_in_range_handles_basic_cases() {
        assert!(address_in_range("A1", "A1:C10"));
        assert!(address_in_range("B5", "A1:C10"));
        assert!(!address_in_range("D5", "A1:C10"));
        assert!(!address_in_range("A11", "A1:C10"));
        assert!(address_in_range("A5", "A:A"));
    }
}
