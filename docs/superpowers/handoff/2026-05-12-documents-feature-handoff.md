# Documents Feature — Agent Handoff (2026-05-12)

This document gives a fresh agent on any machine the full context to continue
implementing the documents feature without any prior conversation history.

---

## What the feature is

Word/Excel document generation and granular patching as versioned artifacts.

An LLM agent can:
1. Create an Excel workbook or Word document from a typed JSON IR
2. Apply surgical patches (per-cell, per-block, per-run) rather than rewriting the whole document
3. Read/download any version, roll back, list versions
4. Do all of this via synthetic LLM tools (the LLM calls them) **or** via dedicated DAG nodes

The key design is the **Intermediate Representation (IR)**: a typed JSON document that
is the source-of-truth for every artifact. Renderers convert IR → `.xlsx` / `.docx`.
Patches mutate only the IR fields they target, the rest stays untouched. This pattern
is called JSON-Patch / OT-lite / mini-CRDT in the literature; the IR is a DOM.

---

## Architecture (hexagonal)

```
src/libs/colmena/src/documents/
├── domain/
│   ├── ids.rs          — ArtifactId, VersionId, SessionId, ArtifactKind
│   ├── artifact.rs     — ArtifactMeta, VersionData, PatchApplied, ArtifactSummary
│   ├── patch.rs        — Patch, PatchOp (all Excel + Word ops, JsonSchema)
│   ├── error.rs        — DocumentError, StorageError, RenderError, IndexError
│   ├── ports.rs        — ArtifactStore, IRRenderer, IRValidator, IdGenerator,
│   │                     SessionArtifactIndex  (all traits / ports)
│   ├── mod.rs
│   └── ir/
│       ├── excel.rs    — ExcelIR, SheetIR, TableIR, CellValue, …
│       ├── word.rs     — WordIR, Block enum, Run, ListItem, …
│       ├── common.rs   — shared types (StyleDef, …)
│       └── mod.rs      — pub use, SCHEMA_VERSION constant
├── application/
│   ├── create_document.rs  — CreateDocumentUseCase
│   ├── apply_patch.rs      — ApplyPatchUseCase
│   ├── read_document.rs    — ReadDocumentUseCase
│   ├── get_head.rs         — GetHeadUseCase
│   ├── list_versions.rs    — ListVersionsUseCase
│   ├── rollback.rs         — RollbackUseCase
│   ├── apply_excel_ops.rs  — ExcelOpApplier (applies PatchOps to ExcelIR)
│   ├── apply_word_ops.rs   — WordOpApplier (applies PatchOps to WordIR)
│   ├── runtime.rs          — DocumentRuntime (bundles use cases; from_config is async)
│   └── mod.rs
├── infrastructure/
│   ├── ids.rs              — UlidIdGenerator
│   ├── render/
│   │   ├── excel_renderer.rs  — ExcelRenderer (rust_xlsxwriter)
│   │   └── word_renderer.rs   — WordRenderer (docx-rs)
│   ├── storage/
│   │   ├── local_fs_store.rs  — LocalFsStore (Block A — DONE)
│   │   ├── gcs_store.rs       — GcsArtifactStore (Block B — DONE, feature = "gcs")
│   │   └── mod.rs
│   └── validation/
│       ├── excel_validator.rs
│       └── word_validator.rs
└── mod.rs

DAG nodes (share DocumentRuntime via tokio::sync::OnceCell):
src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs
  — document_create, document_edit, document_read

LLM synthetic tools (registered inside llm.rs):
src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
  — document_create_tool, document_edit_tool, document_read_tool, document_list_tool
```

### Storage object layout (both LocalFS and GCS)

```
{prefix}/artifacts/{artifact_id}/
    meta.json              ← ArtifactMeta
    HEAD                   ← current VersionId (plain text, e.g. "v3")
    ._manifest.json        ← VersionManifest { versions: ["v1","v2","v3"] }
    versions/
        {version_id}/
            ir.json
            render.{xlsx|docx}
            patch_applied.json
            blobs/{name}   ← optional binary attachments
```

---

## Implementation status

### ✅ Block A — Excel MVP + LocalFS + synthetic LLM tools + DAG nodes

Everything in the domain, application, LocalFS storage, Excel render/validate,
Word render/validate, DAG nodes, LLM synthetic tools.

Smoke test: `tests/graphs/documents/smoke_create_edit_read.json`
Integration test (LLM tool calling): `tests/graphs/documents/llm_tool_integration.json`

