# Toolkit packages + tool summaries Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship two interlocked features for the LLM-tool integration: (1) every Rust-side synthetic tool gains a one-line `summary` enforced by CI, so `lazy_tool_loading: true` works competently; (2) a generic `ToolkitPackage` registry lets users enable a bundle of tools (e.g. `gsheets`) with one alias, with optional per-tool exclusion via `!toolname`.

**Architecture:**
- `ToolDefinition` gets a `summary: Option<String>` field. A new builder `build_synthetic_tool_with_summary(name, description, summary)` ships alongside the existing `build_synthetic_tool` (which now produces `summary = None`). Catalog-construction sites in `llm.rs` read the new field.
- Every synthetic tool builder migrates to the `_with_summary` variant. A CI test (`every_synthetic_tool_has_summary`) iterates the full catalog and fails the build if any registered synthetic tool's `summary` is `None` or out of size bounds.
- A new module `toolkit_packages.rs` exposes `pub static TOOLKIT_PACKAGES: &[ToolkitPackage]`. `filter_enabled_tools` in `llm.rs` expands package aliases and respects `!`-prefixed exclusion entries.

**Tech Stack:** Rust 1.95, `schemars`, existing colmena synthetic-tools machinery, the `lazy_tools_catalog` module already shipped.

**Spec:** [docs/superpowers/specs/2026-06-06-toolkit-packages-design.md](../specs/2026-06-06-toolkit-packages-design.md)

---

## File Structure

**New files:**
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs` — package registry
- `tests/graphs/agents/gsheets_package_smoke.json` — E2E smoke graph
- `docs/developer_guide/40_toolkit_packages.md` — canonical reference

**Modified files:**
- `src/libs/colmena/src/llm/domain/tools.rs` — add `summary` field to `ToolDefinition`, add `with_summary` builder method
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` — add `build_synthetic_tool_with_summary`, register the new module, add CI test
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs` — migrate 9 builders
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs` — migrate 1 builder
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs` — migrate ~10 builders
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs` — migrate 1 builder
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs` — migrate 1 builder
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs` — migrate 7 builders
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs` — migrate (or skip — `describe_tool` is dynamic; see Task 5)
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs` — migrate 1 builder
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs` — migrate 1 builder
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/recall_history.rs` — migrate 1 builder
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` — extend `filter_enabled_tools`; CatalogEntry construction sites (lines ~1559, ~2001, ~2101) use the new `summary` field
- `docs/developer_guide/39_gsheets.md` — "Recommended activation" section
- `docs/developer_guide/29_lazy_tool_loading.md` — confirm summary requirement
- `docs/developer_guide/DEVELOPER_GUIDE.md` — index entry for `40_toolkit_packages.md`
- `docs/node_as_tools_reference.json` — `toolkit_packages` section
- `docs/CHANGELOG_2026-06.md` — E-T15 + E-T16 entries
- `docs/BACKLOG.md` — deferred items

---

## Task 0 (E-T15a): Audit synthetic tool builders

**Goal:** Enumerate every synthetic tool builder in the codebase. This list drives Tasks 2–5.

**Files:**
- Create: `docs/superpowers/plans/2026-06-06-synthetic-tools-audit.md`

- [ ] **Step 1: Grep every `pub fn tool_*` and `pub fn build_*_tool` across `llm_synthetic_tools/`**

Run:
```bash
grep -rn "^pub fn tool_\|^pub fn build_.*_tool" \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/ \
  --include="*.rs"
```

Expected output: a list of ~25–35 builder functions across the modules.

- [ ] **Step 2: Write the audit doc**

Path: `docs/superpowers/plans/2026-06-06-synthetic-tools-audit.md`

Format (one section per source file, each tool gets a row):

```markdown
# Synthetic tools audit (2026-06-06)

Generated for E-T15. Covers every Rust-side synthetic tool builder.

## gsheets_tools.rs
| Builder | Tool name (constant) | Has summary today | Proposed summary |
|---|---|---|---|
| `tool_create_spreadsheet` | `GSHEETS_CREATE_SPREADSHEET_TOOL` | No | Create a new Google Sheets workbook and return its URL |
| ... | ... | ... | ... |

## crdt_doc_tools.rs
...

## document_tools.rs
...

(repeat for every file)
```

The "Proposed summary" column is filled from the spec §5.3 table where available, and otherwise drafted by the implementer. The implementer MUST count the total to confirm the spec's 35–40 estimate (record the actual count in a final summary line at the bottom of the doc).

- [ ] **Step 3: Commit the audit**

```bash
git add docs/superpowers/plans/2026-06-06-synthetic-tools-audit.md
git commit -m "docs(plan): synthetic tools audit for E-T15

Enumerates every Rust-side synthetic tool builder and proposes a
one-line summary for each. Drives the migration in Tasks 2-5.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 1 (E-T15b): Add `summary` field to `ToolDefinition` + new builder

**Goal:** Schema change + new `build_synthetic_tool_with_summary` helper. Keep the old `build_synthetic_tool` working (produces `summary = None`) so the migration in Tasks 2–5 is incremental.

**Files:**
- Modify: `src/libs/colmena/src/llm/domain/tools.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (3 CatalogEntry sites)

- [ ] **Step 1: Write the failing test for `ToolDefinition::with_summary`**

In `src/libs/colmena/src/llm/domain/tools.rs`, find the existing `#[cfg(test)]` mod (or create one at the bottom). Add:

```rust
#[test]
fn with_summary_sets_field_and_chains() {
    let td = ToolDefinition::new(
        "demo".to_string(),
        "Does a demo thing".to_string(),
        ToolParameters::new(),
    )
    .with_summary("Run a demo".to_string());
    assert_eq!(td.summary.as_deref(), Some("Run a demo"));
    assert_eq!(td.name, "demo");
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --lib with_summary_sets_field_and_chains
```

Expected: compile error — `with_summary` method does not exist AND `summary` field does not exist on `ToolDefinition`.

- [ ] **Step 3: Add `summary` field + builder method**

