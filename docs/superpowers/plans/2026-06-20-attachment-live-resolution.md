# Attachment live resolution (Approach A) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Make same-turn-generated/edited/uploaded attachments resolvable by every `fetch_attachment_bytes`/`fetch_attachment_stream` tool (gdocs_insert_image attachment mode, sql_bulk, attachment_run_python) by adding a live `AttachmentRegistry` fallback to `DagToolExecutor::lookup_storage_key` — unifying with how `http_request` already resolves `$attachment`.

**Architecture:** `lookup_storage_key` becomes async: snapshot-first (in-memory `attachment_catalog`), then live-registry-fallback (`registry.lookup_by_document_id(agent_session_id, document_id)`). Registry wired into the executor from `llm.rs` (already in scope there). Additive (`None` default) → ADP unaffected.

**Tech Stack:** Rust, `mockall` (AttachmentRegistry mock), tokio. Package `colmena_dag_engine`.

**Spec:** [`docs/superpowers/specs/2026-06-20-attachment-live-resolution-design.md`](../specs/2026-06-20-attachment-live-resolution-design.md)

---

## Task 1: Executor — live-registry fallback in `lookup_storage_key`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`
- Test: same file inline `#[cfg(test)]`

- [ ] **Step 1: Add the field + builder**

In the `DagToolExecutor` struct (near `attachment_storage`, ~line 115) add:

```rust
    /// Plan A live fallback: when a `document_id` is not in the start-of-turn
    /// `attachment_catalog` snapshot (e.g. an image generated mid-loop), resolve
    /// it live via the registry — the same source `http_request`'s
    /// AttachmentStreamResolver uses. `None` → snapshot-only (legacy).
    attachment_registry:
        Option<std::sync::Arc<dyn crate::llm::domain::attachments::AttachmentRegistry>>,
```

In the constructor defaults (near `attachment_storage: None`, ~line 229) add `attachment_registry: None,`.

Add a builder (near `with_attachment_storage`, ~line 443):

```rust
    /// Wire the live attachment registry used as a fallback when a
    /// `document_id` is absent from the snapshot catalog (mid-turn outputs).
    pub fn with_attachment_registry(
        mut self,
        registry: std::sync::Arc<dyn crate::llm::domain::attachments::AttachmentRegistry>,
    ) -> Self {
        self.attachment_registry = Some(registry);
        self
    }
```

- [ ] **Step 2: Write the failing unit test (live fallback resolves a non-snapshot id)**

Add to the test module (use the existing mockall import style; `AttachmentRegistry` is `#[automock]` — verify by grepping `MockAttachmentRegistry` usage elsewhere, else add `use mockall::predicate::*;` + build `MockAttachmentRegistry`). Also needs a mock/stub `OutputStorageRepository` returning bytes — reuse the existing test storage helper in this file (grep `attachment_storage` tests near line 3631 `fetch_attachment_bytes_succeeds_when_wired_correctly` and copy its storage stub):

```rust
    #[tokio::test]
    async fn fetch_attachment_bytes_falls_back_to_live_registry_on_snapshot_miss() {
        use crate::llm::domain::attachments::{AttachmentSource, ConversationAttachment};
        // Registry returns a row for a doc id that is NOT in the (empty) snapshot.
        let mut reg = crate::llm::domain::attachments::MockAttachmentRegistry::new();
        reg.expect_lookup_by_document_id()
            .returning(|_, doc| {
                Ok(Some(ConversationAttachment {
                    // fill the fields exactly as the struct requires — copy from a
                    // neighboring test that builds a ConversationAttachment; set
                    // document_id = doc, storage_key = Some("sk-live"), source =
                    // AttachmentSource::Path("sk-live".into()), etc.
                    document_id: doc.to_string(),
                    storage_key: Some("sk-live".to_string()),
                    source: AttachmentSource::Path("sk-live".into()),
                    ../* copy remaining required fields from a neighboring builder */
                }))
            });
        reg.expect_touch_last_used().returning(|_, _| Ok(()));
        let storage = /* in-memory storage stub that maps "sk-live" -> bytes b"PNG";
                         copy the stub from fetch_attachment_bytes_succeeds_when_wired_correctly */;
        let exec = DagToolExecutor::new(/* same args as neighboring tests */)
            .with_agent_session_id("sess-1".to_string()) // confirm the exact builder name
            .with_attachment_storage(std::sync::Arc::new(storage))
            .with_attachment_registry(std::sync::Arc::new(reg));
        // NO with_attachments(...) — snapshot is None, forcing the live path.
        let bytes = exec.fetch_attachment_bytes("img_generated_mid_turn").await.unwrap();
        assert_eq!(bytes.bytes, b"PNG");
    }
```

