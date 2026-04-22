# Documents Feature Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `documents/` module to Colmena that lets an LLM agent create and edit Word/Excel artifacts through a typed patch protocol, with versioned IR source-of-truth, dual storage (LocalFS + GCS), agent+user concurrency with auto-rebase, and dual integration as synthetic LLM tools and DAG nodes.

**Architecture:** New hexagonal module `src/libs/colmena/src/documents/` with `domain/` (IR, PatchOp, ports, errors), `application/` (use cases: create/apply_patch/read/rollback/get_head/list_versions/list_artifacts/download/rebase/diff), and `infrastructure/` (LocalFS + GCS stores, rust_xlsxwriter + docx-rs renderers, validators, conflict detectors, diff, session-artifact index over existing SQLite/Postgres pools). Two integration surfaces sharing application layer: synthetic LLM tools under `dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs` and DAG nodes under `dag_engine/infrastructure/nodes/` (`document_create`, `document_edit`, `document_read`).

**Tech Stack:** Rust (tokio async), `rust_xlsxwriter` (Excel render), `docx-rs` (Word render), `schemars` (JSON Schema derivation for LLM-visible tool inputs), `google-cloud-storage` (GCS adapter), `sqlx` (SQLite + Postgres indexing reusing existing pool registry), `calamine` + `docx-rust` read-side for round-trip verification tests.

**Design spec:** [docs/superpowers/specs/2026-04-21-documents-feature-design.md](../specs/2026-04-21-documents-feature-design.md)

---

## Conventions for this plan

- All commits use: `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>`
- Run Rust tests with `cargo test --lib <module>` — crate is `colmena_dag_engine`.
- TDD: test → fail → minimal impl → pass → commit. One action per step (2-5 min).
- Every task ends with a commit. Don't batch commits across tasks.
- When a task says "verify it fails with X", the specific failure is load-bearing.
- The plan is split into 5 phases: **A** (Excel MVP), **B** (Word), **C** (Session index + DB), **D** (Concurrency/Rebase), **E** (GCS + DAG nodes + skill + docs).

---

## Phase A — Excel MVP with LocalFS and synthetic LLM tools

Phase A delivers an end-to-end working Excel flow: create doc → apply patches → read → rollback → list, persisted on LocalFS, exposed as synthetic LLM tools (no DAG nodes, no Word, no concurrency, no DB index yet).

### Task A0: Scaffold module, dependencies, and registration

**Files:**
- Modify: `src/libs/colmena/Cargo.toml`
- Modify: `src/libs/colmena/src/lib.rs`
- Create: `src/libs/colmena/src/documents/mod.rs`
- Create: `src/libs/colmena/src/documents/domain/mod.rs`
- Create: `src/libs/colmena/src/documents/application/mod.rs`
- Create: `src/libs/colmena/src/documents/infrastructure/mod.rs`

- [ ] **Step 1: Add dependencies to Cargo.toml**

In `src/libs/colmena/Cargo.toml` under `[dependencies]`, after the `include_dir = "0.7"` line, add:

```toml
# Documents feature
rust_xlsxwriter = "0.77"
docx-rs = "0.4"
schemars = { version = "0.8", features = ["chrono", "uuid1", "preserve_order"] }
ulid = "1"
```

Under `[dev-dependencies]` add:

```toml
calamine = "0.24"
```

- [ ] **Step 2: Create module scaffolding**

Create `src/libs/colmena/src/documents/mod.rs`:

```rust
//! Documents — Word/Excel artifact generation and granular editing.
//!
//! See `docs/superpowers/specs/2026-04-21-documents-feature-design.md`.

pub mod domain;
pub mod application;
pub mod infrastructure;
```

Create `src/libs/colmena/src/documents/domain/mod.rs`, `.../application/mod.rs`, `.../infrastructure/mod.rs` — all empty files for now.

- [ ] **Step 3: Register module in lib.rs**

Edit `src/libs/colmena/src/lib.rs`. After `pub mod skills;` add:

```rust
pub mod documents;
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check --lib`
Expected: success, new deps resolve, no warnings about unused documents module.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/Cargo.toml src/libs/colmena/Cargo.lock src/libs/colmena/src/lib.rs src/libs/colmena/src/documents/
git commit -m "$(cat <<'EOF'
feat(documents): scaffold module and add xlsx/docx/schemars/ulid deps

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A1: Domain value objects — ArtifactId, VersionId, SessionId, ArtifactKind

**Files:**
- Create: `src/libs/colmena/src/documents/domain/ids.rs`
- Modify: `src/libs/colmena/src/documents/domain/mod.rs`

- [ ] **Step 1: Write failing test**

Create `src/libs/colmena/src/documents/domain/ids.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArtifactId(pub String);

impl ArtifactId {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.0) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VersionId(pub String);

impl VersionId {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn initial() -> Self { Self("v1".to_string()) }
    pub fn next(&self) -> Self {
        let n: u64 = self.0.trim_start_matches('v').parse().unwrap_or(0);
        Self(format!("v{}", n + 1))
    }
    pub fn as_str(&self) -> &str { &self.0 }
    pub fn number(&self) -> Option<u64> {
        self.0.trim_start_matches('v').parse().ok()
    }
}

impl fmt::Display for VersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.0) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.0) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    Excel,
    Word,
}

impl ArtifactKind {
    pub fn extension(&self) -> &'static str {
        match self {
            ArtifactKind::Excel => "xlsx",
            ArtifactKind::Word => "docx",
        }
    }
    pub fn mime(&self) -> &'static str {
        match self {
            ArtifactKind::Excel => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ArtifactKind::Word => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_id_next_increments() {
        let v = VersionId::initial();
        assert_eq!(v.as_str(), "v1");
        assert_eq!(v.next().as_str(), "v2");
        assert_eq!(VersionId::new("v99").next().as_str(), "v100");
    }

    #[test]
    fn version_id_number_parses() {
        assert_eq!(VersionId::new("v7").number(), Some(7));
        assert_eq!(VersionId::new("vx").number(), None);
    }

    #[test]
    fn artifact_kind_extension() {
        assert_eq!(ArtifactKind::Excel.extension(), "xlsx");
        assert_eq!(ArtifactKind::Word.extension(), "docx");
    }
}
```

- [ ] **Step 2: Register in mod.rs**

Edit `src/libs/colmena/src/documents/domain/mod.rs`:

```rust
pub mod ids;

pub use ids::{ArtifactId, ArtifactKind, SessionId, VersionId};
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib documents::domain::ids`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/domain/
git commit -m "$(cat <<'EOF'
feat(documents): add domain ID value objects (ArtifactId, VersionId, SessionId, ArtifactKind)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A2: Domain errors

**Files:**
- Create: `src/libs/colmena/src/documents/domain/error.rs`
- Modify: `src/libs/colmena/src/documents/domain/mod.rs`

- [ ] **Step 1: Write error enums**

Create `src/libs/colmena/src/documents/domain/error.rs`:

```rust
use super::ids::{ArtifactId, SessionId, VersionId};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error, Serialize)]
pub enum StorageError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("precondition failed (generation mismatch): {0}")]
    PreconditionFailed(String),

    #[error("transient error: {0}")]
    Transient(String),

    #[error("backend error: {0}")]
    Backend(String),
}

#[derive(Debug, Error, Serialize)]
pub enum IndexError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("backend error: {0}")]
    Backend(String),
}

#[derive(Debug, Error, Serialize)]
pub enum RenderError {
    #[error("render failed: {0}")]
    Failed(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct ConflictDetail {
    pub incoming_op: serde_json::Value,
    pub conflicting_with: serde_json::Value,
    pub in_version: VersionId,
    pub reason: String,
}

#[derive(Debug, Error, Serialize)]
pub enum DocumentError {
    #[error("artifact not found: {0}")]
    ArtifactNotFound(ArtifactId),

    #[error("version not found: {artifact}/{version}")]
    VersionNotFound { artifact: ArtifactId, version: VersionId },

    #[error("version conflict: base {base}, current {current}")]
    VersionConflict {
        artifact: ArtifactId,
        base: VersionId,
        current: VersionId,
        conflicts: Vec<ConflictDetail>,
    },

    #[error("IR validation failed at {path}: {reason}")]
    IRValidationFailed { path: String, reason: String },

    #[error("invalid patch op: {reason}")]
    InvalidPatchOp { reason: String, op: serde_json::Value },

    #[error("render failed: {0}")]
    RenderFailed(String),

    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error(transparent)]
    Index(#[from] IndexError),

    #[error("session isolation violation: artifact {0} not in session {1}")]
    SessionIsolationViolation(ArtifactId, SessionId),
}

impl From<RenderError> for DocumentError {
    fn from(e: RenderError) -> Self {
        DocumentError::RenderFailed(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display() {
        let e = DocumentError::ArtifactNotFound(ArtifactId::new("art_x"));
        assert_eq!(e.to_string(), "artifact not found: art_x");
    }

    #[test]
    fn storage_into_document_error() {
        let s = StorageError::NotFound("x".into());
        let d: DocumentError = s.into();
        assert!(matches!(d, DocumentError::Storage(_)));
    }
}
```

- [ ] **Step 2: Register in mod.rs**

Append to `src/libs/colmena/src/documents/domain/mod.rs`:

```rust
pub mod error;

pub use error::{ConflictDetail, DocumentError, IndexError, RenderError, StorageError};
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib documents::domain::error`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/domain/
git commit -m "$(cat <<'EOF'
feat(documents): add domain error types (DocumentError, StorageError, IndexError, RenderError)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A3: Excel IR domain types

**Files:**
- Create: `src/libs/colmena/src/documents/domain/ir/mod.rs`
- Create: `src/libs/colmena/src/documents/domain/ir/common.rs`
- Create: `src/libs/colmena/src/documents/domain/ir/excel.rs`
- Modify: `src/libs/colmena/src/documents/domain/mod.rs`

- [ ] **Step 1: Create common IR types**

Create `src/libs/colmena/src/documents/domain/ir/common.rs`:

```rust
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<FontSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alignment: Option<Alignment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number_format: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FontSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underline: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Alignment {
    Left, Center, Right, Justify,
}
```

- [ ] **Step 2: Create Excel IR types**

Create `src/libs/colmena/src/documents/domain/ir/excel.rs`:

```rust
use super::common::{NamedStyle};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExcelIR {
    pub kind: ExcelKindTag,
    pub artifact_id: String,
    pub version_id: String,
    pub schema_version: String,
    pub workbook: Workbook,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExcelKindTag { Excel }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Workbook {
    pub sheets: Vec<Sheet>,
    #[serde(default)]
    pub named_styles: BTreeMap<String, NamedStyle>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    pub id: String,
    pub name: String,
    pub order: u32,
    #[serde(default)]
    pub columns: Vec<ColumnSpec>,
    #[serde(default)]
    pub cells: BTreeMap<String, Cell>,
    #[serde(default)]
    pub tables: Vec<NamedTable>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnSpec {
    pub index: u32,
    pub width: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub value: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub value_type: Option<CellType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CellType {
    String,
    Number,
    Boolean,
    Date,
    Formula,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedTable {
    pub id: String,
    pub name: String,
    pub range: String,
    #[serde(default)]
    pub header_row: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_preset: Option<String>,
}

impl ExcelIR {
    pub fn empty(artifact_id: impl Into<String>, version_id: impl Into<String>) -> Self {
        Self {
            kind: ExcelKindTag::Excel,
            artifact_id: artifact_id.into(),
            version_id: version_id.into(),
            schema_version: super::common::SCHEMA_VERSION.to_string(),
            workbook: Workbook::default(),
        }
    }

    pub fn sheet_mut(&mut self, sheet_id: &str) -> Option<&mut Sheet> {
        self.workbook.sheets.iter_mut().find(|s| s.id == sheet_id)
    }

    pub fn sheet(&self, sheet_id: &str) -> Option<&Sheet> {
        self.workbook.sheets.iter().find(|s| s.id == sheet_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_empty_excel_ir() {
        let ir = ExcelIR::empty("art_x", "v1");
        let json = serde_json::to_string(&ir).unwrap();
        let back: ExcelIR = serde_json::from_str(&json).unwrap();
        assert_eq!(back.artifact_id, "art_x");
        assert_eq!(back.version_id, "v1");
        assert!(back.workbook.sheets.is_empty());
    }

    #[test]
    fn roundtrip_ir_with_cells() {
        let mut ir = ExcelIR::empty("art_x", "v1");
        let mut cells = BTreeMap::new();
        cells.insert("A1".into(), Cell {
            value: serde_json::json!("Hello"),
            value_type: Some(CellType::String),
            format: None,
            style_ref: None,
        });
        ir.workbook.sheets.push(Sheet {
            id: "sheet_01".into(),
            name: "Ventas".into(),
            order: 0,
            columns: vec![],
            cells,
            tables: vec![],
        });
        let json = serde_json::to_value(&ir).unwrap();
        assert_eq!(json["workbook"]["sheets"][0]["cells"]["A1"]["value"], "Hello");
    }
}
```

- [ ] **Step 3: Create IR mod file**

Create `src/libs/colmena/src/documents/domain/ir/mod.rs`:

```rust
pub mod common;
pub mod excel;

pub use common::{Alignment, FontSpec, NamedStyle, SCHEMA_VERSION};
pub use excel::{Cell, CellType, ColumnSpec, ExcelIR, ExcelKindTag, NamedTable, Sheet, Workbook};
```

- [ ] **Step 4: Register in domain/mod.rs**

Append to `src/libs/colmena/src/documents/domain/mod.rs`:

```rust
pub mod ir;
```

- [ ] **Step 5: Run tests**

Run: `cargo test --lib documents::domain::ir::excel`
Expected: 2 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/documents/domain/
git commit -m "$(cat <<'EOF'
feat(documents): add Excel IR domain types (Workbook, Sheet, Cell, NamedTable)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A4: Artifact metadata and version data

**Files:**
- Create: `src/libs/colmena/src/documents/domain/artifact.rs`
- Modify: `src/libs/colmena/src/documents/domain/mod.rs`

- [ ] **Step 1: Write artifact metadata types**

Create `src/libs/colmena/src/documents/domain/artifact.rs`:

```rust
use super::ids::{ArtifactId, ArtifactKind, SessionId, VersionId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactMeta {
    pub artifact_id: ArtifactId,
    pub kind: ArtifactKind,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub current_version: VersionId,
    pub retention_limit: u32,
    pub pin_initial: bool,
    pub schema_version: String,
    pub session_id: SessionId,
    pub label: String,
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
}

impl ArtifactMeta {
    pub fn initial(
        artifact_id: ArtifactId,
        kind: ArtifactKind,
        session_id: SessionId,
        label: String,
        retention_limit: u32,
    ) -> Self {
        let now = Utc::now();
        Self {
            artifact_id,
            kind,
            created_at: now,
            updated_at: now,
            current_version: VersionId::initial(),
            retention_limit,
            pin_initial: true,
            schema_version: super::ir::SCHEMA_VERSION.to_string(),
            session_id,
            label,
            tags: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchApplied {
    pub patch: serde_json::Value,
    pub applied_at: DateTime<Utc>,
    pub resulted_in: VersionId,
    pub summary: PatchSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PatchSummary {
    #[serde(default)]
    pub natural_language: Vec<String>,
    #[serde(default)]
    pub structured: Vec<serde_json::Value>,
}

pub struct VersionData {
    pub ir: serde_json::Value,
    pub rendered_binary: Vec<u8>,
    pub rendered_extension: &'static str,
    pub patch_applied: PatchApplied,
    pub blobs: Vec<(String, Vec<u8>)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactSummary {
    pub artifact_id: ArtifactId,
    pub session_id: SessionId,
    pub kind: ArtifactKind,
    pub label: Option<String>,
    pub current_version: VersionId,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_meta_sets_v1() {
        let m = ArtifactMeta::initial(
            ArtifactId::new("art_x"),
            ArtifactKind::Excel,
            SessionId::new("sess_1"),
            "Test".into(),
            20,
        );
        assert_eq!(m.current_version, VersionId::initial());
        assert!(m.pin_initial);
        assert_eq!(m.retention_limit, 20);
    }

    #[test]
    fn meta_roundtrip_json() {
        let m = ArtifactMeta::initial(
            ArtifactId::new("art_x"),
            ArtifactKind::Word,
            SessionId::new("sess_1"),
            "R".into(),
            5,
        );
        let j = serde_json::to_string(&m).unwrap();
        let back: ArtifactMeta = serde_json::from_str(&j).unwrap();
        assert_eq!(back.kind, ArtifactKind::Word);
    }
}
```

- [ ] **Step 2: Register in mod.rs**

Append to `src/libs/colmena/src/documents/domain/mod.rs`:

```rust
pub mod artifact;

pub use artifact::{ArtifactMeta, ArtifactSummary, PatchApplied, PatchSummary, VersionData};
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib documents::domain::artifact`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/domain/
git commit -m "$(cat <<'EOF'
feat(documents): add artifact metadata and version data types

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A5: Excel PatchOp enum with schemars

**Files:**
- Create: `src/libs/colmena/src/documents/domain/patch.rs`
- Modify: `src/libs/colmena/src/documents/domain/mod.rs`

- [ ] **Step 1: Create patch types**