In `src/libs/colmena/src/llm/domain/tools.rs`, modify the struct (currently at lines 31–49):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    /// The name of the tool (e.g., "add", "http_request")
    pub name: String,

    /// Human-readable description of what the tool does
    pub description: String,

    /// Short (≤ 200 char) one-line summary surfaced in lazy-tool-loading
    /// catalogs. When `None`, the catalog falls back to a truncated
    /// `description`. Every synthetic tool MUST set this; DAG nodes used as
    /// tools may omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// JSON Schema for the tool's parameters
    pub parameters: ToolParameters,

    /// Raw JSON Schema override. When `Some`, providers (OpenAI/Anthropic/Gemini)
    /// send this object verbatim as the tool's input schema and ignore
    /// `parameters`. Lets synthetic tools expose schemars-derived schemas with
    /// nested objects, tagged unions and arrays — shapes that don't fit the
    /// flat `ParameterProperty` model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema_override: Option<serde_json::Value>,
}
```

Update `ToolDefinition::new` to initialize the new field to `None`:

```rust
impl ToolDefinition {
    /// Create a new tool definition
    pub fn new(name: String, description: String, parameters: ToolParameters) -> Self {
        Self {
            name,
            description,
            summary: None,
            parameters,
            input_schema_override: None,
        }
    }

    /// Builder: attach a one-line summary for lazy catalogs.
    pub fn with_summary(mut self, summary: String) -> Self {
        self.summary = Some(summary);
        self
    }

    // existing with_input_schema_override unchanged
    pub fn with_input_schema_override(mut self, schema: serde_json::Value) -> Self {
        self.input_schema_override = Some(schema);
        self
    }

    // existing validate unchanged
    // ...
}
```

- [ ] **Step 4: Re-run the test to verify it passes**

```bash
cargo test --lib with_summary_sets_field_and_chains
```

Expected: `test result: ok. 1 passed`.

- [ ] **Step 5: Write a failing test for `build_synthetic_tool_with_summary`**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`, find the `#[cfg(test)] mod sanitize_tests` (or add a new test module). Add:

```rust
#[cfg(test)]
mod synthetic_builder_tests {
    use super::*;
    use schemars::JsonSchema;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, JsonSchema)]
    struct FakeArgs {
        pub x: String,
    }

    #[test]
    fn build_synthetic_tool_with_summary_sets_summary() {
        let td = build_synthetic_tool_with_summary::<FakeArgs>(
            "fake_tool",
            "A fake tool used only in tests",
            "Run a fake operation",
        );
        assert_eq!(td.name, "fake_tool");
        assert_eq!(td.summary.as_deref(), Some("Run a fake operation"));
        assert!(td.input_schema_override.is_some());
    }

    #[test]
    fn build_synthetic_tool_without_summary_is_none() {
        let td = build_synthetic_tool::<FakeArgs>("fake_tool", "A fake tool");
        assert!(td.summary.is_none());
    }
}
```

- [ ] **Step 6: Run the test to verify it fails**

```bash
cargo test --lib synthetic_builder_tests
```

Expected: compile error — `build_synthetic_tool_with_summary` does not exist.