Run smoke test:
```bash
cargo run --bin dag_engine -- run tests/graphs/documents/smoke_create_edit_read.json
```

### ✅ Block B — GCS ArtifactStore (feature-gated)

File: `src/libs/colmena/src/documents/infrastructure/storage/gcs_store.rs`

Crate feature: `gcs = ["dep:google-cloud-storage"]` (in `src/libs/colmena/Cargo.toml`)

Key decisions:
- `google-cloud-storage` v1.4.0 only exposes `write_object` and `read_object` on
  `Storage`. `delete_object`, `list_objects`, `StorageControl` are all private/crate-internal.
- Worked around `list_objects` by maintaining a `._manifest.json` per artifact (stores
  an ordered `Vec<String>` of version IDs).
- Worked around `delete_object` with **soft-delete**: `delete_version` removes the entry
  from the manifest but leaves GCS objects in place. `delete_artifact` writes tombstone
  `"DELETED"` to HEAD and clears the manifest. GCS objects are reclaimed by bucket
  lifecycle rules — configure `age > 90` days in your GCS bucket settings.
- HEAD uses Compare-And-Swap via `set_if_generation_match`:
  - `expected_current = None` → `generation = 0` (create-only)
  - `expected_current = Some(v)` → read HEAD to get current generation, verify content
    matches `v`, then write with that generation as precondition; HTTP 412 → `PreconditionFailed`

`DocumentRuntime::from_config` is **async** (needed for GCS client `build().await`).
All callers were updated accordingly:
- `document_nodes.rs` → `build_runtime` is now `async fn`, uses `OnceCell::get_or_try_init`
  with an async closure
- `llm.rs` → `DocumentRuntime::from_config(&doc_cfg).await`

Build with GCS:
```bash
cargo build --features gcs
cargo test --lib documents --features gcs
```

Config when using GCS in a graph node:
```json
{
  "storage_backend": "gcs",
  "gcs_bucket": "my-colmena-bucket",
  "gcs_prefix": "colmena/documents"
}
```

### ❌ Block C — SessionArtifactIndex (NOT YET IMPLEMENTED)

The `SessionArtifactIndex` port (defined in `domain/ports.rs`) maps
`session_id → [artifact_id]`. It allows an agent to list *its own* artifacts and
enforces session isolation.

What needs implementing:

**1. `InMemorySessionIndex`** in `infrastructure/` — simple `HashMap` behind an `Arc<RwLock<...>>`.
Useful for tests and single-process deployments.

**2. `SqliteSessionIndex`** / `PostgresSessionIndex`** — backed by the existing
SQLite/Postgres pool registry (`SqlitePool` / `PgPool` from sqlx). Should reuse the
pool registry from `src/libs/colmena/src/shared/`.