Create `src/libs/colmena/src/documents/domain/patch.rs`:

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Patch {
    /// ID of the artifact this patch targets (e.g. "art_abc123").
    pub artifact_id: String,

    /// Version the caller based this patch on (e.g. "v3"). Server rebases
    /// automatically when current version is newer and ops don't conflict.
    pub base_version: String,

    /// Who authored this patch. Only "user" patches generate narration for the LLM.
    #[serde(default = "default_source")]
    pub source: PatchSource,

    /// Ordered list of operations applied atomically.
    pub ops: Vec<PatchOp>,
}

fn default_source() -> PatchSource { PatchSource::Agent }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PatchSource { Agent, User }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op")]
pub enum PatchOp {
    /// Set the value of a single cell. Creates it if missing, overwrites if
    /// present. Use for isolated changes. For contiguous bulk updates, prefer
    /// `set_range`.
    #[serde(rename = "set_cell")]
    SetCell {
        /// Stable sheet ID (e.g. "sheet_01"). NOT the display name.
        sheet_id: String,
        /// A1-style address (e.g. "B5", "AA120"). Case-insensitive.
        address: String,
        /// The value. Type inferred from JSON type unless `value_type` overrides.
        value: serde_json::Value,
        /// Optional: override the inferred type. Use for numbers stored as text,
        /// or formula strings (prefix "=").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value_type: Option<String>,
        /// Optional: Excel number format spec (e.g. "#,##0", "0.00%").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
        /// Optional: reference to a style defined in `named_styles`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style_ref: Option<String>,
    },

    /// Bulk write a rectangular region. Rows are outer array, columns inner.
    /// Existing cells in the range are overwritten.
    #[serde(rename = "set_range")]
    SetRange {
        sheet_id: String,
        /// Range in A1 notation (e.g. "A1:C10").
        range: String,
        /// 2D array of values, row-major. Missing/null cells are left untouched.
        values: Vec<Vec<serde_json::Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value_types: Option<Vec<Vec<Option<String>>>>,
    },

    /// Remove all cells in a range. Does NOT delete rows/columns.
    #[serde(rename = "clear_range")]
    ClearRange { sheet_id: String, range: String },

    /// Insert a row, shifting subsequent rows down. `before_row` is 1-indexed.
    #[serde(rename = "insert_row")]
    InsertRow {
        sheet_id: String,
        before_row: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<Vec<serde_json::Value>>,
    },

    /// Delete a row, shifting subsequent rows up. `row_index` is 1-indexed.
    #[serde(rename = "delete_row")]
    DeleteRow { sheet_id: String, row_index: u32 },

    /// Insert a column, shifting subsequent columns right. `before_col` is 0-indexed (A=0).
    #[serde(rename = "insert_column")]
    InsertColumn {
        sheet_id: String,
        before_col: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<Vec<serde_json::Value>>,
    },

    /// Delete a column, shifting subsequent columns left. `col_index` is 0-indexed.
    #[serde(rename = "delete_column")]
    DeleteColumn { sheet_id: String, col_index: u32 },

    /// Create a new sheet. Returns the generated sheet_id in the output.
    #[serde(rename = "add_sheet")]
    AddSheet {
        /// Display name for the sheet.
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at_index: Option<u32>,
    },

    /// Rename an existing sheet. sheet_id is stable.
    #[serde(rename = "rename_sheet")]
    RenameSheet { sheet_id: String, new_name: String },

    /// Delete a sheet and all its cells/tables.
    #[serde(rename = "delete_sheet")]
    DeleteSheet { sheet_id: String },

    /// Reorder sheets. `order` is the full list of sheet IDs in desired order.
    #[serde(rename = "reorder_sheets")]
    ReorderSheets { order: Vec<String> },

    /// Define a named table over a range.
    #[serde(rename = "create_table")]
    CreateTable {
        sheet_id: String,
        range: String,
        name: String,
        #[serde(default)]
        header_row: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style_preset: Option<String>,
    },

    /// Change the extent of a named table.
    #[serde(rename = "resize_table")]
    ResizeTable { table_id: String, new_range: String },

    /// Delete a named table (cells inside range persist).
    #[serde(rename = "delete_table")]
    DeleteTable { table_id: String },

    /// Set the width of a column. `col` is 0-indexed.
    #[serde(rename = "set_column_width")]
    SetColumnWidth { sheet_id: String, col: u32, width: f64 },

    /// Create or update a named style referenced by cells via `style_ref`.
    #[serde(rename = "define_style")]
    DefineStyle {
        style_ref: String,
        definition: serde_json::Value,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_op_serializes_with_tag() {
        let op = PatchOp::SetCell {
            sheet_id: "sheet_01".into(),
            address: "A1".into(),
            value: serde_json::json!("hi"),
            value_type: None,
            format: None,
            style_ref: None,
        };
        let j = serde_json::to_value(&op).unwrap();
        assert_eq!(j["op"], "set_cell");
        assert_eq!(j["sheet_id"], "sheet_01");
    }

    #[test]
    fn patch_op_schema_generates() {
        let schema = schemars::schema_for!(PatchOp);
        let s = serde_json::to_string(&schema).unwrap();
        assert!(s.contains("set_cell"));
        assert!(s.contains("set_range"));
        assert!(s.contains("A1-style address"));
    }

    #[test]
    fn patch_roundtrip() {
        let p = Patch {
            artifact_id: "art_x".into(),
            base_version: "v1".into(),
            source: PatchSource::Agent,
            ops: vec![PatchOp::DeleteSheet { sheet_id: "sheet_01".into() }],
        };
        let j = serde_json::to_string(&p).unwrap();
        let back: Patch = serde_json::from_str(&j).unwrap();
        assert_eq!(back.ops.len(), 1);
    }
}
```

- [ ] **Step 2: Register in mod.rs**

Append to `src/libs/colmena/src/documents/domain/mod.rs`:

```rust
pub mod patch;

pub use patch::{Patch, PatchOp, PatchSource};
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib documents::domain::patch`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/domain/
git commit -m "$(cat <<'EOF'
feat(documents): add Patch envelope and Excel PatchOp enum with JsonSchema

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A6: Domain ports (ArtifactStore, IRRenderer, IRValidator, IdGenerator)

**Files:**
- Create: `src/libs/colmena/src/documents/domain/ports.rs`
- Modify: `src/libs/colmena/src/documents/domain/mod.rs`

- [ ] **Step 1: Write port traits**

Create `src/libs/colmena/src/documents/domain/ports.rs`:

```rust
use super::artifact::{ArtifactMeta, VersionData};
use super::error::{DocumentError, RenderError, StorageError};
use super::ids::{ArtifactId, VersionId};
use async_trait::async_trait;

#[async_trait]
pub trait ArtifactStore: Send + Sync {
    async fn create_artifact(&self, meta: &ArtifactMeta) -> Result<(), StorageError>;

    async fn write_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
        data: &VersionData,
    ) -> Result<(), StorageError>;

    async fn read_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
    ) -> Result<VersionData, StorageError>;

    async fn read_current(&self, id: &ArtifactId) -> Result<VersionData, StorageError>;

    async fn list_versions(&self, id: &ArtifactId) -> Result<Vec<VersionId>, StorageError>;

    async fn set_head(
        &self,
        id: &ArtifactId,
        expected_current: Option<&VersionId>,
        new: &VersionId,
    ) -> Result<(), StorageError>;

    async fn delete_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
    ) -> Result<(), StorageError>;

    async fn read_meta(&self, id: &ArtifactId) -> Result<ArtifactMeta, StorageError>;

    async fn update_meta(&self, id: &ArtifactId, meta: &ArtifactMeta) -> Result<(), StorageError>;

    async fn delete_artifact(&self, id: &ArtifactId) -> Result<(), StorageError>;
}

#[async_trait]
pub trait IRRenderer: Send + Sync {
    async fn render(&self, ir: &serde_json::Value) -> Result<Vec<u8>, RenderError>;
    fn target_extension(&self) -> &'static str;
    fn target_mime(&self) -> &'static str;
}

pub trait IRValidator: Send + Sync {
    fn validate(&self, ir: &serde_json::Value) -> Result<(), DocumentError>;
}

pub trait IdGenerator: Send + Sync {
    fn new_artifact_id(&self) -> String;
    fn new_sheet_id(&self) -> String;
    fn new_table_id(&self) -> String;
    fn new_block_id(&self) -> String;
    fn new_run_id(&self) -> String;
    fn new_row_id(&self) -> String;
    fn new_list_item_id(&self) -> String;
}
```

- [ ] **Step 2: Register in mod.rs**

Append to `src/libs/colmena/src/documents/domain/mod.rs`:

```rust
pub mod ports;

pub use ports::{ArtifactStore, IRRenderer, IRValidator, IdGenerator};
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check --lib`
Expected: success.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/domain/
git commit -m "$(cat <<'EOF'
feat(documents): add domain ports (ArtifactStore, IRRenderer, IRValidator, IdGenerator)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A7: Id generator implementations (ULID + deterministic for tests)

**Files:**
- Create: `src/libs/colmena/src/documents/infrastructure/ids.rs`
- Modify: `src/libs/colmena/src/documents/infrastructure/mod.rs`

- [ ] **Step 1: Write IdGenerator implementations**

Create `src/libs/colmena/src/documents/infrastructure/ids.rs`:

```rust
use crate::documents::domain::IdGenerator;
use std::sync::Mutex;

pub struct UlidIdGenerator;

impl UlidIdGenerator {
    fn short_ulid() -> String {
        let ulid = ulid::Ulid::new().to_string();
        ulid[..12].to_ascii_lowercase()
    }
}

impl IdGenerator for UlidIdGenerator {
    fn new_artifact_id(&self) -> String { format!("art_{}", Self::short_ulid()) }
    fn new_sheet_id(&self) -> String   { format!("sheet_{}", Self::short_ulid()) }
    fn new_table_id(&self) -> String   { format!("tbl_{}", Self::short_ulid()) }
    fn new_block_id(&self) -> String   { format!("blk_{}", Self::short_ulid()) }
    fn new_run_id(&self) -> String     { format!("run_{}", Self::short_ulid()) }
    fn new_row_id(&self) -> String     { format!("row_{}", Self::short_ulid()) }
    fn new_list_item_id(&self) -> String { format!("li_{}", Self::short_ulid()) }
}

/// Deterministic counter-based generator for tests. Each category has its own counter.
pub struct CountingIdGenerator {
    counters: Mutex<[u64; 7]>,
}

impl Default for CountingIdGenerator {
    fn default() -> Self { Self { counters: Mutex::new([0; 7]) } }
}

impl CountingIdGenerator {
    fn next(&self, idx: usize) -> u64 {
        let mut g = self.counters.lock().unwrap();
        g[idx] += 1;
        g[idx]
    }
}

