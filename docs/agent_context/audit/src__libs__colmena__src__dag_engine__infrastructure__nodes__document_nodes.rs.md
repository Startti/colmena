# src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs

**Layer:** infrastructure  
**Purpose:** Implements three DAG node types (`document_create`, `document_edit`, `document_read`) that wrap the documents module's application layer. Each node lazily builds a `DocumentRuntime` via `OnceCell` and exposes document creation, patching, and reading as graph steps or LLM tools with `$DYNAMIC` support.

## Symbols

- `DocNodeError` (enum, private) — Error type for node configuration and missing-field errors; two variants: `Config(String)` and `MissingField(&'static str)`.
- `resolve_session_id()` (fn, private) — Resolves session_id with priority: input `__colmena_session_id` > input `session_id` > config `session_id`, defaults to "default".
- `build_runtime()` (async fn, private) — Lazily initializes `DocumentRuntime` from config using `OnceCell::get_or_try_init`.
- `document_error_to_value()` (fn, private) — Converts `DocumentError` to JSON; special handling for `VersionConflict` with current_version and conflicts fields.
- `DocumentCreateNode` (struct, pub) — DAG node for creating new document artifacts (Excel or Word); contains `OnceCell<Arc<DocumentRuntime>>`.
- `DocumentCreateNode::new()` (fn, pub) — Constructor; initializes with empty `OnceCell`.
- `DocumentCreateNode::default()` (fn, pub) — Default trait; delegates to `new()`.
- `DocumentCreateNode::execute()` (async fn, pub via ExecutableNode trait) — Extracts `kind`, `initial_ir`, `label`, `retention_limit`, session_id from inputs/config; calls `runtime.create.execute()`; returns `{artifact_id, version_id, label}` or error.
- `DocumentCreateNode::default_output()` (fn, pub via ExecutableNode trait) — Returns "output" as the default output port.
- `DocumentCreateNode::description()` (fn, pub via ExecutableNode trait) — Returns node description for tool catalog.
- `DocumentCreateNode::schema()` (fn, pub via ExecutableNode trait) — Returns JSON schema of config fields and output structure.
- `DocumentEditNode` (struct, pub) — DAG node for applying patches to existing artifacts; contains `OnceCell<Arc<DocumentRuntime>>`.
- `DocumentEditNode::new()` (fn, pub) — Constructor; initializes with empty `OnceCell`.
- `DocumentEditNode::default()` (fn, pub) — Default trait; delegates to `new()`.
- `DocumentEditNode::execute()` (async fn, pub via ExecutableNode trait) — Extracts `artifact_id`, `base_version`, `ops` array from inputs/config; parses `ops` as `Vec<PatchOp>`; calls `runtime.apply.execute()`; returns `{version_id, diff_summary}` or structured `VersionConflict` error with current_version and conflicts.
- `DocumentEditNode::default_output()` (fn, pub via ExecutableNode trait) — Returns "output" as the default output port.
- `DocumentEditNode::description()` (fn, pub via ExecutableNode trait) — Returns node description for tool catalog.
- `DocumentEditNode::schema()` (fn, pub via ExecutableNode trait) — Returns JSON schema of config fields and output structure.
- `DocumentReadNode` (struct, pub) — DAG node for reading artifacts at a given or current version; contains `OnceCell<Arc<DocumentRuntime>>`.
- `DocumentReadNode::new()` (fn, pub) — Constructor; initializes with empty `OnceCell`.
- `DocumentReadNode::default()` (fn, pub) — Default trait; delegates to `new()`.
- `DocumentReadNode::execute()` (async fn, pub via ExecutableNode trait) — Extracts `artifact_id`, optional `version` from inputs/config; calls `runtime.read.execute()`; returns `{ir, version_id}` or error.
- `DocumentReadNode::default_output()` (fn, pub via ExecutableNode trait) — Returns "output" as the default output port.
- `DocumentReadNode::description()` (fn, pub via ExecutableNode trait) — Returns node description for tool catalog.
- `DocumentReadNode::schema()` (fn, pub via ExecutableNode trait) — Returns JSON schema of config fields and output structure.
- `tests` (module, test) — Five test cases:
  - `make_inputs()` — Helper; returns empty `HashMap`.
  - `create_node_returns_artifact_id_and_version()` — Tests `DocumentCreateNode::execute` returns artifact_id (starts with "art_") and version_id "v1".
  - `create_then_read_roundtrip()` — Tests create + read flow; verifies created IR can be read back with matching sheet structure.
  - `edit_node_applies_patch_and_advances_version()` — Tests `DocumentEditNode::execute` with a single `set_cell` op; verifies version advances to v2.
  - `edit_node_reports_version_conflict_on_stale_base()` — Tests error handling when patch targets stale base_version; verifies `VersionConflict` error returned.
  - `nodes_have_descriptions()` — Sanity check that all three node types have descriptions (non-None).

## File-level notes

- **Pattern consistency:** All three node types follow the same OnceCell-lazy-init pattern as `sql_node.rs`, ensuring the `DocumentRuntime` (and its store / renderers / use cases) is built once per node instance and reused across invocations.
- **Input resolution hierarchy:** Matches the LLM node convention — `__colmena_session_id` (engine context) > `session_id` input > `session_id` config, defaulting to "default" for standalone graph runs.
- **Error strategy:** Defensive error handling — `DocumentError` variants map to structured JSON (especially `VersionConflict` with full conflict list), other errors fallback to string representation. Safe but loses specific error detail for unmapped variants.
- **Test coverage:** Good integration coverage — create, read, edit, and version-conflict paths verified. Tests use `tempdir()` for isolation. No tests of `$DYNAMIC` variable resolution or tool-exposure details (those likely tested at engine level).
- **$DYNAMIC support:** Documented in comments (§11.2, §11.3 references) but not exercised in these tests; input fields like `initial_ir`, `ops` are designed to be resolved by the engine's variable resolver before reaching `execute()`.
- **No external trait impls:** All three nodes are pure `ExecutableNode` implementations; no custom traits or observability hooks (the `observer` parameter is accepted but unused, matching the pattern for nodes that don't emit side events).
