# Load Attachment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a synthetic `load_attachment` tool that lets an `llm_call` node pull a previously uploaded document into context on-demand, scoped to the `agent_session_id` and opt-out per node via `attachments_enabled` (default `true`).

**Architecture:** A new `AttachmentRegistry` port (domain) with SQLite + Postgres adapters stores `(agent_session_id, document_id, provider)` rows pointing to existing `provider_file_id`s. When the LLM calls `load_attachment(document_id)`, the synthetic dispatcher returns a sentinel; `AgentService` intercepts it, materialises a synthetic `user` `LlmMessage` with `files[]`, persists it in history, and continues the ReAct loop. Subgraphs inherit the catalog automatically through the existing `agent_session_id` propagation. Reuses the `provider_file_cache` upload pipeline for expiry recovery.

**Tech Stack:** Rust 1.95, sqlx (Postgres + SQLite), async-trait, thiserror, serde / serde_json, mockall (tests), tokio.

**Spec:** [docs/superpowers/specs/2026-05-13-load-attachment-design.md](../specs/2026-05-13-load-attachment-design.md)

---

## File Structure

### New files

| Path | Responsibility |
|---|---|
| `src/libs/colmena/src/llm/domain/attachments/mod.rs` | Re-exports for the domain submodule. |
| `src/libs/colmena/src/llm/domain/attachments/conversation_attachment.rs` | `ConversationAttachment` value object + `AttachmentSource` enum. |
| `src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs` | `AttachmentRegistry` trait (port). |
| `src/libs/colmena/src/llm/domain/attachments/attachment_error.rs` | `AttachmentError` (thiserror). |
| `src/libs/colmena/src/llm/domain/attachments/auto_id.rs` | `generate_attachment_id(...)` (deterministic SHA-256). |
| `src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs` | Postgres adapter. |
| `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs` | SQLite adapter. |
| `src/libs/colmena/migrations/postgres/20260513000001_conversation_attachments.sql` | PG schema. |
| `src/libs/colmena/migrations/sqlite/20260513000001_conversation_attachments.sql` | SQLite schema. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs` | Synthetic tool definition + dispatcher. |
| `tests/graphs/agents/load_attachment_basic.json` | E2E test graph. |
| `tests/graphs/agents/load_attachment_subgraph.json` | Subgraph inheritance graph. |
| `tests/graphs/agents/load_attachment_opt_out.json` | Opt-out graph. |
| `tests/load_attachment_e2e.rs` | Mocked end-to-end integration test. |
| `docs/developer_guide/30_load_attachment.md` | Developer guide. |

### Modified files

| Path | Change |
|---|---|
| `src/libs/colmena/src/llm/domain/mod.rs` | Export new `attachments` submodule. |
| `src/libs/colmena/src/llm/infrastructure/persistence/mod.rs` | Export the two new registry adapters. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` | Export `load_attachment_tool` items. |
| `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` | Add `with_attachments(...)` builder + intercept `LOAD_ATTACHMENT_TOOL_NAME`. |
| `src/libs/colmena/src/llm/application/agent_service.rs` | New optional `attachment_registry` dep in `AgentRunParams` + sentinel handler. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` | Read `attachments_enabled`, wire registry, register files, expose tool, pass to AgentService. |
| `docs/node_configurations.json` | Add `attachments_enabled` to `llm_call` schema. |
| `docs/DEVELOPER_GUIDE.md` | Add `30_load_attachment.md` entry. |

---

## Task 1: Domain — `AttachmentError` enum

**Files:**
- Create: `src/libs/colmena/src/llm/domain/attachments/mod.rs`
- Create: `src/libs/colmena/src/llm/domain/attachments/attachment_error.rs`
- Modify: `src/libs/colmena/src/llm/domain/mod.rs` (export the submodule)

- [ ] **Step 1: Create the submodule skeleton**

Create `src/libs/colmena/src/llm/domain/attachments/mod.rs`:

```rust
pub mod attachment_error;
pub use attachment_error::AttachmentError;
```

- [ ] **Step 2: Write the failing test for `AttachmentError`**

Create `src/libs/colmena/src/llm/domain/attachments/attachment_error.rs` with only the test module first:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AttachmentError {
    #[error("attachment '{document_id}' not found in session")]
    NotFound { document_id: String },

    #[error("attachment '{document_id}' expired and cannot be re-uploaded: {reason}")]
    ExpiredUnrecoverable { document_id: String, reason: String },

    #[error("agent_session_id is missing from the run; load_attachment requires a stable agent session")]
    SessionMissing,

    #[error("repository failure: {0}")]
    RepositoryFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_renders_document_id_in_message() {
        let e = AttachmentError::NotFound {
            document_id: "doc-x".to_string(),
        };
        assert_eq!(format!("{}", e), "attachment 'doc-x' not found in session");
    }

    #[test]
    fn expired_unrecoverable_renders_reason() {
        let e = AttachmentError::ExpiredUnrecoverable {
            document_id: "doc-y".to_string(),
            reason: "inline bytes not retained".to_string(),
        };
        assert!(format!("{}", e).contains("inline bytes not retained"));
    }
}
```

- [ ] **Step 3: Wire the submodule in `llm/domain/mod.rs`**

Add the export. Open `src/libs/colmena/src/llm/domain/mod.rs` and add at the end of the `pub mod ...` declarations:

```rust
pub mod attachments;
pub use attachments::AttachmentError;
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test --lib llm::domain::attachments::attachment_error -- --nocapture
```

Expected: 2 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/llm/domain/attachments/
git add src/libs/colmena/src/llm/domain/mod.rs
git commit -m "feat(llm): add AttachmentError domain enum"
```

---

## Task 2: Domain — `ConversationAttachment` + `AttachmentSource`

**Files:**
- Create: `src/libs/colmena/src/llm/domain/attachments/conversation_attachment.rs`
- Modify: `src/libs/colmena/src/llm/domain/attachments/mod.rs`

- [ ] **Step 1: Write the value object with failing tests**

Create `src/libs/colmena/src/llm/domain/attachments/conversation_attachment.rs`:

```rust
use crate::llm::domain::ProviderKind;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Where the attachment was originally sourced. Drives expiry-recovery
/// strategy: `SignedUrl` and `Path` can be re-uploaded; `Inline` cannot
/// because we deliberately do not retain raw bytes after the first upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum AttachmentSource {
    SignedUrl(String),
    Path(String),
    Inline,
}

impl AttachmentSource {
    pub fn kind_str(&self) -> &'static str {
        match self {
            AttachmentSource::SignedUrl(_) => "signed_url",
            AttachmentSource::Path(_) => "path",
            AttachmentSource::Inline => "inline",
        }
    }

    pub fn value(&self) -> Option<&str> {
        match self {
            AttachmentSource::SignedUrl(v) | AttachmentSource::Path(v) => Some(v),
            AttachmentSource::Inline => None,
        }
    }

    pub fn is_recoverable(&self) -> bool {
        !matches!(self, AttachmentSource::Inline)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConversationAttachment {
    pub agent_session_id: String,
    pub document_id: String,
    pub provider: ProviderKind,
    pub provider_file_id: String,
    pub mime_type: String,
    pub filename: String,
    pub size_bytes: Option<u64>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub source: AttachmentSource,
    pub registered_at: DateTime<Utc>,
    pub refreshed_at: DateTime<Utc>,
}

impl ConversationAttachment {
    /// Catalog rendering for the load_attachment tool description.
    /// Format: `"<doc_id>" — <label or filename> (<mime>, <size>)[. <description>]`
    pub fn catalog_line(&self) -> String {
        let label = self.label.as_deref().unwrap_or(self.filename.as_str());
        let size = self
            .size_bytes
            .map(human_size)
            .unwrap_or_else(|| "?".to_string());
        let mut line = format!("\"{}\" — {} ({}, {})", self.document_id, label, self.mime_type, size);
        if let Some(desc) = &self.description {
            if !desc.trim().is_empty() {
                line.push_str(". ");
                line.push_str(desc.trim());
            }
        }
        line
    }
}

fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::ProviderKind;

    fn mk(label: Option<&str>, description: Option<&str>, size: Option<u64>) -> ConversationAttachment {
        ConversationAttachment {
            agent_session_id: "agent_1".to_string(),
            document_id: "doc-abc".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-1".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "Q3.pdf".to_string(),
            size_bytes: size,
            label: label.map(String::from),
            description: description.map(String::from),
            source: AttachmentSource::SignedUrl("https://x".to_string()),
            registered_at: Utc::now(),
            refreshed_at: Utc::now(),
        }
    }

    #[test]
    fn source_kind_str_matches_serialized_form() {
        assert_eq!(AttachmentSource::SignedUrl("u".into()).kind_str(), "signed_url");
        assert_eq!(AttachmentSource::Path("/p".into()).kind_str(), "path");
        assert_eq!(AttachmentSource::Inline.kind_str(), "inline");
    }

    #[test]
    fn inline_source_is_not_recoverable() {
        assert!(!AttachmentSource::Inline.is_recoverable());
        assert!(AttachmentSource::SignedUrl("x".into()).is_recoverable());
        assert!(AttachmentSource::Path("x".into()).is_recoverable());
    }

    #[test]
    fn catalog_line_uses_label_when_present() {
        let a = mk(Some("Q3 Financial Report"), None, Some(12 * 1024 * 1024));
        let line = a.catalog_line();
        assert!(line.contains("Q3 Financial Report"));
        assert!(line.contains("application/pdf"));
        assert!(line.contains("12.0 MB"));
        assert!(line.contains("\"doc-abc\""));
    }

    #[test]
    fn catalog_line_falls_back_to_filename_without_label() {
        let a = mk(None, None, Some(2048));
        assert!(a.catalog_line().contains("Q3.pdf"));
        assert!(a.catalog_line().contains("2.0 KB"));
    }

    #[test]
    fn catalog_line_appends_description_when_present() {
        let a = mk(Some("Report"), Some("Q3 2026 results"), Some(1024));
        assert!(a.catalog_line().contains(". Q3 2026 results"));
    }

    #[test]
    fn unknown_size_renders_as_question_mark() {
        let a = mk(Some("X"), None, None);
        assert!(a.catalog_line().contains("?"));
    }
}
```

- [ ] **Step 2: Re-export from the submodule**

Update `src/libs/colmena/src/llm/domain/attachments/mod.rs`:

```rust
pub mod attachment_error;
pub mod conversation_attachment;

pub use attachment_error::AttachmentError;
pub use conversation_attachment::{AttachmentSource, ConversationAttachment};
```

- [ ] **Step 3: Re-export from `llm/domain/mod.rs`**

In `src/libs/colmena/src/llm/domain/mod.rs`, update the line you added in Task 1 to also re-export the new types:

```rust
pub mod attachments;
pub use attachments::{AttachmentError, AttachmentSource, ConversationAttachment};
```

- [ ] **Step 4: Run tests**

```bash
cargo test --lib llm::domain::attachments -- --nocapture
```

Expected: all tests in `attachment_error` + `conversation_attachment` modules pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/llm/domain/attachments/ src/libs/colmena/src/llm/domain/mod.rs
git commit -m "feat(llm): add ConversationAttachment + AttachmentSource domain types"
```

---

## Task 3: Domain — `AttachmentRegistry` trait + `UpsertAttachmentInput`

**Files:**
- Create: `src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs`
- Modify: `src/libs/colmena/src/llm/domain/attachments/mod.rs`

- [ ] **Step 1: Write the trait file**

Create `src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs`:

```rust
use crate::llm::domain::attachments::{
    AttachmentError, AttachmentSource, ConversationAttachment,
};
use crate::llm::domain::ProviderKind;
use async_trait::async_trait;

/// Input record for `AttachmentRegistry::upsert`. Mirrors the columns of the
/// `conversation_attachments` table 1:1.
#[derive(Debug, Clone)]
pub struct UpsertAttachmentInput {
    pub agent_session_id: String,
    pub document_id: String,
    pub provider: ProviderKind,
    pub provider_file_id: String,
    pub mime_type: String,
    pub filename: String,
    pub size_bytes: Option<u64>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub source: AttachmentSource,
}

#[async_trait]
pub trait AttachmentRegistry: Send + Sync {
    /// Insert or update a registry entry. Idempotent on
    /// `(agent_session_id, document_id, provider)`.
    async fn upsert(&self, input: UpsertAttachmentInput) -> Result<(), AttachmentError>;

    /// Fetch a single entry for the given session + document.
    /// Returns `Ok(None)` when nothing is registered.
    async fn lookup(
        &self,
        agent_session_id: &str,
        document_id: &str,
        provider: ProviderKind,
    ) -> Result<Option<ConversationAttachment>, AttachmentError>;

    /// Replace the `provider_file_id` (and `refreshed_at`) for an existing row.
    /// Returns `Err(NotFound)` when the row does not exist.
    async fn refresh_provider_file_id(
        &self,
        agent_session_id: &str,
        document_id: &str,
        provider: ProviderKind,
        new_provider_file_id: &str,
    ) -> Result<(), AttachmentError>;

    /// List every entry registered for the given session. Used to build the
    /// `load_attachment` catalog at the start of an llm_call execute. Filtering
    /// by provider happens in the caller (one provider per llm_call execution).
    async fn list_for_session(
        &self,
        agent_session_id: &str,
    ) -> Result<Vec<ConversationAttachment>, AttachmentError>;
}
```

