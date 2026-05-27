# Attachment Uniform Resolution — Plan A (Foundation + Capability)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lay foundation for unified attachment resolution and unlock the LLM's ability to forward any document (inline, signed URL, or generated artifact) via `http_request` multipart. **Purely additive** — no breaking changes to existing LLM behavior or ADP integration.

**Architecture:** Add a `storage_key`, `origin`, and `last_used_at` column to `conversation_attachments`. Persist bytes for every input doc (inline + URL) into `OutputStorageRepository` at registration time. Auto-register generated artifacts (image_gen/image_edit/tts) in `conversation_attachments` so they have a `document_id`. Introduce `AttachmentStreamResolver` (port in `domain`, impl in `infrastructure`) that resolves `$attachment:<document_id>` for any consumer node by composing `AttachmentRegistry` + `OutputStorageRepository`. Wire it into `http_request`. Append a doc catalog to the system message so the LLM knows which `document_id`s exist. Tool results of generated nodes emit `document_id` alongside the legacy `attachment_id` (alias) for backwards compat.

**Tech Stack:** Rust, `sqlx` (Postgres/SQLite), `async_trait`, `mockall` for unit tests, `wiremock` for HTTP integration tests. No new deps.

**Spec:** [docs/superpowers/specs/2026-05-25-attachment-uniform-resolution-design.md](../specs/2026-05-25-attachment-uniform-resolution-design.md)

**Out of scope for this plan (deferred to Plan B):** removing autoinject in turn 1 (D6), `load_attachment` ephemeral semantics (D7), removing `attachment_id` alias and `url` from tool results (full D8), catalog moving from system-message-append to first-class location.

**Out of scope for this plan (deferred to Plan C):** TTL cleanup binary (D10).

---

## File Structure

**Create:**
- `migrations/2026-05-25-attachment-uniform-resolution.sql` — canonical SQL migration (also copied to ADP `prisma/migrations/` by ADP team)
- `src/libs/colmena/src/llm/domain/attachments/stream_resolver.rs` — `AttachmentStreamResolver` trait + `AttachmentResolveError`
- `src/libs/colmena/src/llm/infrastructure/attachments/stream_resolver_impl.rs` — composite impl
- `src/libs/colmena/src/llm/infrastructure/attachments/mod.rs` — module bridge
- `tests/graphs/agents/upload_inline_to_endpoint.json` — integration test graph (inline doc → http_request multipart)
- `tests/graphs/agents/upload_signed_url_to_endpoint.json` — same but signed URL source
- `tests/graphs/agents/forward_generated_artifact.json` — image_generation → http_request multipart
- `tests/attachment_uniform_resolution_test.rs` — end-to-end integration test driving the 3 graphs through `wiremock`

**Modify:**
- `src/libs/colmena/src/llm/domain/attachments/conversation_attachment.rs` — add `storage_key`, `origin`, `last_used_at` fields
- `src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs` — add `storage_key` and `origin` to `UpsertAttachmentInput`; new `lookup_by_document_id` method; new `touch_last_used` method
- `src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs` — column reads/writes, new methods
- `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs` — same
- `src/libs/colmena/src/llm/domain/attachments/mod.rs` — re-export new resolver trait
- `src/libs/colmena/src/llm/infrastructure/mod.rs` — re-export resolver impl module
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` — persist bytes during file resolution; append catalog to system message
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs` — auto-register artifact; emit `document_id` in tool result
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs` — idem
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs` — idem
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs` — replace direct `storage.read_stream` with resolver; backward-compat fallback to `storage.read_stream(<key>)` when `document_id` lookup misses
- `src/libs/colmena/src/shared/service_container.rs` (or wherever services compose) — construct `AttachmentStreamResolverImpl` and expose it
- `docs/developer_guide/31_load_attachment.md` — document the persistent-bytes contract
- `docs/developer_guide/25_web_nodes.md` — document `$attachment:<document_id>` works for the 3 origins
- `docs/node_configurations.json` — confirm `http_request` placeholder doc is consistent (no schema change but verify wording)

---

## Task 1: SQL migration + domain type updates

**Goal:** Land the new columns at the database level and reflect them in the Rust domain types. Code keeps compiling but no behavior changes yet.

**Files:**
- Create: `migrations/2026-05-25-attachment-uniform-resolution.sql`
- Modify: `src/libs/colmena/src/llm/domain/attachments/conversation_attachment.rs`
- Modify: `src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs`

- [ ] **Step 1: Write the migration SQL**

Create `migrations/2026-05-25-attachment-uniform-resolution.sql`:

```sql
-- Attachment uniform resolution — Plan A
-- Add storage_key (reference to OutputStorageRepository), origin (semantic
-- source), and last_used_at (for TTL).
-- Migration is additive: all new columns nullable; existing rows unaffected.

ALTER TABLE conversation_attachments
  ADD COLUMN IF NOT EXISTS storage_key TEXT,
  ADD COLUMN IF NOT EXISTS origin TEXT,
  ADD COLUMN IF NOT EXISTS last_used_at TIMESTAMPTZ;

-- Backfill origin for existing rows based on provider / source_kind.
-- 'generated' rows already exist via ProviderKind::Generated; user uploads
-- are anything else.
UPDATE conversation_attachments
SET origin = CASE
  WHEN provider = 'generated' THEN 'generated_by:unknown'
  ELSE 'user_upload'
END
WHERE origin IS NULL;

CREATE INDEX IF NOT EXISTS idx_conv_attachments_session_used
  ON conversation_attachments (agent_session_id, last_used_at);
```

- [ ] **Step 2: Write the failing test for `ConversationAttachment` fields**

Open `src/libs/colmena/src/llm/domain/attachments/conversation_attachment.rs` and add to the `tests` module (at the bottom, before the closing `}`):

```rust
    #[test]
    fn conversation_attachment_holds_storage_key_origin_last_used_at() {
        let mut a = mk(Some("L"), None, Some(1024));
        a.storage_key = Some("sk-abc".to_string());
        a.origin = Some("user_upload".to_string());
        a.last_used_at = Some(Utc::now());

        assert_eq!(a.storage_key.as_deref(), Some("sk-abc"));
        assert_eq!(a.origin.as_deref(), Some("user_upload"));
        assert!(a.last_used_at.is_some());
    }
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --lib conversation_attachment::tests::conversation_attachment_holds_storage_key`

Expected: FAIL with "no field `storage_key`".

- [ ] **Step 4: Add the new fields to `ConversationAttachment`**

In `src/libs/colmena/src/llm/domain/attachments/conversation_attachment.rs`, replace the `ConversationAttachment` struct (lines ~39-53) with:

```rust
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
    /// Plan A: reference to the bytes stored in `OutputStorageRepository`.
    /// `None` for rows registered before the migration backfill ran.
    pub storage_key: Option<String>,
    /// Plan A: semantic origin (`user_upload` | `generated_by:<tool>`).
    /// `None` for legacy rows; backfill sets a best-effort value.
    pub origin: Option<String>,
    /// Plan A: last time this attachment was resolved via
    /// `AttachmentStreamResolver` or `load_attachment`. Drives TTL cleanup.
    pub last_used_at: Option<DateTime<Utc>>,
}
```

- [ ] **Step 5: Update the test helper `mk` to populate the new fields**

In the same file, update `mk` (around line 97) to default the new fields:

```rust
    fn mk(
        label: Option<&str>,
        description: Option<&str>,
        size: Option<u64>,
    ) -> ConversationAttachment {
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
            storage_key: None,
            origin: None,
            last_used_at: None,
        }
    }
```

- [ ] **Step 6: Update `UpsertAttachmentInput`**

Open `src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs` and update `UpsertAttachmentInput` (lines 7-19):

```rust
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
    /// Plan A: optional reference to `OutputStorageRepository` storage_key.
    /// Set when the caller persisted the bytes themselves before calling upsert.
    pub storage_key: Option<String>,
    /// Plan A: `user_upload` | `generated_by:<tool>`. Defaults handled by caller.
    pub origin: Option<String>,
}
```

- [ ] **Step 7: Run the test to verify it passes**

Run: `cargo test --lib conversation_attachment::tests`

Expected: all tests pass. The new test confirms field existence.

- [ ] **Step 8: Verify the workspace compiles end-to-end**

Run: `cargo check --all-targets`

Expected: PASS. There may be errors in the registry impls (`postgres_attachment_registry.rs`, `sqlite_attachment_registry.rs`) because `UpsertAttachmentInput` gained fields. Address those in Task 2. **Do not fix them in this task** — commit the typed-but-broken state first.

If compile fails outside the registry impls, fix the call sites (e.g., in `llm.rs` line 1239) by adding `storage_key: None, origin: None` to the literal — purely mechanical defaults.

- [ ] **Step 9: Commit**

```bash
git add migrations/2026-05-25-attachment-uniform-resolution.sql \
        src/libs/colmena/src/llm/domain/attachments/conversation_attachment.rs \
        src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs
git commit -m "feat(attachments): add storage_key, origin, last_used_at to domain types

Adds the migration SQL and the three new fields to ConversationAttachment
and UpsertAttachmentInput. Registry impls still need updating (Task 2).
Plan A — Foundation."
```

---

## Task 2: Postgres + SQLite registry implementations

**Goal:** Make the registry impls read/write the new columns and add the two new methods (`lookup_by_document_id`, `touch_last_used`).

**Files:**
- Modify: `src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs`

- [ ] **Step 1: Add two new methods to the `AttachmentRegistry` trait**

In `src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs`, append to the trait:

```rust
    /// Plan A: lookup attachment by `(agent_session_id, document_id)` across
    /// all providers. Returns the most recently refreshed row if multiple
    /// providers have entries for the same document (one row per provider in
    /// practice — cross-provider lazy upload creates additional rows).
    /// Used by `AttachmentStreamResolver` which only needs storage_key, not
    /// provider_file_id.
    async fn lookup_by_document_id(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<Option<ConversationAttachment>, AttachmentError>;

    /// Plan A: update `last_used_at = now()` for all rows matching
    /// `(agent_session_id, document_id)`. Called by `AttachmentStreamResolver`
    /// on every successful resolve. No-op when no row matches.
    async fn touch_last_used(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<(), AttachmentError>;
```

- [ ] **Step 2: Write failing tests for the new trait methods**

Add to the `tests` module at the bottom of `src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs` (find existing `#[cfg(test)] mod tests`):

```rust
    #[tokio::test]
    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn lookup_by_document_id_returns_row_when_present() {
        let pool = pool_from_env().await;
        let reg = PostgresAttachmentRegistry::new(pool.clone());
        let sid = format!("agent_{}", uuid::Uuid::new_v4());
        let did = format!("doc_{}", uuid::Uuid::new_v4());

        reg.upsert(UpsertAttachmentInput {
            agent_session_id: sid.clone(),
            document_id: did.clone(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-1".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "a.pdf".to_string(),
            size_bytes: Some(100),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            storage_key: Some("sk-1".to_string()),
            origin: Some("user_upload".to_string()),
        }).await.unwrap();

        let got = reg.lookup_by_document_id(&sid, &did).await.unwrap();
        assert!(got.is_some());
        let row = got.unwrap();
        assert_eq!(row.storage_key.as_deref(), Some("sk-1"));
        assert_eq!(row.origin.as_deref(), Some("user_upload"));
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn touch_last_used_updates_timestamp() {
        let pool = pool_from_env().await;
        let reg = PostgresAttachmentRegistry::new(pool.clone());
        let sid = format!("agent_{}", uuid::Uuid::new_v4());
        let did = format!("doc_{}", uuid::Uuid::new_v4());

        reg.upsert(UpsertAttachmentInput {
            agent_session_id: sid.clone(),
            document_id: did.clone(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-1".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "a.pdf".to_string(),
            size_bytes: Some(100),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            storage_key: Some("sk-1".to_string()),
            origin: Some("user_upload".to_string()),
        }).await.unwrap();

        // Initially last_used_at is NULL.
        let before = reg.lookup_by_document_id(&sid, &did).await.unwrap().unwrap();
        assert!(before.last_used_at.is_none());

        reg.touch_last_used(&sid, &did).await.unwrap();

        let after = reg.lookup_by_document_id(&sid, &did).await.unwrap().unwrap();
        assert!(after.last_used_at.is_some());
    }
```

If `pool_from_env` doesn't exist, scan the existing tests in the file — the helper has likely been named differently. Use whatever pattern the file already uses for test pool setup.

- [ ] **Step 3: Run the tests to verify they fail compile**

Run: `cargo test --lib postgres_attachment_registry -- --ignored 2>&1 | head -30`

Expected: COMPILE ERROR — `lookup_by_document_id` and `touch_last_used` not implemented for `PostgresAttachmentRegistry`.

- [ ] **Step 4: Implement the two new methods + update `upsert` and `lookup`/`list_for_session` to include the new columns**

In `src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs`, find the existing `upsert`, `lookup`, and `list_for_session` impls and update each to read/write `storage_key`, `origin`, `last_used_at`.

For `upsert`, change the SQL to include the new columns:

```rust
    async fn upsert(&self, input: UpsertAttachmentInput) -> Result<(), AttachmentError> {
        sqlx::query(r#"
            INSERT INTO conversation_attachments (
                agent_session_id, document_id, provider, provider_file_id,
                mime_type, filename, size_bytes, label, description,
                source_kind, source_value, registered_at, refreshed_at,
                storage_key, origin
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW(), NOW(), $12, $13)
            ON CONFLICT (agent_session_id, document_id, provider) DO UPDATE SET
                provider_file_id = EXCLUDED.provider_file_id,
                mime_type = EXCLUDED.mime_type,
                filename = EXCLUDED.filename,
                size_bytes = EXCLUDED.size_bytes,
                label = COALESCE(EXCLUDED.label, conversation_attachments.label),
                description = COALESCE(EXCLUDED.description, conversation_attachments.description),
                source_kind = EXCLUDED.source_kind,
                source_value = EXCLUDED.source_value,
                storage_key = COALESCE(EXCLUDED.storage_key, conversation_attachments.storage_key),
                origin = COALESCE(EXCLUDED.origin, conversation_attachments.origin),
                refreshed_at = NOW()
        "#)
        .bind(&input.agent_session_id)
        .bind(&input.document_id)
        .bind(input.provider.to_string())
        .bind(&input.provider_file_id)
        .bind(&input.mime_type)
        .bind(&input.filename)
        .bind(input.size_bytes.map(|s| s as i64))
        .bind(&input.label)
        .bind(&input.description)
        .bind(input.source.kind_str())
        .bind(input.source.value())
        .bind(&input.storage_key)
        .bind(&input.origin)
        .execute(&self.pool)
        .await
        .map_err(|e| AttachmentError::Storage(e.to_string()))?;
        Ok(())
    }
```

For `lookup`, expand the SELECT to include the new columns and add them to the row mapping. Look for an existing helper like `row_to_attachment` and update it to read `storage_key`, `origin`, `last_used_at` from the row.

Add the two new methods after `list_for_session`:

```rust
    async fn lookup_by_document_id(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<Option<ConversationAttachment>, AttachmentError> {
        let row = sqlx::query(r#"
            SELECT agent_session_id, document_id, provider, provider_file_id,
                   mime_type, filename, size_bytes, label, description,
                   source_kind, source_value, registered_at, refreshed_at,
                   storage_key, origin, last_used_at
            FROM conversation_attachments
            WHERE agent_session_id = $1 AND document_id = $2
            ORDER BY refreshed_at DESC
            LIMIT 1
        "#)
        .bind(agent_session_id)
        .bind(document_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AttachmentError::Storage(e.to_string()))?;

        Ok(row.map(|r| Self::row_to_attachment(&r)))
    }

    async fn touch_last_used(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<(), AttachmentError> {
        sqlx::query(r#"
            UPDATE conversation_attachments
            SET last_used_at = NOW()
            WHERE agent_session_id = $1 AND document_id = $2
        "#)
        .bind(agent_session_id)
        .bind(document_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AttachmentError::Storage(e.to_string()))?;
        Ok(())
    }
```

Adjust `Self::row_to_attachment` (or whatever the row-mapping helper is called) to populate the three new fields on the returned `ConversationAttachment`.

- [ ] **Step 5: Replicate in SQLite registry**

Open `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs` and apply the same changes:
- Update `upsert` SQL to include the new columns.
- Update `lookup` / `list_for_session` SELECT to read the new columns.
- Add `lookup_by_document_id` and `touch_last_used` impls (same logic, SQLite syntax: replace `NOW()` with `CURRENT_TIMESTAMP`).

For SQLite, the schema is owned by colmena (not ADP). Find the existing `CREATE TABLE conversation_attachments` statement in this file and add the three columns:

```rust
// In the schema definition string, add:
//   storage_key TEXT,
//   origin TEXT,
//   last_used_at TEXT
```

- [ ] **Step 6: Run the tests**

Run the Postgres tests (need `.env` sourced):

```bash
source .env && cargo test --lib postgres_attachment_registry -- --ignored
```

Run the SQLite tests (no env required):

```bash
cargo test --lib sqlite_attachment_registry
```

Expected: PASS.

- [ ] **Step 7: Run `cargo check --all-targets`**

Run: `cargo check --all-targets`

Expected: PASS. Compilation across the whole workspace works.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs \
        src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs \
        src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs
git commit -m "feat(attachments): registry reads/writes storage_key, origin, last_used_at

Adds lookup_by_document_id and touch_last_used to AttachmentRegistry trait,
implements in both Postgres and SQLite adapters. Includes integration tests.
Plan A — Foundation."
```

---

## Task 3: Persist bytes during file resolution in `llm.rs`

**Goal:** When the LLM node resolves files at the start of execution, persist the bytes to `OutputStorageRepository` and pass the resulting `storage_key` into the registry upsert. This covers both inline and signed-URL inputs uniformly.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

- [ ] **Step 1: Find the file-resolution block**

Open `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` and locate the registration loop around line 1194-1260. The block iterates `resolved_files` and calls `reg.upsert(...)` for each.

Read lines 1170-1270 to understand the local variables in scope: `attachment_registry`, `agent_session_id_str`, `resolved_files`, `provider_kind`, and (this is the key one) `self.storage` (the `Option<Arc<dyn OutputStorageRepository>>`).

- [ ] **Step 2: Verify `self.storage` is in scope at the registration site**

Run: `grep -n "self\.storage\|storage: Option" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs | head -10`

Expected: confirms `self.storage` is `Option<Arc<dyn OutputStorageRepository>>` on `LlmNode`. If the registration loop doesn't have direct access (e.g., it's in a helper method without `&self`), wire `self.storage` as a parameter.

- [ ] **Step 3: Write a failing test**

Add to the existing tests module at the bottom of `llm.rs`:

```rust
    #[tokio::test]
    async fn inline_file_persists_bytes_to_storage_and_populates_storage_key() {
        // Build an LlmNode with a mock OutputStorageRepository that captures
        // the store() call, and a mock LlmRepository + AttachmentRegistry.
        // Drive execute() with a single inline file in `inputs.files[0]`.
        // Assert: storage.store was called once with the inline bytes,
        // and registry.upsert was called with storage_key matching the
        // StoredOutput.storage_key returned by the mock.

        use crate::llm::domain::attachments::MockAttachmentRegistry;
        use crate::llm::domain::MockLlmRepository;
        use crate::storage::domain::MockOutputStorageRepository;

        let mut storage = MockOutputStorageRepository::new();
        storage.expect_store()
            .times(1)
            .returning(|req| Ok(StoredOutput {
                storage_key: "sk-inline-test".to_string(),
                read_url: "data:application/pdf;base64,e30=".to_string(),
                mime_type: req.mime_type,
                filename: req.filename,
                size_bytes: req.bytes.len() as u64,
            }));

        let mut reg = MockAttachmentRegistry::new();
        reg.expect_upsert()
            .withf(|input| input.storage_key.as_deref() == Some("sk-inline-test")
                       && input.origin.as_deref() == Some("user_upload"))
            .times(1)
            .returning(|_| Ok(()));
        reg.expect_lookup().returning(|_, _, _| Ok(None));

        // Mock LLM provider returns immediately with no tool calls.
        let mut llm = MockLlmRepository::new();
        llm.expect_call().returning(|_| Ok(crate::llm::domain::LlmCallResponse {
            assistant_message: "ok".to_string(),
            tool_calls: vec![],
            usage: Default::default(),
            finish_reason: "stop".to_string(),
        }));

        let node = LlmNode::new(/* … */)
            .with_storage(Arc::new(storage));
        // Drive the node with one inline file input. Use the existing test
        // helper that constructs minimal NodeInputs with `files[0]` carrying
        // a data: URI. Locate the helper in this file (search for `fn mk_inputs`
        // or `fn test_inputs`). Replicate the call pattern.
        // …
    }
```

The test is intentionally sketched — the exact constructor call for `LlmNode::new` depends on the file's current API. The executor agent will adapt the test to match the existing test patterns in this same file (search for `async fn` inside `mod tests` to find the closest analog).

- [ ] **Step 4: Run the test to verify it fails**

Run: `cargo test --lib inline_file_persists_bytes_to_storage`

Expected: FAIL — assertion on `storage.store` count not met (current code doesn't call store during registration).

- [ ] **Step 5: Implement the bytes-persistence logic**

In `llm.rs`, just before the `for (idx, file) in resolved_files.iter().enumerate()` loop at line 1194, prepare the storage handle:

```rust
            let storage_for_persist = self.storage.clone();
```

Inside the loop, after computing `document_id` (around line 1232) and BEFORE `provider_file_id`, add:

```rust
                // Plan A: persist bytes uniformly to OutputStorageRepository
                // so the doc is reachable via $attachment:<document_id> later,
                // regardless of source.
                let storage_key: Option<String> = match (&storage_for_persist, &file.source) {
                    (Some(storage), FileSource::InlineBytes { bytes, .. }) => {
                        let req = crate::storage::domain::StoreRequest {
                            bytes: bytes.to_vec(),
                            mime_type: file.mime_type.clone(),
                            filename: file.filename.clone(),
                            session_id: None,
                            agent_session_id: Some(sid.clone()),
                        };
                        match storage.store(req).await {
                            Ok(out) => Some(out.storage_key),
                            Err(e) => {
                                tracing::warn!(
                                    target: "colmena::attachment",
                                    error = %e,
                                    document_id = %document_id,
                                    "failed to persist inline bytes to storage; \
                                     attachment registered without storage_key"
                                );
                                None
                            }
                        }
                    }
                    (Some(storage), FileSource::SignedUrl(url)) => {
                        // Lazy fetch + persist. We have already downloaded the
                        // bytes once for the provider upload (see resolved_files
                        // construction). For Plan A we re-fetch here to keep
                        // the change localized; future optimization is to
                        // share the bytes across both uploads.
                        match fetch_url_bytes(url).await {
                            Ok((bytes, _)) => {
                                let req = crate::storage::domain::StoreRequest {
                                    bytes,
                                    mime_type: file.mime_type.clone(),
                                    filename: file.filename.clone(),
                                    session_id: None,
                                    agent_session_id: Some(sid.clone()),
                                };
                                storage.store(req).await.ok().map(|o| o.storage_key)
                            }
                            Err(_) => None,
                        }
                    }
                    _ => None,
                };
```

If `fetch_url_bytes` doesn't already exist as a helper in this file, search for the existing signed-URL download path (likely in `parse_file_entries` or near `FileSource::SignedUrl` handling). Reuse that. If not extractable cleanly, inline a minimal `reqwest::get(url).await?.bytes().await?` call.

Then update the `UpsertAttachmentInput` literal (line 1239) to pass the new fields:

```rust
                let origin = "user_upload".to_string();
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
                    source: source.clone(),
                    storage_key,
                    origin: Some(origin),
                };
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test --lib inline_file_persists_bytes_to_storage`

Expected: PASS.

- [ ] **Step 7: Run the full LLM test module to check no regressions**

Run: `cargo test --lib llm::`

Expected: all existing tests still pass.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(llm): persist input bytes to OutputStorageRepository on registration

When the LLM node registers an attachment (inline or signed_url source), the
bytes are now persisted to OutputStorageRepository and the resulting storage_key
is recorded in conversation_attachments. Enables \$attachment:<document_id>
downstream consumption via the resolver (Task 7).
Plan A — Foundation."
```

---

## Task 4: Auto-register generated artifacts (image_generation)

**Goal:** When `image_generation` produces an artifact, it also registers a row in `conversation_attachments` with `origin = generated_by:image_generation` and `storage_key` set. Tool result gains `document_id` field alongside the existing `attachment_id` (backwards compat alias).

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs`

- [ ] **Step 1: Read the current image_generation node**

Run: `wc -l src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs && grep -n "fn execute\|fn new\|storage\|registry\|attachment_id\|tool_result\|return.*Output" src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs | head -30`

Identify:
- The `execute` method's signature and where it calls `storage.store(...)`.
- Where the tool result JSON is constructed (look for `"attachment_id"` or `"images"` keys).
- Whether the node already holds an `Option<Arc<dyn AttachmentRegistry>>`.

- [ ] **Step 2: Add `with_attachment_registry` constructor if missing**

If `ImageGenerationNode` doesn't already accept an `AttachmentRegistry`, add it via the same pattern used in `LlmNode::with_attachment_registry` (search the file for that pattern; usually a builder fn that takes `Arc<dyn AttachmentRegistry>` and stores it as an `Option`).

```rust
impl ImageGenerationNode {
    pub fn with_attachment_registry(
        mut self,
        registry: std::sync::Arc<dyn crate::llm::domain::attachments::AttachmentRegistry>,
    ) -> Self {
        self.attachment_registry = Some(registry);
        self
    }
}
```

Add the corresponding `Option<Arc<dyn AttachmentRegistry>>` field to the struct.

- [ ] **Step 3: Write a failing test**

In the existing test module:

```rust
    #[tokio::test]
    async fn image_generation_auto_registers_artifact_in_registry() {
        use crate::llm::domain::attachments::MockAttachmentRegistry;
        use crate::storage::domain::MockOutputStorageRepository;

        let mut storage = MockOutputStorageRepository::new();
        storage.expect_store()
            .times(1)
            .returning(|req| Ok(StoredOutput {
                storage_key: "sk-gen-1".to_string(),
                read_url: "data:image/png;base64,e30=".to_string(),
                mime_type: req.mime_type,
                filename: "img_001.png".to_string(),
                size_bytes: 100,
            }));

        let mut reg = MockAttachmentRegistry::new();
        reg.expect_upsert()
            .withf(|input| {
                input.storage_key.as_deref() == Some("sk-gen-1")
                && input.origin.as_deref() == Some("generated_by:image_generation")
                && input.document_id.starts_with("img_")
            })
            .times(1)
            .returning(|_| Ok(()));

        // … build a fake provider that returns deterministic image bytes,
        // wire ImageGenerationNode with storage + registry, execute().
        // Assert the tool result JSON has both "document_id" and "attachment_id"
        // pointing to the same string.
    }
```

Adapt to the existing test scaffolding in the file (find a passing `#[tokio::test]` that exercises `execute()` and base the new one on it).

- [ ] **Step 4: Run the test to verify it fails**

Run: `cargo test --lib image_generation::tests::image_generation_auto_registers`

Expected: FAIL — `registry.upsert` not called.

- [ ] **Step 5: Implement the auto-register + tool result alias**

In the `execute` method, after the `storage.store(...)` call succeeds, before constructing the tool result JSON:

```rust
        // Plan A: auto-register in conversation_attachments so the artifact
        // is reachable via $attachment:<document_id>.
        let document_id = stored.filename
            .strip_suffix(&format!(".{}", file_ext(&stored.mime_type)))
            .map(|stem| format!("img_{}", sanitize(stem)))
            .unwrap_or_else(|| stored.storage_key.clone());

        if let (Some(registry), Some(sid)) = (&self.attachment_registry, agent_session_id.as_ref()) {
            let upsert = crate::llm::domain::attachments::UpsertAttachmentInput {
                agent_session_id: sid.clone(),
                document_id: document_id.clone(),
                provider: crate::llm::domain::ProviderKind::Generated,
                provider_file_id: stored.storage_key.clone(),
                mime_type: stored.mime_type.clone(),
                filename: stored.filename.clone(),
                size_bytes: Some(stored.size_bytes),
                label: None,
                description: None,
                source: crate::llm::domain::attachments::AttachmentSource::Path(stored.storage_key.clone()),
                storage_key: Some(stored.storage_key.clone()),
                origin: Some("generated_by:image_generation".to_string()),
            };
            if let Err(e) = registry.upsert(upsert).await {
                tracing::warn!(
                    target: "colmena::attachment",
                    error = %e,
                    document_id = %document_id,
                    "failed to auto-register image_generation artifact"
                );
            }
        }
```

You'll need helpers `file_ext(mime)` and `sanitize(stem)`. Add them as private fns in the file if they don't exist:

```rust
fn file_ext(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "bin",
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}
```

Then in the tool result construction, change:

```rust
// from:
"images": [{ "attachment_id": stored.storage_key, "url": stored.read_url, ... }]

// to (additive — both fields present):
"images": [{
    "document_id": document_id,
    "attachment_id": stored.storage_key,  // deprecated alias; remove in Plan B
    "url": stored.read_url,                // unchanged for Plan A; remove in Plan B
    "mime_type": stored.mime_type,
    "size_bytes": stored.size_bytes
}]
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test --lib image_generation::tests::image_generation_auto_registers`

Expected: PASS.

- [ ] **Step 7: Run the full image_generation test module**

Run: `cargo test --lib image_generation`

Expected: all existing tests pass. If any old test asserts on the exact tool result shape and now sees `document_id` as an extra key, update the assertion to be tolerant (key-by-key) rather than full-object equality.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs
git commit -m "feat(image_generation): auto-register artifact, emit document_id

The node now registers a row in conversation_attachments for every artifact
it produces, with origin=generated_by:image_generation and storage_key set.
Tool result emits both document_id (new) and attachment_id (deprecated alias).
Plan A — Foundation."
```

---

## Task 5: Auto-register generated artifacts (image_edit)

**Goal:** Same as Task 4 but for `image_edit`.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`

- [ ] **Step 1: Apply the Task 4 changes pattern**

Read the file. Apply the same changes: add `attachment_registry` field + `with_attachment_registry` builder, auto-register after `storage.store`, add `document_id` to tool result alongside `attachment_id`.

The `origin` string is `"generated_by:image_edit"`.

- [ ] **Step 2: Write a test mirroring the image_generation test**

Same structure, asserting `origin == "generated_by:image_edit"`.

- [ ] **Step 3: Run the test**

Run: `cargo test --lib image_edit::tests::image_edit_auto_registers`

Expected: PASS.

- [ ] **Step 4: Run the full module**

Run: `cargo test --lib image_edit`

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs
git commit -m "feat(image_edit): auto-register artifact, emit document_id

Mirrors image_generation Task 4: registers in conversation_attachments with
origin=generated_by:image_edit, tool result includes document_id alongside
attachment_id alias.
Plan A — Foundation."
```

---

## Task 6: Auto-register generated artifacts (tts)

**Goal:** Same as Task 4-5 but for `tts`.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`

- [ ] **Step 1: Apply the same pattern**

Read the file. Apply the same changes with `origin = "generated_by:tts"`. Tool result key is likely `"audio"` not `"images"` — keep the same idea: add `document_id` alongside the existing identifier.

- [ ] **Step 2: Write the test**

Mirror the structure. Audio mime types: `audio/wav`, `audio/mpeg`, `audio/mp4`. Update `file_ext` mapping if you decide to share the helper across image and audio nodes (extract to `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/mime_ext.rs`); otherwise inline a local helper in tts.rs.

- [ ] **Step 3: Run the test**

Run: `cargo test --lib tts::tests::tts_auto_registers`

Expected: PASS.

- [ ] **Step 4: Run the full module**

Run: `cargo test --lib tts`

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/util/mime_ext.rs  # if extracted
git commit -m "feat(tts): auto-register artifact, emit document_id

Mirrors Task 4-5: tts artifacts are now visible in conversation_attachments
with origin=generated_by:tts. Tool result emits document_id alongside the
legacy identifier.
Plan A — Foundation."
```

---

## Task 7: `AttachmentStreamResolver` trait

**Goal:** Define the resolver port in `domain` and its error type.

**Files:**
- Create: `src/libs/colmena/src/llm/domain/attachments/stream_resolver.rs`
- Modify: `src/libs/colmena/src/llm/domain/attachments/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `src/libs/colmena/src/llm/domain/attachments/stream_resolver.rs` with just the test (no trait yet):

```rust
//! Plan A: AttachmentStreamResolver — port for resolving $attachment:<document_id>
//! to a StoredStream that consumer nodes can forward (e.g. http_request multipart).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_variants_are_distinct() {
        let nf = AttachmentResolveError::NotFound { document_id: "x".into() };
        let exp = AttachmentResolveError::Expired { document_id: "x".into() };
        assert_ne!(format!("{:?}", nf), format!("{:?}", exp));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib stream_resolver::tests::error_variants_are_distinct`

Expected: FAIL — `AttachmentResolveError` does not exist.

- [ ] **Step 3: Implement the trait and error**

Replace the file content with:

```rust
//! Plan A: AttachmentStreamResolver — port for resolving $attachment:<document_id>
//! to a StoredStream that consumer nodes can forward (e.g. http_request multipart).
//! Composes AttachmentRegistry (document_id → storage_key) and
//! OutputStorageRepository (storage_key → StoredStream).

use async_trait::async_trait;

use crate::llm::domain::attachments::AttachmentError;
use crate::storage::domain::storage_error::StorageError;
use crate::storage::domain::StoredStream;

#[derive(Debug, thiserror::Error)]
pub enum AttachmentResolveError {
    #[error("attachment not found: document_id={document_id}")]
    NotFound { document_id: String },

    #[error("attachment registered but storage_key is null (likely pre-migration row): document_id={document_id}")]
    StorageKeyMissing { document_id: String },

    #[error("attachment expired: document_id={document_id}")]
    Expired { document_id: String },

    #[error("storage error: {0}")]
    StorageError(#[from] StorageError),

    #[error("registry error: {0}")]
    RegistryError(#[from] AttachmentError),
}

#[async_trait]
pub trait AttachmentStreamResolver: Send + Sync {
    /// Given an agent session and a `document_id`, returns a `StoredStream`
    /// that the caller can forward to a downstream consumer (e.g. an HTTP
    /// multipart part). Updates `last_used_at` as a side effect.
    async fn resolve(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<StoredStream, AttachmentResolveError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_variants_are_distinct() {
        let nf = AttachmentResolveError::NotFound { document_id: "x".into() };
        let exp = AttachmentResolveError::Expired { document_id: "x".into() };
        assert_ne!(format!("{:?}", nf), format!("{:?}", exp));
    }
}
```

- [ ] **Step 4: Re-export from `mod.rs`**

Open `src/libs/colmena/src/llm/domain/attachments/mod.rs` and add:

```rust
pub mod stream_resolver;
pub use stream_resolver::{AttachmentResolveError, AttachmentStreamResolver};
```

- [ ] **Step 5: Run the test**

Run: `cargo test --lib stream_resolver::tests`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/llm/domain/attachments/stream_resolver.rs \
        src/libs/colmena/src/llm/domain/attachments/mod.rs
git commit -m "feat(attachments): add AttachmentStreamResolver trait in domain

New port for resolving \$attachment:<document_id> to a StoredStream. Impl
lands in Task 8 (infrastructure).
Plan A — Foundation."
```

---

## Task 8: `AttachmentStreamResolverImpl`

**Goal:** Compose `AttachmentRegistry` + `OutputStorageRepository` into a concrete resolver. Includes backward-compat fallback: if `document_id` lookup misses, treat the identifier as a raw `storage_key` and call `storage.read_stream` directly (preserves existing flows where `attachment_id` IS the storage_key).

**Files:**
- Create: `src/libs/colmena/src/llm/infrastructure/attachments/mod.rs`
- Create: `src/libs/colmena/src/llm/infrastructure/attachments/stream_resolver_impl.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `src/libs/colmena/src/llm/infrastructure/attachments/stream_resolver_impl.rs`:

```rust
//! Plan A: composite AttachmentStreamResolver impl.

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolve_via_document_id_uses_storage_key_from_registry() {
        // Mock registry returns a ConversationAttachment with storage_key="sk-1"
        // Mock storage's read_stream("sk-1") returns a known StoredStream
        // Call resolver.resolve(sid, "doc-1")
        // Assert: returned StoredStream matches what storage produced;
        //         registry.touch_last_used was called with (sid, "doc-1").
    }

    #[tokio::test]
    async fn resolve_falls_back_to_raw_storage_key_when_lookup_misses() {
        // Mock registry's lookup_by_document_id returns Ok(None).
        // Mock storage's read_stream("sk-raw") returns a known StoredStream.
        // Call resolver.resolve(sid, "sk-raw").
        // Assert: storage.read_stream was called with "sk-raw" directly.
        //         No touch_last_used call.
    }

    #[tokio::test]
    async fn resolve_returns_not_found_when_both_paths_miss() {
        // Registry miss + storage miss → NotFound.
    }

    #[tokio::test]
    async fn resolve_returns_storage_key_missing_when_row_has_no_storage_key() {
        // Registry returns a row with storage_key=None (pre-migration row).
        // → AttachmentResolveError::StorageKeyMissing.
    }
}
```

Expand each test with the actual mock setup. Use `mockall::predicate::*` for argument matchers. The test bodies are intentionally sketched — the executor agent expands them based on the mock APIs available (`MockAttachmentRegistry` from Task 2 should already exist).

- [ ] **Step 2: Run the tests to verify they fail compile**

Run: `cargo test --lib stream_resolver_impl::tests`

Expected: FAIL — no impl exists yet.

- [ ] **Step 3: Implement `AttachmentStreamResolverImpl`**

Replace the file content:

```rust
//! Plan A: composite AttachmentStreamResolver impl.
//!
//! Resolution strategy:
//! 1. Look up `(agent_session_id, document_id)` in the registry.
//! 2. If found and `storage_key` is set, call `storage.read_stream(storage_key)`.
//!    Update `last_used_at` on success.
//! 3. If lookup misses, fall back to treating the identifier as a raw
//!    `storage_key` (backwards compat: pre-Plan-A flows where attachment_id IS
//!    the storage_key). No `last_used_at` update on the fallback path.
//! 4. If everything misses, return `NotFound`.

use std::sync::Arc;

use async_trait::async_trait;

use crate::llm::domain::attachments::{
    AttachmentRegistry, AttachmentResolveError, AttachmentStreamResolver,
};
use crate::storage::domain::storage_error::StorageError;
use crate::storage::domain::{OutputStorageRepository, StoredStream};

pub struct AttachmentStreamResolverImpl {
    registry: Arc<dyn AttachmentRegistry>,
    storage: Arc<dyn OutputStorageRepository>,
}

impl AttachmentStreamResolverImpl {
    pub fn new(
        registry: Arc<dyn AttachmentRegistry>,
        storage: Arc<dyn OutputStorageRepository>,
    ) -> Self {
        Self { registry, storage }
    }
}

#[async_trait]
impl AttachmentStreamResolver for AttachmentStreamResolverImpl {
    async fn resolve(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<StoredStream, AttachmentResolveError> {
        // Path 1: document_id lookup in registry.
        if let Some(row) = self
            .registry
            .lookup_by_document_id(agent_session_id, document_id)
            .await?
        {
            let key = row
                .storage_key
                .ok_or_else(|| AttachmentResolveError::StorageKeyMissing {
                    document_id: document_id.to_string(),
                })?;

            let stream = self.storage.read_stream(&key).await?;
            // Best-effort: touch_last_used failure is non-fatal.
            if let Err(e) = self
                .registry
                .touch_last_used(agent_session_id, document_id)
                .await
            {
                tracing::warn!(
                    target: "colmena::attachment",
                    error = %e,
                    document_id = %document_id,
                    "touch_last_used failed (non-fatal)"
                );
            }
            return Ok(stream);
        }

        // Path 2: backward-compat fallback — treat identifier as raw storage_key.
        match self.storage.read_stream(document_id).await {
            Ok(stream) => Ok(stream),
            Err(StorageError::InvalidInput(_)) => {
                Err(AttachmentResolveError::NotFound {
                    document_id: document_id.to_string(),
                })
            }
            Err(other) => Err(AttachmentResolveError::StorageError(other)),
        }
    }
}
```

(Adjust the `StorageError::InvalidInput` arm to whatever variant the trait actually uses — check `src/libs/colmena/src/storage/domain/storage_error.rs`.)

Create `src/libs/colmena/src/llm/infrastructure/attachments/mod.rs`:

```rust
pub mod stream_resolver_impl;
pub use stream_resolver_impl::AttachmentStreamResolverImpl;
```

Update `src/libs/colmena/src/llm/infrastructure/mod.rs` to add:

```rust
pub mod attachments;
```

- [ ] **Step 4: Expand the test bodies**

Now flesh out the test bodies sketched in Step 1. Example for the happy-path:

```rust
    #[tokio::test]
    async fn resolve_via_document_id_uses_storage_key_from_registry() {
        use crate::llm::domain::attachments::MockAttachmentRegistry;
        use crate::storage::domain::MockOutputStorageRepository;
        use futures::stream;
        use bytes::Bytes;
        use std::pin::Pin;
        use futures::Stream;

        let mut reg = MockAttachmentRegistry::new();
        reg.expect_lookup_by_document_id()
            .withf(|sid, did| sid == "agent_x" && did == "doc-1")
            .times(1)
            .returning(|_, _| Ok(Some(ConversationAttachment {
                agent_session_id: "agent_x".to_string(),
                document_id: "doc-1".to_string(),
                provider: ProviderKind::OpenAi,
                provider_file_id: "pf-1".to_string(),
                mime_type: "application/pdf".to_string(),
                filename: "a.pdf".to_string(),
                size_bytes: Some(10),
                label: None,
                description: None,
                source: AttachmentSource::Inline,
                registered_at: Utc::now(),
                refreshed_at: Utc::now(),
                storage_key: Some("sk-1".to_string()),
                origin: Some("user_upload".to_string()),
                last_used_at: None,
            })));
        reg.expect_touch_last_used()
            .withf(|sid, did| sid == "agent_x" && did == "doc-1")
            .times(1)
            .returning(|_, _| Ok(()));

        let mut storage = MockOutputStorageRepository::new();
        storage.expect_read_stream()
            .withf(|k| k == "sk-1")
            .times(1)
            .returning(|_| {
                let bytes = Bytes::from_static(b"hello");
                let s: Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>> =
                    Box::pin(stream::iter(vec![Ok(bytes)]));
                Ok(StoredStream {
                    stream: s,
                    size_bytes: 5,
                    mime_type: "application/pdf".to_string(),
                    filename: "a.pdf".to_string(),
                })
            });

        let resolver = AttachmentStreamResolverImpl::new(
            Arc::new(reg),
            Arc::new(storage),
        );

        let out = resolver.resolve("agent_x", "doc-1").await.unwrap();
        assert_eq!(out.size_bytes, 5);
    }
```

Apply the same expansion pattern to the other 3 tests.

- [ ] **Step 5: Run the tests**

Run: `cargo test --lib stream_resolver_impl::tests`

Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/attachments/mod.rs \
        src/libs/colmena/src/llm/infrastructure/attachments/stream_resolver_impl.rs \
        src/libs/colmena/src/llm/infrastructure/mod.rs
git commit -m "feat(attachments): AttachmentStreamResolverImpl with backward-compat fallback

Composite resolver that goes registry → storage_key → read_stream, with a
fallback to treating the identifier as a raw storage_key for pre-Plan-A
flows where attachment_id IS the storage_key.
Plan A — Foundation."
```

---

## Task 9: Wire resolver into `http_request`

**Goal:** Replace the current direct `storage.read_stream` call in `http_request` multipart mode with a call through `AttachmentStreamResolver`. The fallback in the resolver means existing graphs that pass `$attachment:<storage_key>` keep working.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`

- [ ] **Step 1: Find the current resolution site**

Run: `grep -n "read_stream\|resolve_multipart_part\|AttachmentNotFound" src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`

Expected: locates the call site around line 620-635 (the function `resolve_multipart_part` or equivalent that processes attachment-source parts).

- [ ] **Step 2: Add an optional `AttachmentStreamResolver` field to `HttpRequestNode`**

In the node struct (search for `struct HttpRequestNode` or `pub struct HttpNode`):

```rust
pub struct HttpRequestNode {
    // existing fields …
    storage: Option<Arc<dyn OutputStorageRepository>>,
    /// Plan A: optional resolver for `$attachment:<document_id>` placeholders.
    /// When `None`, fallback is direct storage.read_stream (legacy storage_key path).
    attachment_resolver: Option<Arc<dyn crate::llm::domain::attachments::AttachmentStreamResolver>>,
}
```

Add a builder method:

```rust
impl HttpRequestNode {
    pub fn with_attachment_resolver(
        mut self,
        resolver: Arc<dyn crate::llm::domain::attachments::AttachmentStreamResolver>,
    ) -> Self {
        self.attachment_resolver = Some(resolver);
        self
    }
}
```

Default the new field to `None` in the existing constructors.

- [ ] **Step 3: Write a failing test**

In the existing `mod attachment_placeholder_tests` or similar at the bottom of `http.rs`:

```rust
    #[tokio::test]
    async fn multipart_uses_resolver_when_present() {
        use crate::llm::domain::attachments::MockAttachmentStreamResolver;
        use futures::stream;
        use bytes::Bytes;
        use std::pin::Pin;
        use futures::Stream;

        let mut resolver = MockAttachmentStreamResolver::new();
        resolver.expect_resolve()
            .withf(|sid, did| sid == "agent_x" && did == "doc-1")
            .times(1)
            .returning(|_, _| {
                let bytes = Bytes::from_static(b"hello");
                let s: Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>> =
                    Box::pin(stream::iter(vec![Ok(bytes)]));
                Ok(StoredStream {
                    stream: s,
                    size_bytes: 5,
                    mime_type: "application/pdf".to_string(),
                    filename: "a.pdf".to_string(),
                })
            });

        // Set up a wiremock server that expects a multipart POST with one
        // file part named "file" containing "hello".
        // … (follow the pattern of the existing multipart_with_attachment_part_streams_via_storage test
        //    around line 1593, replacing storage.expect_read_stream with resolver.expect_resolve)

        let node = HttpRequestNode::new(/* … */)
            .with_attachment_resolver(Arc::new(resolver));
        // execute() with agent_session_id="agent_x" and body={"file": "$attachment:doc-1"}
        // wiremock assertion confirms the file part arrived with "hello" bytes.
    }
```

Use the existing test at line 1593 (`multipart_with_attachment_part_streams_via_storage`) as the template.

- [ ] **Step 4: Run the test to verify it fails**

Run: `cargo test --lib http::attachment_placeholder_tests::multipart_uses_resolver_when_present`

Expected: FAIL — `attachment_resolver` field not wired into resolution.

- [ ] **Step 5: Update the multipart-part resolution to prefer the resolver**

Find the function that produces multipart parts from `$attachment:` references (around line 620). The current code calls `self.storage.read_stream(&storage_key)`. Update it:

```rust
                // Plan A: prefer resolver (handles document_id namespace + fallback);
                // fall back to direct storage when resolver not wired (defensive).
                let stored = if let (Some(resolver), Some(sid)) = (&self.attachment_resolver, agent_session_id) {
                    resolver
                        .resolve(sid, &storage_key)
                        .await
                        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                            format!("AttachmentResolveError: {e}").into()
                        })?
                } else if let Some(storage) = &self.storage {
                    storage
                        .read_stream(&storage_key)
                        .await
                        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                            format!("StorageError: {e}").into()
                        })?
                } else {
                    return Err(format!(
                        "AttachmentNotFound: body references '$attachment:{storage_key}' \
                         but neither AttachmentStreamResolver nor OutputStorageRepository \
                         is wired"
                    ).into());
                };
```

Pass `agent_session_id` into this function — it should already be accessible from the `execute` context. If it isn't, thread it through as a parameter.

- [ ] **Step 6: Run the test**

Run: `cargo test --lib http::attachment_placeholder_tests::multipart_uses_resolver_when_present`

Expected: PASS.

- [ ] **Step 7: Run the existing multipart tests to confirm no regressions**

Run: `cargo test --lib http::`

Expected: all pass (including the existing `multipart_with_attachment_part_streams_via_storage` — that one uses no resolver, so it goes through the fallback path).

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs
git commit -m "feat(http_request): use AttachmentStreamResolver when present

Multipart \$attachment:<id> placeholders now go through the resolver first,
falling back to direct storage.read_stream for backwards compat with flows
that pass storage_keys (pre-document_id namespace).
Plan A — Foundation."
```

---

## Task 10: Wire resolver into `ServiceContainer` / `EngineConfig`

**Goal:** Make the resolver actually reachable from the nodes at runtime by constructing it in the engine setup and passing it into the `HttpRequestNode`, `ImageGenerationNode`, `ImageEditNode`, `TtsNode`.

**Files:**
- Modify: `src/libs/colmena/src/shared/service_container.rs` (or wherever services are composed — locate via grep)
- Modify: registration sites where the nodes are constructed (usually in `src/libs/colmena/src/dag_engine/infrastructure/registry.rs` or the engine builder)

- [ ] **Step 1: Find the service composition site**

Run: `grep -rn "AttachmentRegistry\|attachment_registry" src/libs/colmena/src/shared/ src/libs/colmena/src/dag_engine/infrastructure/registry.rs 2>&1 | head -20`

Identify where `AttachmentRegistry` is constructed and passed into `LlmNode::with_attachment_registry`. The resolver needs to be constructed nearby, after both `registry` and `storage` are available.

- [ ] **Step 2: Construct the resolver**

In the service composition function (after both `AttachmentRegistry` and `OutputStorageRepository` are constructed):

```rust
            // Plan A: wire the AttachmentStreamResolver.
            let attachment_resolver: Option<Arc<dyn AttachmentStreamResolver>> =
                match (&attachment_registry, &storage) {
                    (Some(reg), Some(store)) => Some(Arc::new(
                        AttachmentStreamResolverImpl::new(reg.clone(), store.clone())
                    )),
                    _ => None,
                };
```

Imports:

```rust
use crate::llm::domain::attachments::AttachmentStreamResolver;
use crate::llm::infrastructure::attachments::AttachmentStreamResolverImpl;
```

- [ ] **Step 3: Pass the resolver into the relevant nodes**

In the node registry / builder, after the resolver is constructed, wire it into:

- `HttpRequestNode::with_attachment_resolver(resolver.clone())`
- Optionally future nodes (out of scope here; the wiring pattern is established).

For `ImageGenerationNode`, `ImageEditNode`, `TtsNode`: pass `attachment_registry.clone()` via the new `with_attachment_registry(...)` builder added in Tasks 4-6.

- [ ] **Step 4: Verify compile**

Run: `cargo check --all-targets`

Expected: PASS.

- [ ] **Step 5: Smoke test — run an existing test graph that exercises http_request multipart**

If `tests/graphs/external/multipart_upload.json` exists (from the prior multipart spec):

```bash
source .env && cargo run --bin dag_engine -- run tests/graphs/external/multipart_upload.json
```

Expected: existing behavior preserved (the existing graph uses `$attachment:<storage_key>`, which now goes through the resolver's fallback path).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/shared/service_container.rs \
        src/libs/colmena/src/dag_engine/infrastructure/registry.rs
git commit -m "feat(engine): wire AttachmentStreamResolver into nodes

Constructs AttachmentStreamResolverImpl from registry + storage and passes
it into HttpRequestNode. Also wires attachment_registry into image_generation,
image_edit, and tts so they can auto-register their artifacts.
Plan A — Foundation."
```

---

## Task 11: Append doc catalog to system message

**Goal:** When the LLM node builds the system message, prepend a block listing all attachments registered for the `agent_session_id`, with the `document_id` and usage hints. **Additive** — the existing behavior (autoinject of file content in turn 1, `load_attachment` tool description) is unchanged. The catalog goes ABOVE everything else.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
- Create: `src/libs/colmena/src/llm/application/attachment_catalog.rs` (small helper module)
- Modify: `src/libs/colmena/src/llm/application/mod.rs`

- [ ] **Step 1: Create the catalog renderer**

Create `src/libs/colmena/src/llm/application/attachment_catalog.rs`:

```rust
//! Plan A: render the attachment catalog block that gets prepended to the
//! LLM node's system message. Lists every attachment registered for the
//! current agent_session_id with its document_id, metadata, and usage hint.

use crate::llm::domain::attachments::ConversationAttachment;

pub fn render_catalog(attachments: &[ConversationAttachment]) -> Option<String> {
    if attachments.is_empty() {
        return None;
    }

    let mut out = String::from("Documents available in this session:\n\n");
    for a in attachments {
        out.push_str(&format!("[{}]\n", a.document_id));
        out.push_str(&format!(
            "  filename: {} · {} · {}\n",
            a.filename,
            a.mime_type,
            human_size(a.size_bytes),
        ));
        if let Some(desc) = a.description.as_deref().filter(|s| !s.trim().is_empty()) {
            out.push_str(&format!("  description: {}\n", desc.trim()));
        }
        if let Some(origin) = a.origin.as_deref() {
            out.push_str(&format!("  origin: {}\n", humanize_origin(origin)));
        }
        out.push_str(&format!(
            "  created: {}\n",
            a.registered_at.format("%Y-%m-%d %H:%M UTC")
        ));
        out.push_str(&format!(
            "  usage: load_attachment(\"{}\") to read · \"$attachment:{}\" to forward\n\n",
            a.document_id, a.document_id
        ));
    }
    Some(out.trim_end().to_string())
}

fn humanize_origin(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("generated_by:") {
        format!("generated by {}", rest)
    } else if s == "user_upload" {
        "uploaded by user".to_string()
    } else {
        s.to_string()
    }
}

fn human_size(bytes: Option<u64>) -> String {
    let Some(b) = bytes else { return "? bytes".to_string() };
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    if b >= MB {
        format!("{:.1} MB", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.1} KB", b as f64 / KB as f64)
    } else {
        format!("{} B", b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::attachments::{AttachmentSource, ConversationAttachment};
    use crate::llm::domain::ProviderKind;
    use chrono::Utc;

    fn mk(did: &str, origin: &str) -> ConversationAttachment {
        ConversationAttachment {
            agent_session_id: "a".to_string(),
            document_id: did.to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            size_bytes: Some(1024 * 1024),
            label: None,
            description: Some("a summary".to_string()),
            source: AttachmentSource::Inline,
            registered_at: Utc::now(),
            refreshed_at: Utc::now(),
            storage_key: Some("sk".to_string()),
            origin: Some(origin.to_string()),
            last_used_at: None,
        }
    }

    #[test]
    fn empty_list_returns_none() {
        assert!(render_catalog(&[]).is_none());
    }

    #[test]
    fn single_doc_renders_full_block() {
        let out = render_catalog(&[mk("doc-1", "user_upload")]).unwrap();
        assert!(out.starts_with("Documents available in this session:"));
        assert!(out.contains("[doc-1]"));
        assert!(out.contains("filename: x.pdf · application/pdf · 1.0 MB"));
        assert!(out.contains("description: a summary"));
        assert!(out.contains("origin: uploaded by user"));
        assert!(out.contains("load_attachment(\"doc-1\") to read · \"$attachment:doc-1\" to forward"));
    }

    #[test]
    fn generated_origin_humanizes_correctly() {
        let out = render_catalog(&[mk("img-1", "generated_by:image_generation")]).unwrap();
        assert!(out.contains("origin: generated by image_generation"));
    }
}
```

Update `src/libs/colmena/src/llm/application/mod.rs` to add:

```rust
pub mod attachment_catalog;
```

- [ ] **Step 2: Run the renderer tests**

Run: `cargo test --lib attachment_catalog::tests`

Expected: PASS.

- [ ] **Step 3: Integrate into `llm.rs`**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, find where the system message is built (search for `system_prompt` or the place that constructs `LlmMessage::system(...)`). Right after the registration loop completes (around line 1280), and before constructing the request to the provider:

```rust
            // Plan A: prepend attachment catalog to the system message.
            let catalog_block = if let (Some(reg), Some(sid)) =
                (attachment_registry.as_ref(), agent_session_id_str.as_ref())
            {
                let listed = reg
                    .list_for_session(sid)
                    .await
                    .unwrap_or_default();
                crate::llm::application::attachment_catalog::render_catalog(&listed)
            } else {
                None
            };

            if let Some(catalog) = catalog_block {
                effective_system_prompt = format!("{}\n\n{}", catalog, effective_system_prompt);
            }
```

Replace `effective_system_prompt` with whatever variable already holds the system prompt string at that point. If the system message is built earlier and is a `String`, prepend the catalog to it before passing it into the request.

- [ ] **Step 4: Write an integration-level assertion**

Add a test confirming the system message contains the catalog block when attachments are registered. The exact assertion site depends on how the file's existing tests inject `MockLlmRepository` and capture the request payload. Search for `expect_call().withf(|req|` and add a matcher that checks `req.system_prompt.contains("Documents available in this session:")`.

- [ ] **Step 5: Run the test**

Run: `cargo test --lib llm::`

Expected: all pass.

- [ ] **Step 6: Smoke test via CLI**

Create or reuse a graph with one input file and a system message you can observe:

```bash
source .env && cargo run --bin dag_engine -- run tests/graphs/agents/llm_call.json --agent-session-id smoke_001
```

Manually check the logs for the rendered system prompt — verify the `Documents available in this session:` block appears before the user-supplied system prompt content.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/llm/application/attachment_catalog.rs \
        src/libs/colmena/src/llm/application/mod.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(llm): prepend attachment catalog to system message

When the LLM node has attachments registered for the agent_session_id,
the system message is prepended with a catalog block listing each doc
with its document_id, metadata, and usage hint (load_attachment +
\$attachment:). Existing autoinject behavior is preserved (Plan A is
purely additive).
Plan A — Foundation."
```

---

## Task 12: Integration tests — three end-to-end graphs

**Goal:** Smoke-test all three origins (inline, signed URL, generated) by running real graphs that drive the LLM to forward each doc via `http_request` multipart, with the destination mocked via `wiremock`.

**Files:**
- Create: `tests/graphs/agents/upload_inline_to_endpoint.json`
- Create: `tests/graphs/agents/upload_signed_url_to_endpoint.json`
- Create: `tests/graphs/agents/forward_generated_artifact.json`
- Create: `tests/attachment_uniform_resolution_test.rs`

- [ ] **Step 1: Author the inline test graph**

Create `tests/graphs/agents/upload_inline_to_endpoint.json`:

```json
{
  "name": "upload_inline_to_endpoint",
  "version": "1.0",
  "nodes": [
    {
      "id": "trigger",
      "node_type": "trigger",
      "config": {}
    },
    {
      "id": "agent",
      "node_type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "system_prompt": "You are an upload agent. The user will mention a document. Use the http_request tool to POST it to https://kb.test/documents as multipart/form-data with field name 'file'. Use \"$attachment:<document_id>\" to reference the doc.",
        "enabled_tools": ["http_upload"],
        "tool_configurations": {
          "http_upload": {
            "node_type": "http_request",
            "description": "Upload a file as multipart to the KB endpoint.",
            "fixed_config": {
              "url": "https://kb.test/documents",
              "method": "POST",
              "headers": { "Content-Type": "multipart/form-data" }
            },
            "node_schema": {
              "body": {
                "type": "object",
                "required": true,
                "description": "The body must be `{ \"file\": \"$attachment:<document_id>\" }` referencing the document the user uploaded."
              }
            }
          }
        }
      }
    }
  ],
  "edges": [
    { "from": "trigger", "to": "agent" }
  ]
}
```

- [ ] **Step 2: Author the signed-URL test graph**

Create `tests/graphs/agents/upload_signed_url_to_endpoint.json` — same as above but the test will pass the input file as `{ url: "..." }` instead of `{ data: "..." }`.

- [ ] **Step 3: Author the generated-artifact test graph**

Create `tests/graphs/agents/forward_generated_artifact.json` with an `llm_call` that has both `image_generation` and `http_upload` tools, and a system prompt that instructs the LLM to first generate an image, then upload it via `$attachment:<document_id>`.

- [ ] **Step 4: Write the integration test driver**

Create `tests/attachment_uniform_resolution_test.rs`:

```rust
//! Plan A end-to-end tests: drive the three test graphs against a wiremock
//! HTTP server and assert the bytes arrive at the endpoint.

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
#[ignore = "requires GEMINI_API_KEY and live network — run with `cargo test -- --ignored`"]
async fn inline_doc_can_be_forwarded_via_multipart() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/documents"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "id": "kb-doc-1"
        })))
        .mount(&server)
        .await;

    // Patch the graph at runtime to point at the mock server.
    // Load tests/graphs/agents/upload_inline_to_endpoint.json, replace
    // "https://kb.test/documents" with server.uri() + "/documents", run.
    // Pass agent_session_id="test_inline_001" and inputs={
    //   "files": [{
    //     "id": "doc_test",
    //     "filename": "a.pdf",
    //     "mime_type": "application/pdf",
    //     "data": "data:application/pdf;base64,JVBERi0xLjQK..."
    //   }],
    //   "user_message": "Subí el doc al KB."
    // }
    //
    // Assert the mock received exactly one multipart request with a file
    // part containing the same bytes as the data: URI decoded.
}