- [ ] **Step 7: Add `build_synthetic_tool_with_summary` next to `build_synthetic_tool`**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`, after the existing `build_synthetic_tool` (currently lines 32–43), add:

```rust
/// Like [`build_synthetic_tool`], but additionally attaches a one-line
/// `summary` (≤ 200 chars) used by `lazy_tool_loading` catalogs. Every
/// synthetic tool registered in colmena MUST go through this builder so
/// the `every_synthetic_tool_has_summary` test passes at CI time.
pub(super) fn build_synthetic_tool_with_summary<T: JsonSchema>(
    name: &str,
    description: &str,
    summary: &str,
) -> ToolDefinition {
    build_synthetic_tool::<T>(name, description).with_summary(summary.to_string())
}
```

- [ ] **Step 8: Re-run the test**

```bash
cargo test --lib synthetic_builder_tests
```

Expected: `test result: ok. 2 passed`.

- [ ] **Step 9: Update CatalogEntry construction sites in `llm.rs` to use the new `summary` field**

Find the 3 sites by grepping:

```bash
grep -n "CatalogEntry {" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
```

Expected output:
```
1559:                catalog.push(CatalogEntry {
2001:                    catalog.push(CatalogEntry {
2101:                    catalog.push(CatalogEntry {
```

At each site, the existing code constructs `summary` by truncating the description. Replace each with a call to `summary_for_catalog`. Example pattern (the existing code may already do this; if so, leave it). For each site, the construction should look like:

```rust
catalog.push(CatalogEntry {
    name: td.name.clone(),
    summary: crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::summary_for_catalog(
        td.summary.as_deref(),
        &td.description,
    ),
});
```

Where `summary_for_catalog` is already exported by `lazy_tools_catalog.rs` (re-export it from `mod.rs` if not yet `pub`). Verify:

```bash
grep -n "summary_for_catalog\|pub use lazy_tools_catalog" \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs
```

If `summary_for_catalog` is not re-exported, add to `mod.rs`:

```rust
pub use lazy_tools_catalog::summary_for_catalog;
```

- [ ] **Step 10: Run the full test suite to make sure nothing regressed**

```bash
cargo test --lib --quiet 2>&1 | tail -10
```

Expected: all tests pass. No new failures introduced.

- [ ] **Step 11: Commit**

```bash
git add src/libs/colmena/src/llm/domain/tools.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(llm): add summary field to ToolDefinition + build_synthetic_tool_with_summary

E-T15b. Adds Option<String> summary to ToolDefinition with serde
skip-when-none. New build_synthetic_tool_with_summary helper sits next
to the existing builder; the old one still works (produces summary=None)
so the migration in subsequent tasks is incremental. CatalogEntry
construction sites in llm.rs now read from ToolDefinition.summary via
summary_for_catalog (with description-truncation fallback unchanged).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2 (E-T15c-1): Migrate gsheets builders to use summary

**Goal:** Convert all 10 gsheets tool builders to call `build_synthetic_tool_with_summary` with the summaries from spec §5.3.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs`

- [ ] **Step 1: Update each builder in `gsheets_tools.rs`**

Find every `pub fn tool_*()` in the file (~9 functions). For each, replace the body's `build_synthetic_tool` call with `build_synthetic_tool_with_summary`, adding the matching summary from this table:

| Builder fn | Summary string |
|---|---|
| `tool_create_spreadsheet` | `"Create a new Google Sheets workbook and return its URL"` |
| `tool_create_from_xlsx` | `"Upload a local .xlsx attachment and convert it into a new Google Sheet"` |
| `tool_export_xlsx` | `"Download an existing Google Sheet as .xlsx bytes attachment"` |
| `tool_list_sheets` | `"List every tab (sheet) inside a spreadsheet by ID"` |
| `tool_add_sheet` | `"Create a new tab inside an existing spreadsheet"` |
| `tool_delete_sheet` | `"Permanently delete a tab from a spreadsheet"` |
| `tool_read` | `"Read a cell range from a tab; supports formatted, unformatted, and formula render modes"` |
| `tool_set_cell` | `"Write one value or formula into a single cell"` |
| `tool_set_range` | `"Write a 2-D values array starting at a given address"` |

Example transformation (showing one tool — apply the same pattern to all 9):

```rust
// BEFORE
pub fn tool_list_sheets() -> ToolDefinition {
    super::build_synthetic_tool::<ListSheetsArgs>(
        GSHEETS_LIST_SHEETS_TOOL,
        "List every sheet (tab) in a Google Sheets spreadsheet. ...",
    )
}

// AFTER
pub fn tool_list_sheets() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ListSheetsArgs>(
        GSHEETS_LIST_SHEETS_TOOL,
        "List every sheet (tab) in a Google Sheets spreadsheet. ...",
        "List every tab (sheet) inside a spreadsheet by ID",
    )
}
```

- [ ] **Step 2: Update the `gsheets_run_python` builder**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs`, find `pub fn tool_gsheets_run_python` and convert identically with:

```rust
pub fn tool_gsheets_run_python() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<GsheetsRunPythonArgs>(
        TOOL_GSHEETS_RUN_PYTHON,
        "Run sandboxed Python (pandas/numpy/scipy.stats) over data loaded directly \
         from Google Sheets. Each `bindings` entry becomes a Python global (a list \
         of dicts you can pass to pd.DataFrame). Rows NEVER pass through the LLM — \
         only the final `output`. Use this for any analysis over more than ~50 rows.",
        "Run sandboxed pandas analysis over sheet ranges loaded directly by the dispatcher (rows never pass through the LLM)",
    )
}
```

- [ ] **Step 3: Run the gsheets tests to verify nothing broke**

```bash
cargo test --lib gsheets --quiet 2>&1 | tail -5
```

Expected: all existing gsheets tests still pass (28+ tests).

- [ ] **Step 4: Add a pinning test in `gsheets_run_python.rs`**

Append to the existing `#[cfg(test)] mod tests` in `gsheets_run_python.rs`:

```rust
#[test]
fn tool_def_has_summary() {
    let td = tool_gsheets_run_python();
    assert!(
        td.summary.is_some(),
        "gsheets_run_python must declare a summary for lazy_tool_loading"
    );
    let s = td.summary.unwrap();
    assert!(s.len() >= 10 && s.len() <= 200, "summary length out of bounds: {}", s.len());
}
```

Run:
```bash
cargo test --lib tool_def_has_summary --quiet 2>&1 | tail -5
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs
git commit -m "feat(gsheets): add summary to all 10 synthetic tool builders

E-T15c-1. Migrates every gsheets_* tool to build_synthetic_tool_with_summary
so lazy_tool_loading: true sees a curated one-liner instead of a truncated
description fragment. Includes a pinning test for tool_gsheets_run_python.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3 (E-T15c-2): Migrate crdt_doc builders to use summary

**Goal:** Convert every `pub fn tool_*` in the crdt_doc modules.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs`

- [ ] **Step 1: Enumerate the crdt_doc builders**

```bash
grep -n "^pub fn tool_" \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs
```

Expected: ~10–12 functions (the exact list comes from Task 0's audit doc).

- [ ] **Step 2: Migrate each builder, using these proposed summaries**

| Builder fn | Summary string |
|---|---|
| `tool_list_sheets` (CRDT) | `"List every tab (sheet) inside the current CRDT document artifact"` |
| `tool_list_sheets_of` | `"List tabs inside a specific CRDT artifact by ID"` |
| `tool_read` | `"Read a cell range from the current CRDT document; supports include_formulas for round-trip"` |
| `tool_set_cell` | `"Write a value or formula into one cell of the current CRDT document"` |
| `tool_set_range` | `"Write a 2-D values array into the current CRDT document starting at an address"` |
| `tool_add_sheet` | `"Create a new tab inside the current CRDT document"` |
| `tool_get_recent_changes` | `"List recent CRDT change events since the agent's cursor"` |
| `tool_list_my_artifacts` | `"List CRDT artifacts owned by or shared with the current session"` |
| `tool_create_artifact` | `"Create a new empty CRDT spreadsheet artifact"` |
| `tool_run_python` (CRDT) | `"Run sandboxed pandas analysis over CRDT sheets without loading rows through the LLM"` |
| `tool_import_sheet` | `"Clone a sheet from another CRDT artifact into the current one"` |

For each builder, swap `build_synthetic_tool` for `build_synthetic_tool_with_summary` and add the summary as the third argument. If the audit (Task 0) revealed a builder not listed here, use the audit's "Proposed summary" column.

- [ ] **Step 3: Run the crdt_doc tests**

```bash
cargo test --lib crdt_doc --quiet 2>&1 | tail -5
```

Expected: all existing tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs
git commit -m "feat(crdt_doc): add summary to every synthetic tool builder

E-T15c-2. Migrates each crdt_doc_* tool to build_synthetic_tool_with_summary.
Aligns the CRDT toolkit with gsheets so lazy_tool_loading works identically
across both.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4 (E-T15c-3): Migrate document_tools builders to use summary

**Goal:** Convert every `pub fn build_*_tool` in `document_tools.rs`.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`

- [ ] **Step 1: Enumerate the document builders**

```bash
grep -n "^pub fn build_.*_tool" \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs
```

Expected: 7 functions.

- [ ] **Step 2: Migrate each, using these proposed summaries**

| Builder fn | Summary string |
|---|---|
| `build_document_create_tool` | `"Create a new versioned document artifact with initial content"` |
| `build_document_read_tool` | `"Read the head version of a document artifact"` |
| `build_document_apply_patch_tool` | `"Apply a structured patch (JSON-Patch-like ops) to a document; creates a new version"` |
| `build_document_get_head_tool` | `"Get metadata about the current head version of a document"` |
| `build_document_list_versions_tool` | `"List all historical versions of a document artifact"` |
| `build_document_rollback_tool` | `"Roll back a document to a prior version (creates a new head referencing the old content)"` |
| `build_document_list_my_artifacts_tool` | `"List document artifacts owned by or shared with the current session"` |

Use the same migration pattern as Task 2/3.

- [ ] **Step 3: Run tests**

```bash
cargo test --lib document_tools --quiet 2>&1 | tail -5
```

Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs
git commit -m "feat(documents): add summary to all 7 document tool builders

E-T15c-3. Same lazy_tool_loading migration applied to the document_* tools.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5 (E-T15c-4): Migrate remaining helper tools

**Goal:** Cover the four "helper" tools: `load_skill`, `load_attachment`, `recall_history`, and decide what to do with `describe_tool` (which is constructed dynamically per turn — see Task 1 inspection notes).

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/recall_history.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs` (likely no-op — see below)

- [ ] **Step 1: Find each helper builder**

```bash
grep -n "^pub fn build_.*_tool\|^pub fn tool_" \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/recall_history.rs
```

Expected: 1 builder per file (3 total).

- [ ] **Step 2: Migrate each with these summaries**

| Builder | Summary string |
|---|---|
| `load_skill` builder | `"Load a markdown skill bundle into the conversation; reveals built-in or user-provided guidance on demand"` |
| `load_attachment` builder | `"Materialize a registered attachment's content (with auto-summary for large files) into the conversation"` |
| `recall_history` builder | `"Fetch verbatim past messages from the compacted summary by ID range or substring search"` |

- [ ] **Step 3: Inspect `describe_tool` — usually no migration needed**

`describe_tool` is built dynamically per turn from the pending catalog (see `lazy_tools_catalog.rs::build_describe_tool_definition`). It does NOT go through `build_synthetic_tool`. Two cases:

1. If `describe_tool` is *constructed via* `build_synthetic_tool`, migrate it.
2. If it's hand-built (currently the case per the lazy_tools_catalog source), leave it alone, but the CI test in Task 6 must skip describe_tool by name OR the test must accept that `describe_tool` is bypassed because it's special.

Run:

```bash
grep -n "describe_tool" \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/lazy_tools_catalog.rs
```

Document the decision (migrate vs skip) in the commit message.

- [ ] **Step 4: Run tests**

```bash
cargo test --lib load_skill load_attachment recall_history --quiet 2>&1 | tail -5
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/recall_history.rs
# Add describe_tool.rs only if it was modified
git commit -m "feat(synthetic-tools): add summary to helper tool builders

E-T15c-4. load_skill, load_attachment, recall_history now declare summaries.
describe_tool intentionally left unchanged (built dynamically per turn).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6 (E-T15d): CI test — every synthetic tool has a summary

**Goal:** Build refuses to ship if any synthetic tool is registered without a summary in the [10, 200] char range. The describe_tool exception is hard-coded.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`

- [ ] **Step 1: Identify the canonical "list every synthetic tool" function**

```bash
grep -rn "build_all_.*_tools\|fn all_synthetic_tools" \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/ \
  --include="*.rs"
```

Expected: at least `build_all_crdt_doc_tools`, plus per-module collectors. If a single unified function does not exist, the test will compose one inline by calling each module's collector.

- [ ] **Step 2: Write the failing test**

Append to `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`:

```rust
#[cfg(test)]
mod summary_coverage_tests {
    //! Enforces that EVERY synthetic tool registered in colmena declares a
    //! `summary` between 10 and 200 chars. The build refuses to ship if a
    //! new tool is added without one.
    //!
    //! `describe_tool` is exempted: it is constructed dynamically per turn
    //! by `lazy_tools_catalog::build_describe_tool_definition` and does not
    //! go through the synthetic-tool builder.

    use crate::llm::domain::tools::ToolDefinition;

    /// Returns every synthetic ToolDefinition the colmena library registers.
    fn all_synthetic_tools() -> Vec<ToolDefinition> {
        let mut tools = Vec::new();

        // gsheets — 10 tools
        tools.push(super::gsheets_tools::tool_create_spreadsheet());
        tools.push(super::gsheets_tools::tool_create_from_xlsx());
        tools.push(super::gsheets_tools::tool_export_xlsx());
        tools.push(super::gsheets_tools::tool_list_sheets());
        tools.push(super::gsheets_tools::tool_add_sheet());
        tools.push(super::gsheets_tools::tool_delete_sheet());
        tools.push(super::gsheets_tools::tool_read());
        tools.push(super::gsheets_tools::tool_set_cell());
        tools.push(super::gsheets_tools::tool_set_range());
        tools.push(super::gsheets_run_python::tool_gsheets_run_python());

        // crdt_doc — use the existing collector
        tools.extend(super::crdt_doc_tools::build_all_crdt_doc_tools());

        // documents — use the existing collector
        tools.extend(super::document_tools::build_all_document_tools());

        // helpers
        // (use whatever public builder each helper exposes; consult the audit
        // doc from Task 0 if name resolution fails)
        // tools.push(super::load_skill_tool::build_load_skill_tool());
        // tools.push(super::load_attachment_tool::build_load_attachment_tool());
        // tools.push(super::recall_history::build_recall_history_tool());

        tools
    }

    #[test]
    fn every_synthetic_tool_has_summary() {
        let tools = all_synthetic_tools();
        let mut missing: Vec<String> = Vec::new();
        let mut out_of_bounds: Vec<(String, usize)> = Vec::new();

        for td in &tools {
            match td.summary.as_deref() {
                None => missing.push(td.name.clone()),
                Some(s) => {
                    let len = s.chars().count();
                    if !(10..=200).contains(&len) {
                        out_of_bounds.push((td.name.clone(), len));
                    }
                }
            }
        }

        assert!(
            missing.is_empty(),
            "These synthetic tools are missing a summary: {:?}. \
             Add one via build_synthetic_tool_with_summary.",
            missing
        );
        assert!(
            out_of_bounds.is_empty(),
            "These synthetic tools have summaries outside the 10..=200 char range: {:?}",
            out_of_bounds
        );
        assert!(
            !tools.is_empty(),
            "all_synthetic_tools() returned 0 entries — wiring bug"
        );
    }
}
```

The commented-out helper lines are placeholders for the actual builder names; the implementer fills them in from Task 5's commit. Removing the comments is the last step.

- [ ] **Step 3: Run the test**

```bash
cargo test --lib every_synthetic_tool_has_summary --quiet 2>&1 | tail -10
```

Expected: PASS. If any tool is missing a summary, the test prints the list — fix the offending builder by going back to Task 2/3/4/5 and re-run.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs
git commit -m "test(synthetic-tools): every_synthetic_tool_has_summary CI gate

E-T15d. New test iterates every registered synthetic tool and refuses to
build if any has summary=None or summary length outside [10, 200] chars.
describe_tool is exempted (dynamic per-turn construction).

This closes the lazy_tool_loading regression vector: no new synthetic
tool can ship without explicit catalog metadata.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 7 (E-T16a): `toolkit_packages.rs` module + registry + unit tests

**Goal:** New module that exposes the `ToolkitPackage` struct and a static registry with `gsheets` as the first entry. Module is purely additive — no changes to `llm.rs` yet.

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` (add `pub mod toolkit_packages;`)

- [ ] **Step 1: Create the module file with the registry and `find_package`**

Path: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs`

```rust
//! Toolkit-package registry. Lets a user write `enabled_tools: ["gsheets"]`
//! to enable every gsheets_* tool at once, with optional `!toolname`
//! exclusion entries.
//!
//! Naming convention (enforced by test): package aliases MUST NOT contain
//! `_`. Individual tool names MUST contain `_` after the package namespace
//! (e.g. `gsheets_read`). The single-underscore boundary is how a human
//! reading a graph JSON disambiguates "package" from "tool" at a glance.

/// A curated bundle of tools exposed under a single alias.
pub struct ToolkitPackage {
    /// Alias used in `enabled_tools`. Must not contain `_`.
    pub alias: &'static str,
    /// One-line human description shown in docs / future introspection tools.
    pub description: &'static str,
    /// Exact names of every tool this package activates. Order is preserved
    /// in the expansion.
    pub tools: &'static [&'static str],
}

/// The registry. New packages append here as a single struct literal.
pub static TOOLKIT_PACKAGES: &[ToolkitPackage] = &[
    ToolkitPackage {
        alias: "gsheets",
        description: "Read, write, and analyze Google Sheets workbooks (10 tools)",
        tools: &[
            "gsheets_create_spreadsheet",
            "gsheets_create_from_xlsx",
            "gsheets_export_xlsx",
            "gsheets_list_sheets",
            "gsheets_add_sheet",
            "gsheets_delete_sheet",
            "gsheets_read",
            "gsheets_set_cell",
            "gsheets_set_range",
            "gsheets_run_python",
        ],
    },
];

/// Linear-scan lookup. The registry is small (≪ 50 entries) so a HashMap
/// would be over-engineering.
pub fn find_package(alias: &str) -> Option<&'static ToolkitPackage> {
    TOOLKIT_PACKAGES.iter().find(|p| p.alias == alias)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_aliases_have_no_underscore() {
        for pkg in TOOLKIT_PACKAGES {
            assert!(
                !pkg.alias.contains('_'),
                "Package alias '{}' must not contain '_' — reserved for tool names",
                pkg.alias
            );
        }
    }

    #[test]
    fn gsheets_package_has_all_ten_tools() {
        let pkg = find_package("gsheets").expect("gsheets package must exist");
        assert_eq!(pkg.tools.len(), 10, "gsheets package must list 10 tools");
        for required in &[
            "gsheets_create_spreadsheet",
            "gsheets_create_from_xlsx",
            "gsheets_export_xlsx",
            "gsheets_list_sheets",
            "gsheets_add_sheet",
            "gsheets_delete_sheet",
            "gsheets_read",
            "gsheets_set_cell",
            "gsheets_set_range",
            "gsheets_run_python",
        ] {
            assert!(
                pkg.tools.contains(required),
                "gsheets package missing tool: {}",
                required
            );
        }
    }

    #[test]
    fn find_package_returns_some_for_known_alias() {
        assert!(find_package("gsheets").is_some());
    }

    #[test]
    fn find_package_returns_none_for_unknown() {
        assert!(find_package("gsheetz").is_none());
        assert!(find_package("").is_none());
        assert!(find_package("gsheets_read").is_none());
    }
}
```

- [ ] **Step 2: Register the module in `mod.rs`**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`, find the section of `pub mod` declarations near the top and add:

```rust
pub mod toolkit_packages;
```

Then add a `pub use` to expose the API:

```rust
pub use toolkit_packages::{find_package, ToolkitPackage, TOOLKIT_PACKAGES};
```

- [ ] **Step 3: Run the unit tests**

```bash
cargo test --lib toolkit_packages --quiet 2>&1 | tail -10
```

Expected: `test result: ok. 4 passed`.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs
git commit -m "feat(toolkit-packages): add ToolkitPackage registry with gsheets as first entry

E-T16a. Purely additive new module. No callers yet — filter_enabled_tools
integration is the next task. Includes 4 unit tests, the key one being
package_aliases_have_no_underscore which enforces the naming convention.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 8 (E-T16b): Extend `filter_enabled_tools` with package + exclusion support

**Goal:** Update `filter_enabled_tools` in `llm.rs` so `enabled_tools` entries expand via `TOOLKIT_PACKAGES` and `!`-prefixed entries are treated as exclusions.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (around line 51)

- [ ] **Step 1: Write failing tests in the existing `filter_enabled_tools_tests` module**

The test module is at `llm.rs:4003+`. Add these tests (place after existing ones):

```rust
#[test]
fn package_alias_expands_to_all_tools() {
    // 11 fake tools: 10 with gsheets_ prefix + 1 unrelated
    let all_tools = build_fake_catalog(&[
        "gsheets_create_spreadsheet",
        "gsheets_create_from_xlsx",
        "gsheets_export_xlsx",
        "gsheets_list_sheets",
        "gsheets_add_sheet",
        "gsheets_delete_sheet",
        "gsheets_read",
        "gsheets_set_cell",
        "gsheets_set_range",
        "gsheets_run_python",
        "tavily_web",
    ]);
    let enabled = json!(["gsheets"]);
    let configured: std::collections::HashSet<String> = std::collections::HashSet::new();
    let filtered = filter_enabled_tools(all_tools, Some(&enabled), &configured);
    assert_eq!(filtered.len(), 10, "gsheets alias must expand to 10 tools");
    assert!(filtered.iter().all(|t| t.name.starts_with("gsheets_")));
}

#[test]
fn package_plus_individual_tool_works() {
    let all_tools = build_fake_catalog(&[
        "gsheets_read",
        "gsheets_set_cell",
        "gsheets_create_spreadsheet",
        "gsheets_create_from_xlsx",
        "gsheets_export_xlsx",
        "gsheets_list_sheets",
        "gsheets_add_sheet",
        "gsheets_delete_sheet",
        "gsheets_set_range",
        "gsheets_run_python",
        "tavily_web",
    ]);
    let enabled = json!(["gsheets", "tavily_web"]);
    let filtered = filter_enabled_tools(
        all_tools,
        Some(&enabled),
        &std::collections::HashSet::new(),
    );
    assert_eq!(filtered.len(), 11);
}

#[test]
fn exclusion_removes_tool_from_package() {
    let all_tools = build_fake_catalog(&[
        "gsheets_read",
        "gsheets_delete_sheet",
        "gsheets_list_sheets",
        "gsheets_add_sheet",
        "gsheets_set_cell",
        "gsheets_set_range",
        "gsheets_create_spreadsheet",
        "gsheets_create_from_xlsx",
        "gsheets_export_xlsx",
        "gsheets_run_python",
    ]);
    let enabled = json!(["gsheets", "!gsheets_delete_sheet"]);
    let filtered = filter_enabled_tools(
        all_tools,
        Some(&enabled),
        &std::collections::HashSet::new(),
    );
    assert_eq!(filtered.len(), 9);
    assert!(!filtered.iter().any(|t| t.name == "gsheets_delete_sheet"));
}

#[test]
fn exclusion_order_independent() {
    let all_tools = build_fake_catalog(&[
        "gsheets_read",
        "gsheets_delete_sheet",
        "gsheets_list_sheets",
        "gsheets_add_sheet",
        "gsheets_set_cell",
        "gsheets_set_range",
        "gsheets_create_spreadsheet",
        "gsheets_create_from_xlsx",
        "gsheets_export_xlsx",
        "gsheets_run_python",
    ]);
    let order_a = json!(["gsheets", "!gsheets_read"]);
    let order_b = json!(["!gsheets_read", "gsheets"]);
    let configured = std::collections::HashSet::new();
    let names_a: std::collections::HashSet<String> =
        filter_enabled_tools(all_tools.clone(), Some(&order_a), &configured)
            .into_iter()
            .map(|t| t.name)
            .collect();
    let names_b: std::collections::HashSet<String> =
        filter_enabled_tools(all_tools, Some(&order_b), &configured)
            .into_iter()
            .map(|t| t.name)
            .collect();
    assert_eq!(names_a, names_b, "exclusion order must not matter");
}

#[test]
fn exclusion_of_package_removes_all_its_tools() {
    let all_tools = build_fake_catalog(&[
        "gsheets_read",
        "gsheets_set_cell",
        "tavily_web",
        "current_time",
        // ... add the rest of the gsheets 10 for completeness
        "gsheets_create_spreadsheet",
        "gsheets_create_from_xlsx",
        "gsheets_export_xlsx",
        "gsheets_list_sheets",
        "gsheets_add_sheet",
        "gsheets_delete_sheet",
        "gsheets_set_range",
        "gsheets_run_python",
    ]);
    let enabled = json!(["*", "!gsheets"]);
    let filtered = filter_enabled_tools(
        all_tools,
        Some(&enabled),
        &std::collections::HashSet::new(),
    );
    let names: std::collections::HashSet<String> =
        filtered.into_iter().map(|t| t.name).collect();
    assert!(!names.iter().any(|n| n.starts_with("gsheets_")));
    assert!(names.contains("tavily_web"));
    assert!(names.contains("current_time"));
}

#[test]
fn unknown_alias_silently_ignored() {
    let all_tools = build_fake_catalog(&["gsheets_read", "tavily_web"]);
    let enabled = json!(["gsheetz"]);
    let filtered = filter_enabled_tools(
        all_tools,
        Some(&enabled),
        &std::collections::HashSet::new(),
    );
    assert_eq!(filtered.len(), 0, "unknown alias produces empty result, no panic");
}

#[test]
fn exact_tool_name_match_still_works_unchanged() {
    // Back-compat: `gsheets_read` listed verbatim must still enable that one tool.
    let all_tools = build_fake_catalog(&["gsheets_read", "tavily_web"]);
    let enabled = json!(["gsheets_read"]);
    let filtered = filter_enabled_tools(
        all_tools,
        Some(&enabled),
        &std::collections::HashSet::new(),
    );
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].name, "gsheets_read");
}

#[test]
fn empty_exclusion_logged_and_ignored() {
    let all_tools = build_fake_catalog(&["gsheets_read", "tavily_web"]);
    let enabled = json!(["gsheets_read", "!"]);
    let filtered = filter_enabled_tools(
        all_tools,
        Some(&enabled),
        &std::collections::HashSet::new(),
    );
    // The "!" with empty name is ignored; gsheets_read still included.
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].name, "gsheets_read");
}
```

A helper `build_fake_catalog(names: &[&str]) -> Vec<ToolDefinition>` may already exist in the test module — if not, add it at the top of the test module:

```rust
fn build_fake_catalog(names: &[&str]) -> Vec<ToolDefinition> {
    names
        .iter()
        .map(|n| {
            ToolDefinition::new(
                n.to_string(),
                format!("description of {}", n),
                ToolParameters::new(),
            )
        })
        .collect()
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --lib filter_enabled_tools_tests --quiet 2>&1 | tail -10
```

Expected: the new tests fail because `filter_enabled_tools` doesn't yet expand packages or handle `!`.

- [ ] **Step 3: Update `filter_enabled_tools` to implement the 3-pass algorithm**

Replace the body of `filter_enabled_tools` (currently at `llm.rs:51–109`) with:

```rust
pub(crate) fn filter_enabled_tools(
    all_tools: Vec<crate::llm::domain::ToolDefinition>,
    enabled_tools_config: Option<&Value>,
    configured_aliases: &std::collections::HashSet<String>,
) -> Vec<crate::llm::domain::ToolDefinition> {
    use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::find_package;

    // PASS 1 — parse user input into raw_includes, raw_excludes, wildcard
    let mut raw_includes: Vec<String> =
        configured_aliases.iter().cloned().collect();
    let mut raw_excludes: Vec<String> = Vec::new();
    let mut wildcard_all = false;

    if let Some(enabled) = enabled_tools_config {
        let mut visit = |v: &Value| {
            if let Some(s) = v.as_str() {
                if s == "*" {
                    wildcard_all = true;
                } else if let Some(stripped) = s.strip_prefix('!') {
                    if stripped.is_empty() {
                        eprintln!("filter_enabled_tools: empty exclusion entry '!' ignored");
                    } else {
                        raw_excludes.push(stripped.to_string());
                    }
                } else if !raw_includes.iter().any(|n| n == s) {
                    raw_includes.push(s.to_string());
                }
            }
        };
        if let Some(arr) = enabled.as_array() {
            for v in arr {
                visit(v);
            }
        } else {
            visit(enabled);
        }
    }

    // PASS 2 — expand packages on both sides
    let expand = |name: &str| -> Vec<String> {
        if let Some(pkg) = find_package(name) {
            pkg.tools.iter().map(|t| t.to_string()).collect()
        } else {
            vec![name.to_string()]
        }
    };
    let mut final_includes: std::collections::HashSet<String> = std::collections::HashSet::new();
    for n in &raw_includes {
        for expanded in expand(n) {
            final_includes.insert(expanded);
        }
    }
    let mut final_excludes: std::collections::HashSet<String> = std::collections::HashSet::new();
    for n in &raw_excludes {
        for expanded in expand(n) {
            final_excludes.insert(expanded);
        }
    }

    // Back-compat: include any tool whose name matches `{alias}__` for any
    // alias in raw_includes (covers api_explorer-style toolkits).
    for alias in &raw_includes {
        let prefix = format!("{}__", alias);
        for tool in &all_tools {
            if tool.name.starts_with(&prefix) {
                final_includes.insert(tool.name.clone());
            }
        }
    }

    // PASS 3 — filter
    let predicate = |t: &crate::llm::domain::ToolDefinition| -> bool {
        if final_excludes.contains(&t.name) {
            return false;
        }
        if wildcard_all {
            return true;
        }
        final_includes.contains(&t.name)
    };
    all_tools.into_iter().filter(|t| predicate(t)).collect()
}
```

The expanded `use` statement at the top of the file may need updating to ensure `find_package` is reachable — but the inline `use` inside the function avoids touching the file-level imports.

- [ ] **Step 4: Run the new tests**

```bash
cargo test --lib filter_enabled_tools_tests --quiet 2>&1 | tail -15
```

Expected: all new tests pass + all existing tests still pass.

- [ ] **Step 5: Run the full test suite as a safety net**

```bash
cargo test --lib --quiet 2>&1 | tail -10
```

Expected: every previous test still passes (no regressions in api_explorer behavior).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(llm): filter_enabled_tools supports package aliases + ! exclusion

E-T16b. Implements the 3-pass algorithm from the spec:
  1. parse user entries → raw_includes, raw_excludes, wildcard flag
  2. expand package aliases via toolkit_packages::find_package
  3. set-diff (includes - excludes), with wildcard as a fast path

Preserves the existing api_explorer __ prefix-rule for back-compat. Adds
8 new unit tests covering package expansion, exclusion, order independence,
unknown alias handling, wildcard interaction, and back-compat.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 9 (E-T16c): E2E smoke graph

**Goal:** A real graph JSON exercising `enabled_tools: ["gsheets"]` end-to-end against the local engine. Runs successfully and exposes all 10 gsheets tools to the agent without an explicit list.

**Files:**
- Create: `tests/graphs/agents/gsheets_package_smoke.json`

- [ ] **Step 1: Create the smoke graph**

```bash
mkdir -p tests/graphs/agents
```

Path: `tests/graphs/agents/gsheets_package_smoke.json`

```json
{
  "_comment": "Smoke test for E-T16: enabled_tools: ['gsheets'] expands to all 10 gsheets tools via the toolkit_packages registry, without listing them individually. Requires set -a; source .env; set +a; export PYTHONPATH=/Users/danielgarcia/startti/colmena/.venv/lib/python3.14/site-packages; cargo run --bin dag_engine -- run tests/graphs/agents/gsheets_package_smoke.json --agent-session-id smoke_$(date +%s) --include-extra-info",
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/gsheets-pkg-smoke",
        "method": "POST",
        "test_payload": {
          "prompt": "Listame las pestañas del spreadsheet 1F7AsFx4yW4uVnJRaRWwpzQuSNvruqGohI2B2NRygT-Y. Solo una llamada a gsheets_list_sheets y reportame el nombre de las pestañas."
        }
      }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "stream": false,
        "max_iterations": 4,
        "lazy_tool_loading": false,
        "connection_url": "${DATABASE_URL}",
        "system_message": "You are a smoke-test agent. Use gsheets_list_sheets to answer the user, then reply.",
        "enabled_tools": ["gsheets"]
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "trigger", "to": "agent" },
    { "from": "agent", "to": "log" }
  ]
}
```

- [ ] **Step 2: Run the smoke graph and confirm it works**

```bash
set -a; source .env; set +a
export PYTHONPATH=/Users/danielgarcia/startti/colmena/.venv/lib/python3.14/site-packages
cargo run --release --bin dag_engine -- run tests/graphs/agents/gsheets_package_smoke.json \
  --agent-session-id smoke_$(date +%s) --include-extra-info 2>&1 | tee /tmp/colmena_e2e/gsheets_pkg_smoke.sse