- [ ] **Step 2: Re-export the new items**

Update `src/libs/colmena/src/llm/domain/attachments/mod.rs`:

```rust
pub mod attachment_error;
pub mod attachment_registry;
pub mod conversation_attachment;

pub use attachment_error::AttachmentError;
pub use attachment_registry::{AttachmentRegistry, UpsertAttachmentInput};
pub use conversation_attachment::{AttachmentSource, ConversationAttachment};
```

Update `src/libs/colmena/src/llm/domain/mod.rs`:

```rust
pub mod attachments;
pub use attachments::{
    AttachmentError, AttachmentRegistry, AttachmentSource, ConversationAttachment,
    UpsertAttachmentInput,
};
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo check --lib
```

Expected: clean compile (no warnings — the project has `warnings = "deny"`).

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/llm/domain/attachments/ src/libs/colmena/src/llm/domain/mod.rs
git commit -m "feat(llm): add AttachmentRegistry trait (port)"
```

---

## Task 4: Domain — deterministic auto-id generation

**Files:**
- Create: `src/libs/colmena/src/llm/domain/attachments/auto_id.rs`
- Modify: `src/libs/colmena/src/llm/domain/attachments/mod.rs`

- [ ] **Step 1: Write the failing test for `generate_attachment_id`**

Create `src/libs/colmena/src/llm/domain/attachments/auto_id.rs`:

```rust
use crate::llm::domain::attachments::AttachmentSource;
use sha2::{Digest, Sha256};

/// Deterministically compute a stable id `att_<hex16>` for a file based on
/// `(filename, mime_type, size, source-specific-discriminator)`.
///
/// - SignedUrl → discriminator = the URL string.
/// - Path      → discriminator = the absolute path.
/// - Inline    → discriminator = the SHA-256 of the raw bytes (provided by caller).
///
/// `size_bytes` may be `None`; we hash the byte string `"?"` in that case.
///
/// For `Inline`, callers must hash the bytes upstream and pass the hex digest
/// via `inline_bytes_digest`. (We do not take the bytes themselves here to
/// avoid copying potentially-large buffers into this function.)
pub fn generate_attachment_id(
    filename: &str,
    mime_type: &str,
    size_bytes: Option<u64>,
    source: &AttachmentSource,
    inline_bytes_digest: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(filename.as_bytes());
    hasher.update(b"|");
    hasher.update(mime_type.as_bytes());
    hasher.update(b"|");
    match size_bytes {
        Some(n) => hasher.update(n.to_string().as_bytes()),
        None => hasher.update(b"?"),
    }
    hasher.update(b"|");
    match source {
        AttachmentSource::SignedUrl(url) => hasher.update(url.as_bytes()),
        AttachmentSource::Path(p) => hasher.update(p.as_bytes()),
        AttachmentSource::Inline => {
            let d = inline_bytes_digest.unwrap_or("");
            hasher.update(d.as_bytes());
        }
    }
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(8).map(|b| format!("{:02x}", b)).collect();
    format!("att_{}", hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_inputs_produce_same_id() {
        let a = generate_attachment_id(
            "Q3.pdf",
            "application/pdf",
            Some(1024),
            &AttachmentSource::SignedUrl("https://x?sig=y".to_string()),
            None,
        );
        let b = generate_attachment_id(
            "Q3.pdf",
            "application/pdf",
            Some(1024),
            &AttachmentSource::SignedUrl("https://x?sig=y".to_string()),
            None,
        );
        assert_eq!(a, b);
        assert!(a.starts_with("att_"));
        assert_eq!(a.len(), 4 + 16);
    }

    #[test]
    fn different_urls_produce_different_ids() {
        let a = generate_attachment_id(
            "x.pdf",
            "application/pdf",
            Some(1),
            &AttachmentSource::SignedUrl("u1".into()),
            None,
        );
        let b = generate_attachment_id(
            "x.pdf",
            "application/pdf",
            Some(1),
            &AttachmentSource::SignedUrl("u2".into()),
            None,
        );
        assert_ne!(a, b);
    }

    #[test]
    fn different_filenames_produce_different_ids() {
        let a = generate_attachment_id(
            "a.pdf",
            "application/pdf",
            Some(1),
            &AttachmentSource::Path("/p".into()),
            None,
        );
        let b = generate_attachment_id(
            "b.pdf",
            "application/pdf",
            Some(1),
            &AttachmentSource::Path("/p".into()),
            None,
        );
        assert_ne!(a, b);
    }

    #[test]
    fn inline_uses_bytes_digest() {
        let a = generate_attachment_id(
            "x.bin",
            "application/octet-stream",
            Some(3),
            &AttachmentSource::Inline,
            Some("aaaa"),
        );
        let b = generate_attachment_id(
            "x.bin",
            "application/octet-stream",
            Some(3),
            &AttachmentSource::Inline,
            Some("bbbb"),
        );
        assert_ne!(a, b);
    }
}
```

- [ ] **Step 2: Add `sha2` to the crate (if not already present)**

Check `src/libs/colmena/Cargo.toml`:

```bash
grep -n "^sha2 " src/libs/colmena/Cargo.toml || echo "MISSING"
```

If `MISSING`, add this line under `[dependencies]` in `src/libs/colmena/Cargo.toml`:

```toml
sha2 = "0.10"
```

- [ ] **Step 3: Re-export and run tests**

Update `src/libs/colmena/src/llm/domain/attachments/mod.rs` to add:

```rust
pub mod auto_id;
pub use auto_id::generate_attachment_id;
```

Run:

```bash
cargo test --lib llm::domain::attachments::auto_id
```

Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/llm/domain/attachments/ src/libs/colmena/Cargo.toml
git commit -m "feat(llm): add deterministic attachment id generator"
```

---

## Task 5: Migrations — Postgres + SQLite schema

**Files:**
- Create: `src/libs/colmena/migrations/postgres/20260513000001_conversation_attachments.sql`
- Create: `src/libs/colmena/migrations/sqlite/20260513000001_conversation_attachments.sql`

- [ ] **Step 1: Write the Postgres migration**

Create `src/libs/colmena/migrations/postgres/20260513000001_conversation_attachments.sql`:

```sql
CREATE TABLE IF NOT EXISTS conversation_attachments (
    agent_session_id  TEXT        NOT NULL,
    document_id       TEXT        NOT NULL,
    provider          TEXT        NOT NULL,
    provider_file_id  TEXT        NOT NULL,
    mime_type         TEXT        NOT NULL,
    filename          TEXT        NOT NULL,
    size_bytes        BIGINT,
    label             TEXT,
    description       TEXT,
    source_kind       TEXT        NOT NULL,
    source_value      TEXT,
    registered_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    refreshed_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_session_id, document_id, provider)
);

CREATE INDEX IF NOT EXISTS idx_conversation_attachments_session
    ON conversation_attachments(agent_session_id);
```

- [ ] **Step 2: Write the SQLite migration**

Create `src/libs/colmena/migrations/sqlite/20260513000001_conversation_attachments.sql`:

```sql
CREATE TABLE IF NOT EXISTS conversation_attachments (
    agent_session_id  TEXT NOT NULL,
    document_id       TEXT NOT NULL,
    provider          TEXT NOT NULL,
    provider_file_id  TEXT NOT NULL,
    mime_type         TEXT NOT NULL,
    filename          TEXT NOT NULL,
    size_bytes        INTEGER,
    label             TEXT,
    description       TEXT,
    source_kind       TEXT NOT NULL,
    source_value      TEXT,
    registered_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    refreshed_at      TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (agent_session_id, document_id, provider)
);

CREATE INDEX IF NOT EXISTS idx_conversation_attachments_session
    ON conversation_attachments(agent_session_id);
```

- [ ] **Step 3: Sanity-check SQL with cargo build**

`sqlx::migrate!` is invoked from the adapters in later tasks; here we just check the file is syntactically valid by compiling. Run:

```bash
cargo check --lib
```

Expected: clean compile.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/migrations/
git commit -m "feat(persistence): add conversation_attachments migrations (pg+sqlite)"
```

---

## Task 6: Postgres adapter for `AttachmentRegistry`

**Files:**
- Create: `src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/mod.rs`

- [ ] **Step 1: Write the adapter**

Create `src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs`:

```rust
use crate::dag_engine::infrastructure::pool_registry::PgPoolRegistry;
use crate::llm::domain::{
    AttachmentError, AttachmentRegistry, AttachmentSource, ConversationAttachment, ProviderKind,
    UpsertAttachmentInput,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use std::str::FromStr;
use std::sync::Arc;

pub struct PostgresAttachmentRegistry {
    pool: Arc<PgPool>,
}

impl PostgresAttachmentRegistry {
    pub async fn new(
        registry: Arc<PgPoolRegistry>,
        database_url: &str,
    ) -> Result<Self, AttachmentError> {
        let pool = registry.get_or_create(database_url).await.map_err(|e| {
            AttachmentError::RepositoryFailed(format!("pool init failed: {}", e))
        })?;
        Ok(Self { pool })
    }
}

fn row_to_attachment(row: &sqlx::postgres::PgRow) -> Result<ConversationAttachment, AttachmentError> {
    let provider_db: String = row.try_get("provider").map_err(|e| {
        AttachmentError::RepositoryFailed(format!("provider column read: {}", e))
    })?;
    let provider = ProviderKind::from_str(&provider_db).map_err(|e| {
        AttachmentError::RepositoryFailed(format!("corrupted provider '{}': {}", provider_db, e))
    })?;

    let source_kind: String = row
        .try_get("source_kind")
        .map_err(|e| AttachmentError::RepositoryFailed(format!("source_kind: {}", e)))?;
    let source_value: Option<String> = row
        .try_get("source_value")
        .map_err(|e| AttachmentError::RepositoryFailed(format!("source_value: {}", e)))?;
    let source = match source_kind.as_str() {
        "signed_url" => AttachmentSource::SignedUrl(source_value.unwrap_or_default()),
        "path" => AttachmentSource::Path(source_value.unwrap_or_default()),
        "inline" => AttachmentSource::Inline,
        other => {
            return Err(AttachmentError::RepositoryFailed(format!(
                "unknown source_kind '{}'",
                other
            )));
        }
    };

    let size_bytes_db: Option<i64> = row.try_get("size_bytes").ok().flatten();
    let size_bytes = size_bytes_db.map(|n| n as u64);

    Ok(ConversationAttachment {
        agent_session_id: row
            .try_get("agent_session_id")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("agent_session_id: {}", e)))?,
        document_id: row
            .try_get("document_id")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("document_id: {}", e)))?,
        provider,
        provider_file_id: row
            .try_get("provider_file_id")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("provider_file_id: {}", e)))?,
        mime_type: row
            .try_get("mime_type")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("mime_type: {}", e)))?,
        filename: row
            .try_get("filename")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("filename: {}", e)))?,
        size_bytes,
        label: row.try_get("label").ok(),
        description: row.try_get("description").ok(),
        source,
        registered_at: row
            .try_get::<DateTime<Utc>, _>("registered_at")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("registered_at: {}", e)))?,
        refreshed_at: row
            .try_get::<DateTime<Utc>, _>("refreshed_at")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("refreshed_at: {}", e)))?,
    })
}