impl IdGenerator for CountingIdGenerator {
    fn new_artifact_id(&self) -> String { format!("art_{:02}", self.next(0)) }
    fn new_sheet_id(&self) -> String   { format!("sheet_{:02}", self.next(1)) }
    fn new_table_id(&self) -> String   { format!("tbl_{:02}", self.next(2)) }
    fn new_block_id(&self) -> String   { format!("blk_{:02}", self.next(3)) }
    fn new_run_id(&self) -> String     { format!("run_{:02}", self.next(4)) }
    fn new_row_id(&self) -> String     { format!("row_{:02}", self.next(5)) }
    fn new_list_item_id(&self) -> String { format!("li_{:02}", self.next(6)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulid_generator_prefixes_correctly() {
        let g = UlidIdGenerator;
        assert!(g.new_artifact_id().starts_with("art_"));
        assert!(g.new_sheet_id().starts_with("sheet_"));
        assert_ne!(g.new_artifact_id(), g.new_artifact_id());
    }

    #[test]
    fn counting_generator_is_deterministic() {
        let g = CountingIdGenerator::default();
        assert_eq!(g.new_artifact_id(), "art_01");
        assert_eq!(g.new_artifact_id(), "art_02");
        assert_eq!(g.new_sheet_id(), "sheet_01");
    }
}
```

- [ ] **Step 2: Register in infrastructure/mod.rs**

Edit `src/libs/colmena/src/documents/infrastructure/mod.rs`:

```rust
pub mod ids;

pub use ids::{CountingIdGenerator, UlidIdGenerator};
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib documents::infrastructure::ids`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/infrastructure/
git commit -m "$(cat <<'EOF'
feat(documents): add ULID and counting ID generator implementations

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A8: Excel IR validator

**Files:**
- Create: `src/libs/colmena/src/documents/infrastructure/validation/mod.rs`
- Create: `src/libs/colmena/src/documents/infrastructure/validation/excel_validator.rs`
- Modify: `src/libs/colmena/src/documents/infrastructure/mod.rs`

- [ ] **Step 1: Write validator with failing test cases**

Create `src/libs/colmena/src/documents/infrastructure/validation/excel_validator.rs`:

```rust
use crate::documents::domain::ir::ExcelIR;
use crate::documents::domain::{DocumentError, IRValidator};
use std::collections::HashSet;

pub struct ExcelValidator;

impl IRValidator for ExcelValidator {
    fn validate(&self, ir_value: &serde_json::Value) -> Result<(), DocumentError> {
        let ir: ExcelIR = serde_json::from_value(ir_value.clone())
            .map_err(|e| DocumentError::IRValidationFailed {
                path: "/".into(),
                reason: format!("not a valid Excel IR: {e}"),
            })?;

        // 1. Unique sheet IDs
        let mut seen_sheet_ids: HashSet<&str> = HashSet::new();
        for (i, sheet) in ir.workbook.sheets.iter().enumerate() {
            if !seen_sheet_ids.insert(&sheet.id) {
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/workbook/sheets/{i}/id"),
                    reason: format!("duplicate sheet ID: {}", sheet.id),
                });
            }
        }

        // 2. Unique table IDs globally
        let mut seen_table_ids: HashSet<&str> = HashSet::new();
        for sheet in &ir.workbook.sheets {
            for (i, t) in sheet.tables.iter().enumerate() {
                if !seen_table_ids.insert(&t.id) {
                    return Err(DocumentError::IRValidationFailed {
                        path: format!("/workbook/sheets/{}/tables/{i}/id", sheet.id),
                        reason: format!("duplicate table ID: {}", t.id),
                    });
                }
            }
        }

        // 3. Cell style_ref exists in named_styles
        for sheet in &ir.workbook.sheets {
            for (addr, cell) in &sheet.cells {
                if let Some(sref) = &cell.style_ref {
                    if !ir.workbook.named_styles.contains_key(sref) {
                        return Err(DocumentError::IRValidationFailed {
                            path: format!("/workbook/sheets/{}/cells/{addr}/style_ref", sheet.id),
                            reason: format!("style_ref '{sref}' not defined in named_styles"),
                        });
                    }
                }
            }
        }

        // 4. Cell type-value consistency (best-effort on JSON values)
        for sheet in &ir.workbook.sheets {
            for (addr, cell) in &sheet.cells {
                if let Some(ct) = &cell.value_type {
                    use crate::documents::domain::ir::CellType;
                    let ok = match ct {
                        CellType::String | CellType::Formula => cell.value.is_string(),
                        CellType::Number => cell.value.is_number(),
                        CellType::Boolean => cell.value.is_boolean(),
                        CellType::Date => cell.value.is_string(),
                    };
                    if !ok {
                        return Err(DocumentError::IRValidationFailed {
                            path: format!("/workbook/sheets/{}/cells/{addr}", sheet.id),
                            reason: format!("value does not match declared type {:?}", ct),
                        });
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ir::{Cell, CellType, ExcelIR, Sheet};
    use std::collections::BTreeMap;

    fn base_ir() -> ExcelIR {
        let mut ir = ExcelIR::empty("art_x", "v1");
        ir.workbook.sheets.push(Sheet {
            id: "sheet_01".into(),
            name: "S".into(),
            order: 0,
            columns: vec![],
            cells: BTreeMap::new(),
            tables: vec![],
        });
        ir
    }

    #[test]
    fn empty_ir_is_valid() {
        let v = ExcelValidator;
        v.validate(&serde_json::to_value(&base_ir()).unwrap()).unwrap();
    }

    #[test]
    fn duplicate_sheet_ids_fail() {
        let mut ir = base_ir();
        ir.workbook.sheets.push(Sheet {
            id: "sheet_01".into(),
            name: "B".into(),
            order: 1,
            columns: vec![],
            cells: BTreeMap::new(),
            tables: vec![],
        });
        let v = ExcelValidator;
        let err = v.validate(&serde_json::to_value(&ir).unwrap()).unwrap_err();
        assert!(matches!(err, DocumentError::IRValidationFailed { .. }));
    }

    #[test]
    fn dangling_style_ref_fails() {
        let mut ir = base_ir();
        let mut cells = BTreeMap::new();
        cells.insert("A1".into(), Cell {
            value: serde_json::json!("hi"),
            value_type: Some(CellType::String),
            format: None,
            style_ref: Some("missing".into()),
        });
        ir.workbook.sheets[0].cells = cells;
        let v = ExcelValidator;
        assert!(v.validate(&serde_json::to_value(&ir).unwrap()).is_err());
    }

    #[test]
    fn type_mismatch_fails() {
        let mut ir = base_ir();
        let mut cells = BTreeMap::new();
        cells.insert("A1".into(), Cell {
            value: serde_json::json!("notanumber"),
            value_type: Some(CellType::Number),
            format: None,
            style_ref: None,
        });
        ir.workbook.sheets[0].cells = cells;
        let v = ExcelValidator;
        assert!(v.validate(&serde_json::to_value(&ir).unwrap()).is_err());
    }
}
```

- [ ] **Step 2: Create validation/mod.rs**

Create `src/libs/colmena/src/documents/infrastructure/validation/mod.rs`:

```rust
pub mod excel_validator;

pub use excel_validator::ExcelValidator;
```

- [ ] **Step 3: Register in infrastructure/mod.rs**

Append to `src/libs/colmena/src/documents/infrastructure/mod.rs`:

```rust
pub mod validation;
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib documents::infrastructure::validation::excel_validator`
Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/infrastructure/
git commit -m "$(cat <<'EOF'
feat(documents): add Excel IR validator (unique IDs, style refs, type consistency)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A9: Excel renderer via rust_xlsxwriter

**Files:**
- Create: `src/libs/colmena/src/documents/infrastructure/render/mod.rs`
- Create: `src/libs/colmena/src/documents/infrastructure/render/excel_renderer.rs`
- Modify: `src/libs/colmena/src/documents/infrastructure/mod.rs`

- [ ] **Step 1: Implement renderer**

Create `src/libs/colmena/src/documents/infrastructure/render/excel_renderer.rs`:

```rust
use crate::documents::domain::ir::{CellType, ExcelIR};
use crate::documents::domain::{IRRenderer, RenderError};
use async_trait::async_trait;
use rust_xlsxwriter::{Format, Workbook};

pub struct ExcelRenderer;

impl ExcelRenderer {
    fn render_sync(ir: &ExcelIR) -> Result<Vec<u8>, RenderError> {
        let mut wb = Workbook::new();
        let mut sorted = ir.workbook.sheets.clone();
        sorted.sort_by_key(|s| s.order);

        for sheet in &sorted {
            let ws = wb.add_worksheet();
            ws.set_name(&sheet.name)
                .map_err(|e| RenderError::Failed(format!("set_name: {e}")))?;

            for col in &sheet.columns {
                ws.set_column_width(col.index as u16, col.width)
                    .map_err(|e| RenderError::Failed(format!("set_column_width: {e}")))?;
            }

            for (addr, cell) in &sheet.cells {
                let (row, col) = parse_a1(addr)
                    .ok_or_else(|| RenderError::Failed(format!("invalid address: {addr}")))?;

                let mut fmt = Format::new();
                if let Some(f) = &cell.format {
                    fmt = fmt.set_num_format(f);
                }
                if let Some(sref) = &cell.style_ref {
                    if let Some(style) = ir.workbook.named_styles.get(sref) {
                        if let Some(font) = &style.font {
                            if font.bold.unwrap_or(false) { fmt = fmt.set_bold(); }
                            if font.italic.unwrap_or(false) { fmt = fmt.set_italic(); }
                            if let Some(sz) = font.size { fmt = fmt.set_font_size(sz); }
                            if let Some(color) = &font.color {
                                if let Ok(c) = u32::from_str_radix(color.trim_start_matches('#'), 16) {
                                    fmt = fmt.set_font_color(rust_xlsxwriter::Color::RGB(c));
                                }
                            }
                        }
                        if let Some(fill) = &style.fill {
                            if let Ok(c) = u32::from_str_radix(fill.trim_start_matches('#'), 16) {
                                fmt = fmt.set_background_color(rust_xlsxwriter::Color::RGB(c));
                            }
                        }
                    }
                }

                let vt = cell.value_type.clone().unwrap_or_else(|| infer_type(&cell.value));
                write_cell(ws, row, col, &cell.value, vt, &fmt)?;
            }

            for table in &sheet.tables {
                let (first, last) = parse_range(&table.range)
                    .ok_or_else(|| RenderError::Failed(format!("invalid range: {}", table.range)))?;
                let mut t = rust_xlsxwriter::Table::new().set_name(&table.name);
                if !table.header_row { t = t.set_header_row(false); }
                ws.add_table(first.0, first.1, last.0, last.1, &t)
                    .map_err(|e| RenderError::Failed(format!("add_table: {e}")))?;
            }
        }

        wb.save_to_buffer()
            .map_err(|e| RenderError::Failed(format!("save_to_buffer: {e}")))
    }
}

fn infer_type(value: &serde_json::Value) -> CellType {
    if value.is_number() { CellType::Number }
    else if value.is_boolean() { CellType::Boolean }
    else if value.as_str().map(|s| s.starts_with('=')).unwrap_or(false) { CellType::Formula }
    else { CellType::String }
}

fn write_cell(
    ws: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    col: u16,
    value: &serde_json::Value,
    ct: CellType,
    fmt: &Format,
) -> Result<(), RenderError> {
    let err = |e: rust_xlsxwriter::XlsxError| RenderError::Failed(format!("write: {e}"));
    match ct {
        CellType::String => {
            let s = value.as_str().map(|s| s.to_string()).unwrap_or_else(|| value.to_string());
            ws.write_string_with_format(row, col, &s, fmt).map_err(err)?;
        }
        CellType::Number => {
            let n = value.as_f64().unwrap_or(0.0);
            ws.write_number_with_format(row, col, n, fmt).map_err(err)?;
        }
        CellType::Boolean => {
            let b = value.as_bool().unwrap_or(false);
            ws.write_boolean_with_format(row, col, b, fmt).map_err(err)?;
        }
        CellType::Date => {
            let s = value.as_str().map(|s| s.to_string()).unwrap_or_default();
            ws.write_string_with_format(row, col, &s, fmt).map_err(err)?;
        }
        CellType::Formula => {
            let s = value.as_str().map(|s| s.to_string()).unwrap_or_default();
            let formula = rust_xlsxwriter::Formula::new(&s);
            ws.write_formula_with_format(row, col, formula, fmt).map_err(err)?;
        }
    }
    Ok(())
}

fn parse_a1(addr: &str) -> Option<(u32, u16)> {
    let addr = addr.to_ascii_uppercase();
    let split = addr.find(|c: char| c.is_ascii_digit())?;
    let (col_str, row_str) = addr.split_at(split);
    let row: u32 = row_str.parse().ok()?;
    if row == 0 { return None; }
    let mut col: u32 = 0;
    for c in col_str.chars() {
        if !c.is_ascii_alphabetic() { return None; }
        col = col * 26 + (c as u32 - 'A' as u32 + 1);
    }
    Some((row - 1, (col - 1) as u16))
}

fn parse_range(range: &str) -> Option<((u32, u16), (u32, u16))> {
    let (a, b) = range.split_once(':')?;
    Some((parse_a1(a)?, parse_a1(b)?))
}

#[async_trait]
impl IRRenderer for ExcelRenderer {
    async fn render(&self, ir: &serde_json::Value) -> Result<Vec<u8>, RenderError> {
        let ir: ExcelIR = serde_json::from_value(ir.clone())
            .map_err(|e| RenderError::Failed(format!("parse IR: {e}")))?;
        Self::render_sync(&ir)
    }
    fn target_extension(&self) -> &'static str { "xlsx" }
    fn target_mime(&self) -> &'static str {
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ir::{Cell, Sheet};
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn renders_minimal_xlsx() {
        let mut ir = ExcelIR::empty("art_x", "v1");
        let mut cells = BTreeMap::new();
        cells.insert("A1".into(), Cell {
            value: serde_json::json!("Hello"),
            value_type: None, format: None, style_ref: None,
        });
        cells.insert("B1".into(), Cell {
            value: serde_json::json!(42),
            value_type: None, format: None, style_ref: None,
        });
        ir.workbook.sheets.push(Sheet {
            id: "s1".into(), name: "Sheet1".into(), order: 0,
            columns: vec![], cells, tables: vec![],
        });

        let r = ExcelRenderer;
        let bytes = r.render(&serde_json::to_value(&ir).unwrap()).await.unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[..2], b"PK");
    }

    #[test]
    fn parses_a1() {
        assert_eq!(parse_a1("A1"), Some((0, 0)));
        assert_eq!(parse_a1("B5"), Some((4, 1)));
        assert_eq!(parse_a1("AA1"), Some((0, 26)));
        assert_eq!(parse_a1("0"), None);
    }
}
```

- [ ] **Step 2: Create render/mod.rs**

Create `src/libs/colmena/src/documents/infrastructure/render/mod.rs`:

```rust
pub mod excel_renderer;

pub use excel_renderer::ExcelRenderer;
```

- [ ] **Step 3: Register in infrastructure/mod.rs**

Append to `src/libs/colmena/src/documents/infrastructure/mod.rs`:

```rust
pub mod render;
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib documents::infrastructure::render::excel_renderer`
Expected: 2 tests pass.

- [ ] **Step 5: Round-trip smoke test with calamine**

Add to the test module:

```rust
    #[tokio::test]
    async fn calamine_can_reopen_rendered_xlsx() {
        use calamine::{Reader, Xlsx};
        use std::io::Cursor;

        let mut ir = ExcelIR::empty("art_x", "v1");
        let mut cells = BTreeMap::new();
        cells.insert("A1".into(), Cell {
            value: serde_json::json!("Producto"), value_type: None, format: None, style_ref: None,
        });
        cells.insert("A2".into(), Cell {
            value: serde_json::json!("Widget"), value_type: None, format: None, style_ref: None,
        });
        ir.workbook.sheets.push(Sheet {
            id: "s1".into(), name: "Ventas".into(), order: 0,
            columns: vec![], cells, tables: vec![],
        });

        let bytes = ExcelRenderer.render(&serde_json::to_value(&ir).unwrap()).await.unwrap();
        let mut xl: Xlsx<_> = calamine::open_workbook_from_rs(Cursor::new(bytes)).unwrap();
        let range = xl.worksheet_range("Ventas").unwrap();
        assert_eq!(range.get((0, 0)).unwrap().to_string(), "Producto");
        assert_eq!(range.get((1, 0)).unwrap().to_string(), "Widget");
    }
```

Run: `cargo test --lib documents::infrastructure::render::excel_renderer`
Expected: 3 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/documents/infrastructure/
git commit -m "$(cat <<'EOF'
feat(documents): add Excel renderer via rust_xlsxwriter with calamine round-trip test

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A10: LocalFsStore adapter

**Files:**
- Create: `src/libs/colmena/src/documents/infrastructure/storage/mod.rs`
- Create: `src/libs/colmena/src/documents/infrastructure/storage/local_fs_store.rs`
- Modify: `src/libs/colmena/src/documents/infrastructure/mod.rs`

- [ ] **Step 1: Implement LocalFsStore**

Create `src/libs/colmena/src/documents/infrastructure/storage/local_fs_store.rs`:

```rust
use crate::documents::domain::artifact::{ArtifactMeta, PatchApplied, VersionData};
use crate::documents::domain::ids::{ArtifactId, VersionId};
use crate::documents::domain::ports::ArtifactStore;
use crate::documents::domain::StorageError;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::fs;

pub struct LocalFsStore {
    root: PathBuf,
}

impl LocalFsStore {
    pub fn new(root: impl Into<PathBuf>) -> Self { Self { root: root.into() } }

    fn art_dir(&self, id: &ArtifactId) -> PathBuf {
        self.root.join("artifacts").join(&id.0)
    }
    fn meta_path(&self, id: &ArtifactId) -> PathBuf { self.art_dir(id).join("meta.json") }
    fn head_path(&self, id: &ArtifactId) -> PathBuf { self.art_dir(id).join("HEAD") }
    fn version_dir(&self, id: &ArtifactId, v: &VersionId) -> PathBuf {
        self.art_dir(id).join("versions").join(&v.0)
    }

    async fn atomic_write(path: &Path, data: &[u8]) -> Result<(), StorageError> {
        let parent = path.parent()
            .ok_or_else(|| StorageError::Backend(format!("no parent: {}", path.display())))?;
        fs::create_dir_all(parent).await
            .map_err(|e| StorageError::Backend(format!("mkdir: {e}")))?;
        let tmp = path.with_extension(format!(
            "{}.tmp",
            path.extension().and_then(|e| e.to_str()).unwrap_or("part")
        ));
        fs::write(&tmp, data).await
            .map_err(|e| StorageError::Backend(format!("write tmp: {e}")))?;
        fs::rename(&tmp, path).await
            .map_err(|e| StorageError::Backend(format!("rename: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl ArtifactStore for LocalFsStore {
    async fn create_artifact(&self, meta: &ArtifactMeta) -> Result<(), StorageError> {
        fs::create_dir_all(self.art_dir(&meta.artifact_id)).await
            .map_err(|e| StorageError::Backend(format!("mkdir: {e}")))?;
        let bytes = serde_json::to_vec_pretty(meta)
            .map_err(|e| StorageError::Backend(format!("ser meta: {e}")))?;
        Self::atomic_write(&self.meta_path(&meta.artifact_id), &bytes).await?;
        Ok(())
    }

    async fn write_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
        data: &VersionData,
    ) -> Result<(), StorageError> {
        let vdir = self.version_dir(id, version);
        fs::create_dir_all(&vdir).await
            .map_err(|e| StorageError::Backend(format!("mkdir: {e}")))?;
        let ir_bytes = serde_json::to_vec_pretty(&data.ir)
            .map_err(|e| StorageError::Backend(format!("ser ir: {e}")))?;
        Self::atomic_write(&vdir.join("ir.json"), &ir_bytes).await?;
        let render_name = format!("render.{}", data.rendered_extension);
        Self::atomic_write(&vdir.join(&render_name), &data.rendered_binary).await?;
        let patch_bytes = serde_json::to_vec_pretty(&data.patch_applied)
            .map_err(|e| StorageError::Backend(format!("ser patch: {e}")))?;
        Self::atomic_write(&vdir.join("patch_applied.json"), &patch_bytes).await?;
        if !data.blobs.is_empty() {
            let blobs_dir = vdir.join("blobs");
            fs::create_dir_all(&blobs_dir).await
                .map_err(|e| StorageError::Backend(format!("mkdir blobs: {e}")))?;
            for (name, bytes) in &data.blobs {
                Self::atomic_write(&blobs_dir.join(name), bytes).await?;
            }
        }
        Ok(())
    }

    async fn read_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
    ) -> Result<VersionData, StorageError> {
        let vdir = self.version_dir(id, version);
        if !vdir.exists() {
            return Err(StorageError::NotFound(format!("{}/{}", id.0, version.0)));
        }
        let ir_bytes = fs::read(vdir.join("ir.json")).await
            .map_err(|e| StorageError::Backend(format!("read ir: {e}")))?;
        let ir: serde_json::Value = serde_json::from_slice(&ir_bytes)
            .map_err(|e| StorageError::Backend(format!("parse ir: {e}")))?;

        let meta = self.read_meta(id).await?;
        let ext = meta.kind.extension();
        let render = fs::read(vdir.join(format!("render.{ext}"))).await
            .map_err(|e| StorageError::Backend(format!("read render: {e}")))?;

        let pa_bytes = fs::read(vdir.join("patch_applied.json")).await
            .map_err(|e| StorageError::Backend(format!("read patch: {e}")))?;
        let patch_applied: PatchApplied = serde_json::from_slice(&pa_bytes)
            .map_err(|e| StorageError::Backend(format!("parse patch: {e}")))?;

        Ok(VersionData {
            ir,
            rendered_binary: render,
            rendered_extension: match meta.kind {
                crate::documents::domain::ArtifactKind::Excel => "xlsx",
                crate::documents::domain::ArtifactKind::Word => "docx",
            },
            patch_applied,
            blobs: Vec::new(),
        })
    }

    async fn read_current(&self, id: &ArtifactId) -> Result<VersionData, StorageError> {
        let head = fs::read_to_string(self.head_path(id)).await
            .map_err(|e| StorageError::Backend(format!("read HEAD: {e}")))?;
        let v = VersionId::new(head.trim());
        self.read_version(id, &v).await
    }

    async fn list_versions(&self, id: &ArtifactId) -> Result<Vec<VersionId>, StorageError> {
        let vers_dir = self.art_dir(id).join("versions");
        if !vers_dir.exists() { return Ok(vec![]); }
        let mut rd = fs::read_dir(&vers_dir).await
            .map_err(|e| StorageError::Backend(format!("readdir: {e}")))?;
        let mut out = Vec::new();
        while let Some(entry) = rd.next_entry().await
            .map_err(|e| StorageError::Backend(format!("readdir entry: {e}")))?
        {
            if let Some(name) = entry.file_name().to_str() {
                out.push(VersionId::new(name));
            }
        }
        out.sort_by_key(|v| v.number().unwrap_or(0));
        Ok(out)
    }

    async fn set_head(
        &self,
        id: &ArtifactId,
        expected_current: Option<&VersionId>,
        new: &VersionId,
    ) -> Result<(), StorageError> {
        let head_path = self.head_path(id);
        if let Some(expected) = expected_current {
            let cur = fs::read_to_string(&head_path).await.unwrap_or_default();
            if cur.trim() != expected.0 {
                return Err(StorageError::PreconditionFailed(format!(
                    "HEAD is {}, expected {}", cur.trim(), expected.0
                )));
            }
        }
        Self::atomic_write(&head_path, new.0.as_bytes()).await?;
        Ok(())
    }

    async fn delete_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
    ) -> Result<(), StorageError> {
        let vdir = self.version_dir(id, version);
        if vdir.exists() {
            fs::remove_dir_all(&vdir).await
                .map_err(|e| StorageError::Backend(format!("rmdir: {e}")))?;
        }
        Ok(())
    }

    async fn read_meta(&self, id: &ArtifactId) -> Result<ArtifactMeta, StorageError> {
        let path = self.meta_path(id);
        if !path.exists() { return Err(StorageError::NotFound(id.0.clone())); }
        let bytes = fs::read(&path).await
            .map_err(|e| StorageError::Backend(format!("read meta: {e}")))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| StorageError::Backend(format!("parse meta: {e}")))
    }

    async fn update_meta(&self, id: &ArtifactId, meta: &ArtifactMeta) -> Result<(), StorageError> {
        let bytes = serde_json::to_vec_pretty(meta)
            .map_err(|e| StorageError::Backend(format!("ser meta: {e}")))?;
        Self::atomic_write(&self.meta_path(id), &bytes).await?;
        Ok(())
    }

    async fn delete_artifact(&self, id: &ArtifactId) -> Result<(), StorageError> {
        let dir = self.art_dir(id);
        if dir.exists() {
            fs::remove_dir_all(&dir).await
                .map_err(|e| StorageError::Backend(format!("rmdir art: {e}")))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::artifact::PatchSummary;
    use crate::documents::domain::ArtifactKind;
    use crate::documents::domain::ids::SessionId;
    use tempfile::tempdir;

    fn sample_version_data() -> VersionData {
        VersionData {
            ir: serde_json::json!({"kind": "excel"}),
            rendered_binary: vec![1, 2, 3],
            rendered_extension: "xlsx",
            patch_applied: PatchApplied {
                patch: serde_json::json!({}),
                applied_at: chrono::Utc::now(),
                resulted_in: VersionId::initial(),
                summary: PatchSummary::default(),
            },
            blobs: vec![],
        }
    }

    #[tokio::test]
    async fn create_write_read_cycle() {
        let tmp = tempdir().unwrap();
        let s = LocalFsStore::new(tmp.path());
        let meta = ArtifactMeta::initial(
            ArtifactId::new("art_01"),
            ArtifactKind::Excel,
            SessionId::new("sess_1"),
            "t".into(),
            10,
        );
        s.create_artifact(&meta).await.unwrap();
        s.write_version(&meta.artifact_id, &VersionId::initial(), &sample_version_data()).await.unwrap();
        s.set_head(&meta.artifact_id, None, &VersionId::initial()).await.unwrap();

        let current = s.read_current(&meta.artifact_id).await.unwrap();
        assert_eq!(current.rendered_binary, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn set_head_precondition_mismatch_fails() {
        let tmp = tempdir().unwrap();
        let s = LocalFsStore::new(tmp.path());
        let id = ArtifactId::new("art_01");
        let meta = ArtifactMeta::initial(id.clone(), ArtifactKind::Excel, SessionId::new("s"), "t".into(), 5);
        s.create_artifact(&meta).await.unwrap();
        s.set_head(&id, None, &VersionId::new("v1")).await.unwrap();
        let err = s.set_head(&id, Some(&VersionId::new("v999")), &VersionId::new("v2")).await;
        assert!(matches!(err, Err(StorageError::PreconditionFailed(_))));
    }

    #[tokio::test]
    async fn list_versions_sorted() {
        let tmp = tempdir().unwrap();
        let s = LocalFsStore::new(tmp.path());
        let id = ArtifactId::new("art_01");
        let meta = ArtifactMeta::initial(id.clone(), ArtifactKind::Excel, SessionId::new("s"), "t".into(), 5);
        s.create_artifact(&meta).await.unwrap();
        for v in &["v1", "v2", "v10"] {
            s.write_version(&id, &VersionId::new(*v), &sample_version_data()).await.unwrap();
        }
        let list = s.list_versions(&id).await.unwrap();
        assert_eq!(list.iter().map(|v| v.0.clone()).collect::<Vec<_>>(), vec!["v1", "v2", "v10"]);
    }
}
```

- [ ] **Step 2: Create storage/mod.rs**

Create `src/libs/colmena/src/documents/infrastructure/storage/mod.rs`:

```rust
pub mod local_fs_store;

pub use local_fs_store::LocalFsStore;
```

- [ ] **Step 3: Register in infrastructure/mod.rs**

Append to `src/libs/colmena/src/documents/infrastructure/mod.rs`:

```rust
pub mod storage;
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib documents::infrastructure::storage::local_fs_store`
Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/infrastructure/
git commit -m "$(cat <<'EOF'
feat(documents): add LocalFsStore adapter with atomic writes and HEAD CAS

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A11: Excel patch applier (sequential, no rebase)

**Files:**
- Create: `src/libs/colmena/src/documents/application/mod.rs`
- Create: `src/libs/colmena/src/documents/application/apply_excel_ops.rs`

- [ ] **Step 1: Implement op application**

Create `src/libs/colmena/src/documents/application/apply_excel_ops.rs`:

```rust
use crate::documents::domain::ir::{Cell, CellType, ExcelIR, NamedStyle, NamedTable, Sheet};
use crate::documents::domain::patch::PatchOp;
use crate::documents::domain::{DocumentError, IdGenerator};
use std::collections::BTreeMap;

pub struct ExcelOpApplier<'a> {
    pub ids: &'a dyn IdGenerator,
}

impl<'a> ExcelOpApplier<'a> {
    pub fn apply(&self, ir: &mut ExcelIR, op: &PatchOp) -> Result<(), DocumentError> {
        match op {
            PatchOp::SetCell { sheet_id, address, value, value_type, format, style_ref } => {
                let sheet = ir.sheet_mut(sheet_id)
                    .ok_or_else(|| DocumentError::InvalidPatchOp {
                        reason: format!("sheet not found: {sheet_id}"),
                        op: serde_json::to_value(op).unwrap(),
                    })?;
                let ct = value_type.as_deref().and_then(parse_cell_type);
                sheet.cells.insert(address.to_ascii_uppercase(), Cell {
                    value: value.clone(),
                    value_type: ct,
                    format: format.clone(),
                    style_ref: style_ref.clone(),
                });
            }
            PatchOp::SetRange { sheet_id, range, values, value_types } => {
                let sheet = ir.sheet_mut(sheet_id).ok_or_else(|| invalid(op, "sheet not found"))?;
                let (first, _last) = parse_range(range).ok_or_else(|| invalid(op, "invalid range"))?;
                for (r, row) in values.iter().enumerate() {
                    for (c, v) in row.iter().enumerate() {
                        if v.is_null() { continue; }
                        let addr = to_a1(first.0 + r as u32, first.1 + c as u16);
                        let ct = value_types.as_ref()
                            .and_then(|m| m.get(r))
                            .and_then(|row| row.get(c))
                            .and_then(|o| o.as_deref())
                            .and_then(parse_cell_type);
                        sheet.cells.insert(addr, Cell {
                            value: v.clone(), value_type: ct, format: None, style_ref: None,
                        });
                    }
                }
            }
            PatchOp::ClearRange { sheet_id, range } => {
                let sheet = ir.sheet_mut(sheet_id).ok_or_else(|| invalid(op, "sheet not found"))?;
                let (first, last) = parse_range(range).ok_or_else(|| invalid(op, "invalid range"))?;
                let addrs: Vec<String> = sheet.cells.keys()
                    .filter(|a| in_range(a, first, last))
                    .cloned().collect();
                for a in addrs { sheet.cells.remove(&a); }
            }
            PatchOp::InsertRow { sheet_id, before_row, values } => {
                let sheet = ir.sheet_mut(sheet_id).ok_or_else(|| invalid(op, "sheet not found"))?;
                shift_rows(&mut sheet.cells, *before_row, 1);
                if let Some(vs) = values {
                    for (c, v) in vs.iter().enumerate() {
                        if v.is_null() { continue; }
                        let addr = to_a1(*before_row - 1, c as u16);
                        sheet.cells.insert(addr, Cell {
                            value: v.clone(), value_type: None, format: None, style_ref: None,
                        });
                    }
                }
            }
            PatchOp::DeleteRow { sheet_id, row_index } => {
                let sheet = ir.sheet_mut(sheet_id).ok_or_else(|| invalid(op, "sheet not found"))?;
                let to_remove: Vec<String> = sheet.cells.keys()
                    .filter(|a| cell_row(a).map(|r| r == *row_index).unwrap_or(false))
                    .cloned().collect();
                for a in to_remove { sheet.cells.remove(&a); }
                shift_rows(&mut sheet.cells, *row_index + 1, -1);
            }
            PatchOp::InsertColumn { sheet_id, before_col, values } => {
                let sheet = ir.sheet_mut(sheet_id).ok_or_else(|| invalid(op, "sheet not found"))?;
                shift_cols(&mut sheet.cells, *before_col, 1);
                if let Some(vs) = values {
                    for (r, v) in vs.iter().enumerate() {
                        if v.is_null() { continue; }
                        let addr = to_a1(r as u32, *before_col as u16);
                        sheet.cells.insert(addr, Cell {
                            value: v.clone(), value_type: None, format: None, style_ref: None,
                        });
                    }
                }
            }
            PatchOp::DeleteColumn { sheet_id, col_index } => {
                let sheet = ir.sheet_mut(sheet_id).ok_or_else(|| invalid(op, "sheet not found"))?;
                let to_remove: Vec<String> = sheet.cells.keys()
                    .filter(|a| cell_col(a).map(|c| c == *col_index as u16).unwrap_or(false))
                    .cloned().collect();
                for a in to_remove { sheet.cells.remove(&a); }
                shift_cols(&mut sheet.cells, *col_index + 1, -1);
            }
            PatchOp::AddSheet { name, at_index } => {
                let id = self.ids.new_sheet_id();
                let order = at_index.unwrap_or(ir.workbook.sheets.len() as u32);
                for s in ir.workbook.sheets.iter_mut() {
                    if s.order >= order { s.order += 1; }
                }
                ir.workbook.sheets.push(Sheet {
                    id, name: name.clone(), order,
                    columns: vec![], cells: BTreeMap::new(), tables: vec![],
                });
            }
            PatchOp::RenameSheet { sheet_id, new_name } => {
                let sheet = ir.sheet_mut(sheet_id).ok_or_else(|| invalid(op, "sheet not found"))?;
                sheet.name = new_name.clone();
            }
            PatchOp::DeleteSheet { sheet_id } => {
                let Some(pos) = ir.workbook.sheets.iter().position(|s| &s.id == sheet_id) else {
                    return Err(invalid(op, "sheet not found"));
                };
                let removed_order = ir.workbook.sheets[pos].order;
                ir.workbook.sheets.remove(pos);
                for s in ir.workbook.sheets.iter_mut() {
                    if s.order > removed_order { s.order -= 1; }
                }
            }
            PatchOp::ReorderSheets { order } => {
                for (i, sid) in order.iter().enumerate() {
                    if let Some(s) = ir.sheet_mut(sid) { s.order = i as u32; }
                }
            }
            PatchOp::CreateTable { sheet_id, range, name, header_row, style_preset } => {
                let id = self.ids.new_table_id();
                let sheet = ir.sheet_mut(sheet_id).ok_or_else(|| invalid(op, "sheet not found"))?;
                sheet.tables.push(NamedTable {
                    id, name: name.clone(), range: range.clone(),
                    header_row: *header_row, style_preset: style_preset.clone(),
                });
            }
            PatchOp::ResizeTable { table_id, new_range } => {
                for sheet in ir.workbook.sheets.iter_mut() {
                    if let Some(t) = sheet.tables.iter_mut().find(|t| &t.id == table_id) {
                        t.range = new_range.clone();
                        return Ok(());
                    }
                }
                return Err(invalid(op, "table not found"));
            }
            PatchOp::DeleteTable { table_id } => {
                for sheet in ir.workbook.sheets.iter_mut() {
                    sheet.tables.retain(|t| &t.id != table_id);
                }
            }
            PatchOp::SetColumnWidth { sheet_id, col, width } => {
                let sheet = ir.sheet_mut(sheet_id).ok_or_else(|| invalid(op, "sheet not found"))?;
                if let Some(c) = sheet.columns.iter_mut().find(|c| c.index == *col) {
                    c.width = *width;
                } else {
                    sheet.columns.push(crate::documents::domain::ir::ColumnSpec {
                        index: *col, width: *width,
                    });
                }
            }
            PatchOp::DefineStyle { style_ref, definition } => {
                let style: NamedStyle = serde_json::from_value(definition.clone())
                    .map_err(|e| invalid(op, &format!("bad style: {e}")))?;
                ir.workbook.named_styles.insert(style_ref.clone(), style);
            }
        }
        Ok(())
    }
}

fn invalid(op: &PatchOp, reason: &str) -> DocumentError {
    DocumentError::InvalidPatchOp {
        reason: reason.to_string(),
        op: serde_json::to_value(op).unwrap_or(serde_json::Value::Null),
    }
}

fn parse_cell_type(s: &str) -> Option<CellType> {
    match s {
        "string" => Some(CellType::String),
        "number" => Some(CellType::Number),
        "boolean" => Some(CellType::Boolean),
        "date" => Some(CellType::Date),
        "formula" => Some(CellType::Formula),
        _ => None,
    }
}

fn parse_a1(addr: &str) -> Option<(u32, u16)> {
    let addr = addr.to_ascii_uppercase();
    let split = addr.find(|c: char| c.is_ascii_digit())?;
    let (col_str, row_str) = addr.split_at(split);
    let row: u32 = row_str.parse().ok()?;
    if row == 0 { return None; }
    let mut col: u32 = 0;
    for c in col_str.chars() {
        if !c.is_ascii_alphabetic() { return None; }
        col = col * 26 + (c as u32 - 'A' as u32 + 1);
    }
    Some((row - 1, (col - 1) as u16))
}

fn parse_range(r: &str) -> Option<((u32, u16), (u32, u16))> {
    let (a, b) = r.split_once(':')?;
    Some((parse_a1(a)?, parse_a1(b)?))
}

fn to_a1(row: u32, col: u16) -> String {
    let mut n = col as u32 + 1;
    let mut s = String::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        s.insert(0, (b'A' + rem) as char);
        n = (n - 1) / 26;
    }
    format!("{}{}", s, row + 1)
}

fn cell_row(addr: &str) -> Option<u32> { parse_a1(addr).map(|(r, _)| r + 1) }
fn cell_col(addr: &str) -> Option<u16> { parse_a1(addr).map(|(_, c)| c) }

fn in_range(addr: &str, first: (u32, u16), last: (u32, u16)) -> bool {
    let Some((r, c)) = parse_a1(addr) else { return false; };
    r >= first.0 && r <= last.0 && c >= first.1 && c <= last.1
}

fn shift_rows(cells: &mut BTreeMap<String, Cell>, from_row: u32, delta: i32) {
    let mut moves: Vec<(String, String, Cell)> = Vec::new();
    for (addr, cell) in cells.iter() {
        if let Some((r, c)) = parse_a1(addr) {
            if r + 1 >= from_row {
                let new_r = (r as i32 + delta) as u32;
                let new_addr = to_a1(new_r, c);
                moves.push((addr.clone(), new_addr, cell.clone()));
            }
        }
    }
    for (old, _, _) in &moves { cells.remove(old); }
    for (_, new, cell) in moves { cells.insert(new, cell); }
}

fn shift_cols(cells: &mut BTreeMap<String, Cell>, from_col: u32, delta: i32) {
    let mut moves: Vec<(String, String, Cell)> = Vec::new();
    for (addr, cell) in cells.iter() {
        if let Some((r, c)) = parse_a1(addr) {
            if c as u32 >= from_col {
                let new_c = (c as i32 + delta) as u16;
                let new_addr = to_a1(r, new_c);
                moves.push((addr.clone(), new_addr, cell.clone()));
            }
        }
    }
    for (old, _, _) in &moves { cells.remove(old); }
    for (_, new, cell) in moves { cells.insert(new, cell); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::infrastructure::ids::CountingIdGenerator;

    fn sheet_ir() -> ExcelIR {
        let mut ir = ExcelIR::empty("art_x", "v1");
        ir.workbook.sheets.push(Sheet {
            id: "s1".into(), name: "Hoja1".into(), order: 0,
            columns: vec![], cells: BTreeMap::new(), tables: vec![],
        });
        ir
    }

    #[test]
    fn set_cell_inserts() {
        let ids = CountingIdGenerator::default();
        let applier = ExcelOpApplier { ids: &ids };
        let mut ir = sheet_ir();
        applier.apply(&mut ir, &PatchOp::SetCell {
            sheet_id: "s1".into(), address: "B5".into(),
            value: serde_json::json!(42), value_type: None, format: None, style_ref: None,
        }).unwrap();
        assert_eq!(ir.workbook.sheets[0].cells["B5"].value, serde_json::json!(42));
    }

    #[test]
    fn insert_row_shifts_cells_down() {
        let ids = CountingIdGenerator::default();
        let applier = ExcelOpApplier { ids: &ids };
        let mut ir = sheet_ir();
        ir.workbook.sheets[0].cells.insert("A1".into(), Cell {
            value: serde_json::json!("top"), value_type: None, format: None, style_ref: None,
        });
        ir.workbook.sheets[0].cells.insert("A5".into(), Cell {
            value: serde_json::json!("fifth"), value_type: None, format: None, style_ref: None,
        });
        applier.apply(&mut ir, &PatchOp::InsertRow {
            sheet_id: "s1".into(), before_row: 3, values: None,
        }).unwrap();
        assert!(ir.workbook.sheets[0].cells.contains_key("A1"));
        assert!(ir.workbook.sheets[0].cells.contains_key("A6"));
        assert!(!ir.workbook.sheets[0].cells.contains_key("A5"));
    }

    #[test]
    fn add_sheet_allocates_id() {
        let ids = CountingIdGenerator::default();
        let applier = ExcelOpApplier { ids: &ids };
        let mut ir = sheet_ir();
        applier.apply(&mut ir, &PatchOp::AddSheet {
            name: "Costos".into(), at_index: None,
        }).unwrap();
        assert_eq!(ir.workbook.sheets.len(), 2);
        assert_eq!(ir.workbook.sheets[1].id, "sheet_01");
    }

    #[test]
    fn to_a1_roundtrip() {
        assert_eq!(to_a1(0, 0), "A1");
        assert_eq!(to_a1(4, 1), "B5");
        assert_eq!(to_a1(0, 26), "AA1");
    }
}
```

- [ ] **Step 2: Create application/mod.rs**

Create `src/libs/colmena/src/documents/application/mod.rs`:

```rust
pub mod apply_excel_ops;
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib documents::application::apply_excel_ops`
Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/application/
git commit -m "$(cat <<'EOF'
feat(documents): add Excel PatchOp applier with shift logic for row/col insertions

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A12: CreateDocumentUseCase

**Files:**
- Create: `src/libs/colmena/src/documents/application/create_document.rs`
- Modify: `src/libs/colmena/src/documents/application/mod.rs`

- [ ] **Step 1: Write use case**

Create `src/libs/colmena/src/documents/application/create_document.rs`:

```rust
use crate::documents::domain::artifact::{ArtifactMeta, PatchApplied, PatchSummary, VersionData};
use crate::documents::domain::ids::{ArtifactId, ArtifactKind, SessionId, VersionId};
use crate::documents::domain::patch::PatchSource;
use crate::documents::domain::{ArtifactStore, DocumentError, IRRenderer, IRValidator, IdGenerator};
use chrono::Utc;
use std::sync::Arc;

pub struct CreateDocumentInput {
    pub kind: ArtifactKind,
    pub session_id: SessionId,
    pub label: Option<String>,
    pub retention_limit: Option<u32>,
    pub initial_ir: Option<serde_json::Value>,
    pub source: PatchSource,
}

pub struct CreateDocumentOutput {
    pub artifact_id: ArtifactId,
    pub version_id: VersionId,
    pub label: String,
    pub meta: ArtifactMeta,
}

pub struct CreateDocumentUseCase {
    pub store: Arc<dyn ArtifactStore>,
    pub excel_renderer: Arc<dyn IRRenderer>,
    pub excel_validator: Arc<dyn IRValidator>,
    pub word_renderer: Arc<dyn IRRenderer>,
    pub word_validator: Arc<dyn IRValidator>,
    pub ids: Arc<dyn IdGenerator>,
    pub default_retention: u32,
}

impl CreateDocumentUseCase {
    pub async fn execute(&self, input: CreateDocumentInput) -> Result<CreateDocumentOutput, DocumentError> {
        let artifact_id = ArtifactId::new(self.ids.new_artifact_id());
        let label = input.label.unwrap_or_else(|| default_label(input.kind));
        let retention = input.retention_limit.unwrap_or(self.default_retention);

        let meta = ArtifactMeta::initial(
            artifact_id.clone(),
            input.kind,
            input.session_id.clone(),
            label.clone(),
            retention,
        );

        let ir = input.initial_ir.unwrap_or_else(|| empty_ir(&artifact_id, input.kind));
        let mut ir = ir;
        if let Some(obj) = ir.as_object_mut() {
            obj.insert("artifact_id".into(), serde_json::json!(artifact_id.0));
            obj.insert("version_id".into(), serde_json::json!("v1"));
        }

        let (validator, renderer): (&Arc<dyn IRValidator>, &Arc<dyn IRRenderer>) = match input.kind {
            ArtifactKind::Excel => (&self.excel_validator, &self.excel_renderer),
            ArtifactKind::Word => (&self.word_validator, &self.word_renderer),
        };
        validator.validate(&ir)?;
        let bytes = renderer.render(&ir).await?;
        let ext = input.kind.extension();

        let patch_applied = PatchApplied {
            patch: serde_json::json!({
                "artifact_id": artifact_id.0,
                "base_version": "",
                "source": input.source,
                "ops": []
            }),
            applied_at: Utc::now(),
            resulted_in: VersionId::initial(),
            summary: PatchSummary::default(),
        };

        let version_data = VersionData {
            ir,
            rendered_binary: bytes,
            rendered_extension: match input.kind {
                ArtifactKind::Excel => "xlsx",
                ArtifactKind::Word => "docx",
            },
            patch_applied,
            blobs: vec![],
        };
        let _ = ext;

        self.store.create_artifact(&meta).await?;
        self.store.write_version(&artifact_id, &VersionId::initial(), &version_data).await?;
        self.store.set_head(&artifact_id, None, &VersionId::initial()).await?;

        Ok(CreateDocumentOutput {
            artifact_id,
            version_id: VersionId::initial(),
            label,
            meta,
        })
    }
}

fn default_label(kind: ArtifactKind) -> String {
    let now = Utc::now().format("%Y-%m-%d %H:%M");
    let k = match kind { ArtifactKind::Excel => "Excel", ArtifactKind::Word => "Word" };
    format!("Untitled {k} {now}")
}

fn empty_ir(id: &ArtifactId, kind: ArtifactKind) -> serde_json::Value {
    match kind {
        ArtifactKind::Excel => serde_json::json!({
            "kind": "excel",
            "artifact_id": id.0,
            "version_id": "v1",
            "schema_version": crate::documents::domain::ir::SCHEMA_VERSION,
            "workbook": { "sheets": [], "named_styles": {} }
        }),
        ArtifactKind::Word => serde_json::json!({
            "kind": "word",
            "artifact_id": id.0,
            "version_id": "v1",
            "schema_version": crate::documents::domain::ir::SCHEMA_VERSION,
            "document": { "blocks": [], "named_styles": {} }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::infrastructure::ids::CountingIdGenerator;
    use crate::documents::infrastructure::render::ExcelRenderer;
    use crate::documents::infrastructure::storage::LocalFsStore;
    use crate::documents::infrastructure::validation::ExcelValidator;
    use async_trait::async_trait;
    use tempfile::tempdir;

    struct NoopRenderer;
    #[async_trait]
    impl IRRenderer for NoopRenderer {
        async fn render(&self, _ir: &serde_json::Value) -> Result<Vec<u8>, crate::documents::domain::RenderError> {
            Ok(vec![])
        }
        fn target_extension(&self) -> &'static str { "docx" }
        fn target_mime(&self) -> &'static str { "application/octet-stream" }
    }
    struct NoopValidator;
    impl IRValidator for NoopValidator {
        fn validate(&self, _ir: &serde_json::Value) -> Result<(), DocumentError> { Ok(()) }
    }

    #[tokio::test]
    async fn creates_empty_excel_artifact() {
        let tmp = tempdir().unwrap();
        let uc = CreateDocumentUseCase {
            store: Arc::new(LocalFsStore::new(tmp.path())),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopRenderer),
            word_validator: Arc::new(NoopValidator),
            ids: Arc::new(CountingIdGenerator::default()),
            default_retention: 10,
        };
        let out = uc.execute(CreateDocumentInput {
            kind: ArtifactKind::Excel,
            session_id: SessionId::new("sess_1"),
            label: None,
            retention_limit: None,
            initial_ir: None,
            source: PatchSource::Agent,
        }).await.unwrap();
        assert_eq!(out.artifact_id.0, "art_01");
        assert_eq!(out.version_id, VersionId::initial());
        assert!(out.label.starts_with("Untitled Excel"));
    }
}
```

- [ ] **Step 2: Register in application/mod.rs**

Append:

```rust
pub mod create_document;
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib documents::application::create_document`
Expected: 1 test passes.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/application/
git commit -m "$(cat <<'EOF'
feat(documents): add CreateDocumentUseCase with default label and kind-dispatched validation/render

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A13: ApplyPatchUseCase (Excel only, no rebase yet)

**Files:**
- Create: `src/libs/colmena/src/documents/application/apply_patch.rs`
- Modify: `src/libs/colmena/src/documents/application/mod.rs`

- [ ] **Step 1: Write use case**

Create `src/libs/colmena/src/documents/application/apply_patch.rs`:

```rust
use crate::documents::application::apply_excel_ops::ExcelOpApplier;
use crate::documents::domain::artifact::{PatchApplied, PatchSummary, VersionData};
use crate::documents::domain::ids::{ArtifactId, ArtifactKind, VersionId};
use crate::documents::domain::ir::ExcelIR;
use crate::documents::domain::patch::Patch;
use crate::documents::domain::{
    ArtifactStore, DocumentError, IRRenderer, IRValidator, IdGenerator,
};
use chrono::Utc;
use std::sync::Arc;

pub struct ApplyPatchInput {
    pub patch: Patch,
}

pub struct ApplyPatchOutput {
    pub version_id: VersionId,
    pub summary: PatchSummary,
}

pub struct ApplyPatchUseCase {
    pub store: Arc<dyn ArtifactStore>,
    pub excel_renderer: Arc<dyn IRRenderer>,
    pub excel_validator: Arc<dyn IRValidator>,
    pub word_renderer: Arc<dyn IRRenderer>,
    pub word_validator: Arc<dyn IRValidator>,
    pub ids: Arc<dyn IdGenerator>,
}

impl ApplyPatchUseCase {
    pub async fn execute(&self, input: ApplyPatchInput) -> Result<ApplyPatchOutput, DocumentError> {
        let artifact_id = ArtifactId::new(input.patch.artifact_id.clone());
        let meta = self.store.read_meta(&artifact_id).await?;
        let current = meta.current_version.clone();

        if input.patch.base_version != current.0 {
            return Err(DocumentError::VersionConflict {
                artifact: artifact_id.clone(),
                base: VersionId::new(input.patch.base_version.clone()),
                current: current.clone(),
                conflicts: vec![],
            });
        }

        let current_data = self.store.read_version(&artifact_id, &current).await?;

        match meta.kind {
            ArtifactKind::Excel => {
                let mut ir: ExcelIR = serde_json::from_value(current_data.ir.clone())
                    .map_err(|e| DocumentError::IRValidationFailed {
                        path: "/".into(),
                        reason: format!("parse current IR: {e}"),
                    })?;
                let applier = ExcelOpApplier { ids: self.ids.as_ref() };
                for op in &input.patch.ops {
                    applier.apply(&mut ir, op)?;
                }
                let new_version = current.next();
                ir.version_id = new_version.0.clone();
                let ir_value = serde_json::to_value(&ir).unwrap();
                self.excel_validator.validate(&ir_value)?;
                let rendered = self.excel_renderer.render(&ir_value).await?;

                let patch_applied = PatchApplied {
                    patch: serde_json::to_value(&input.patch).unwrap(),
                    applied_at: Utc::now(),
                    resulted_in: new_version.clone(),
                    summary: PatchSummary::default(),
                };
                let version_data = VersionData {
                    ir: ir_value,
                    rendered_binary: rendered,
                    rendered_extension: "xlsx",
                    patch_applied,
                    blobs: vec![],
                };

                self.store.write_version(&artifact_id, &new_version, &version_data).await?;
                self.store.set_head(&artifact_id, Some(&current), &new_version).await?;

                let mut new_meta = meta.clone();
                new_meta.current_version = new_version.clone();
                new_meta.updated_at = Utc::now();
                self.store.update_meta(&artifact_id, &new_meta).await?;

                Ok(ApplyPatchOutput { version_id: new_version, summary: PatchSummary::default() })
            }
            ArtifactKind::Word => Err(DocumentError::InvalidPatchOp {
                reason: "Word patches not yet implemented (Phase B)".into(),
                op: serde_json::Value::Null,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::application::create_document::{CreateDocumentInput, CreateDocumentUseCase};
    use crate::documents::domain::ids::SessionId;
    use crate::documents::domain::patch::{PatchOp, PatchSource};
    use crate::documents::infrastructure::ids::CountingIdGenerator;
    use crate::documents::infrastructure::render::ExcelRenderer;
    use crate::documents::infrastructure::storage::LocalFsStore;
    use crate::documents::infrastructure::validation::ExcelValidator;
    use async_trait::async_trait;
    use tempfile::tempdir;

    struct NoopR;
    #[async_trait]
    impl IRRenderer for NoopR {
        async fn render(&self, _ir: &serde_json::Value) -> Result<Vec<u8>, crate::documents::domain::RenderError> { Ok(vec![]) }
        fn target_extension(&self) -> &'static str { "docx" }
        fn target_mime(&self) -> &'static str { "x" }
    }
    struct NoopV;
    impl IRValidator for NoopV {
        fn validate(&self, _ir: &serde_json::Value) -> Result<(), DocumentError> { Ok(()) }
    }

    #[tokio::test]
    async fn apply_set_cell_creates_v2() {
        let tmp = tempdir().unwrap();
        let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp.path()));
        let ids = Arc::new(CountingIdGenerator::default());
        let create = CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            ids: ids.clone(),
            default_retention: 10,
        };
        let out = create.execute(CreateDocumentInput {
            kind: ArtifactKind::Excel,
            session_id: SessionId::new("s"),
            label: None, retention_limit: None,
            initial_ir: Some(serde_json::json!({
                "kind": "excel",
                "artifact_id": "x", "version_id": "v1",
                "schema_version": "1.0.0",
                "workbook": { "sheets": [
                    {"id": "s1", "name": "Hoja1", "order": 0, "columns": [], "cells": {}, "tables": []}
                ], "named_styles": {} }
            })),
            source: PatchSource::Agent,
        }).await.unwrap();

        let apply = ApplyPatchUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            ids: ids.clone(),
        };
        let res = apply.execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: out.artifact_id.0.clone(),
                base_version: "v1".into(),
                source: PatchSource::Agent,
                ops: vec![PatchOp::SetCell {
                    sheet_id: "s1".into(), address: "B5".into(),
                    value: serde_json::json!(42),
                    value_type: None, format: None, style_ref: None,
                }],
            },
        }).await.unwrap();
        assert_eq!(res.version_id.0, "v2");
    }

    #[tokio::test]
    async fn apply_with_stale_base_returns_conflict() {
        let tmp = tempdir().unwrap();
        let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp.path()));
        let ids = Arc::new(CountingIdGenerator::default());
        let create = CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            ids: ids.clone(),
            default_retention: 10,
        };
        let out = create.execute(CreateDocumentInput {
            kind: ArtifactKind::Excel,
            session_id: SessionId::new("s"),
            label: None, retention_limit: None,
            initial_ir: Some(serde_json::json!({
                "kind": "excel", "artifact_id": "x", "version_id": "v1",
                "schema_version": "1.0.0",
                "workbook": {"sheets": [{"id": "s1", "name": "H", "order": 0, "columns": [], "cells": {}, "tables": []}], "named_styles": {}}
            })),
            source: PatchSource::Agent,
        }).await.unwrap();

        let apply = ApplyPatchUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            ids: ids.clone(),
        };
        let err = apply.execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: out.artifact_id.0,
                base_version: "v0".into(),
                source: PatchSource::Agent,
                ops: vec![],
            },
        }).await.unwrap_err();
        assert!(matches!(err, DocumentError::VersionConflict { .. }));
    }
}
```

- [ ] **Step 2: Register in mod.rs**

Append:

```rust
pub mod apply_patch;
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib documents::application::apply_patch`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/application/
git commit -m "$(cat <<'EOF'
feat(documents): add ApplyPatchUseCase for Excel (no rebase — fast-fail on stale base)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A14: Read, list_versions, rollback, get_head use cases

**Files:**
- Create: `src/libs/colmena/src/documents/application/read_document.rs`
- Create: `src/libs/colmena/src/documents/application/list_versions.rs`
- Create: `src/libs/colmena/src/documents/application/rollback.rs`
- Create: `src/libs/colmena/src/documents/application/get_head.rs`
- Modify: `src/libs/colmena/src/documents/application/mod.rs`

- [ ] **Step 1: Write read_document.rs**

```rust
use crate::documents::domain::ids::{ArtifactId, VersionId};
use crate::documents::domain::{ArtifactStore, DocumentError};
use std::sync::Arc;