```

Expected: the SSE shows `gsheets_list_sheets` being called and returning OK, confirming the package alias activated the tool without listing it explicitly. Save the SSE under `/tmp/colmena_e2e/` per project convention.

- [ ] **Step 3: Commit**

```bash
git add tests/graphs/agents/gsheets_package_smoke.json
git commit -m "test(e2e): smoke graph for enabled_tools: ['gsheets'] package alias

E-T16c. Confirms that the package registry + filter_enabled_tools changes
work end-to-end: the agent sees gsheets tools without an explicit list.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 10 (E-T16d): Docs sweep

**Goal:** Document the new mechanism in the developer guide, update node_as_tools_reference.json, write CHANGELOG entries, and refresh BACKLOG.

**Files:**
- Modify: `docs/developer_guide/39_gsheets.md`
- Create: `docs/developer_guide/40_toolkit_packages.md`
- Modify: `docs/developer_guide/29_lazy_tool_loading.md`
- Modify: `docs/developer_guide/DEVELOPER_GUIDE.md`
- Modify: `docs/node_as_tools_reference.json`
- Modify: `docs/CHANGELOG_2026-06.md`
- Modify: `docs/BACKLOG.md`

- [ ] **Step 1: Create `docs/developer_guide/40_toolkit_packages.md`**