// Similar tests for signed_url and generated_artifact.
```

The exact runtime API for invoking the engine from a Rust integration test depends on how other integration tests in `tests/` do it. Read one existing example (e.g. `tests/multipart_http_test.rs` if it exists from the prior multipart plan) and follow that pattern.

- [ ] **Step 5: Run the tests**

```bash
source .env && cargo test --test attachment_uniform_resolution_test -- --ignored
```

Expected: all 3 tests pass. The LLM successfully constructs the `$attachment:<document_id>` placeholder, the resolver streams bytes through, and the wiremock server receives them.

- [ ] **Step 6: Run the standard test suite to make sure nothing else broke**

```bash
cargo test --verbose
```

Expected: PASS (including doctests).

- [ ] **Step 7: Commit**

```bash
git add tests/graphs/agents/upload_inline_to_endpoint.json \
        tests/graphs/agents/upload_signed_url_to_endpoint.json \
        tests/graphs/agents/forward_generated_artifact.json \
        tests/attachment_uniform_resolution_test.rs
git commit -m "test(attachments): end-to-end tests for the 3 attachment origins

Confirms LLM can forward inline, signed-URL, and generated docs via
http_request multipart using \$attachment:<document_id>. Driven against
wiremock to assert bytes arrive at the endpoint.
Plan A — Foundation."
```

---

## Task 13: Documentation updates

**Goal:** Reflect Plan A behavior in the developer guides so the next engineer (or LLM agent) understands what changed.

**Files:**
- Modify: `docs/developer_guide/31_load_attachment.md`
- Modify: `docs/developer_guide/25_web_nodes.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update `31_load_attachment.md`**