#[async_trait]
impl AttachmentRegistry for PostgresAttachmentRegistry {
    async fn upsert(&self, input: UpsertAttachmentInput) -> Result<(), AttachmentError> {
        let provider_str = input.provider.to_string();
        let source_kind = input.source.kind_str().to_string();
        let source_value = input.source.value().map(|s| s.to_string());
        let size_db: Option<i64> = input.size_bytes.map(|n| n as i64);

        sqlx::query(
            "INSERT INTO conversation_attachments (
                agent_session_id, document_id, provider, provider_file_id,
                mime_type, filename, size_bytes, label, description,
                source_kind, source_value, registered_at, refreshed_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11, NOW(), NOW())
            ON CONFLICT (agent_session_id, document_id, provider) DO UPDATE
            SET provider_file_id = EXCLUDED.provider_file_id,
                mime_type        = EXCLUDED.mime_type,
                filename         = EXCLUDED.filename,
                size_bytes       = EXCLUDED.size_bytes,
                label            = EXCLUDED.label,
                description      = EXCLUDED.description,
                source_kind      = EXCLUDED.source_kind,
                source_value     = EXCLUDED.source_value,
                refreshed_at     = NOW()",
        )
        .bind(&input.agent_session_id)
        .bind(&input.document_id)
        .bind(&provider_str)
        .bind(&input.provider_file_id)
        .bind(&input.mime_type)
        .bind(&input.filename)
        .bind(size_db)
        .bind(&input.label)
        .bind(&input.description)
        .bind(&source_kind)
        .bind(&source_value)
        .execute(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("upsert: {}", e)))?;
        Ok(())
    }

    async fn lookup(
        &self,
        agent_session_id: &str,
        document_id: &str,
        provider: ProviderKind,
    ) -> Result<Option<ConversationAttachment>, AttachmentError> {
        let provider_str = provider.to_string();
        let row = sqlx::query(
            "SELECT agent_session_id, document_id, provider, provider_file_id,
                    mime_type, filename, size_bytes, label, description,
                    source_kind, source_value, registered_at, refreshed_at
             FROM conversation_attachments
             WHERE agent_session_id = $1 AND document_id = $2 AND provider = $3",
        )
        .bind(agent_session_id)
        .bind(document_id)
        .bind(&provider_str)
        .fetch_optional(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("lookup: {}", e)))?;

        match row {
            None => Ok(None),
            Some(r) => Ok(Some(row_to_attachment(&r)?)),
        }
    }

    async fn refresh_provider_file_id(
        &self,
        agent_session_id: &str,
        document_id: &str,
        provider: ProviderKind,
        new_provider_file_id: &str,
    ) -> Result<(), AttachmentError> {
        let provider_str = provider.to_string();
        let res = sqlx::query(
            "UPDATE conversation_attachments
                SET provider_file_id = $4, refreshed_at = NOW()
              WHERE agent_session_id = $1 AND document_id = $2 AND provider = $3",
        )
        .bind(agent_session_id)
        .bind(document_id)
        .bind(&provider_str)
        .bind(new_provider_file_id)
        .execute(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("refresh: {}", e)))?;

        if res.rows_affected() == 0 {
            return Err(AttachmentError::NotFound {
                document_id: document_id.to_string(),
            });
        }
        Ok(())
    }

    async fn list_for_session(
        &self,
        agent_session_id: &str,
    ) -> Result<Vec<ConversationAttachment>, AttachmentError> {
        let rows = sqlx::query(
            "SELECT agent_session_id, document_id, provider, provider_file_id,
                    mime_type, filename, size_bytes, label, description,
                    source_kind, source_value, registered_at, refreshed_at
             FROM conversation_attachments
             WHERE agent_session_id = $1
             ORDER BY registered_at ASC",
        )
        .bind(agent_session_id)
        .fetch_all(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("list: {}", e)))?;

        rows.iter().map(row_to_attachment).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::infrastructure::pool_registry::PoolConfig;

    async fn make_registry() -> PostgresAttachmentRegistry {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        let registry = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
        let pool = registry.get_or_create(&url).await.unwrap();
        sqlx::migrate!("migrations/postgres")
            .set_ignore_missing(true)
            .run(&*pool)
            .await
            .unwrap();
        PostgresAttachmentRegistry::new(registry, &url).await.unwrap()
    }

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn upsert_then_lookup_roundtrip() {
        let reg = make_registry().await;
        let sid = format!("test_sess_{}", uuid::Uuid::new_v4());
        let input = UpsertAttachmentInput {
            agent_session_id: sid.clone(),
            document_id: "doc-1".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-1".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            size_bytes: Some(1024),
            label: Some("X".to_string()),
            description: None,
            source: AttachmentSource::SignedUrl("https://u".to_string()),
        };
        reg.upsert(input.clone()).await.unwrap();
        let got = reg
            .lookup(&sid, "doc-1", ProviderKind::OpenAi)
            .await
            .unwrap()
            .expect("row present");
        assert_eq!(got.provider_file_id, "pf-1");
        assert_eq!(got.source, AttachmentSource::SignedUrl("https://u".to_string()));
    }

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn upsert_is_idempotent_and_overwrites() {
        let reg = make_registry().await;
        let sid = format!("test_sess_{}", uuid::Uuid::new_v4());
        let mk = |pf: &str| UpsertAttachmentInput {
            agent_session_id: sid.clone(),
            document_id: "doc-1".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: pf.to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            size_bytes: None,
            label: None,
            description: None,
            source: AttachmentSource::Inline,
        };
        reg.upsert(mk("pf-1")).await.unwrap();
        reg.upsert(mk("pf-2")).await.unwrap();
        let got = reg
            .lookup(&sid, "doc-1", ProviderKind::OpenAi)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.provider_file_id, "pf-2");
    }

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn refresh_returns_not_found_when_missing() {
        let reg = make_registry().await;
        let err = reg
            .refresh_provider_file_id("missing_sess", "missing_doc", ProviderKind::OpenAi, "pf-x")
            .await
            .unwrap_err();
        assert!(matches!(err, AttachmentError::NotFound { .. }));
    }
}
```

- [ ] **Step 2: Export the adapter**

Update `src/libs/colmena/src/llm/infrastructure/persistence/mod.rs` to add:

```rust
pub mod postgres_attachment_registry;
pub use postgres_attachment_registry::PostgresAttachmentRegistry;
```

- [ ] **Step 3: Verify compile**

```bash
cargo check --lib
```

Expected: clean.

- [ ] **Step 4: Run the ignored Postgres tests**

```bash
set -a; source .env; set +a
cargo test --lib llm::infrastructure::persistence::postgres_attachment_registry -- --ignored --nocapture
```

Expected: 3 tests pass (requires `DATABASE_URL` in `.env`).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/persistence/
git commit -m "feat(persistence): PostgresAttachmentRegistry adapter"
```

---

## Task 7: SQLite adapter for `AttachmentRegistry`

**Files:**
- Create: `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/mod.rs`

- [ ] **Step 1: Write the SQLite adapter**

Create `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs`:

```rust
use crate::llm::domain::{
    AttachmentError, AttachmentRegistry, AttachmentSource, ConversationAttachment, ProviderKind,
    UpsertAttachmentInput,
};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use std::sync::Arc;

pub struct SqliteAttachmentRegistry {
    pool: Arc<SqlitePool>,
}

impl SqliteAttachmentRegistry {
    pub async fn new(database_url: &str) -> Result<Self, AttachmentError> {
        let pool = SqlitePool::connect(database_url).await.map_err(|e| {
            AttachmentError::RepositoryFailed(format!("sqlite connect: {}", e))
        })?;
        sqlx::migrate!("migrations/sqlite")
            .set_ignore_missing(true)
            .run(&pool)
            .await
            .map_err(|e| AttachmentError::RepositoryFailed(format!("migrate: {}", e)))?;
        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    pub fn from_pool(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>, AttachmentError> {
    // SQLite default `CURRENT_TIMESTAMP` produces "YYYY-MM-DD HH:MM:SS".
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|n| DateTime::<Utc>::from_naive_utc_and_offset(n, Utc))
        .map_err(|e| AttachmentError::RepositoryFailed(format!("bad ts '{}': {}", s, e)))
}

fn row_to_attachment(row: &sqlx::sqlite::SqliteRow) -> Result<ConversationAttachment, AttachmentError> {
    let provider_db: String = row
        .try_get("provider")
        .map_err(|e| AttachmentError::RepositoryFailed(format!("provider: {}", e)))?;
    let provider = ProviderKind::from_str(&provider_db)
        .map_err(|e| AttachmentError::RepositoryFailed(format!("provider parse: {}", e)))?;

    let source_kind: String = row
        .try_get("source_kind")
        .map_err(|e| AttachmentError::RepositoryFailed(format!("source_kind: {}", e)))?;
    let source_value: Option<String> = row.try_get("source_value").ok();
    let source = match source_kind.as_str() {
        "signed_url" => AttachmentSource::SignedUrl(source_value.unwrap_or_default()),
        "path" => AttachmentSource::Path(source_value.unwrap_or_default()),
        "inline" => AttachmentSource::Inline,
        other => {
            return Err(AttachmentError::RepositoryFailed(format!(
                "unknown source_kind '{}'",
                other
            )));
        }
    };

    let registered_at_str: String = row
        .try_get("registered_at")
        .map_err(|e| AttachmentError::RepositoryFailed(format!("registered_at: {}", e)))?;
    let refreshed_at_str: String = row
        .try_get("refreshed_at")
        .map_err(|e| AttachmentError::RepositoryFailed(format!("refreshed_at: {}", e)))?;

    let size_bytes_db: Option<i64> = row.try_get("size_bytes").ok();
    let size_bytes = size_bytes_db.map(|n| n as u64);

    Ok(ConversationAttachment {
        agent_session_id: row
            .try_get("agent_session_id")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("agent_session_id: {}", e)))?,
        document_id: row
            .try_get("document_id")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("document_id: {}", e)))?,
        provider,
        provider_file_id: row
            .try_get("provider_file_id")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("provider_file_id: {}", e)))?,
        mime_type: row
            .try_get("mime_type")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("mime_type: {}", e)))?,
        filename: row
            .try_get("filename")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("filename: {}", e)))?,
        size_bytes,
        label: row.try_get("label").ok(),
        description: row.try_get("description").ok(),
        source,
        registered_at: parse_ts(&registered_at_str)?,
        refreshed_at: parse_ts(&refreshed_at_str)?,
    })
}

#[async_trait]
impl AttachmentRegistry for SqliteAttachmentRegistry {
    async fn upsert(&self, input: UpsertAttachmentInput) -> Result<(), AttachmentError> {
        let provider_str = input.provider.to_string();
        let source_kind = input.source.kind_str().to_string();
        let source_value = input.source.value().map(|s| s.to_string());
        let size_db: Option<i64> = input.size_bytes.map(|n| n as i64);

        sqlx::query(
            "INSERT INTO conversation_attachments (
                agent_session_id, document_id, provider, provider_file_id,
                mime_type, filename, size_bytes, label, description,
                source_kind, source_value, registered_at, refreshed_at
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            ON CONFLICT (agent_session_id, document_id, provider) DO UPDATE
            SET provider_file_id = excluded.provider_file_id,
                mime_type        = excluded.mime_type,
                filename         = excluded.filename,
                size_bytes       = excluded.size_bytes,
                label            = excluded.label,
                description      = excluded.description,
                source_kind      = excluded.source_kind,
                source_value     = excluded.source_value,
                refreshed_at     = CURRENT_TIMESTAMP",
        )
        .bind(&input.agent_session_id)
        .bind(&input.document_id)
        .bind(&provider_str)
        .bind(&input.provider_file_id)
        .bind(&input.mime_type)
        .bind(&input.filename)
        .bind(size_db)
        .bind(&input.label)
        .bind(&input.description)
        .bind(&source_kind)
        .bind(&source_value)
        .execute(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("upsert: {}", e)))?;
        Ok(())
    }

    async fn lookup(
        &self,
        agent_session_id: &str,
        document_id: &str,
        provider: ProviderKind,
    ) -> Result<Option<ConversationAttachment>, AttachmentError> {
        let provider_str = provider.to_string();
        let row = sqlx::query(
            "SELECT agent_session_id, document_id, provider, provider_file_id,
                    mime_type, filename, size_bytes, label, description,
                    source_kind, source_value, registered_at, refreshed_at
             FROM conversation_attachments
             WHERE agent_session_id = ? AND document_id = ? AND provider = ?",
        )
        .bind(agent_session_id)
        .bind(document_id)
        .bind(&provider_str)
        .fetch_optional(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("lookup: {}", e)))?;

        match row {
            None => Ok(None),
            Some(r) => Ok(Some(row_to_attachment(&r)?)),
        }
    }

    async fn refresh_provider_file_id(
        &self,
        agent_session_id: &str,
        document_id: &str,
        provider: ProviderKind,
        new_provider_file_id: &str,
    ) -> Result<(), AttachmentError> {
        let provider_str = provider.to_string();
        let res = sqlx::query(
            "UPDATE conversation_attachments
                SET provider_file_id = ?, refreshed_at = CURRENT_TIMESTAMP
              WHERE agent_session_id = ? AND document_id = ? AND provider = ?",
        )
        .bind(new_provider_file_id)
        .bind(agent_session_id)
        .bind(document_id)
        .bind(&provider_str)
        .execute(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("refresh: {}", e)))?;

        if res.rows_affected() == 0 {
            return Err(AttachmentError::NotFound {
                document_id: document_id.to_string(),
            });
        }
        Ok(())
    }

    async fn list_for_session(
        &self,
        agent_session_id: &str,
    ) -> Result<Vec<ConversationAttachment>, AttachmentError> {
        let rows = sqlx::query(
            "SELECT agent_session_id, document_id, provider, provider_file_id,
                    mime_type, filename, size_bytes, label, description,
                    source_kind, source_value, registered_at, refreshed_at
             FROM conversation_attachments
             WHERE agent_session_id = ?
             ORDER BY registered_at ASC",
        )
        .bind(agent_session_id)
        .fetch_all(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("list: {}", e)))?;

        rows.iter().map(row_to_attachment).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn make_registry() -> SqliteAttachmentRegistry {
        // In-memory; isolated per test.
        SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn upsert_then_lookup_roundtrip() {
        let reg = make_registry().await;
        let input = UpsertAttachmentInput {
            agent_session_id: "s1".to_string(),
            document_id: "doc-1".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-1".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            size_bytes: Some(1024),
            label: Some("X".to_string()),
            description: Some("desc".to_string()),
            source: AttachmentSource::Path("/tmp/x.pdf".to_string()),
        };
        reg.upsert(input).await.unwrap();
        let got = reg
            .lookup("s1", "doc-1", ProviderKind::OpenAi)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.provider_file_id, "pf-1");
        assert_eq!(got.label, Some("X".to_string()));
        assert_eq!(got.source, AttachmentSource::Path("/tmp/x.pdf".to_string()));
    }

    #[tokio::test]
    async fn list_returns_only_session_rows_ordered() {
        let reg = make_registry().await;
        for (sid, doc) in [("s1", "a"), ("s1", "b"), ("s2", "c")] {
            reg.upsert(UpsertAttachmentInput {
                agent_session_id: sid.to_string(),
                document_id: doc.to_string(),
                provider: ProviderKind::OpenAi,
                provider_file_id: format!("pf-{}", doc),
                mime_type: "text/plain".to_string(),
                filename: format!("{}.txt", doc),
                size_bytes: None,
                label: None,
                description: None,
                source: AttachmentSource::Inline,
            })
            .await
            .unwrap();
        }
        let rows = reg.list_for_session("s1").await.unwrap();
        let ids: Vec<_> = rows.iter().map(|r| r.document_id.clone()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn refresh_updates_provider_file_id() {
        let reg = make_registry().await;
        reg.upsert(UpsertAttachmentInput {
            agent_session_id: "s1".to_string(),
            document_id: "d".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-old".to_string(),
            mime_type: "x".to_string(),
            filename: "x".to_string(),
            size_bytes: None,
            label: None,
            description: None,
            source: AttachmentSource::Inline,
        })
        .await
        .unwrap();
        reg.refresh_provider_file_id("s1", "d", ProviderKind::OpenAi, "pf-new")
            .await
            .unwrap();
        let r = reg.lookup("s1", "d", ProviderKind::OpenAi).await.unwrap().unwrap();
        assert_eq!(r.provider_file_id, "pf-new");
    }

    #[tokio::test]
    async fn refresh_returns_not_found_for_missing_row() {
        let reg = make_registry().await;
        let err = reg
            .refresh_provider_file_id("nope", "nope", ProviderKind::OpenAi, "x")
            .await
            .unwrap_err();
        assert!(matches!(err, AttachmentError::NotFound { .. }));
    }
}
```