Write a doc that covers, in order:
1. Concept (one paragraph).
2. Syntax with 6 examples (see spec §6).
3. Naming convention (no `_` in alias, enforced by test).
4. Exclusion semantics (`!` prefix, order-independent, set-based).
5. Decisions on edge cases (table from spec §8).
6. How to register a new package (link the `toolkit_packages.rs` file; show the struct literal pattern).
7. Comparison with `api_explorer`'s `__` prefix-rule (back-compat path).

Each section can be 3-8 lines. Total doc length: ~150-200 lines including the examples.

- [ ] **Step 2: Update `docs/developer_guide/39_gsheets.md`**

Add a "Recommended activation" subsection at the top of the existing doc, ABOVE the per-tool detail:

```markdown
## Recommended activation

Enable the whole gsheets surface with one line:

​```json
"enabled_tools": ["gsheets"]
​```

This expands to all 10 gsheets tools via the toolkit-packages registry.
For a read-only-style agent, exclude write tools:

​```json
"enabled_tools": ["gsheets", "!gsheets_delete_sheet", "!gsheets_add_sheet", "!gsheets_create_spreadsheet"]
​```

See [40_toolkit_packages.md](40_toolkit_packages.md) for the full syntax.
```

(The backticks are escaped here with zero-width chars — the implementer uses real backticks in the doc.)