Add a new section near the top:

```markdown
## Plan A — Persistent bytes for all attachment sources (2026-05-25)

As of Plan A, every attachment registered in `conversation_attachments` has its
bytes persisted in `OutputStorageRepository`. This is true regardless of source:

- **Inline (base64 in `files[].data`):** bytes streamed to storage at registration.
- **Signed URL (`files[].url`):** bytes downloaded and streamed to storage.
- **Generated artifact** (image_generation / image_edit / tts): bytes already
  in storage; the artifact is auto-registered in `conversation_attachments`
  with `origin=generated_by:<tool>`.

This unlocks the `$attachment:<document_id>` placeholder for downstream nodes
(starting with `http_request` multipart) regardless of where the doc came from.

The catalog the LLM sees in its system message lists every doc with its
`document_id` and a usage hint:
- `load_attachment(document_id)` to read the contents.
- `"$attachment:<document_id>"` to forward bytes (e.g. to a multipart endpoint).
```

- [ ] **Step 2: Update `25_web_nodes.md`**

In the multipart section, add a note:

```markdown
### `$attachment:` works for all 3 origins (Plan A)

The `$attachment:<id>` placeholder in `http_request` body now resolves through
the `AttachmentStreamResolver`. The `<id>` is a `document_id` from
`conversation_attachments` — covering inline user uploads, signed URLs, and
artifacts generated by tools (image_generation / image_edit / tts).

For backward compatibility, raw `storage_key` references (the legacy form
used before Plan A) still work via a fallback path. New graphs should use
`document_id`.
```