Schema (create once on startup if table doesn't exist):
```sql
CREATE TABLE IF NOT EXISTS document_artifacts (
    artifact_id  TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL,
    kind         TEXT NOT NULL,         -- "excel" | "word"
    label        TEXT,
    current_version TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_doc_artifacts_session ON document_artifacts(session_id);
```

Methods map to `SessionArtifactIndex` trait:
- `register` → `INSERT OR REPLACE INTO document_artifacts ...`
- `list_by_session` → `SELECT * FROM document_artifacts WHERE session_id = ?`
- `lookup` → `SELECT * FROM document_artifacts WHERE artifact_id = ?`
- `update_head` → `UPDATE document_artifacts SET current_version=?, updated_at=? WHERE artifact_id=?`
- `unregister` → `DELETE FROM document_artifacts WHERE artifact_id=?`

**3. Wire into DocumentRuntime** — add `pub index: Arc<dyn SessionArtifactIndex>` to
`DocumentRuntime`. Expose `list_artifacts` use case that calls `index.list_by_session`.

**4. Call `register` in `CreateDocumentUseCase`** right after `store.create_artifact`.

**5. Call `update_head` in `ApplyPatchUseCase`** after `store.set_head`.

Key file to modify: `src/libs/colmena/src/documents/application/runtime.rs`

Config fields for the index backend (add to `from_config`):
```json
{
  "index_backend": "memory",        // "memory" | "sqlite" | "postgres"
  "index_db_url": "sqlite://.colmena/documents.db"
}
```

### ❌ Block D — Concurrency / Rebase (NOT YET IMPLEMENTED)

Currently `ApplyPatchUseCase` returns `DocumentError::VersionConflict` if
`patch.base_version != meta.current_version`. This is correct but the caller gets
an error instead of an auto-rebase.

Auto-rebase logic (to implement in `apply_patch.rs`):
1. If `base_version < current_version`, read all patch history between base and current.
2. Check each patch op for conflict with the incoming ops (same cell address / same block
   id being mutated vs deleted, etc.).
3. If no conflict → fast-forward: apply incoming patch on top of current IR, succeed.
4. If conflict → return `VersionConflict` with the conflicting op details populated.

For now, the simplest useful behavior is:
- Same-cell / same-block write+write → **last-writer-wins**: rebase silently succeeds,
  incoming patch wins.
- Delete+write or write+delete on same target → return conflict.

The spec is in `docs/superpowers/specs/2026-04-21-documents-feature-design.md` §8.

### ❌ Block E — Documentation + integration tests (NOT YET IMPLEMENTED)

1. Update `docs/node_configurations.json` with `document_create`, `document_edit`,
   `document_read` node schemas (config fields, types, descriptions).
2. Update `docs/node_as_tools_reference.json` with document tool examples.
3. Add integration test graphs in `tests/graphs/documents/`:
   - `gcs_roundtrip.json` (requires `--features gcs` + real GCS bucket)
   - `word_create_edit_read.json`
   - `rollback.json`
   - `multi_agent_conflict.json` (two edit nodes on same artifact to verify conflict path)
4. GCS bucket setup instructions (see section below).

---

## DAG node config schemas

### `document_create`

| Field | Type | Required | Default | Notes |
|-------|------|----------|---------|-------|
| `kind` | `"excel"` \| `"word"` | ✅ | — | Document type |
| `label` | string | — | `""` | Human-readable name |
| `storage_backend` | `"localfs"` \| `"gcs"` | — | `"localfs"` | |
| `storage_root` | string | — | `.colmena/documents` | localfs path |
| `gcs_bucket` | string | if gcs | — | Bare bucket name |
| `gcs_prefix` | string | — | `"colmena/documents"` | GCS path prefix |
| `default_retention` | u32 | — | `20` | Versions to keep |
| `initial_ir` | object | — | empty doc | Pre-populated IR |

Outputs: `{ "artifact_id": "...", "version_id": "v1" }`

### `document_edit`

| Field | Type | Required | Default | Notes |
|-------|------|----------|---------|-------|
| `artifact_id` | string | ✅ | — | From `create` output |
| `base_version` | string | ✅ | — | From previous output |
| `ops` | array | ✅ | — | `PatchOp[]` |
| `storage_backend` / `storage_root` / `gcs_*` | same as create | | | |

Outputs: `{ "artifact_id": "...", "version_id": "v2", "summary": {...} }`

### `document_read`

| Field | Type | Required | Default | Notes |
|-------|------|----------|---------|-------|
| `artifact_id` | string | ✅ | — | |
| `version` | string | — | HEAD | `null` = current |
| `storage_backend` / `storage_root` / `gcs_*` | same as create | | | |

Outputs: `{ "artifact_id": "...", "version": "...", "ir": {...} }`

---

## Key types reference

### `PatchOp` variants (complete list)

Excel ops: `set_cell`, `set_range`, `clear_range`, `insert_row`, `delete_row`,
`insert_column`, `delete_column`, `add_sheet`, `rename_sheet`, `delete_sheet`,
`reorder_sheets`, `create_table`, `resize_table`, `delete_table`, `set_column_width`,
`define_style`

Word ops: `insert_block`, `delete_block`, `replace_block`, `move_block`,
`set_heading_level`, `replace_run_text`, `set_run_style`, `insert_run`, `delete_run`,
`insert_list_item`, `replace_list_item`, `delete_list_item`, `insert_table_row`,
`delete_table_row`, `update_table_cell`

All ops use `#[serde(tag = "op")]` — the discriminant key is `"op"`.

### `DocumentError` variants (conflict vs other)

```rust
DocumentError::VersionConflict { artifact, base, current, conflicts: Vec<String> }
DocumentError::ArtifactNotFound(ArtifactId)
DocumentError::VersionNotFound { artifact, version }
DocumentError::IRValidationFailed { path, reason }
DocumentError::StorageError(StorageError)
DocumentError::RenderError(RenderError)
```

### `StorageError`

```rust
StorageError::NotFound(String)          // key that was missing
StorageError::PreconditionFailed(String) // CAS / generation check failed
StorageError::Backend(String)           // generic I/O or GCS error
```

---

## GCS bucket setup

1. Create a GCS bucket in your project:
   ```bash
   gcloud storage buckets create gs://my-colmena-bucket \
     --location=us-central1 \
     --uniform-bucket-level-access
   ```

2. Add a lifecycle rule to reclaim orphaned objects (soft-deleted):
   ```bash
   gcloud storage buckets update gs://my-colmena-bucket \
     --lifecycle-file=- <<'EOF'
   {
     "lifecycle": {
       "rule": [{
         "action": { "type": "Delete" },
         "condition": { "age": 90 }
       }]
     }
   }
   EOF
   ```

3. Application credentials: the GCS client uses ADC (Application Default Credentials).
   Either run `gcloud auth application-default login` locally, or set
   `GOOGLE_APPLICATION_CREDENTIALS=/path/to/service-account.json`.

4. Build and test with GCS feature:
   ```bash
   cargo build --features gcs
   cargo test --lib documents --features gcs
   # To run the spike:
   cargo run --example gcs_spike --features gcs
   ```

---

## Build and test commands

```bash
# Unit tests for all document submodules
cargo test --lib documents

# Unit tests for document_nodes (DAG integration)
cargo test --lib document_nodes

# Run smoke graph (LocalFS, no API key needed)
cargo run --bin dag_engine -- run tests/graphs/documents/smoke_create_edit_read.json

# Run LLM integration graph (needs ANTHROPIC_API_KEY or similar)
source .env
cargo run --bin dag_engine -- run tests/graphs/documents/llm_tool_integration.json

# Compile-check GCS feature
cargo build --features gcs

# Full CI check (what the pipeline runs)
cargo test --verbose
```

**Important — crate name is `colmena_dag_engine` not `colmena`:**
```bash
cargo test -p colmena_dag_engine --lib documents   # explicit package form
```

---

## Known limitations / technical debt

1. **No `SessionArtifactIndex` impl** — `list_artifacts` LLM tool currently returns an
   error or empty list. Block C must be done before multi-artifact sessions are useful.

2. **No auto-rebase** — concurrent edits from agent + user always conflict. Block D adds
   last-writer-wins for non-conflicting cells.

3. **Soft-delete** — GCS objects are never hard-deleted; lifecycle rules on the bucket
   are the cleanup mechanism. This is by design (no `delete_object` on the public API).

4. **Manifest is last-writer-wins** — two concurrent writers to `._manifest.json` can
   clobber each other's version registration. In practice the manifest is only a
   convenience (fast `list_versions`); the actual data is still there in the versioned
   object paths. A future Block could add CAS to manifest writes too.

5. **`render.{ext}` uses hardcoded extension in `read_version`** — reads `meta.kind`
   to determine extension. If kind is changed after creation (not a supported operation),
   the read will fail to find the render blob.

---

## Files that touch the documents feature

| File | Role |
|------|------|
| `src/libs/colmena/src/documents/**` | All domain, application, infrastructure code |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs` | DAG nodes |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` | Synthetic LLM tools wired here |
| `src/libs/colmena/src/dag_engine/infrastructure/registry.rs` | Node type registration |
| `src/libs/colmena/Cargo.toml` | Feature flag `gcs`, dependency declarations |
| `tests/graphs/documents/` | JSON test graphs |
| `docs/superpowers/specs/2026-04-21-documents-feature-design.md` | Full design spec |
| `docs/superpowers/plans/2026-04-21-documents-feature.md` | Implementation plan (task checklist) |

---

## Where to continue

Next immediate task is **Block C — SessionArtifactIndex**. The simplest path:

1. Add `InMemorySessionIndex` struct in a new file
   `src/libs/colmena/src/documents/infrastructure/session_index/memory.rs` implementing
   the `SessionArtifactIndex` trait from `domain/ports.rs`.

2. Add it to `DocumentRuntime` as `pub index: Arc<dyn SessionArtifactIndex>`.

3. Wire `register` call into `CreateDocumentUseCase::execute` after `store.create_artifact`.

4. Wire `update_head` call into `ApplyPatchUseCase::execute` after `store.set_head`.

5. Expose a `document_list` use case (`list_by_session`) and add the DAG node /
   LLM synthetic tool.

6. (Optional) Add `SqliteSessionIndex` for persistent single-machine deployments.

The design spec (§9) has the full rationale:
`docs/superpowers/specs/2026-04-21-documents-feature-design.md`