NOTE: this test stitches together helpers that already exist in the test module. Before writing, read `fetch_attachment_bytes_succeeds_when_wired_correctly` (~line 3631) and `fetch_attachment_bytes_fails_when_catalog_not_wired` (~3644) to copy the EXACT constructor args, the storage stub, the `with_*` builder names (`with_agent_session_id` vs `with_agent_session`), and the `ConversationAttachment` field set. Match them verbatim.

- [ ] **Step 3: Run — verify it fails**

Run: `cargo test --lib dag_tool_executor::tests::fetch_attachment_bytes_falls_back_to_live_registry 2>&1 | tail -15`
Expected: fail (currently `lookup_storage_key` errors "not in catalog" because it never queries the registry).

- [ ] **Step 4: Make `lookup_storage_key` async with the live fallback**

Replace the current sync `fn lookup_storage_key(&self, document_id: &str) -> Result<String, String>` with an async version:

```rust
    async fn lookup_storage_key(&self, document_id: &str) -> Result<String, String> {
        // 1. Fast path: start-of-turn snapshot (no DB hit).
        if let Some(catalog) = self.attachment_catalog.as_ref() {
            if let Some(entry) = catalog.iter().find(|a| a.document_id == document_id) {
                return entry.storage_key.clone().ok_or_else(|| {
                    format!(
                        "attachment '{document_id}' has no storage_key — it likely \
                         originated from a pre-Plan-A path that did not persist bytes."
                    )
                });
            }
        }
        // 2. Live fallback: query the registry (catches mid-turn outputs).
        if let (Some(reg), Some(sid)) =
            (self.attachment_registry.as_ref(), self.agent_session_id.as_ref())
        {
            match reg.lookup_by_document_id(sid, document_id).await {
                Ok(Some(row)) => {
                    let key = row.storage_key.clone().ok_or_else(|| {
                        format!("attachment '{document_id}' found in registry but has no storage_key")
                    })?;
                    let _ = reg.touch_last_used(sid, document_id).await;
                    return Ok(key);
                }
                Ok(None) => {
                    return Err(format!(
                        "attachment '{document_id}' not found in the snapshot catalog \
                         nor the live registry for session '{sid}'."
                    ));
                }
                Err(e) => return Err(format!("attachment registry lookup failed: {e}")),
            }
        }
        // 3. Nothing wired.
        Err(format!(
            "attachment '{document_id}' lookup failed: no attachment_catalog wired \
             and no live registry available."
        ))
    }
```

Then in `fetch_attachment_bytes` and `fetch_attachment_stream`, change `self.lookup_storage_key(document_id)?` → `self.lookup_storage_key(document_id).await?`.