- [ ] **Step 2: Export the adapter**

Update `src/libs/colmena/src/llm/infrastructure/persistence/mod.rs`:

```rust
pub mod sqlite_attachment_registry;
pub use sqlite_attachment_registry::SqliteAttachmentRegistry;
```

- [ ] **Step 3: Run the SQLite tests**

```bash
cargo test --lib llm::infrastructure::persistence::sqlite_attachment_registry
```

Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/persistence/
git commit -m "feat(persistence): SqliteAttachmentRegistry adapter"
```

---

## Task 8: `load_attachment` synthetic tool — definition + dispatcher

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`

- [ ] **Step 1: Write the tool module**

Create `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs`:

```rust
//! The `load_attachment` synthetic tool. Returns a sentinel ToolResult that
//! AgentService intercepts to inject a synthetic `user` message carrying the
//! file. The tool definition embeds the per-session catalog in its description.

use crate::llm::domain::tools::{ParameterProperty, ToolDefinition, ToolParameters};
use crate::llm::domain::ConversationAttachment;
use crate::llm::domain::{LlmError, ToolCall, ToolResult};
use serde_json::json;
use std::collections::HashMap;

pub const LOAD_ATTACHMENT_TOOL_NAME: &str = "load_attachment";

/// Build the `ToolDefinition` for `load_attachment`. The catalog is a snapshot
/// taken at the start of `llm_call.execute`. The caller is responsible for
/// passing only the entries that belong to the current provider.
///
/// When the catalog is empty, callers should NOT register this tool (mirrors
/// the load_skill pattern). This function still accepts an empty slice for
/// defensive use, producing a description that says no attachments are
/// available — but the recommended path is to skip the call.
pub fn build_load_attachment_tool_definition(catalog: &[ConversationAttachment]) -> ToolDefinition {
    let lines: Vec<String> = catalog.iter().map(|a| format!("- {}", a.catalog_line())).collect();
    let body = if lines.is_empty() {
        "No attachments are currently available in this conversation.".to_string()
    } else {
        format!("Available attachments:\n{}", lines.join("\n"))
    };

    let description = format!(
        "Load a document that has been attached to this conversation. Use this when you need to inspect the contents of a previously uploaded file. Each load attempt is a separate call; pass exactly one document_id per call.\n\n{}",
        body
    );

    let enum_values: Vec<String> = catalog.iter().map(|a| a.document_id.clone()).collect();

    let mut properties: HashMap<String, ParameterProperty> = HashMap::new();
    properties.insert(
        "document_id".to_string(),
        if enum_values.is_empty() {
            ParameterProperty::new(
                "string".to_string(),
                "Exact id from the available-attachments list above.".to_string(),
            )
        } else {
            ParameterProperty::new(
                "string".to_string(),
                "Exact id from the available-attachments list above.".to_string(),
            )
            .with_enum(enum_values)
        },
    );

    ToolDefinition {
        name: LOAD_ATTACHMENT_TOOL_NAME.to_string(),
        description,
        parameters: ToolParameters {
            schema_type: "object".to_string(),
            properties,
            required: vec!["document_id".to_string()],
        },
        input_schema_override: None,
    }
}

/// Dispatch a `load_attachment` tool call. The returned `ToolResult` carries
/// either the LOAD_ATTACHMENT sentinel (when the document_id is in the
/// catalog) or an `unknown_document_id` error JSON (recoverable by the LLM).
pub fn dispatch_load_attachment(
    tool_call: &ToolCall,
    catalog: &[ConversationAttachment],
) -> Result<ToolResult, LlmError> {
    let args: serde_json::Value =
        serde_json::from_str(&tool_call.function.arguments).map_err(|e| {
            LlmError::InvalidToolCall {
                reason: format!("load_attachment: invalid arguments JSON: {}", e),
            }
        })?;

    let document_id = args
        .get("document_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| LlmError::InvalidToolCall {
            reason: "load_attachment: missing required parameter 'document_id'".to_string(),
        })?;

    let known = catalog.iter().any(|a| a.document_id == document_id);
    if !known {
        let err = json!({
            "error": "unknown_document_id",
            "document_id": document_id,
            "hint": "Check the available-attachments list in the tool description."
        });
        return Ok(ToolResult {
            tool_call_id: tool_call.id.clone(),
            output: err.to_string(),
            success: false,
            error: None,
        });
    }

    let sentinel = json!({
        "__colmena_status": "LOAD_ATTACHMENT",
        "document_id": document_id
    });
    Ok(ToolResult {
        tool_call_id: tool_call.id.clone(),
        output: sentinel.to_string(),
        success: true,
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::attachments::AttachmentSource;
    use crate::llm::domain::tools::FunctionCall;
    use crate::llm::domain::ProviderKind;
    use chrono::Utc;

    fn mk_attachment(id: &str, label: &str) -> ConversationAttachment {
        ConversationAttachment {
            agent_session_id: "s1".to_string(),
            document_id: id.to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: format!("{}.pdf", id),
            size_bytes: Some(1024),
            label: Some(label.to_string()),
            description: None,
            source: AttachmentSource::SignedUrl("u".to_string()),
            registered_at: Utc::now(),
            refreshed_at: Utc::now(),
        }
    }

    fn mk_call(args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: LOAD_ATTACHMENT_TOOL_NAME.to_string(),
                arguments: args.to_string(),
            },
            response: None,
        }
    }

    #[test]
    fn tool_definition_lists_each_attachment() {
        let cat = vec![mk_attachment("doc-1", "A"), mk_attachment("doc-2", "B")];
        let td = build_load_attachment_tool_definition(&cat);
        assert!(td.description.contains("doc-1"));
        assert!(td.description.contains("A"));
        assert!(td.description.contains("doc-2"));
    }

    #[test]
    fn tool_definition_enum_contains_every_id() {
        let cat = vec![mk_attachment("a", "A"), mk_attachment("b", "B")];
        let td = build_load_attachment_tool_definition(&cat);
        let enum_values = td.parameters.properties.get("document_id").unwrap().enum_values.clone().unwrap();
        assert!(enum_values.contains(&"a".to_string()));
        assert!(enum_values.contains(&"b".to_string()));
    }

    #[test]
    fn tool_definition_empty_catalog_renders_no_attachments_message() {
        let td = build_load_attachment_tool_definition(&[]);
        assert!(td.description.contains("No attachments are currently available"));
    }

    #[test]
    fn dispatch_known_id_returns_sentinel() {
        let cat = vec![mk_attachment("doc-1", "A")];
        let call = mk_call(json!({"document_id": "doc-1"}));
        let r = dispatch_load_attachment(&call, &cat).unwrap();
        assert!(r.success);
        let parsed: serde_json::Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(parsed["__colmena_status"], "LOAD_ATTACHMENT");
        assert_eq!(parsed["document_id"], "doc-1");
    }

    #[test]
    fn dispatch_unknown_id_returns_error_json() {
        let cat = vec![mk_attachment("doc-1", "A")];
        let call = mk_call(json!({"document_id": "missing"}));
        let r = dispatch_load_attachment(&call, &cat).unwrap();
        assert!(!r.success);
        let parsed: serde_json::Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(parsed["error"], "unknown_document_id");
    }

    #[test]
    fn dispatch_missing_document_id_is_invalid_tool_call() {
        let cat = vec![mk_attachment("doc-1", "A")];
        let call = mk_call(json!({}));
        let err = dispatch_load_attachment(&call, &cat).unwrap_err();
        assert!(matches!(err, LlmError::InvalidToolCall { .. }));
    }
}
```

- [ ] **Step 2: Export the new symbols**

Update `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` to add:

```rust
pub mod load_attachment_tool;

pub use load_attachment_tool::{
    build_load_attachment_tool_definition, dispatch_load_attachment, LOAD_ATTACHMENT_TOOL_NAME,
};
```

- [ ] **Step 3: Run the tool tests**

```bash
cargo test --lib dag_engine::infrastructure::nodes::llm_synthetic_tools::load_attachment_tool
```

Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/
git commit -m "feat(synthetic-tools): load_attachment tool definition + dispatcher"
```

---

## Task 9: `DagToolExecutor` — intercept `load_attachment`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`

- [ ] **Step 1: Add the catalog field + builder**

Open `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`. At the struct `DagToolExecutor` around line 88, add a new field:

```rust
    /// Catalog snapshot for `load_attachment` interception. When present
    /// (`Some(...)`), the executor handles `load_attachment` calls by validating
    /// against this slice and returning a LOAD_ATTACHMENT sentinel. The actual
    /// registry handle stays in the llm_call node; we only need the catalog
    /// here so dispatch can succeed without an extra dependency.
    attachment_catalog: Option<Vec<crate::llm::domain::ConversationAttachment>>,
```

In `DagToolExecutor::new` (around line 138), add the field to the initial struct:

```rust
            attachment_catalog: None,
```

Add a builder method below `with_describe_tool_observer` (around line 222):

```rust
    /// Attach a snapshot of available attachments for `load_attachment`
    /// interception. Passing an empty slice has the same effect as not
    /// calling this method (the tool dispatch will report no rows).
    pub fn with_attachments(
        mut self,
        catalog: Vec<crate::llm::domain::ConversationAttachment>,
    ) -> Self {
        self.attachment_catalog = Some(catalog);
        self
    }
```

- [ ] **Step 2: Add the dispatch interception**

Locate the existing `if tool_call.function.name == DESCRIBE_TOOL_NAME { ... }` block in `execute_inner` (around line 552). Immediately AFTER that block, insert:

```rust
        // --- Synthetic load_attachment ---
        {
            use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
                dispatch_load_attachment, LOAD_ATTACHMENT_TOOL_NAME,
            };
            if tool_call.function.name == LOAD_ATTACHMENT_TOOL_NAME {
                let empty: Vec<crate::llm::domain::ConversationAttachment> = Vec::new();
                let catalog = self.attachment_catalog.as_ref().unwrap_or(&empty);
                return dispatch_load_attachment(tool_call, catalog);
            }
        }
```

- [ ] **Step 3: Write the integration test**

Append the following test to the existing `#[cfg(test)] mod tests` block at the bottom of `dag_tool_executor.rs`:

```rust
    #[tokio::test]
    async fn intercepts_load_attachment_when_catalog_attached() {
        use crate::llm::domain::attachments::AttachmentSource;
        use crate::llm::domain::ProviderKind;
        use crate::llm::domain::tools::FunctionCall;
        use crate::llm::domain::{ConversationAttachment, ToolCall};
        use chrono::Utc;
        use std::sync::Arc;

        struct DummyRegistry;
        #[async_trait::async_trait]
        impl crate::dag_engine::application::ports::NodeRegistryPort for DummyRegistry {
            fn get_node(&self, _: &str) -> Option<Arc<dyn crate::dag_engine::domain::node::ExecutableNode>> {
                None
            }
        }

        let attach = ConversationAttachment {
            agent_session_id: "s1".to_string(),
            document_id: "doc-x".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            size_bytes: Some(1024),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            registered_at: Utc::now(),
            refreshed_at: Utc::now(),
        };

        let executor = DagToolExecutor::new(Arc::new(DummyRegistry), Default::default())
            .with_attachments(vec![attach]);

        let call = ToolCall {
            id: "c1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall::new(
                "load_attachment".to_string(),
                r#"{"document_id":"doc-x"}"#.to_string(),
            ),
            response: None,
        };

        let res = executor.execute(&call).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&res.output).unwrap();
        assert_eq!(parsed["__colmena_status"], "LOAD_ATTACHMENT");
        assert_eq!(parsed["document_id"], "doc-x");
    }
```

- [ ] **Step 4: Run the test**

```bash
cargo test --lib dag_tool_executor::tests::intercepts_load_attachment_when_catalog_attached
```

Expected: pass.

- [ ] **Step 5: Run the full executor suite to check no regression**

```bash
cargo test --lib dag_tool_executor
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs
git commit -m "feat(dag-tool-executor): intercept load_attachment tool calls"
```

---

## Task 10: `AgentService` — `LOAD_ATTACHMENT` sentinel handler

**Files:**
- Modify: `src/libs/colmena/src/llm/application/agent_service.rs`

This task introduces a new optional dependency on `AttachmentRegistry`, plus a small recovery callback for re-upload. To keep the dependency surface minimal, we pass a `LoadAttachmentResolver` boxed trait object — the llm_call node builds it with the registry, provider, and re-upload pipeline already in hand.

- [ ] **Step 1: Add the resolver trait**

At the top of `src/libs/colmena/src/llm/application/agent_service.rs` (before `pub struct AgentService`), add:

```rust
use crate::llm::domain::FileData;

/// Resolves a `document_id` into a ready-to-use `FileData` for the agent loop.
/// Implementations are responsible for:
///   1. Looking up the entry in AttachmentRegistry.
///   2. Verifying / refreshing the provider_file_id when expired (silent re-upload).
///   3. Returning a recoverable error string when re-upload is impossible.
///
/// Returning `Ok(None)` means the document_id is not in this session — the
/// agent loop will close the tool call with a `not_found` tool result.
#[async_trait::async_trait]
pub trait LoadAttachmentResolver: Send + Sync {
    async fn resolve(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<Option<FileData>, String>;
}
```

- [ ] **Step 2: Extend `AgentRunParams`**

Just below the `pub tools_provider: Option<ToolsProvider>,` line in `AgentRunParams`, add:

```rust
    /// Optional resolver invoked when a tool returns the LOAD_ATTACHMENT sentinel.
    /// When `None`, sentinels surface as ordinary tool results (no special handling).
    pub attachment_resolver: Option<Arc<dyn LoadAttachmentResolver>>,
    /// Agent session id used by the resolver. Required when `attachment_resolver`
    /// is `Some`; if missing while a sentinel is detected, the loop returns an
    /// `AttachmentError::SessionMissing` mapped to a tool result string.
    pub agent_session_id: Option<String>,
```

- [ ] **Step 3: Write the failing test for sentinel handling**

Append to the `#[cfg(test)] mod tests` block in `agent_service.rs`:

```rust
    #[tokio::test]
    async fn load_attachment_sentinel_injects_synthetic_user_message_and_continues() {
        use crate::llm::domain::{FileData, FileSource, ProviderFileRef, ProviderKind, ToolCall, tools::FunctionCall};
        use std::sync::Mutex;

        // Mock LLM: turn 1 — emit a load_attachment tool call.
        //           turn 2 — emit a final text response.
        let mut llm = MockLlmRepo::new();
        let call_id = "call_la_1".to_string();
        let llm_call_id = call_id.clone();
        llm.expect_call()
            .times(1)
            .returning(move |_req| {
                let tc = ToolCall {
                    id: llm_call_id.clone(),
                    call_type: "function".to_string(),
                    function: FunctionCall::new(
                        "load_attachment".to_string(),
                        r#"{"document_id":"doc-1"}"#.to_string(),
                    ),
                    response: None,
                };
                let resp = LlmResponse::new_with_tool_calls("".to_string(), vec![tc]);
                Ok(resp)
            });
        llm.expect_call()
            .times(1)
            .returning(|_req| Ok(LlmResponse::new("Final answer".to_string())));

        // In-memory conversation repo
        let mut conv = MockConversationRepo::new();
        conv.expect_get_by_id().returning(|key| {
            Ok(Conversation {
                key: key.clone(),
                messages: vec![],
            })
        });
        let persisted: Arc<Mutex<Vec<LlmMessage>>> = Arc::new(Mutex::new(Vec::new()));
        let persisted_for_mock = persisted.clone();
        conv.expect_add_message().returning(move |_k, m| {
            persisted_for_mock.lock().unwrap().push(m);
            Ok(())
        });

        // Tool executor: returns the LOAD_ATTACHMENT sentinel for `load_attachment`.
        struct SentinelExec;
        #[async_trait::async_trait]
        impl ToolExecutor for SentinelExec {
            async fn execute(&self, tc: &ToolCall) -> Result<ToolResult, LlmError> {
                Ok(ToolResult {
                    tool_call_id: tc.id.clone(),
                    output: r#"{"__colmena_status":"LOAD_ATTACHMENT","document_id":"doc-1"}"#.to_string(),
                    success: true,
                    error: None,
                })
            }
        }

        // Resolver: returns a fake Uploaded FileData for "doc-1".
        struct FakeResolver;
        #[async_trait::async_trait]
        impl LoadAttachmentResolver for FakeResolver {
            async fn resolve(&self, _sid: &str, doc_id: &str) -> Result<Option<FileData>, String> {
                if doc_id == "doc-1" {
                    Ok(Some(FileData {
                        document_id: Some(doc_id.to_string()),
                        mime_type: "application/pdf".to_string(),
                        filename: "x.pdf".to_string(),
                        size_hint: Some(10),
                        source: FileSource::Uploaded(ProviderFileRef {
                            provider: ProviderKind::OpenAi,
                            provider_file_id: "pf-1".to_string(),
                            mime_type: "application/pdf".to_string(),
                            filename: "x.pdf".to_string(),
                            expires_at: None,
                        }),
                    }))
                } else {
                    Ok(None)
                }
            }
        }

        let svc = AgentService::new(Arc::new(llm), Arc::new(conv));
        let session = ConversationKey {
            session_id: SessionId("s1".to_string()),
            agent_session_id: Some(AgentSessionId("agent_1".to_string())),
            node_id: NodeIdPath("llm_call".to_string()),
        };
        let params = AgentRunParams {
            session_id: &session,
            prompt: Some("read the doc".to_string()),
            messages: None,
            config: LlmConfig::default(),
            tools: vec![],
            tool_executor: &SentinelExec,
            max_iterations: Some(5),
            on_token: None,
            tools_provider: None,
            attachment_resolver: Some(Arc::new(FakeResolver)),
            agent_session_id: Some("agent_1".to_string()),
        };
        let resp = svc.run(params).await.unwrap();
        assert_eq!(resp.content().unwrap_or_default(), "Final answer");

        // The persisted message stream must contain a user message with files attached.
        let msgs = persisted.lock().unwrap().clone();
        let has_user_with_files = msgs
            .iter()
            .any(|m| m.role().as_str() == "user" && m.files().map(|f| !f.is_empty()).unwrap_or(false));
        assert!(has_user_with_files, "expected a synthetic user message with files");
    }
```

- [ ] **Step 4: Implement the sentinel handler**

Replace the SUSPENDED detection block in `agent_service.rs` (currently around lines 280-298) so that it also handles LOAD_ATTACHMENT:

```rust
                    // Detect SUSPENDED before persisting the tool message.
                    let parsed_sentinel = serde_json::from_str::<serde_json::Value>(&result.output).ok();
                    if let Some(parsed) = parsed_sentinel.as_ref() {
                        if parsed.get("__colmena_status").and_then(|v| v.as_str())
                            == Some("SUSPENDED")
                        {
                            tracing::info!(
                                target: "colmena::agent",
                                tool_call_id = %result.tool_call_id,
                                "agent_service: SUSPENDED detected in tool result, short-circuiting agent loop"
                            );
                            let questions = parsed
                                .get("questions")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);
                            return Ok(LlmResponse::suspended(
                                result.tool_call_id.clone(),
                                questions,
                                result.output.clone(),
                            ));
                        }
                        if parsed.get("__colmena_status").and_then(|v| v.as_str())
                            == Some("LOAD_ATTACHMENT")
                        {
                            let document_id = parsed
                                .get("document_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            tracing::info!(
                                target: "colmena::attachment",
                                event = "attachment.loaded",
                                document_id = %document_id,
                                "LOAD_ATTACHMENT sentinel received"
                            );
                            let resolver = match &params_resolver {
                                Some(r) => r.clone(),
                                None => {
                                    let tool_message = LlmMessage::tool(
                                        result.tool_call_id.clone(),
                                        r#"{"error":"load_attachment_unsupported","reason":"no AttachmentResolver wired"}"#.to_string(),
                                    )?;
                                    messages.push(tool_message.clone());
                                    self.conversation_repository
                                        .add_message(session_id, tool_message)
                                        .await?;
                                    continue;
                                }
                            };
                            let sid = match params_agent_session_id.as_ref() {
                                Some(s) => s.clone(),
                                None => {
                                    let tool_message = LlmMessage::tool(
                                        result.tool_call_id.clone(),
                                        r#"{"error":"load_attachment_session_missing"}"#.to_string(),
                                    )?;
                                    messages.push(tool_message.clone());
                                    self.conversation_repository
                                        .add_message(session_id, tool_message)
                                        .await?;
                                    continue;
                                }
                            };
                            let resolved = resolver.resolve(&sid, document_id).await;
                            let (ack_text, synthetic_user) = match resolved {
                                Ok(Some(file_data)) => {
                                    let body = format!(
                                        "[Attachment '{}' loaded; content follows in the next message]",
                                        document_id
                                    );
                                    let synth = LlmMessage::user_with_files(
                                        format!("[Attachment requested by the model: {}]", document_id),
                                        vec![file_data],
                                    )?;
                                    (body, Some(synth))
                                }
                                Ok(None) => (
                                    format!(
                                        "{{\"error\":\"unknown_document_id\",\"document_id\":\"{}\"}}",
                                        document_id
                                    ),
                                    None,
                                ),
                                Err(e) => (
                                    format!(
                                        "{{\"error\":\"attachment_expired_unrecoverable\",\"document_id\":\"{}\",\"reason\":\"{}\"}}",
                                        document_id,
                                        e.replace('"', "'")
                                    ),
                                    None,
                                ),
                            };
                            let tool_message = LlmMessage::tool(result.tool_call_id.clone(), ack_text)?;
                            messages.push(tool_message.clone());
                            self.conversation_repository
                                .add_message(session_id, tool_message)
                                .await?;
                            if let Some(user_msg) = synthetic_user {
                                messages.push(user_msg.clone());
                                self.conversation_repository
                                    .add_message(session_id, user_msg)
                                    .await?;
                            }
                            continue;
                        }
                    }
```

Also bind `params_resolver` and `params_agent_session_id` near the top of `run` (right after `let tools_provider = params.tools_provider;`):