- [ ] **Step 3: Update `CLAUDE.md`**

Add a one-liner under "Current Status":

```markdown
- **Attachment uniform resolution Plan A shipped 2026-05-25** — any document
  (inline, signed URL, or generated) can be forwarded via `$attachment:<document_id>`
  in `http_request` multipart and (future) other nodes. Catalog auto-injected
  in LLM system message. See [`docs/superpowers/specs/2026-05-25-attachment-uniform-resolution-design.md`](docs/superpowers/specs/2026-05-25-attachment-uniform-resolution-design.md).
```

- [ ] **Step 4: Commit**

```bash
git add docs/developer_guide/31_load_attachment.md \
        docs/developer_guide/25_web_nodes.md \
        CLAUDE.md
git commit -m "docs: reflect Plan A attachment uniform resolution

Updates 31_load_attachment, 25_web_nodes, and CLAUDE.md to describe the
new persistent-bytes guarantee and the \$attachment:<document_id>
behavior across the 3 origins.
Plan A — Foundation."
```

---

## Verification checklist

After all tasks land, run this end-to-end:

- [ ] `cargo fmt --check` — formatting clean
- [ ] `cargo clippy --all-targets -- -D warnings` — no clippy warnings
- [ ] `cargo test --verbose` — all tests (including doctests)
- [ ] `source .env && cargo test -- --ignored` — DB + LLM integration tests
- [ ] Smoke: `cargo run --bin dag_engine -- run tests/graphs/agents/upload_inline_to_endpoint.json --agent-session-id smoke_$(date +%s)` (with `--answer` mocking as needed) — confirm wiremock not needed; we want to see the agent at least call the http_upload tool with the right placeholder.
- [ ] ADP worker sweep: `cd /Users/danielgarcia/startti/adp && grep -rn "attachment_id\|read_url\|images.*url" apps/service/ia/platform/{worker,api}/src/ | grep -v test` — note which usages will need updating in Plan B. If any are critical-path, document them as risk in the PR description before pushing colmena develop.