Grep for any OTHER caller of `lookup_storage_key` (e.g. `lookup_attachment_meta` ~line 549). If a caller is sync and cannot become async, give it a snapshot-only inline lookup (don't route it through the async fn). Confirm `cargo build --lib` after.

- [ ] **Step 5: Run the new test + the existing attachment tests**

Run: `cargo test --lib dag_tool_executor 2>&1 | tail -12`
Expected: the new test passes AND the existing `fetch_attachment_bytes_*` tests still pass (snapshot-hit + not-wired paths unchanged).

- [ ] **Step 6: Build + clippy**

Run: `cargo build --lib 2>&1 | tail -3 && cargo clippy --lib 2>&1 | tail -3`
Expected: clean (zero warnings — `[lints.rust] warnings = "deny"`).

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs
git commit -m "feat(executor): live AttachmentRegistry fallback in lookup_storage_key

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Wire the registry in `llm.rs` + docs

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
- Modify: `docs/CHANGELOG_2026-06.md`, `docs/BACKLOG.md`

- [ ] **Step 1: Pass the registry to the executor**

In `llm.rs`, find where the `DagToolExecutor` is constructed (the block that already references `attachment_registry` + `agent_session_id` to build the catalog snapshot, ~line 2024-2030 builds the catalog; the executor `.with_*(...)` chain is nearby — grep `DagToolExecutor::new` / `.with_attachment_storage(` / `.with_attachments(`). Add to that builder chain:

```rust
                .with_attachment_registry(reg.clone())
```

where `reg` is the same `Arc<dyn AttachmentRegistry>` already used to build the snapshot (`attachment_registry.as_ref()` in the catalog block). Gate it the same way the snapshot is gated (only when the registry is `Some`). If the registry is an `Option`, do:

```rust
                // ... existing builder chain ...
```
then after building, `if let Some(reg) = attachment_registry.clone() { executor = executor.with_attachment_registry(reg); }` — match the existing construction style (mutable `executor` vs chained). Read the surrounding code and follow its pattern exactly.

- [ ] **Step 2: Build + full suite**

Run: `cargo build --lib 2>&1 | tail -3 && cargo test --lib 2>&1 | tail -4`
Expected: clean + all pass.

- [ ] **Step 3: CHANGELOG + BACKLOG**

- `docs/CHANGELOG_2026-06.md`: append a `## NN.` section — "Attachment live resolution: mid-turn generated/edited/uploaded images usable by all fetch_attachment_bytes tools (gdocs_insert_image, sql_bulk, attachment_run_python), via a live AttachmentRegistry fallback in the executor. Unifies with http_request's resolver. Additive; ADP unaffected." End with `**Estado.** done.` + `---`.
- `docs/BACKLOG.md`: in the "Attachment catalog — snapshot same-turn snapshot" finding section, mark it SHIPPED (Approach A) with the date and the CHANGELOG ref. Keep the CLI-registry-not-wired bonus note as still-open (separate item).

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs docs/CHANGELOG_2026-06.md docs/BACKLOG.md
git commit -m "feat(llm): wire live attachment registry into the tool executor + docs

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3 (controller): verify + sweep + push + CI

- [ ] cargo fmt + clippy + `cargo test --lib` (full) green; `cargo test --no-run --all-targets` compiles.
- [ ] ADP sweep: AttachmentRegistry + executor are colmena-internal — confirm no ADP impl/construction breaks (additive `None` default). `grep -rn "with_attachment_registry\|DagToolExecutor" /Users/danielgarcia/startti/adp/apps/service/ia/platform/` (expect empty / unaffected).
- [ ] Push develop + watch CI green.
- [ ] Live verify: re-run `gdocs_insert_image_from_attachment_e2e.json` (generate_image → same-turn attachment insert) against the DEPLOYED worker (registry wired), OR add an `#[ignore]` integration test that builds the executor with a real registry+storage and resolves a mid-turn-registered id. Document the result. (Local CLI still can't — registry not wired; separate backlog item.)

---

## Self-Review
- **Spec coverage:** §4.1 executor→Task 1; §4.2 llm.rs→Task 2; §6 testing→Task 1 Step 2 + Task 3; §5 additive/ADP→Task 3 ADP sweep; §7 caveat→Task 3 live verify. Covered.
- **Type consistency:** `with_attachment_registry(Arc<dyn AttachmentRegistry>)` + async `lookup_storage_key() -> Result<String,String>` used consistently. `lookup_by_document_id(sid, doc) -> Result<Option<ConversationAttachment>>` + `touch_last_used(sid, doc)` match the trait (verified in spec exploration).
- **Placeholders:** the `ConversationAttachment` field set + storage stub + constructor args in the Task 1 test are flagged "copy from neighboring test" — they exist; the implementer must match. Unavoidable (private test-helper shapes).