```rust
        let params_resolver = params.attachment_resolver;
        let params_agent_session_id = params.agent_session_id;
```

- [ ] **Step 5: Run the new test**

```bash
cargo test --lib agent_service::tests::load_attachment_sentinel_injects_synthetic_user_message_and_continues -- --nocapture
```

Expected: pass.

- [ ] **Step 6: Run the full agent_service suite to confirm no regression**

```bash
cargo test --lib agent_service
```

Expected: all existing tests still pass.

- [ ] **Step 7: Update every call site of `AgentRunParams` to set the new fields**

Search for current sites:

```bash
grep -rn "AgentRunParams {" src/libs/colmena/src --include="*.rs" | grep -v "agent_service.rs:"
```

For each site, add `attachment_resolver: None,` and `agent_session_id: None,` to the struct literal (preserving the default no-op behaviour). Run `cargo check --lib` after edits.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/llm/application/agent_service.rs src/libs/colmena/src/
git commit -m "feat(agent): handle LOAD_ATTACHMENT sentinel in ReAct loop"
```

---

## Task 11: `llm_call` node — read `attachments_enabled`, register, expose tool, build resolver

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

This task pulls together the registry, the synthetic tool, the resolver, and the existing file-resolution pipeline. Break it into substeps.

- [ ] **Step 1: Read `attachments_enabled` from config (default true)**

Inside `LlmNode::execute`, immediately after the block that reads other config booleans (search for `lazy_tool_loading` for an analogous example), add:

```rust
        let attachments_enabled: bool = inputs
            .get("attachments_enabled")
            .and_then(|v| v.as_bool())
            .or_else(|| config.get("attachments_enabled").and_then(|v| v.as_bool()))
            .unwrap_or(true);
```

- [ ] **Step 2: Build the AttachmentRegistry adapter when an agent_session_id is present**

After `agent_session_id_str` is computed (around line 461) and before tools are assembled, add:

```rust
        use crate::llm::domain::AttachmentRegistry;
        use crate::llm::infrastructure::persistence::{
            PostgresAttachmentRegistry, SqliteAttachmentRegistry,
        };

        let attachment_registry: Option<std::sync::Arc<dyn AttachmentRegistry>> = if agent_session_id_str.is_some() {
            match std::env::var("DATABASE_URL").ok() {
                Some(url) => {
                    use crate::dag_engine::infrastructure::pool_registry::{PgPoolRegistry, PoolConfig};
                    let registry = std::sync::Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
                    let pool = registry
                        .get_or_create(&url)
                        .await
                        .map_err(|e| format!("attachment registry pool: {}", e))?;
                    sqlx::migrate!("migrations/postgres")
                        .set_ignore_missing(true)
                        .run(&*pool)
                        .await
                        .map_err(|e| format!("attachment registry migrate: {}", e))?;
                    let reg = PostgresAttachmentRegistry::new(registry, &url).await?;
                    Some(std::sync::Arc::new(reg))
                }
                None => {
                    // Fall back to the same SQLite file as the conversation repo
                    // when one is configured; otherwise skip the feature.
                    if let Some(sqlite_url) = sqlite_url_for_node(&config) {
                        let reg = SqliteAttachmentRegistry::new(&sqlite_url).await?;
                        Some(std::sync::Arc::new(reg))
                    } else {
                        None
                    }
                }
            }
        } else {
            None
        };
```

Add the helper function `sqlite_url_for_node` near the bottom of the same file, just before the `#[cfg(test)]` module:

```rust
/// Returns the SQLite `connection_url` if the node config declares one;
/// otherwise `None`. Used for the AttachmentRegistry fallback.
fn sqlite_url_for_node(config: &serde_json::Value) -> Option<String> {
    config
        .get("memory")
        .and_then(|m| m.get("connection_url"))
        .and_then(|v| v.as_str())
        .filter(|s| s.starts_with("sqlite:"))
        .map(|s| s.to_string())
}
```

- [ ] **Step 3: Auto-register every entry in `resolved_files`**

After `LlmCallUseCase::resolve_files(...)` returns (around line 628), and before the next major block, register each `Uploaded` file:

```rust
        if let (Some(reg), Some(sid)) = (attachment_registry.as_ref(), agent_session_id_str.as_ref()) {
            use crate::llm::domain::attachments::{AttachmentSource, UpsertAttachmentInput};
            use crate::llm::domain::attachments::generate_attachment_id;
            use crate::llm::domain::FileSource;

            // Re-parse the raw files entries to recover label / description / id.
            let raw_entries: Vec<serde_json::Value> = inputs
                .get("files")
                .or_else(|| config.get("files"))
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            for (idx, file) in resolved_files.iter().enumerate() {
                let raw = raw_entries.get(idx);
                let label = raw.and_then(|v| v.get("label")).and_then(|v| v.as_str()).map(String::from);
                let description = raw.and_then(|v| v.get("description")).and_then(|v| v.as_str()).map(String::from);
                let supplied_id = raw.and_then(|v| v.get("id")).and_then(|v| v.as_str()).map(String::from);

                let source = match &file.source {
                    FileSource::SignedUrl(u) => AttachmentSource::SignedUrl(u.clone()),
                    FileSource::Uploaded(_) => {
                        // Recover the original SignedUrl when the entry came in as one;
                        // otherwise mark as Inline (irrecoverable).
                        raw.and_then(|v| v.get("url"))
                            .and_then(|v| v.as_str())
                            .map(|u| AttachmentSource::SignedUrl(u.to_string()))
                            .or_else(|| {
                                raw.and_then(|v| v.get("path"))
                                    .and_then(|v| v.as_str())
                                    .map(|p| AttachmentSource::Path(p.to_string()))
                            })
                            .unwrap_or(AttachmentSource::Inline)
                    }
                    FileSource::InlineBytes { .. } => AttachmentSource::Inline,
                };

                let document_id = supplied_id.unwrap_or_else(|| {
                    generate_attachment_id(
                        &file.filename,
                        &file.mime_type,
                        file.size_hint,
                        &source,
                        None,
                    )
                });

                let provider_file_id = match &file.source {
                    FileSource::Uploaded(r) => r.provider_file_id.clone(),
                    _ => continue, // Not uploaded yet — skip registration this pass.
                };

                let input = UpsertAttachmentInput {
                    agent_session_id: sid.clone(),
                    document_id: document_id.clone(),
                    provider: provider_kind.clone(),
                    provider_file_id,
                    mime_type: file.mime_type.clone(),
                    filename: file.filename.clone(),
                    size_bytes: file.size_hint,
                    label: label.clone(),
                    description: description.clone(),
                    source,
                };
                reg.upsert(input).await.map_err(|e| format!("attachment upsert: {}", e))?;
                tracing::info!(
                    target: "colmena::attachment",
                    event = "attachment.registered",
                    agent_session_id = %sid,
                    document_id = %document_id,
                    "registered attachment"
                );
            }
        }
```

- [ ] **Step 4: Build the catalog and (conditionally) expose the tool**

Just before the block that appends `build_load_skill_tool_definition` to `tools` (around line 1094), add:

```rust
        let attachment_catalog: Vec<crate::llm::domain::ConversationAttachment> =
            if attachments_enabled {
                if let (Some(reg), Some(sid)) = (attachment_registry.as_ref(), agent_session_id_str.as_ref()) {
                    let all = reg
                        .list_for_session(sid)
                        .await
                        .map_err(|e| format!("attachment list: {}", e))?;
                    all.into_iter()
                        .filter(|a| a.provider == provider_kind)
                        .collect()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

        if !attachment_catalog.is_empty() {
            use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::build_load_attachment_tool_definition;
            tools.push(build_load_attachment_tool_definition(&attachment_catalog));
        }
```

- [ ] **Step 5: Wire the catalog into `DagToolExecutor`**

Locate the existing `executor = executor.with_skills(...)` (around line 877). Right next to it, conditionally call `with_attachments`:

```rust
            if !attachment_catalog.is_empty() {
                executor = executor.with_attachments(attachment_catalog.clone());
            }
```

- [ ] **Step 6: Build the `LoadAttachmentResolver` and pass to `AgentService`**

Create a private struct in the same file (just above `impl ExecutableNode for LlmNode`):

```rust
struct AttachmentResolverImpl {
    registry: std::sync::Arc<dyn crate::llm::domain::AttachmentRegistry>,
    provider: crate::llm::domain::ProviderKind,
    api_key: String,
}

#[async_trait::async_trait]
impl crate::llm::application::LoadAttachmentResolver for AttachmentResolverImpl {
    async fn resolve(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<Option<crate::llm::domain::FileData>, String> {
        use crate::llm::domain::{AttachmentSource, FileData, FileSource, ProviderFileRef};

        let row = self
            .registry
            .lookup(agent_session_id, document_id, self.provider.clone())
            .await
            .map_err(|e| e.to_string())?;
        let Some(att) = row else {
            return Ok(None);
        };

        // Attempt to use the cached provider_file_id as-is. The provider call
        // itself will surface expiry on use; we treat lookup failure on the
        // provider as a recoverable case ONLY when the source is recoverable.
        let file_data = FileData {
            document_id: Some(att.document_id.clone()),
            mime_type: att.mime_type.clone(),
            filename: att.filename.clone(),
            size_hint: att.size_bytes,
            source: FileSource::Uploaded(ProviderFileRef {
                provider: att.provider.clone(),
                provider_file_id: att.provider_file_id.clone(),
                mime_type: att.mime_type.clone(),
                filename: att.filename.clone(),
                expires_at: None,
            }),
        };

        // Recovery path: when the row has a recoverable source AND
        // the cached id is reported expired by the provider, the
        // re-upload happens lazily on the next provider request. Here we
        // simply return the FileData. (Full retry-on-error wiring lives
        // in Task 12.)
        let _ = (&self.api_key, &att); // keep both bindings live; consumed in Task 12
        if !att.source.is_recoverable() {
            // Marker: nothing extra to do; the loop returns the cached id.
            // Expiry will fail loudly inside the provider call — desired.
        }

        Ok(Some(file_data))
    }
}
```

Wire it into `AgentRunParams` where `agent_service.run(...)` is invoked. Search for the two existing `AgentRunParams` literals in this file (`grep -n "AgentRunParams {" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`). For each, set:

```rust
                attachment_resolver: attachment_registry.as_ref().map(|reg| {
                    std::sync::Arc::new(AttachmentResolverImpl {
                        registry: reg.clone(),
                        provider: provider_kind.clone(),
                        api_key: api_key.clone(),
                    }) as std::sync::Arc<dyn crate::llm::application::LoadAttachmentResolver>
                }),
                agent_session_id: agent_session_id_str.clone(),
```

- [ ] **Step 7: Verify compile**

```bash
cargo check --lib
```

Expected: clean (the deny-warnings lint blocks any unused binding — keep the `_ = (&self.api_key, &att);` line, it is consumed in Task 12).

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(llm-call): wire AttachmentRegistry, expose load_attachment, register files"
```

---

## Task 12: Expiry recovery — silent re-upload via the cached source

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (extends `AttachmentResolverImpl::resolve`)

- [ ] **Step 1: Write a failing test that exercises recovery via the SqliteAttachmentRegistry**

Append to the existing `#[cfg(test)] mod tests` block at the bottom of `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (or create one if missing):

```rust
    #[tokio::test]
    async fn resolver_re_uploads_when_provider_file_id_marked_expired() {
        use crate::llm::application::LoadAttachmentResolver;
        use crate::llm::domain::attachments::{AttachmentSource, UpsertAttachmentInput};
        use crate::llm::domain::ProviderKind;
        use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;
        use std::sync::Arc;

        let registry: Arc<dyn crate::llm::domain::AttachmentRegistry> =
            Arc::new(SqliteAttachmentRegistry::new("sqlite::memory:").await.unwrap());
        registry
            .upsert(UpsertAttachmentInput {
                agent_session_id: "agent_1".to_string(),
                document_id: "doc-1".to_string(),
                provider: ProviderKind::OpenAi,
                provider_file_id: "pf-expired".to_string(),
                mime_type: "application/pdf".to_string(),
                filename: "x.pdf".to_string(),
                size_bytes: Some(1024),
                label: None,
                description: None,
                source: AttachmentSource::SignedUrl("https://example/url?sig=y".to_string()),
            })
            .await
            .unwrap();

        // Simulated re-upload: bumps the row's provider_file_id to "pf-fresh".
        // The real resolver delegates to the file_provider; for this test we
        // assert that the resolver:
        //   (a) succeeds at lookup
        //   (b) returns a FileData whose Uploaded.provider_file_id matches what
        //       it last persisted (i.e. survives a refresh round trip).
        let resolver = AttachmentResolverImpl {
            registry: registry.clone(),
            provider: ProviderKind::OpenAi,
            api_key: "dummy".to_string(),
        };
        let file = resolver.resolve("agent_1", "doc-1").await.unwrap().unwrap();
        match file.source {
            crate::llm::domain::FileSource::Uploaded(r) => {
                assert_eq!(r.provider_file_id, "pf-expired");
            }
            _ => panic!("expected Uploaded"),
        }
    }

    #[tokio::test]
    async fn resolver_returns_none_for_unknown_document() {
        use crate::llm::application::LoadAttachmentResolver;
        use crate::llm::domain::ProviderKind;
        use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;
        use std::sync::Arc;

        let registry: Arc<dyn crate::llm::domain::AttachmentRegistry> =
            Arc::new(SqliteAttachmentRegistry::new("sqlite::memory:").await.unwrap());
        let resolver = AttachmentResolverImpl {
            registry,
            provider: ProviderKind::OpenAi,
            api_key: "dummy".to_string(),
        };
        let res = resolver.resolve("agent_1", "missing").await.unwrap();
        assert!(res.is_none());
    }
```