pub struct ReadDocumentInput {
    pub artifact_id: ArtifactId,
    pub version: Option<VersionId>,
}

pub struct ReadDocumentOutput {
    pub ir: serde_json::Value,
    pub version: VersionId,
}

pub struct ReadDocumentUseCase {
    pub store: Arc<dyn ArtifactStore>,
}

impl ReadDocumentUseCase {
    pub async fn execute(&self, input: ReadDocumentInput) -> Result<ReadDocumentOutput, DocumentError> {
        let data = match input.version {
            Some(v) => self.store.read_version(&input.artifact_id, &v).await?,
            None => self.store.read_current(&input.artifact_id).await?,
        };
        let version = VersionId::new(
            data.ir.get("version_id").and_then(|v| v.as_str()).unwrap_or_default().to_string()
        );
        Ok(ReadDocumentOutput { ir: data.ir, version })
    }
}
```

- [ ] **Step 2: Write list_versions.rs**

```rust
use crate::documents::domain::ids::{ArtifactId, VersionId};
use crate::documents::domain::{ArtifactStore, DocumentError};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
pub struct VersionEntry {
    pub version_id: VersionId,
    pub applied_at: DateTime<Utc>,
    pub source: String,
    pub summary: Vec<String>,
}

pub struct ListVersionsUseCase {
    pub store: Arc<dyn ArtifactStore>,
}