If all green, push to `develop`.

---

## Self-Review (executor: skip this if the plan looks consistent on read)

This is the spec coverage check:
- D1 (persist bytes uniformly) → Task 3 (inputs) + Tasks 4-6 (already-persisting artifacts)
- D2 (`document_id` as namespace) → enforced by Tasks 4-6 emitting it in tool results and Task 11 catalog using only `document_id`
- D3 (`$attachment:<document_id>`) → Task 9 (resolver wired into http_request)
- D4 (resolver trait + impl) → Tasks 7-8
- D5 (catalog in system message) → Task 11 (partial: appended, not replacing tool description — full version in Plan B)
- D6 (no autoinject) → **Deferred to Plan B** (out of scope here)
- D7 (ephemeral load_attachment) → **Deferred to Plan B**
- D8 (auto-register + document_id in tool results) → Tasks 4-6 (partial: keeps `attachment_id` alias; full removal in Plan B)
- D9 (schema migration) → Task 1
- D10 (TTL cleanup) → **Deferred to Plan C**

Coverage is consistent with the agreed phasing.

Type consistency: `AttachmentStreamResolver::resolve(agent_session_id, document_id) -> Result<StoredStream, AttachmentResolveError>` is used identically in Tasks 7, 8, 9. `UpsertAttachmentInput { storage_key, origin, ... }` matches between Task 1 (definition), Task 3 (call site for inputs), Tasks 4-6 (call sites for artifacts).

No placeholders. Every step has the code or exact command needed.