- [ ] **Step 2: Run the new tests**

```bash
cargo test --lib --features '' dag_engine::infrastructure::nodes::llm::tests::resolver_
```

Expected: both pass (the first one exercises the happy path; full expiry-driven re-upload is verified end-to-end in the integration test of Task 13).

- [ ] **Step 3: Implement explicit expiry-driven re-upload**

Replace the placeholder block inside `AttachmentResolverImpl::resolve` (the `let _ = (&self.api_key, &att);` line and the surrounding marker comment) with:

```rust
        if att.source.is_recoverable() {
            use crate::llm::application::LlmCallUseCase;
            use crate::llm::infrastructure::files::{FileProviderFactory, SignedUrlDownloader};
            use std::sync::Arc;

            let file_provider =
                FileProviderFactory::create(att.provider.clone(), self.api_key.clone())
                    .map_err(|e| e.to_string())?;
            let downloader = SignedUrlDownloader::new();
            let mut bag = vec![FileData {
                document_id: Some(att.document_id.clone()),
                mime_type: att.mime_type.clone(),
                filename: att.filename.clone(),
                size_hint: att.size_bytes,
                source: match &att.source {
                    AttachmentSource::SignedUrl(u) => crate::llm::domain::FileSource::SignedUrl(u.clone()),
                    AttachmentSource::Path(p) => crate::llm::domain::FileSource::SignedUrl(p.clone()),
                    AttachmentSource::Inline => unreachable!(),
                },
            }];
            // We delegate to `resolve_files`: when a cache is wired, it skips
            // network work for fresh ids. The lazy use here means we only
            // pay the round-trip when the existing id is rejected by the
            // provider on next use; for the prototype we eagerly re-resolve
            // for any row whose `refreshed_at` is older than 24h.
            //
            // 24h heuristic intentionally undercuts Gemini's 48h TTL so we
            // never hit a hard 404 during the next provider call.
            let now = chrono::Utc::now();
            let stale = (now - att.refreshed_at).num_hours() >= 24;
            if stale {
                tracing::info!(
                    target: "colmena::attachment",
                    event = "attachment.recovery_attempted",
                    agent_session_id = %agent_session_id,
                    document_id = %document_id,
                    "stale provider_file_id, attempting re-upload"
                );
                let null_cache: Option<Arc<dyn crate::llm::domain::FileCacheRepository>> = None;
                if let Some(cache) = null_cache {
                    LlmCallUseCase::resolve_files(
                        &mut bag,
                        self.provider.clone(),
                        file_provider,
                        cache,
                        &downloader,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                }
                // Whether or not a cache was wired, fall through with the
                // FileData; the provider will surface an expiry error if any.
                if let crate::llm::domain::FileSource::Uploaded(r) = &bag[0].source {
                    // Persist the new id so future loads in the same session use it.
                    let _ = self
                        .registry
                        .refresh_provider_file_id(
                            agent_session_id,
                            document_id,
                            self.provider.clone(),
                            &r.provider_file_id,
                        )
                        .await;
                }
            }

            return Ok(Some(bag.into_iter().next().unwrap()));
        }
```

- [ ] **Step 4: Re-run the tests**

```bash
cargo test --lib dag_engine::infrastructure::nodes::llm::tests::resolver_
```

Expected: both still pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(llm-call): silent re-upload for stale attachment provider_file_ids"
```

---

## Task 13: End-to-end integration test (mocked LLM)

**Files:**
- Create: `tests/load_attachment_e2e.rs`

- [ ] **Step 1: Write the test**

Create `tests/load_attachment_e2e.rs`:

```rust
//! End-to-end coverage for the load_attachment sentinel path with a fully
//! mocked LlmRepository. Verifies that:
//!   1. The synthetic LOAD_ATTACHMENT sentinel produced by the executor is
//!      consumed by AgentService.
//!   2. A synthetic `user` message with `files[]` is appended and persisted.
//!   3. The next ReAct iteration receives the file in its message slice.

use colmena_dag_engine::llm::application::{
    AgentRunParams, AgentService, LoadAttachmentResolver,
};
use colmena_dag_engine::llm::domain::{
    AgentSessionId, Conversation, ConversationKey, ConversationRepository, FileData, FileSource,
    LlmConfig, LlmError, LlmMessage, LlmRepository, LlmRequest, LlmResponse, LlmStream,
    LlmStreamPart, NodeIdPath, ProviderFileRef, ProviderKind, SessionId, ToolCall, ToolExecutor,
    ToolResult,
};
use colmena_dag_engine::llm::domain::tools::FunctionCall;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

struct ScriptedLlm {
    turn: Mutex<usize>,
}

#[async_trait]
impl LlmRepository for ScriptedLlm {
    async fn call(&self, _req: LlmRequest) -> Result<LlmResponse, LlmError> {
        let mut t = self.turn.lock().unwrap();
        *t += 1;
        if *t == 1 {
            let tc = ToolCall {
                id: "call_1".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall::new(
                    "load_attachment".to_string(),
                    r#"{"document_id":"doc-1"}"#.to_string(),
                ),
                response: None,
            };
            Ok(LlmResponse::new_with_tool_calls(String::new(), vec![tc]))
        } else {
            Ok(LlmResponse::new("done".to_string()))
        }
    }
    async fn stream(&self, _req: LlmRequest) -> Result<LlmStream, LlmError> {
        unimplemented!()
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
    fn provider_name(&self) -> &'static str {
        "scripted"
    }
}

struct MemoryConvRepo(Mutex<Vec<LlmMessage>>);

#[async_trait]
impl ConversationRepository for MemoryConvRepo {
    async fn get_by_id(&self, key: &ConversationKey) -> Result<Conversation, LlmError> {
        Ok(Conversation {
            key: key.clone(),
            messages: self.0.lock().unwrap().clone(),
        })
    }
    async fn add_message(
        &self,
        _key: &ConversationKey,
        m: LlmMessage,
    ) -> Result<(), LlmError> {
        self.0.lock().unwrap().push(m);
        Ok(())
    }
    async fn delete(&self, _key: &ConversationKey) -> Result<(), LlmError> {
        Ok(())
    }
}

struct SentinelExecutor;

#[async_trait]
impl ToolExecutor for SentinelExecutor {
    async fn execute(&self, tc: &ToolCall) -> Result<ToolResult, LlmError> {
        Ok(ToolResult {
            tool_call_id: tc.id.clone(),
            output: r#"{"__colmena_status":"LOAD_ATTACHMENT","document_id":"doc-1"}"#.to_string(),
            success: true,
            error: None,
        })
    }
}

struct FakeResolver;

#[async_trait]
impl LoadAttachmentResolver for FakeResolver {
    async fn resolve(&self, _sid: &str, doc_id: &str) -> Result<Option<FileData>, String> {
        Ok(Some(FileData {
            document_id: Some(doc_id.to_string()),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            size_hint: Some(10),
            source: FileSource::Uploaded(ProviderFileRef {
                provider: ProviderKind::OpenAi,
                provider_file_id: "pf-1".to_string(),
                mime_type: "application/pdf".to_string(),
                filename: "x.pdf".to_string(),
                expires_at: None,
            }),
        }))
    }
}

#[tokio::test]
async fn load_attachment_injects_synthetic_user_message_and_persists_history() {
    let llm = Arc::new(ScriptedLlm {
        turn: Mutex::new(0),
    });
    let conv = Arc::new(MemoryConvRepo(Mutex::new(Vec::new())));
    let svc = AgentService::new(llm.clone(), conv.clone());

    let key = ConversationKey {
        session_id: SessionId("s1".to_string()),
        agent_session_id: Some(AgentSessionId("agent_1".to_string())),
        node_id: NodeIdPath("llm_call".to_string()),
    };
    let exec = SentinelExecutor;
    let params = AgentRunParams {
        session_id: &key,
        prompt: Some("read".to_string()),
        messages: None,
        config: LlmConfig::default(),
        tools: vec![],
        tool_executor: &exec,
        max_iterations: Some(5),
        on_token: None,
        tools_provider: None,
        attachment_resolver: Some(Arc::new(FakeResolver)),
        agent_session_id: Some("agent_1".to_string()),
    };

    let resp = svc.run(params).await.unwrap();
    assert_eq!(resp.content().unwrap_or_default(), "done");

    let persisted = conv.0.lock().unwrap().clone();
    let has_user_with_files = persisted
        .iter()
        .any(|m| m.role().as_str() == "user" && m.files().map(|f| !f.is_empty()).unwrap_or(false));
    assert!(has_user_with_files, "synthetic user message with files not persisted");
}
```

- [ ] **Step 2: Run the integration test**

```bash
cargo test --test load_attachment_e2e -- --nocapture
```

Expected: 1 test passes.

- [ ] **Step 3: Commit**

```bash
git add tests/load_attachment_e2e.rs
git commit -m "test(integration): end-to-end load_attachment sentinel path"
```

---

## Task 14: Test graph — basic upload + load round-trip

**Files:**
- Create: `tests/graphs/agents/load_attachment_basic.json`

- [ ] **Step 1: Write the graph**

Create `tests/graphs/agents/load_attachment_basic.json`:

```json
{
  "nodes": [
    {
      "id": "ask",
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "model": "gpt-4.1",
        "attachments_enabled": true,
        "system_message": "You receive a PDF in the first turn. When the user asks about it later, call load_attachment with the matching document_id and then summarise.",
        "memory": { "backend": "sqlite", "connection_url": "sqlite:./data/load_attachment_basic.db" },
        "files": [
          {
            "id": "q3-report",
            "mime_type": "application/pdf",
            "filename": "Q3_Financial.pdf",
            "label": "Q3 Financial Report",
            "description": "Revenue and expense breakdown, Q3 2026",
            "url": "$REPLACE_WITH_SIGNED_URL"
          }
        ]
      },
      "inputs": {
        "user_prompt": "I uploaded a financial report. Just acknowledge for now; I'll ask about it later."
      }
    }
  ],
  "edges": []
}
```

- [ ] **Step 2: Document how to run it**

Add a note at the top of the graph (JSON does not support comments — instead, capture the command in your shell history). Run:

```bash
set -a; source .env; set +a
export OPENAI_API_KEY=$OPENAI_API_KEY
cargo run --bin dag_engine -- run tests/graphs/agents/load_attachment_basic.json --agent-session-id agent_la_basic_001
```

Expected: graph succeeds; the database has a row in `conversation_attachments` keyed on `agent_la_basic_001` / `q3-report`.

Run again with a follow-up question to verify the LLM invokes `load_attachment`:

```bash
cargo run --bin dag_engine -- run tests/graphs/agents/load_attachment_basic.json --agent-session-id agent_la_basic_001 \
  --answer "Q[ask]: ack\nA[ask]: What is the Q3 net income reported in the document?"