impl ListVersionsUseCase {
    pub async fn execute(
        &self,
        artifact_id: &ArtifactId,
        limit: Option<u32>,
    ) -> Result<Vec<VersionEntry>, DocumentError> {
        let versions = self.store.list_versions(artifact_id).await?;
        let mut entries: Vec<VersionEntry> = Vec::new();
        let take = limit.map(|n| n as usize).unwrap_or(versions.len());
        for v in versions.iter().rev().take(take) {
            let data = self.store.read_version(artifact_id, v).await?;
            entries.push(VersionEntry {
                version_id: v.clone(),
                applied_at: data.patch_applied.applied_at,
                source: data.patch_applied.patch
                    .get("source").and_then(|s| s.as_str())
                    .unwrap_or("agent").to_string(),
                summary: data.patch_applied.summary.natural_language,
            });
        }
        Ok(entries)
    }
}
```

- [ ] **Step 3: Write rollback.rs**

```rust
use crate::documents::domain::artifact::{PatchApplied, PatchSummary, VersionData};
use crate::documents::domain::ids::{ArtifactId, VersionId};
use crate::documents::domain::patch::PatchSource;
use crate::documents::domain::{ArtifactStore, DocumentError};
use chrono::Utc;
use std::sync::Arc;

pub struct RollbackInput {
    pub artifact_id: ArtifactId,
    pub to_version: VersionId,
}