- [ ] **Step 3: Update `docs/developer_guide/29_lazy_tool_loading.md`**

Add a section noting:
- `summary` is now required on every synthetic tool (CI-enforced).
- DAG nodes used as tools are exempt — their `description` is user-supplied per `tool_configurations`, so a fixed Rust-side summary doesn't fit; the catalog falls back to truncated description for them.
- Reference `every_synthetic_tool_has_summary` test.

- [ ] **Step 4: Update `docs/developer_guide/DEVELOPER_GUIDE.md`**

Add a line under the developer-guide index:

```markdown
- `40_toolkit_packages.md` — Toolkit packages: enable many tools with one alias; exclusion syntax
```

- [ ] **Step 5: Update `docs/node_as_tools_reference.json`**

Add a top-level `toolkit_packages` key with one entry per package:

```json
"toolkit_packages": {
  "gsheets": {
    "description": "Read, write, and analyze Google Sheets workbooks (10 tools)",
    "tools": [
      "gsheets_create_spreadsheet",
      "gsheets_create_from_xlsx",
      "gsheets_export_xlsx",
      "gsheets_list_sheets",
      "gsheets_add_sheet",
      "gsheets_delete_sheet",
      "gsheets_read",
      "gsheets_set_cell",
      "gsheets_set_range",
      "gsheets_run_python"
    ],
    "exclusion_example": "['gsheets', '!gsheets_delete_sheet']"
  }
}
```