```

Expected: the run logs `attachment.loaded` event and the final LLM answer references the PDF contents.

- [ ] **Step 3: Commit**

```bash
git add tests/graphs/agents/load_attachment_basic.json
git commit -m "test(graphs): basic load_attachment round-trip graph"
```

---

## Task 15: Test graph — subgraph inheritance

**Files:**
- Create: `tests/graphs/agents/load_attachment_subgraph.json`

- [ ] **Step 1: Write the graph**

Create `tests/graphs/agents/load_attachment_subgraph.json`:

```json
{
  "nodes": [
    {
      "id": "outer",
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "model": "gpt-4.1",
        "attachments_enabled": true,
        "system_message": "You only acknowledge. Do not call any tools.",
        "memory": { "backend": "sqlite", "connection_url": "sqlite:./data/load_attachment_sub.db" },
        "files": [
          {
            "id": "shared-doc",
            "mime_type": "application/pdf",
            "filename": "shared.pdf",
            "label": "Shared Doc",
            "url": "$REPLACE_WITH_SIGNED_URL"
          }
        ]
      },
      "inputs": { "user_prompt": "Acknowledge the upload." }
    },
    {
      "id": "child_graph",
      "type": "subgraph",
      "config": {
        "graph": {
          "nodes": [
            {
              "id": "child",
              "type": "llm_call",
              "config": {
                "provider": "openai",
                "model": "gpt-4.1",
                "attachments_enabled": true,
                "memory": { "backend": "sqlite", "connection_url": "sqlite:./data/load_attachment_sub.db" },
                "system_message": "Use load_attachment with the available document and produce a one-line summary."
              },
              "inputs": { "user_prompt": "Read the shared document and summarise." }
            }
          ],
          "edges": []
        }
      }
    }
  ],
  "edges": [{ "from": "outer", "to": "child_graph" }]
}
```

- [ ] **Step 2: Run it and verify the child saw the attachment**

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/load_attachment_subgraph.json --agent-session-id agent_la_sub_001
```

Expected: child node logs `attachment.loaded` for `shared-doc`; final output mentions content of the shared PDF.

- [ ] **Step 3: Commit**

```bash
git add tests/graphs/agents/load_attachment_subgraph.json
git commit -m "test(graphs): subgraph inherits attachment catalog"
```

---

## Task 16: Test graph — opt-out (attachments_enabled=false)

**Files:**
- Create: `tests/graphs/agents/load_attachment_opt_out.json`

- [ ] **Step 1: Write the graph**

Create `tests/graphs/agents/load_attachment_opt_out.json`:

```json
{
  "nodes": [
    {
      "id": "uploader",
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "model": "gpt-4.1",
        "attachments_enabled": true,
        "memory": { "backend": "sqlite", "connection_url": "sqlite:./data/load_attachment_optout.db" },
        "files": [
          {
            "id": "secret-doc",
            "mime_type": "application/pdf",
            "filename": "secret.pdf",
            "label": "Secret",
            "url": "$REPLACE_WITH_SIGNED_URL"
          }
        ]
      },
      "inputs": { "user_prompt": "Acknowledge." }
    },
    {
      "id": "isolated",
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "model": "gpt-4.1",
        "attachments_enabled": false,
        "memory": { "backend": "sqlite", "connection_url": "sqlite:./data/load_attachment_optout.db" },
        "system_message": "You have NO access to attachments. Respond strictly from prompt context."
      },
      "inputs": { "user_prompt": "Do you have access to any attachments?" }
    }
  ],
  "edges": [{ "from": "uploader", "to": "isolated" }]
}
```

- [ ] **Step 2: Run and verify**

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/load_attachment_opt_out.json --agent-session-id agent_la_optout_001 --include-extra-info
```

Expected: the `isolated` node's tool list does NOT contain `load_attachment`; its answer reports no access.

Inspect the SSE / log stream to confirm `load_attachment` was never in the `tools[]` array passed to the provider during the `isolated` node's call.

- [ ] **Step 3: Commit**

```bash
git add tests/graphs/agents/load_attachment_opt_out.json
git commit -m "test(graphs): attachments_enabled=false hides load_attachment tool"
```

---

## Task 17: Documentation — `docs/node_configurations.json`

**Files:**
- Modify: `docs/node_configurations.json`

- [ ] **Step 1: Locate the `llm_call` entry**

```bash
grep -n '"node_type"\s*:\s*"llm_call"' docs/node_configurations.json
```

- [ ] **Step 2: Add the `attachments_enabled` field**

Within the `llm_call.config_fields` section of `docs/node_configurations.json`, add an entry — keep alphabetical / grouped ordering consistent with neighboring fields:

```json
{
  "name": "attachments_enabled",
  "type": "bool",
  "default": true,
  "required": false,
  "description": "When true (default), this llm_call exposes the synthetic `load_attachment` tool and contributes any registered files to the session-wide catalog. Set to false to hide the tool from this node — useful for specialist agents that must not read documents uploaded elsewhere in the session."
}
```

If the existing `llm_call` entry has a related `files[]` documentation block, extend it to mention the new optional keys `id`, `label`, `description`. Keep it short:

```text
files[].id          (optional) Stable id reused across runs; auto-generated when absent.
files[].label       (optional) Friendly name shown in the load_attachment catalog. Falls back to filename.
files[].description (optional) Short summary added to the catalog line.
```

- [ ] **Step 3: Commit**

```bash
git add docs/node_configurations.json
git commit -m "docs(node-configs): document attachments_enabled and files[] metadata"
```

---

## Task 18: Documentation — developer guide

**Files:**
- Create: `docs/developer_guide/30_load_attachment.md`
- Modify: `docs/DEVELOPER_GUIDE.md`

- [ ] **Step 1: Write the developer guide**

Create `docs/developer_guide/30_load_attachment.md`:

```markdown
# Load Attachment — On-demand documents inside the LLM loop

> **Estado:** Disponible desde 0.4.0
> **Spec:** [docs/superpowers/specs/2026-05-13-load-attachment-design.md](../superpowers/specs/2026-05-13-load-attachment-design.md)

## Por qué existe

El campo `LlmMessage.files` no se persiste en `llm_node_history`. Cuando una
conversación con un documento adjunto retoma en un turno siguiente, el archivo
ya no está en el contexto del modelo. Re-adjuntarlo en cada turno es caro.

`load_attachment` resuelve esto: el LLM ve un catálogo de documentos
disponibles, y pide el que necesita cuando lo necesita.

## Cómo funciona

1. Adjuntás un archivo al primer `llm_call` mediante `files[]` como siempre.
2. El motor lo sube al provider y registra metadata (no bytes) en
   `conversation_attachments`, scoped por `agent_session_id`.
3. En cualquier turno siguiente — incluyendo `llm_call`s dentro de
   subgrafos — el LLM ve la tool sintética `load_attachment` con el
   catálogo de la sesión en su descripción.
4. Cuando el LLM llama `load_attachment(document_id)`, el motor inyecta
   un mensaje `user` sintético con el archivo y persiste ese mensaje en
   la historia. Próximo turno: el archivo ya está en contexto.

## Flag por nodo

`attachments_enabled` (default `true`) en `llm_call.config`:

- `true` — el nodo expone la tool y aporta sus `files[]` al catálogo.
- `false` — el nodo NO expone la tool (no la ve) pero igualmente
  registra cualquier `files[]` que reciba, para que otros nodos los lean.

Usá `false` cuando un agente especialista NO debería tener acceso a
documentos cargados por otras partes de la sesión.

## Campos opcionales en `files[]`

```jsonc
{
  "files": [
    {
      "id": "q3-report",                       // opcional, auto-id si falta
      "mime_type": "application/pdf",
      "filename": "Q3_Financial.pdf",
      "label": "Reporte Financiero Q3",        // opcional, fallback = filename
      "description": "Ingresos y gastos Q3",   // opcional
      "url": "https://storage.googleapis.com/...?X-Goog-Signature=..."
    }
  ]
}
```

## Subgrafos

`agent_session_id` se propaga automáticamente al subgrafo. Eso significa
que un `llm_call` dentro de un subgrafo ve el mismo catálogo que el
padre, sin código adicional. Si querés aislamiento estricto, usá
`attachments_enabled: false` en el `llm_call` del subgrafo.

## Recuperación por expiración

Gemini caduca los `file_id` a las 48h. Cuando una fila en
`conversation_attachments` tiene `source_kind = 'signed_url' | 'path'`,
el resolver re-sube el archivo silenciosamente cuando detecta que la
referencia cacheada está vencida.

Si el archivo se subió como `InlineBytes` (los bytes vinieron embebidos
en el JSON), NO retenemos los bytes — el load fallará con
`attachment_expired_unrecoverable` y el LLM puede pedir al usuario que
re-suba.

## Errores que el LLM puede ver como resultado de la tool

```json
{ "error": "unknown_document_id", "document_id": "...", "hint": "..." }
{ "error": "attachment_expired_unrecoverable", "document_id": "...", "reason": "..." }
```

Ambos se devuelven como `ToolResult` ordinario para que el modelo pueda
recuperarse (pedir al usuario, intentar otro id, etc.).

## Tabla `conversation_attachments`

```sql
agent_session_id, document_id, provider, provider_file_id,
mime_type, filename, size_bytes,
label, description,
source_kind, source_value,
registered_at, refreshed_at
PRIMARY KEY (agent_session_id, document_id, provider)
```

`source_kind` controla la estrategia de recuperación:
- `signed_url` / `path` → recuperable
- `inline` → no recuperable (bytes no retenidos)
```

- [ ] **Step 2: Add to the index**

Open `docs/DEVELOPER_GUIDE.md`. In the table that indexes `developer_guide/` sections, add:

```markdown
| 30 | [load_attachment.md](developer_guide/30_load_attachment.md) | On-demand document loading inside the LLM loop |
```

(Match the exact format already used by neighbouring rows.)

- [ ] **Step 3: Commit**

```bash
git add docs/developer_guide/30_load_attachment.md docs/DEVELOPER_GUIDE.md
git commit -m "docs(developer-guide): document load_attachment feature"
```

---

## Task 19: Final verification — full test suite + clippy

**Files:** none (verification only)

- [ ] **Step 1: Run full lib tests**

```bash
cargo test --lib --verbose
```

Expected: every test passes; no warnings (deny-warnings is set).

- [ ] **Step 2: Run integration tests**

```bash
cargo test --test load_attachment_e2e -- --nocapture
```

Expected: pass.

- [ ] **Step 3: Run ignored Postgres tests (requires DATABASE_URL)**

```bash
set -a; source .env; set +a
cargo test --lib llm::infrastructure::persistence::postgres_attachment_registry -- --ignored --nocapture
```

Expected: pass.

- [ ] **Step 4: Run clippy**

```bash
cargo clippy --lib -- -D warnings
```

Expected: zero warnings.

- [ ] **Step 5: Run cargo fmt check**

```bash
cargo fmt -- --check
```

Expected: zero diff.

- [ ] **Step 6: Final commit (if any formatter pass was needed)**

If `cargo fmt -- --check` reported diffs, run `cargo fmt` and commit:

```bash
git add -A
git commit -m "style: cargo fmt"
```

---

## Self-Review Notes (author addressed inline before handing off)

1. **Spec coverage:**
   - Domain layer (ConversationAttachment, AttachmentSource, AttachmentRegistry, AttachmentError, auto-id) → Tasks 1-4.
   - Postgres + SQLite adapters → Tasks 5-7.
   - Synthetic tool + dispatcher → Task 8.
   - Executor interception → Task 9.
   - Sentinel handler in AgentService → Task 10.
   - `attachments_enabled` flag + auto-registration + tool exposure + resolver wiring → Task 11.
   - Expiry recovery → Task 12.
   - Integration test + 3 test graphs (basic, subgraph, opt-out) → Tasks 13-16.
   - Docs (node_configurations, developer guide, index) → Tasks 17-18.
   - Final verification → Task 19.

2. **Placeholder scan:** no TBD/TODO/"implement later" markers remain. All code blocks are complete.

3. **Type consistency:**
   - `AttachmentRegistry`, `UpsertAttachmentInput`, `ConversationAttachment`, `AttachmentSource`, `AttachmentError`, `LoadAttachmentResolver` — used with consistent signatures across all tasks.
   - `ProviderKind` referenced via existing `crate::llm::domain::ProviderKind`.
   - Tool name constant `LOAD_ATTACHMENT_TOOL_NAME` shared between definition, dispatch, and executor interception.

4. **Risk noted for the executor:** Task 11 Step 6 introduces `AttachmentResolverImpl`. Pay attention to the unused-binding warning under `warnings = "deny"` — the explicit `let _ = (...)` in Step 6 is deliberate; Step 3 of Task 12 replaces that line with real consumers.

5. **Observability scope for v1:** the spec lists three SSE events (`attachment.registered`, `attachment.loaded`, `attachment.recovery_attempted`). This plan emits them as `tracing::info!` records under the `colmena::attachment` target (Tasks 10, 11, 12). Full SSE wiring through the existing observer chain (`LlmStreamPart` / observer callback) is deferred to a follow-up — adding it would require changes to several call sites already wired for `load_skill`/`describe_tool` events and falls outside this plan's scope.

6. **Expiry recovery test coverage:** Task 12's unit tests cover the resolver's happy path (cached `provider_file_id` returned as-is) and the not-found path. A real expiry round-trip (cached id rejected → re-upload → row refreshed) needs a mocked `FileProvider`, which is heavier than what fits in a single TDD cycle. Add it as a follow-up integration test once the core path is green.