pub struct RollbackOutput {
    pub new_version_id: VersionId,
    pub copied_from: VersionId,
}

pub struct RollbackUseCase {
    pub store: Arc<dyn ArtifactStore>,
}

impl RollbackUseCase {
    pub async fn execute(&self, input: RollbackInput) -> Result<RollbackOutput, DocumentError> {
        let mut meta = self.store.read_meta(&input.artifact_id).await?;
        let target = self.store.read_version(&input.artifact_id, &input.to_version).await?;
        let new_version = meta.current_version.next();

        let mut ir = target.ir.clone();
        if let Some(obj) = ir.as_object_mut() {
            obj.insert("version_id".into(), serde_json::json!(new_version.0));
        }

        let patch_applied = PatchApplied {
            patch: serde_json::json!({
                "artifact_id": input.artifact_id.0,
                "base_version": meta.current_version.0,
                "source": PatchSource::Agent,
                "ops": [{"op": "rollback_from", "target": input.to_version.0}]
            }),
            applied_at: Utc::now(),
            resulted_in: new_version.clone(),
            summary: PatchSummary {
                natural_language: vec![format!("Rolled back to {}", input.to_version.0)],
                structured: vec![],
            },
        };

        let version_data = VersionData {
            ir,
            rendered_binary: target.rendered_binary.clone(),
            rendered_extension: target.rendered_extension,
            patch_applied,
            blobs: vec![],
        };

        self.store.write_version(&input.artifact_id, &new_version, &version_data).await?;
        self.store.set_head(&input.artifact_id, Some(&meta.current_version), &new_version).await?;
        meta.current_version = new_version.clone();
        meta.updated_at = Utc::now();
        self.store.update_meta(&input.artifact_id, &meta).await?;

        Ok(RollbackOutput { new_version_id: new_version, copied_from: input.to_version })
    }
}
```

- [ ] **Step 4: Write get_head.rs**

```rust
use crate::documents::domain::ids::{ArtifactId, VersionId};
use crate::documents::domain::{ArtifactStore, DocumentError};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
pub struct GetHeadOutput {
    pub artifact_id: ArtifactId,
    pub current_version: VersionId,
    pub updated_at: DateTime<Utc>,
    pub last_source: String,
    pub summary_since: Vec<String>,
    pub versions_in_window: Vec<VersionId>,
}

pub struct GetHeadInput {
    pub artifact_id: ArtifactId,
    pub since_version: Option<VersionId>,
}

pub struct GetHeadUseCase {
    pub store: Arc<dyn ArtifactStore>,
}

impl GetHeadUseCase {
    pub async fn execute(&self, input: GetHeadInput) -> Result<GetHeadOutput, DocumentError> {
        let meta = self.store.read_meta(&input.artifact_id).await?;
        let current_data = self.store.read_version(&input.artifact_id, &meta.current_version).await?;
        let last_source = current_data.patch_applied.patch
            .get("source").and_then(|s| s.as_str()).unwrap_or("agent").to_string();

        let (summary_since, window) = if let Some(since) = input.since_version {
            let versions = self.store.list_versions(&input.artifact_id).await?;
            let since_n = since.number().unwrap_or(0);
            let mut lines = Vec::new();
            let mut in_window = Vec::new();
            for v in versions {
                if v.number().unwrap_or(0) > since_n {
                    let d = self.store.read_version(&input.artifact_id, &v).await?;
                    let src = d.patch_applied.patch.get("source")
                        .and_then(|s| s.as_str()).unwrap_or("agent").to_string();
                    if src == "user" {
                        for line in d.patch_applied.summary.natural_language {
                            lines.push(format!("[{}, user, {}] {}",
                                v.0, d.patch_applied.applied_at.format("%H:%M"), line));
                        }
                    }
                    in_window.push(v);
                }
            }
            (lines, in_window)
        } else {
            (vec![], vec![])
        };

        Ok(GetHeadOutput {
            artifact_id: input.artifact_id,
            current_version: meta.current_version,
            updated_at: meta.updated_at,
            last_source,
            summary_since,
            versions_in_window: window,
        })
    }
}
```

- [ ] **Step 5: Register in mod.rs**

Append:

```rust
pub mod read_document;
pub mod list_versions;
pub mod rollback;
pub mod get_head;
```

- [ ] **Step 6: Verify compilation**

Run: `cargo check --lib`
Expected: success.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/documents/application/
git commit -m "$(cat <<'EOF'
feat(documents): add Read, ListVersions, Rollback, GetHead use cases

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A15: Integration test — full create/edit/read/rollback cycle

**Files:**
- Create: `tests/documents_local_fs_cycle.rs`

- [ ] **Step 1: Write integration test**

Create `tests/documents_local_fs_cycle.rs`:

```rust
use colmena::documents::application::apply_patch::{ApplyPatchInput, ApplyPatchUseCase};
use colmena::documents::application::create_document::{CreateDocumentInput, CreateDocumentUseCase};
use colmena::documents::application::get_head::{GetHeadInput, GetHeadUseCase};
use colmena::documents::application::list_versions::ListVersionsUseCase;
use colmena::documents::application::read_document::{ReadDocumentInput, ReadDocumentUseCase};
use colmena::documents::application::rollback::{RollbackInput, RollbackUseCase};
use colmena::documents::domain::ids::{ArtifactKind, SessionId};
use colmena::documents::domain::patch::{Patch, PatchOp, PatchSource};
use colmena::documents::domain::{ArtifactStore, IRRenderer, IRValidator};
use colmena::documents::infrastructure::ids::CountingIdGenerator;
use colmena::documents::infrastructure::render::ExcelRenderer;
use colmena::documents::infrastructure::storage::LocalFsStore;
use colmena::documents::infrastructure::validation::ExcelValidator;
use std::sync::Arc;
use tempfile::tempdir;

struct NoopR;
#[async_trait::async_trait]
impl IRRenderer for NoopR {
    async fn render(&self, _ir: &serde_json::Value) -> Result<Vec<u8>, colmena::documents::domain::RenderError> { Ok(vec![]) }
    fn target_extension(&self) -> &'static str { "docx" }
    fn target_mime(&self) -> &'static str { "x" }
}
struct NoopV;
impl IRValidator for NoopV {
    fn validate(&self, _ir: &serde_json::Value) -> Result<(), colmena::documents::domain::DocumentError> { Ok(()) }
}

#[tokio::test]
async fn full_cycle_excel() {
    let tmp = tempdir().unwrap();
    let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp.path()));
    let ids = Arc::new(CountingIdGenerator::default());

    let create = CreateDocumentUseCase {
        store: store.clone(),
        excel_renderer: Arc::new(ExcelRenderer),
        excel_validator: Arc::new(ExcelValidator),
        word_renderer: Arc::new(NoopR),
        word_validator: Arc::new(NoopV),
        ids: ids.clone(),
        default_retention: 10,
    };
    let apply = ApplyPatchUseCase {
        store: store.clone(),
        excel_renderer: Arc::new(ExcelRenderer),
        excel_validator: Arc::new(ExcelValidator),
        word_renderer: Arc::new(NoopR),
        word_validator: Arc::new(NoopV),
        ids: ids.clone(),
    };
    let read = ReadDocumentUseCase { store: store.clone() };
    let list = ListVersionsUseCase { store: store.clone() };
    let rollback = RollbackUseCase { store: store.clone() };
    let head = GetHeadUseCase { store: store.clone() };

    let created = create.execute(CreateDocumentInput {
        kind: ArtifactKind::Excel,
        session_id: SessionId::new("s"),
        label: Some("Report".into()),
        retention_limit: None,
        initial_ir: Some(serde_json::json!({
            "kind": "excel", "artifact_id": "x", "version_id": "v1",
            "schema_version": "1.0.0",
            "workbook": {"sheets": [{"id": "s1", "name": "H", "order": 0, "columns": [], "cells": {}, "tables": []}], "named_styles": {}}
        })),
        source: PatchSource::Agent,
    }).await.unwrap();

    apply.execute(ApplyPatchInput {
        patch: Patch {
            artifact_id: created.artifact_id.0.clone(),
            base_version: "v1".into(),
            source: PatchSource::Agent,
            ops: vec![PatchOp::SetCell {
                sheet_id: "s1".into(), address: "A1".into(),
                value: serde_json::json!("hello"),
                value_type: None, format: None, style_ref: None,
            }],
        },
    }).await.unwrap();

    let r = read.execute(ReadDocumentInput {
        artifact_id: created.artifact_id.clone(), version: None,
    }).await.unwrap();
    assert_eq!(r.version.0, "v2");
    assert_eq!(r.ir["workbook"]["sheets"][0]["cells"]["A1"]["value"], "hello");

    let versions = list.execute(&created.artifact_id, None).await.unwrap();
    assert_eq!(versions.len(), 2);

    let rb = rollback.execute(RollbackInput {
        artifact_id: created.artifact_id.clone(),
        to_version: colmena::documents::domain::ids::VersionId::new("v1"),
    }).await.unwrap();
    assert_eq!(rb.new_version_id.0, "v3");

    let h = head.execute(GetHeadInput {
        artifact_id: created.artifact_id.clone(),
        since_version: None,
    }).await.unwrap();
    assert_eq!(h.current_version.0, "v3");
}
```

- [ ] **Step 2: Run integration test**

Run: `cargo test --test documents_local_fs_cycle`
Expected: 1 test passes.

- [ ] **Step 3: Commit**

```bash
git add tests/documents_local_fs_cycle.rs
git commit -m "$(cat <<'EOF'
test(documents): add end-to-end integration test for create/apply/read/rollback on LocalFS

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task A16: Synthetic LLM tools (Excel subset — create, apply_patch, read)

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`

- [ ] **Step 1: Read existing synthetic tool pattern**

Read `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs` to understand how `build_load_skill_tool_definition`, `dispatch_load_skill`, and `into_tool_result` are wired.

- [ ] **Step 2: Implement document_tools.rs**

Create `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`:

```rust
//! Synthetic LLM tools for document artifacts.
//!
//! Each tool is a thin adapter: it builds a schemars-derived JSON Schema for
//! the LLM, parses the LLM-provided arguments, injects `session_id` from
//! context (never from the LLM), and dispatches to the matching use case.

use crate::documents::application::apply_patch::{ApplyPatchInput, ApplyPatchUseCase};
use crate::documents::application::create_document::{CreateDocumentInput, CreateDocumentUseCase};
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
                     artifact_id and initial version. Use for any new document task.".into(),
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
                     structured details.".into(),
        input_schema: serde_json::to_value(schema).unwrap(),
    }
}