- [ ] **Step 6: Update `docs/CHANGELOG_2026-06.md`**

Append the entries (use the rolling-changelog style already in the file):

```markdown
- **E-T15 shipped 2026-06-06** — every synthetic tool registered in colmena
  now declares a one-line `summary` enforced by the
  `every_synthetic_tool_has_summary` CI test. Powers `lazy_tool_loading: true`
  across gsheets, crdt_doc, document, and helper tools. DAG nodes used as
  tools are intentionally exempt (their descriptions are user-configured per
  agent and dynamic).
- **E-T16 shipped 2026-06-06** — toolkit packages: `enabled_tools: ["gsheets"]`
  expands to every gsheets_* tool via a static registry. `!toolname`
  exclusion lets users carve out a subset (e.g. read-only agents). Naming
  convention enforced in CI: package aliases must not contain `_`. See
  [docs/developer_guide/40_toolkit_packages.md](developer_guide/40_toolkit_packages.md).
```

- [ ] **Step 7: Update `docs/BACKLOG.md`**

Append two items:

```markdown
- **Toolkit packages v1.1** — auto-inject package description into the agent
  system message when a package is enabled (one-paragraph orientation block).
- **Unknown alias warning** — when `enabled_tools` contains a name that
  matches no tool, package, or `configured_alias`, surface a structured
  warning in `extra_info` instead of silently producing an empty filter.
- **DAG-node summaries** — extend `ExecutableNode` trait with optional
  `summary()` method so DAG nodes used as tools can declare a default; the
  user's `tool_configurations.<name>.summary` overrides it per agent.
```

- [ ] **Step 8: Commit the docs sweep**

```bash
git add docs/developer_guide/39_gsheets.md \
        docs/developer_guide/40_toolkit_packages.md \
        docs/developer_guide/29_lazy_tool_loading.md \
        docs/developer_guide/DEVELOPER_GUIDE.md \
        docs/node_as_tools_reference.json \
        docs/CHANGELOG_2026-06.md \
        docs/BACKLOG.md
git commit -m "docs(E-T15+T16): toolkit packages + summary coverage docs

- New developer guide section 40_toolkit_packages.md with full syntax
- 39_gsheets.md gets a 'Recommended activation' subsection pointing at
  the package alias
- 29_lazy_tool_loading.md confirms the summary requirement and the
  DAG-node exemption
- node_as_tools_reference.json adds the toolkit_packages top-level key
- CHANGELOG_2026-06.md records the two ship entries
- BACKLOG.md captures 3 deferred follow-ups

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Final sweep

- [ ] **Step 1: Full test suite**

```bash
cargo test --lib --quiet 2>&1 | tail -10
```

Expected: all green.

- [ ] **Step 2: Clippy**

```bash
cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
```

Expected: no warnings.

- [ ] **Step 3: Format**

```bash
cargo fmt --check 2>&1 | tail -5
```

Expected: no diff. If there is diff, `cargo fmt` then re-commit as a fixup.

- [ ] **Step 4: Re-run the smoke graph from Task 9**

```bash
set -a; source .env; set +a
export PYTHONPATH=/Users/danielgarcia/startti/colmena/.venv/lib/python3.14/site-packages
cargo run --release --bin dag_engine -- run tests/graphs/agents/gsheets_package_smoke.json \
  --agent-session-id final_$(date +%s) --include-extra-info 2>&1 | tail -30
```

Expected: agent calls `gsheets_list_sheets` and returns the tab list.

---

## Self-review checklist

| Spec section | Plan task(s) | OK? |
|---|---|---|
| §1 Goals — toolkit packages | Tasks 7, 8, 9 | ✅ |
| §1 Goals — summaries for all synthetic tools | Tasks 1–6 | ✅ |
| §3 Open-source rule | Honoured: the registry contains no ADP names | ✅ |
| §4 Naming convention enforced | Task 7 step 1, test `package_aliases_have_no_underscore` | ✅ |
| §5 Components — `ToolkitPackage` struct | Task 7 | ✅ |
| §5 Components — extended `filter_enabled_tools` | Task 8 | ✅ |
| §5 Components — `summary` field on `ToolDefinition` | Task 1 | ✅ |
| §6 Syntax (6 examples) | Tested in Task 8 | ✅ |
| §7 Algorithm (3-pass) | Implemented in Task 8 step 3 | ✅ |
| §8 Edge-case decisions (8 cases) | Tested in Task 8 (cases 1, 3, 4, 6, 7, 9 explicitly; 2, 5 implicit) | ✅ |
| §9 Back-compat matrix | Final sweep step 1 re-runs every test | ✅ |
| §10 Testing (16 items) | Tasks 1, 2, 6, 7, 8, 9 | ✅ |
| §11 Docs (7 files) | Task 10 | ✅ |
| §12 Task breakdown | Tasks 0–10 map 1:1 to E-T15a–d + E-T16a–d (plus optional final sweep) | ✅ |
| §13 BACKLOG | Task 10 step 7 | ✅ |