pub fn build_document_read_tool() -> ToolDefinition {
    let schema = schemars::schema_for!(DocumentReadArgs);
    ToolDefinition {
        name: DOCUMENT_READ_TOOL.into(),
        description: "Read the full IR of a document at a given version (or current)."
            .into(),
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
        "word"  => ArtifactKind::Word,
        other   => return json!({"error": format!("unknown kind: {other}")}),
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
            crate::documents::domain::DocumentError::VersionConflict { current, conflicts, .. } => json!({
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
```

- [ ] **Step 3: Register in synthetic_tools/mod.rs**

Edit `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`:

```rust
//! Synthetic tools for the LLM node — tools that don't map to DAG nodes.

pub mod load_skill_tool;
pub mod document_tools;

pub use load_skill_tool::{
    build_load_skill_tool_definition, dispatch_load_skill, into_tool_result,
    LoadSkillDispatchResult, LOAD_SKILL_TOOL_NAME,
};

pub use document_tools::{
    build_document_apply_patch_tool, build_document_create_tool, build_document_read_tool,
    dispatch_document_apply_patch, dispatch_document_create, dispatch_document_read,
    DocumentToolsContext, DOCUMENT_APPLY_PATCH_TOOL, DOCUMENT_CREATE_TOOL, DOCUMENT_READ_TOOL,
};
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib llm_synthetic_tools::document_tools`
Expected: 2 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/
git commit -m "$(cat <<'EOF'
feat(documents): add synthetic LLM tools for document_create, document_apply_patch, document_read

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

**End of Phase A.** At this point Colmena has a working Excel-only MVP: create, patch, read, rollback, listing, backed by LocalFS, exposed as synthetic LLM tools with `schemars`-documented input. Phases B–E add Word, DB indexing, rebase/concurrency, GCS, DAG nodes, and docs.

---

## Phase B — Word support

Phase B adds Word IR, Word PatchOps, Word validator, Word renderer via `docx-rs`, and wires Word through `CreateDocumentUseCase` and `ApplyPatchUseCase`.

### Task B1: Word IR domain types

**Files:**
- Create: `src/libs/colmena/src/documents/domain/ir/word.rs`
- Modify: `src/libs/colmena/src/documents/domain/ir/mod.rs`

- [ ] **Step 1: Write Word IR types**

Create `src/libs/colmena/src/documents/domain/ir/word.rs`:

```rust
use super::common::NamedStyle;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordIR {
    pub kind: WordKindTag,
    pub artifact_id: String,
    pub version_id: String,
    pub schema_version: String,
    pub document: WordDocument,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WordKindTag { Word }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WordDocument {
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub named_styles: BTreeMap<String, NamedStyle>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Block {
    Heading {
        id: String,
        level: u8,
        runs: Vec<Run>,
    },
    Paragraph {
        id: String,
        runs: Vec<Run>,
    },
    List {
        id: String,
        #[serde(default = "default_list_style")]
        style: ListStyle,
        items: Vec<ListItem>,
    },
    Table {
        id: String,
        rows: Vec<TableRow>,
    },
}

fn default_list_style() -> ListStyle { ListStyle::Bullet }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListStyle { Bullet, Numbered }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underline: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListItem {
    pub id: String,
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableRow {
    pub id: String,
    pub cells: Vec<TableCell>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableCell {
    pub runs: Vec<Run>,
}

impl Block {
    pub fn id(&self) -> &str {
        match self {
            Block::Heading { id, .. } |
            Block::Paragraph { id, .. } |
            Block::List { id, .. } |
            Block::Table { id, .. } => id,
        }
    }
}

impl WordIR {
    pub fn empty(artifact_id: impl Into<String>, version_id: impl Into<String>) -> Self {
        Self {
            kind: WordKindTag::Word,
            artifact_id: artifact_id.into(),
            version_id: version_id.into(),
            schema_version: super::common::SCHEMA_VERSION.to_string(),
            document: WordDocument::default(),
        }
    }

    pub fn block_mut(&mut self, block_id: &str) -> Option<&mut Block> {
        self.document.blocks.iter_mut().find(|b| b.id() == block_id)
    }

    pub fn block_index(&self, block_id: &str) -> Option<usize> {
        self.document.blocks.iter().position(|b| b.id() == block_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_ir_roundtrip() {
        let mut ir = WordIR::empty("art_x", "v1");
        ir.document.blocks.push(Block::Heading {
            id: "blk_01".into(),
            level: 1,
            runs: vec![Run {
                id: "run_01".into(), text: "Title".into(),
                bold: Some(true), italic: None, underline: None, size: None, color: None,
            }],
        });
        let j = serde_json::to_value(&ir).unwrap();
        assert_eq!(j["document"]["blocks"][0]["type"], "heading");
        assert_eq!(j["document"]["blocks"][0]["runs"][0]["text"], "Title");
        let back: WordIR = serde_json::from_value(j).unwrap();
        assert_eq!(back.document.blocks.len(), 1);
    }
}
```

- [ ] **Step 2: Register in IR mod**

Edit `src/libs/colmena/src/documents/domain/ir/mod.rs` — append:

```rust
pub mod word;
pub use word::{Block, ListItem, ListStyle, Run, TableCell, TableRow, WordDocument, WordIR, WordKindTag};
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib documents::domain::ir::word`
Expected: 1 test passes.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/domain/ir/
git commit -m "$(cat <<'EOF'
feat(documents): add Word IR domain types (Block enum, Run, List, Table)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task B2: Word PatchOp variants

**Files:**
- Modify: `src/libs/colmena/src/documents/domain/patch.rs`

- [ ] **Step 1: Add Word variants to PatchOp enum**

Edit `src/libs/colmena/src/documents/domain/patch.rs`. Inside the `PatchOp` enum, after `DefineStyle`, add:

```rust
    // -------- Word ops --------

    /// Insert a new block. Exactly one of `before` or `after` must be provided
    /// (referencing an existing block_id). If both omitted, appends at end.
    #[serde(rename = "insert_block")]
    InsertBlock {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<String>,
        /// Full block JSON (type-tagged). ID will be assigned server-side.
        block: serde_json::Value,
    },

    /// Delete a block by ID.
    #[serde(rename = "delete_block")]
    DeleteBlock { block_id: String },

    /// Replace a block's entire content (preserves the ID).
    #[serde(rename = "replace_block")]
    ReplaceBlock {
        block_id: String,
        block: serde_json::Value,
    },

    /// Move a block to appear right after `after_block_id`.
    #[serde(rename = "move_block")]
    MoveBlock { block_id: String, after_block_id: String },

    /// Change the level of a heading block (1-6).
    #[serde(rename = "set_heading_level")]
    SetHeadingLevel { block_id: String, level: u8 },

    /// Replace the text of a specific run inside a paragraph or heading.
    #[serde(rename = "replace_run_text")]
    ReplaceRunText {
        block_id: String,
        run_id: String,
        new_text: String,
    },

    /// Update style properties of a run (bold/italic/underline/size/color).
    /// `style_patch` is a partial Run — only provided fields are updated.
    #[serde(rename = "set_run_style")]
    SetRunStyle {
        block_id: String,
        run_id: String,
        style_patch: serde_json::Value,
    },

    /// Insert a run at a position inside a paragraph or heading. ID assigned server-side.
    #[serde(rename = "insert_run")]
    InsertRun {
        block_id: String,
        at_index: u32,
        run: serde_json::Value,
    },

    /// Delete a run from a paragraph or heading.
    #[serde(rename = "delete_run")]
    DeleteRun { block_id: String, run_id: String },

    /// Insert an item into a list block.
    #[serde(rename = "insert_list_item")]
    InsertListItem {
        list_block_id: String,
        at_index: u32,
        runs: Vec<serde_json::Value>,
    },

    /// Replace all runs of a list item.
    #[serde(rename = "replace_list_item")]
    ReplaceListItem {
        list_block_id: String,
        item_id: String,
        runs: Vec<serde_json::Value>,
    },

    /// Delete a list item.
    #[serde(rename = "delete_list_item")]
    DeleteListItem { list_block_id: String, item_id: String },

    /// Insert a row in a table block. Exactly one of `before`/`after` must be provided.
    #[serde(rename = "insert_table_row")]
    InsertTableRow {
        table_block_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<String>,
        /// Array of cells, each with a `runs` array.
        cells: Vec<serde_json::Value>,
    },

    /// Delete a table row.
    #[serde(rename = "delete_table_row")]
    DeleteTableRow { table_block_id: String, row_id: String },

    /// Replace a table cell's runs.
    #[serde(rename = "update_table_cell")]
    UpdateTableCell {
        table_block_id: String,
        row_id: String,
        col_index: u32,
        runs: Vec<serde_json::Value>,
    },
```

- [ ] **Step 2: Verify compilation and existing tests pass**

Run: `cargo test --lib documents::domain::patch`
Expected: all 3 tests still pass.

- [ ] **Step 3: Add Word schema test**

Add to the `#[cfg(test)] mod tests` block in `patch.rs`:

```rust
    #[test]
    fn word_ops_in_schema() {
        let schema = schemars::schema_for!(PatchOp);
        let s = serde_json::to_string(&schema).unwrap();
        assert!(s.contains("insert_block"));
        assert!(s.contains("replace_run_text"));
        assert!(s.contains("insert_table_row"));
    }
```

Run: `cargo test --lib documents::domain::patch`
Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/domain/patch.rs
git commit -m "$(cat <<'EOF'
feat(documents): add Word variants to PatchOp enum (blocks, runs, lists, tables)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task B3: Word IR validator

**Files:**
- Create: `src/libs/colmena/src/documents/infrastructure/validation/word_validator.rs`
- Modify: `src/libs/colmena/src/documents/infrastructure/validation/mod.rs`

- [ ] **Step 1: Implement validator**

Create `src/libs/colmena/src/documents/infrastructure/validation/word_validator.rs`:

```rust
use crate::documents::domain::ir::{Block, WordIR};
use crate::documents::domain::{DocumentError, IRValidator};
use std::collections::HashSet;

pub struct WordValidator;

impl IRValidator for WordValidator {
    fn validate(&self, ir_value: &serde_json::Value) -> Result<(), DocumentError> {
        let ir: WordIR = serde_json::from_value(ir_value.clone())
            .map_err(|e| DocumentError::IRValidationFailed {
                path: "/".into(),
                reason: format!("not a valid Word IR: {e}"),
            })?;

        let mut block_ids: HashSet<&str> = HashSet::new();
        for (i, block) in ir.document.blocks.iter().enumerate() {
            if !block_ids.insert(block.id()) {
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/document/blocks/{i}/id"),
                    reason: format!("duplicate block ID: {}", block.id()),
                });
            }
            validate_block(block, i)?;
        }
        Ok(())
    }
}

fn validate_block(block: &Block, idx: usize) -> Result<(), DocumentError> {
    match block {
        Block::Heading { level, runs, .. } => {
            if !(1..=6).contains(level) {
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/document/blocks/{idx}/level"),
                    reason: format!("heading level must be 1..=6, got {level}"),
                });
            }
            check_run_ids(runs, &format!("/document/blocks/{idx}"))?;
        }
        Block::Paragraph { runs, .. } => {
            check_run_ids(runs, &format!("/document/blocks/{idx}"))?;
        }
        Block::List { items, .. } => {
            let mut seen: HashSet<&str> = HashSet::new();
            for (i, it) in items.iter().enumerate() {
                if !seen.insert(&it.id) {
                    return Err(DocumentError::IRValidationFailed {
                        path: format!("/document/blocks/{idx}/items/{i}/id"),
                        reason: format!("duplicate list item ID: {}", it.id),
                    });
                }
                check_run_ids(&it.runs, &format!("/document/blocks/{idx}/items/{i}"))?;
            }
        }
        Block::Table { rows, .. } => {
            let mut seen: HashSet<&str> = HashSet::new();
            for (i, row) in rows.iter().enumerate() {
                if !seen.insert(&row.id) {
                    return Err(DocumentError::IRValidationFailed {
                        path: format!("/document/blocks/{idx}/rows/{i}/id"),
                        reason: format!("duplicate row ID: {}", row.id),
                    });
                }
                for (c, cell) in row.cells.iter().enumerate() {
                    check_run_ids(&cell.runs,
                        &format!("/document/blocks/{idx}/rows/{i}/cells/{c}"))?;
                }
            }
        }
    }
    Ok(())
}

fn check_run_ids(runs: &[crate::documents::domain::ir::Run], scope: &str) -> Result<(), DocumentError> {
    let mut seen: HashSet<&str> = HashSet::new();
    for (i, r) in runs.iter().enumerate() {
        if !seen.insert(&r.id) {
            return Err(DocumentError::IRValidationFailed {
                path: format!("{scope}/runs/{i}/id"),
                reason: format!("duplicate run ID in scope: {}", r.id),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ir::{Block, Run, WordIR};

    #[test]
    fn empty_word_is_valid() {
        let ir = WordIR::empty("x", "v1");
        WordValidator.validate(&serde_json::to_value(&ir).unwrap()).unwrap();
    }

    #[test]
    fn duplicate_block_ids_fail() {
        let mut ir = WordIR::empty("x", "v1");
        ir.document.blocks.push(Block::Paragraph { id: "b1".into(), runs: vec![] });
        ir.document.blocks.push(Block::Paragraph { id: "b1".into(), runs: vec![] });
        assert!(WordValidator.validate(&serde_json::to_value(&ir).unwrap()).is_err());
    }

    #[test]
    fn heading_level_out_of_range_fails() {
        let mut ir = WordIR::empty("x", "v1");
        ir.document.blocks.push(Block::Heading {
            id: "h".into(), level: 9, runs: vec![],
        });
        assert!(WordValidator.validate(&serde_json::to_value(&ir).unwrap()).is_err());
    }

    #[test]
    fn same_run_id_in_different_blocks_ok() {
        let mut ir = WordIR::empty("x", "v1");
        ir.document.blocks.push(Block::Paragraph {
            id: "b1".into(),
            runs: vec![Run { id: "r1".into(), text: "a".into(), bold: None, italic: None, underline: None, size: None, color: None }],
        });
        ir.document.blocks.push(Block::Paragraph {
            id: "b2".into(),
            runs: vec![Run { id: "r1".into(), text: "b".into(), bold: None, italic: None, underline: None, size: None, color: None }],
        });
        WordValidator.validate(&serde_json::to_value(&ir).unwrap()).unwrap();
    }
}
```

- [ ] **Step 2: Register**

Edit `src/libs/colmena/src/documents/infrastructure/validation/mod.rs`:

```rust
pub mod excel_validator;
pub mod word_validator;

pub use excel_validator::ExcelValidator;
pub use word_validator::WordValidator;
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib documents::infrastructure::validation::word_validator`
Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/infrastructure/validation/
git commit -m "$(cat <<'EOF'
feat(documents): add Word IR validator (unique block IDs, heading levels, scoped run IDs)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task B4: Word renderer via docx-rs

**Files:**
- Create: `src/libs/colmena/src/documents/infrastructure/render/word_renderer.rs`
- Modify: `src/libs/colmena/src/documents/infrastructure/render/mod.rs`

- [ ] **Step 1: Implement renderer**

Create `src/libs/colmena/src/documents/infrastructure/render/word_renderer.rs`:

```rust
use crate::documents::domain::ir::{Block, Run, WordIR};
use crate::documents::domain::{IRRenderer, RenderError};
use async_trait::async_trait;
use docx_rs::{Docx, Paragraph, Run as DocxRun, RunFonts, Table as DocxTable, TableCell as DocxCell, TableRow as DocxRow};

pub struct WordRenderer;

impl WordRenderer {
    fn render_sync(ir: &WordIR) -> Result<Vec<u8>, RenderError> {
        let mut doc = Docx::new();
        for block in &ir.document.blocks {
            match block {
                Block::Heading { level, runs, .. } => {
                    let mut p = Paragraph::new().style(&format!("Heading{level}"));
                    for run in runs { p = p.add_run(build_run(run)); }
                    doc = doc.add_paragraph(p);
                }
                Block::Paragraph { runs, .. } => {
                    let mut p = Paragraph::new();
                    for run in runs { p = p.add_run(build_run(run)); }
                    doc = doc.add_paragraph(p);
                }
                Block::List { items, style, .. } => {
                    let num_id = match style {
                        crate::documents::domain::ir::ListStyle::Bullet => 1,
                        crate::documents::domain::ir::ListStyle::Numbered => 2,
                    };
                    for it in items {
                        let mut p = Paragraph::new().numbering(docx_rs::NumberingId::new(num_id), docx_rs::IndentLevel::new(0));
                        for run in &it.runs { p = p.add_run(build_run(run)); }
                        doc = doc.add_paragraph(p);
                    }
                }
                Block::Table { rows, .. } => {
                    let mut drows = Vec::new();
                    for row in rows {
                        let mut dcells = Vec::new();
                        for cell in &row.cells {
                            let mut p = Paragraph::new();
                            for run in &cell.runs { p = p.add_run(build_run(run)); }
                            dcells.push(DocxCell::new().add_paragraph(p));
                        }
                        drows.push(DocxRow::new(dcells));
                    }
                    let tbl = DocxTable::new(drows);
                    doc = doc.add_table(tbl);
                }
            }
        }
        let mut buf: Vec<u8> = Vec::new();
        doc.build().pack(std::io::Cursor::new(&mut buf))
            .map_err(|e| RenderError::Failed(format!("pack docx: {e}")))?;
        Ok(buf)
    }
}

fn build_run(run: &Run) -> DocxRun {
    let mut r = DocxRun::new().add_text(&run.text);
    if run.bold.unwrap_or(false) { r = r.bold(); }
    if run.italic.unwrap_or(false) { r = r.italic(); }
    if run.underline.unwrap_or(false) { r = r.underline("single"); }
    if let Some(sz) = run.size {
        r = r.size((sz * 2.0) as usize);
    }
    if let Some(color) = &run.color {
        r = r.color(color.trim_start_matches('#').to_string());
    }
    if run.size.is_some() {
        r = r.fonts(RunFonts::new().ascii("Calibri"));
    }
    r
}

#[async_trait]
impl IRRenderer for WordRenderer {
    async fn render(&self, ir: &serde_json::Value) -> Result<Vec<u8>, RenderError> {
        let ir: WordIR = serde_json::from_value(ir.clone())
            .map_err(|e| RenderError::Failed(format!("parse word IR: {e}")))?;
        Self::render_sync(&ir)
    }
    fn target_extension(&self) -> &'static str { "docx" }
    fn target_mime(&self) -> &'static str {
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ir::{Block, Run};

    #[tokio::test]
    async fn renders_minimal_docx() {
        let mut ir = WordIR::empty("x", "v1");
        ir.document.blocks.push(Block::Heading {
            id: "h1".into(), level: 1,
            runs: vec![Run { id: "r1".into(), text: "Title".into(), bold: Some(true), italic: None, underline: None, size: None, color: None }],
        });
        ir.document.blocks.push(Block::Paragraph {
            id: "p1".into(),
            runs: vec![Run { id: "r1".into(), text: "body".into(), bold: None, italic: None, underline: None, size: None, color: None }],
        });
        let bytes = WordRenderer.render(&serde_json::to_value(&ir).unwrap()).await.unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[..2], b"PK");
    }
}
```

- [ ] **Step 2: Register**

Edit `src/libs/colmena/src/documents/infrastructure/render/mod.rs`:

```rust
pub mod excel_renderer;
pub mod word_renderer;

pub use excel_renderer::ExcelRenderer;
pub use word_renderer::WordRenderer;
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib documents::infrastructure::render::word_renderer`
Expected: 1 test passes.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/infrastructure/render/
git commit -m "$(cat <<'EOF'
feat(documents): add Word renderer via docx-rs (headings, paragraphs, lists, tables)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task B5: Word patch applier

**Files:**
- Create: `src/libs/colmena/src/documents/application/apply_word_ops.rs`
- Modify: `src/libs/colmena/src/documents/application/apply_patch.rs`
- Modify: `src/libs/colmena/src/documents/application/mod.rs`

- [ ] **Step 1: Implement Word op applier**

Create `src/libs/colmena/src/documents/application/apply_word_ops.rs`:

```rust
use crate::documents::domain::ir::{Block, ListItem, Run, TableCell, TableRow, WordIR};
use crate::documents::domain::patch::PatchOp;
use crate::documents::domain::{DocumentError, IdGenerator};

pub struct WordOpApplier<'a> {
    pub ids: &'a dyn IdGenerator,
}

impl<'a> WordOpApplier<'a> {
    pub fn apply(&self, ir: &mut WordIR, op: &PatchOp) -> Result<(), DocumentError> {
        match op {
            PatchOp::InsertBlock { before, after, block } => {
                let mut new_block: Block = serde_json::from_value(block.clone())
                    .map_err(|e| invalid(op, &format!("bad block: {e}")))?;
                assign_block_ids(&mut new_block, self.ids);
                let pos = if let Some(b) = before {
                    ir.block_index(b).ok_or_else(|| invalid(op, "before block not found"))?
                } else if let Some(a) = after {
                    let i = ir.block_index(a).ok_or_else(|| invalid(op, "after block not found"))?;
                    i + 1
                } else {
                    ir.document.blocks.len()
                };
                ir.document.blocks.insert(pos, new_block);
            }
            PatchOp::DeleteBlock { block_id } => {
                let i = ir.block_index(block_id).ok_or_else(|| invalid(op, "block not found"))?;
                ir.document.blocks.remove(i);
            }
            PatchOp::ReplaceBlock { block_id, block } => {
                let i = ir.block_index(block_id).ok_or_else(|| invalid(op, "block not found"))?;
                let mut new_block: Block = serde_json::from_value(block.clone())
                    .map_err(|e| invalid(op, &format!("bad block: {e}")))?;
                set_block_id(&mut new_block, block_id);
                ir.document.blocks[i] = new_block;
            }
            PatchOp::MoveBlock { block_id, after_block_id } => {
                let src = ir.block_index(block_id).ok_or_else(|| invalid(op, "block not found"))?;
                let block = ir.document.blocks.remove(src);
                let dst_ref = ir.block_index(after_block_id)
                    .ok_or_else(|| invalid(op, "target not found"))?;
                ir.document.blocks.insert(dst_ref + 1, block);
            }
            PatchOp::SetHeadingLevel { block_id, level } => {
                let b = ir.block_mut(block_id).ok_or_else(|| invalid(op, "block not found"))?;
                if let Block::Heading { level: l, .. } = b { *l = *level; }
                else { return Err(invalid(op, "not a heading")); }
            }
            PatchOp::ReplaceRunText { block_id, run_id, new_text } => {
                let b = ir.block_mut(block_id).ok_or_else(|| invalid(op, "block not found"))?;
                let run = find_run_mut(b, run_id)
                    .ok_or_else(|| invalid(op, "run not found"))?;
                run.text = new_text.clone();
            }
            PatchOp::SetRunStyle { block_id, run_id, style_patch } => {
                let b = ir.block_mut(block_id).ok_or_else(|| invalid(op, "block not found"))?;
                let run = find_run_mut(b, run_id)
                    .ok_or_else(|| invalid(op, "run not found"))?;
                if let Some(obj) = style_patch.as_object() {
                    if let Some(v) = obj.get("bold") { run.bold = v.as_bool(); }
                    if let Some(v) = obj.get("italic") { run.italic = v.as_bool(); }
                    if let Some(v) = obj.get("underline") { run.underline = v.as_bool(); }
                    if let Some(v) = obj.get("size") { run.size = v.as_f64(); }
                    if let Some(v) = obj.get("color") { run.color = v.as_str().map(|s| s.to_string()); }
                }
            }
            PatchOp::InsertRun { block_id, at_index, run } => {
                let b = ir.block_mut(block_id).ok_or_else(|| invalid(op, "block not found"))?;
                let mut new_run: Run = serde_json::from_value(run.clone())
                    .map_err(|e| invalid(op, &format!("bad run: {e}")))?;
                new_run.id = self.ids.new_run_id();
                match b {
                    Block::Paragraph { runs, .. } | Block::Heading { runs, .. } => {
                        let pos = (*at_index as usize).min(runs.len());
                        runs.insert(pos, new_run);
                    }
                    _ => return Err(invalid(op, "block doesn't support runs directly")),
                }
            }
            PatchOp::DeleteRun { block_id, run_id } => {
                let b = ir.block_mut(block_id).ok_or_else(|| invalid(op, "block not found"))?;
                match b {
                    Block::Paragraph { runs, .. } | Block::Heading { runs, .. } => {
                        let i = runs.iter().position(|r| &r.id == run_id)
                            .ok_or_else(|| invalid(op, "run not found"))?;
                        runs.remove(i);
                    }
                    _ => return Err(invalid(op, "block doesn't support runs directly")),
                }
            }
            PatchOp::InsertListItem { list_block_id, at_index, runs } => {
                let b = ir.block_mut(list_block_id).ok_or_else(|| invalid(op, "list not found"))?;
                let Block::List { items, .. } = b else { return Err(invalid(op, "not a list")); };
                let mut new_runs: Vec<Run> = Vec::new();
                for r in runs {
                    let mut run: Run = serde_json::from_value(r.clone())
                        .map_err(|e| invalid(op, &format!("bad run: {e}")))?;
                    run.id = self.ids.new_run_id();
                    new_runs.push(run);
                }
                let pos = (*at_index as usize).min(items.len());
                items.insert(pos, ListItem { id: self.ids.new_list_item_id(), runs: new_runs });
            }
            PatchOp::ReplaceListItem { list_block_id, item_id, runs } => {
                let b = ir.block_mut(list_block_id).ok_or_else(|| invalid(op, "list not found"))?;
                let Block::List { items, .. } = b else { return Err(invalid(op, "not a list")); };
                let it = items.iter_mut().find(|i| &i.id == item_id)
                    .ok_or_else(|| invalid(op, "item not found"))?;
                let mut new_runs: Vec<Run> = Vec::new();
                for r in runs {
                    let mut run: Run = serde_json::from_value(r.clone())
                        .map_err(|e| invalid(op, &format!("bad run: {e}")))?;
                    run.id = self.ids.new_run_id();
                    new_runs.push(run);
                }
                it.runs = new_runs;
            }
            PatchOp::DeleteListItem { list_block_id, item_id } => {
                let b = ir.block_mut(list_block_id).ok_or_else(|| invalid(op, "list not found"))?;
                let Block::List { items, .. } = b else { return Err(invalid(op, "not a list")); };
                items.retain(|i| &i.id != item_id);
            }
            PatchOp::InsertTableRow { table_block_id, before, after, cells } => {
                let b = ir.block_mut(table_block_id).ok_or_else(|| invalid(op, "table not found"))?;
                let Block::Table { rows, .. } = b else { return Err(invalid(op, "not a table")); };
                let mut new_cells: Vec<TableCell> = Vec::new();
                for c in cells {
                    let mut cell: TableCell = serde_json::from_value(c.clone())
                        .map_err(|e| invalid(op, &format!("bad cell: {e}")))?;
                    for run in cell.runs.iter_mut() { run.id = self.ids.new_run_id(); }
                    new_cells.push(cell);
                }
                let row = TableRow { id: self.ids.new_row_id(), cells: new_cells };
                let pos = if let Some(b) = before {
                    rows.iter().position(|r| &r.id == b)
                        .ok_or_else(|| invalid(op, "before row not found"))?
                } else if let Some(a) = after {
                    let i = rows.iter().position(|r| &r.id == a)
                        .ok_or_else(|| invalid(op, "after row not found"))?;
                    i + 1
                } else { rows.len() };
                rows.insert(pos, row);
            }
            PatchOp::DeleteTableRow { table_block_id, row_id } => {
                let b = ir.block_mut(table_block_id).ok_or_else(|| invalid(op, "table not found"))?;
                let Block::Table { rows, .. } = b else { return Err(invalid(op, "not a table")); };
                rows.retain(|r| &r.id != row_id);
            }
            PatchOp::UpdateTableCell { table_block_id, row_id, col_index, runs } => {
                let b = ir.block_mut(table_block_id).ok_or_else(|| invalid(op, "table not found"))?;
                let Block::Table { rows, .. } = b else { return Err(invalid(op, "not a table")); };
                let row = rows.iter_mut().find(|r| &r.id == row_id)
                    .ok_or_else(|| invalid(op, "row not found"))?;
                let cell = row.cells.get_mut(*col_index as usize)
                    .ok_or_else(|| invalid(op, "column out of range"))?;
                let mut new_runs: Vec<Run> = Vec::new();
                for r in runs {
                    let mut run: Run = serde_json::from_value(r.clone())
                        .map_err(|e| invalid(op, &format!("bad run: {e}")))?;
                    run.id = self.ids.new_run_id();
                    new_runs.push(run);
                }
                cell.runs = new_runs;
            }

            // Excel ops are no-ops here; the caller dispatches by kind.
            other => return Err(invalid(other, "not a Word op")),
        }
        Ok(())
    }
}

fn invalid(op: &PatchOp, reason: &str) -> DocumentError {
    DocumentError::InvalidPatchOp {
        reason: reason.to_string(),
        op: serde_json::to_value(op).unwrap_or(serde_json::Value::Null),
    }
}

fn assign_block_ids(block: &mut Block, ids: &dyn IdGenerator) {
    match block {
        Block::Heading { id, runs, .. } | Block::Paragraph { id, runs } => {
            *id = ids.new_block_id();
            for r in runs { r.id = ids.new_run_id(); }
        }
        Block::List { id, items, .. } => {
            *id = ids.new_block_id();
            for it in items {
                it.id = ids.new_list_item_id();
                for r in it.runs.iter_mut() { r.id = ids.new_run_id(); }
            }
        }
        Block::Table { id, rows } => {
            *id = ids.new_block_id();
            for row in rows {
                row.id = ids.new_row_id();
                for cell in row.cells.iter_mut() {
                    for r in cell.runs.iter_mut() { r.id = ids.new_run_id(); }
                }
            }
        }
    }
}

fn set_block_id(block: &mut Block, new_id: &str) {
    match block {
        Block::Heading { id, .. } | Block::Paragraph { id, .. }
        | Block::List { id, .. } | Block::Table { id, .. } => *id = new_id.to_string(),
    }
}

fn find_run_mut<'a>(block: &'a mut Block, run_id: &str) -> Option<&'a mut Run> {
    match block {
        Block::Heading { runs, .. } | Block::Paragraph { runs, .. } => {
            runs.iter_mut().find(|r| r.id == run_id)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ir::{Block, Run};
    use crate::documents::infrastructure::ids::CountingIdGenerator;

    fn base_ir() -> WordIR {
        let mut ir = WordIR::empty("x", "v1");
        ir.document.blocks.push(Block::Paragraph {
            id: "b1".into(),
            runs: vec![Run { id: "r1".into(), text: "hello".into(), bold: None, italic: None, underline: None, size: None, color: None }],
        });
        ir
    }

    #[test]
    fn replace_run_text_updates() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = base_ir();
        applier.apply(&mut ir, &PatchOp::ReplaceRunText {
            block_id: "b1".into(), run_id: "r1".into(), new_text: "world".into(),
        }).unwrap();
        if let Block::Paragraph { runs, .. } = &ir.document.blocks[0] {
            assert_eq!(runs[0].text, "world");
        } else { panic!(); }
    }

    #[test]
    fn insert_block_assigns_server_id() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = base_ir();
        applier.apply(&mut ir, &PatchOp::InsertBlock {
            before: None, after: Some("b1".into()),
            block: serde_json::json!({
                "type": "paragraph",
                "id": "CLIENT_SHOULD_IGNORE",
                "runs": [{"id": "ignored", "text": "new"}]
            }),
        }).unwrap();
        assert_eq!(ir.document.blocks.len(), 2);
        assert!(ir.document.blocks[1].id().starts_with("blk_"));
    }
}
```

- [ ] **Step 2: Wire Word into ApplyPatchUseCase**

Edit `src/libs/colmena/src/documents/application/apply_patch.rs`. Replace the Word error-return branch:

```rust
            ArtifactKind::Word => Err(DocumentError::InvalidPatchOp {
                reason: "Word patches not yet implemented (Phase B)".into(),
                op: serde_json::Value::Null,
            }),
```

with:

```rust
            ArtifactKind::Word => {
                use crate::documents::application::apply_word_ops::WordOpApplier;
                use crate::documents::domain::ir::WordIR;

                let mut ir: WordIR = serde_json::from_value(current_data.ir.clone())
                    .map_err(|e| DocumentError::IRValidationFailed {
                        path: "/".into(),
                        reason: format!("parse current Word IR: {e}"),
                    })?;
                let applier = WordOpApplier { ids: self.ids.as_ref() };
                for op in &input.patch.ops {
                    applier.apply(&mut ir, op)?;
                }
                let new_version = current.next();
                ir.version_id = new_version.0.clone();
                let ir_value = serde_json::to_value(&ir).unwrap();
                self.word_validator.validate(&ir_value)?;
                let rendered = self.word_renderer.render(&ir_value).await?;

                let patch_applied = PatchApplied {
                    patch: serde_json::to_value(&input.patch).unwrap(),
                    applied_at: Utc::now(),
                    resulted_in: new_version.clone(),
                    summary: PatchSummary::default(),
                };
                let version_data = VersionData {
                    ir: ir_value,
                    rendered_binary: rendered,
                    rendered_extension: "docx",
                    patch_applied,
                    blobs: vec![],
                };
                self.store.write_version(&artifact_id, &new_version, &version_data).await?;
                self.store.set_head(&artifact_id, Some(&current), &new_version).await?;

                let mut new_meta = meta.clone();
                new_meta.current_version = new_version.clone();
                new_meta.updated_at = Utc::now();
                self.store.update_meta(&artifact_id, &new_meta).await?;

                Ok(ApplyPatchOutput { version_id: new_version, summary: PatchSummary::default() })
            }
```

- [ ] **Step 3: Register module**

Edit `src/libs/colmena/src/documents/application/mod.rs` — append:

```rust
pub mod apply_word_ops;
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib documents::application::apply_word_ops`
Expected: 2 tests pass.

Also: `cargo test --lib documents` (full documents test suite)
Expected: all existing tests still pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/application/
git commit -m "$(cat <<'EOF'
feat(documents): add Word PatchOp applier and wire Word through ApplyPatchUseCase

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task B6: Word end-to-end integration test

**Files:**
- Create: `tests/documents_word_cycle.rs`

- [ ] **Step 1: Write integration test**

Create `tests/documents_word_cycle.rs`:

```rust
use colmena::documents::application::apply_patch::{ApplyPatchInput, ApplyPatchUseCase};
use colmena::documents::application::create_document::{CreateDocumentInput, CreateDocumentUseCase};
use colmena::documents::application::read_document::{ReadDocumentInput, ReadDocumentUseCase};
use colmena::documents::domain::ids::{ArtifactKind, SessionId};
use colmena::documents::domain::patch::{Patch, PatchOp, PatchSource};
use colmena::documents::domain::{ArtifactStore, IRRenderer, IRValidator};
use colmena::documents::infrastructure::ids::CountingIdGenerator;
use colmena::documents::infrastructure::render::{ExcelRenderer, WordRenderer};
use colmena::documents::infrastructure::storage::LocalFsStore;
use colmena::documents::infrastructure::validation::{ExcelValidator, WordValidator};
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn word_create_replace_run_text() {
    let tmp = tempdir().unwrap();
    let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp.path()));
    let ids = Arc::new(CountingIdGenerator::default());

    let create = CreateDocumentUseCase {
        store: store.clone(),
        excel_renderer: Arc::new(ExcelRenderer),
        excel_validator: Arc::new(ExcelValidator),
        word_renderer: Arc::new(WordRenderer),
        word_validator: Arc::new(WordValidator),
        ids: ids.clone(),
        default_retention: 10,
    };
    let apply = ApplyPatchUseCase {
        store: store.clone(),
        excel_renderer: Arc::new(ExcelRenderer),
        excel_validator: Arc::new(ExcelValidator),
        word_renderer: Arc::new(WordRenderer),
        word_validator: Arc::new(WordValidator),
        ids: ids.clone(),
    };
    let read = ReadDocumentUseCase { store: store.clone() };

    let created = create.execute(CreateDocumentInput {
        kind: ArtifactKind::Word,
        session_id: SessionId::new("s"),
        label: Some("Report".into()),
        retention_limit: None,
        initial_ir: Some(serde_json::json!({
            "kind": "word",
            "artifact_id": "x", "version_id": "v1",
            "schema_version": "1.0.0",
            "document": {
                "blocks": [
                    {"type": "paragraph", "id": "b1", "runs": [{"id": "r1", "text": "old"}]}
                ],
                "named_styles": {}
            }
        })),
        source: PatchSource::Agent,
    }).await.unwrap();

    apply.execute(ApplyPatchInput {
        patch: Patch {
            artifact_id: created.artifact_id.0.clone(),
            base_version: "v1".into(),
            source: PatchSource::Agent,
            ops: vec![PatchOp::ReplaceRunText {
                block_id: "b1".into(), run_id: "r1".into(), new_text: "new".into(),
            }],
        },
    }).await.unwrap();

    let r = read.execute(ReadDocumentInput {
        artifact_id: created.artifact_id, version: None,
    }).await.unwrap();
    assert_eq!(r.ir["document"]["blocks"][0]["runs"][0]["text"], "new");
}
```

- [ ] **Step 2: Run integration test**

Run: `cargo test --test documents_word_cycle`
Expected: 1 test passes.

- [ ] **Step 3: Commit**

```bash
git add tests/documents_word_cycle.rs
git commit -m "$(cat <<'EOF'
test(documents): add Word end-to-end integration test (create + replace_run_text + read)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

**End of Phase B.** Both Excel and Word are functional end-to-end.

---

## Phases C–E (deferred)

Phases C (session-artifact index in SQLite/Postgres), D (ConflictDetector + RebaseService for concurrent agent/user edits), and E (GCS backend + DAG nodes + `document_authoring` skill + developer guide) are part of the full v1 scope in the design spec but are **deferred to a follow-up plan**. They're additive on top of Phases A+B without breaking changes:

- **Phase C** adds a new trait `SessionArtifactIndex` with SQLite/Postgres/InMemory adapters, embedded migrations, and wires it into the existing use cases for session isolation and `list_my_artifacts`.
- **Phase D** adds `ConflictDetector` + `RebaseService`, upgrades `ApplyPatchUseCase` to auto-rebase instead of fast-failing on stale `base_version`, and adds `DiffService` for user-edit narration.
- **Phase E** adds `GcsStore` (google-cloud-storage crate), `DocumentStorageConfig` factory, three DAG nodes (`document_create`, `document_edit`, `document_read`), remaining synthetic tools (`rollback`, `list_versions`, `list_my_artifacts`, `get_head`), `DownloadArtifactUseCase` + HTTP endpoints in serve mode, skill markdown files, and docs updates.

When Phases A+B are complete and validated, open a follow-up plan document that picks up where this one ends.





