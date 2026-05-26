# Attachment Uniform Resolution — Plan B (Catalog-Driven Behavior + Tool Result Cleanup)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Activate the cost-optimization behaviors that Plan A laid the foundation for: (a) the LLM no longer auto-receives doc content in the first turn — only the catalog; (b) `load_attachment` results are ephemeral per turn — the doc bytes leave the conversation history once the turn ends; (c) the tool result schema for `image_generation`/`image_edit`/`tts` drops the legacy `attachment_id` alias and `url` field, leaving only `document_id`.

**Architecture:** Two surgical changes in `llm.rs` and `agent_service.rs` flip the LLM's relationship to documents. The first turn's user message stops carrying files — the catalog block prepended to the system message (shipped in Plan A Task 11) is enough for the model to decide what to load or forward. When the model calls `load_attachment`, the synthetic `user_with_files` message that's injected into the in-memory iteration stream is **not** persisted to `llm_node_history` — a short text marker takes its place. The model retains the analysis it generated from the doc (assistant messages stay intact) but stops paying for the doc content on every subsequent turn. The tool-result cleanup is straightforward field removal in three nodes — purely breaking for ADP frontend consumers of `attachment_id`/`url`.

**Tech Stack:** Rust, no new dependencies. Existing patterns: `LlmMessage`, `ConversationRepository`, `LlmCallNode::execute`, `AgentService::execute`.

**Spec:** [docs/superpowers/specs/2026-05-25-attachment-uniform-resolution-design.md](../specs/2026-05-25-attachment-uniform-resolution-design.md) — decisions D6, D7, full D8.

**Depends on:** Plan A landed (`workingbranch/upload_documents_with_inline` HEAD `7b36b9b` or later).

**Out of scope:** TTL cleanup (Plan C — `attachment_gc` binary).

**Breaking changes:** Yes, two distinct breakage classes:
1. **Existing graphs assuming auto-inject** stop seeing doc content in turn 1 unless the model is instructed to call `load_attachment`. Graph authors must update prompts.
2. **ADP frontend** that parses `attachment_id` and `url` from `image_generation`/`image_edit`/`tts` tool results will break when those fields disappear. Requires coordinated rollout — Plan B Task 9 covers the coordination handoff.

---

## File Structure

**Modify:**
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` — disable autoinject in initial user message (lines ~1316-1323); update tests that assumed autoinject; extend `ATTACHMENTS_SYSTEM_PRELUDE` with ephemeral-semantics line (or split into a new const).
- `src/libs/colmena/src/llm/application/agent_service.rs` — make `load_attachment` synthetic `user_with_files` ephemeral: keep it in the in-memory `messages` vec for the rest of the turn, persist a marker via `conversation_repository.add_message` instead.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs` — remove `attachment_id` and `url` keys from tool result JSON.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs` — same.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs` — same.
- `docs/developer_guide/31_load_attachment.md` — document the ephemeral semantics + the no-autoinject behavior.
- `docs/developer_guide/32_multimedia_generation.md` (or wherever tool result schemas are documented) — document the schema change.
- `CLAUDE.md` — bullet under "Current Status".

**Create:**
- `src/libs/colmena/tests/plan_b_behavioral_test.rs` — integration test driver covering (a) no-autoinject + catalog-only behavior, (b) load_attachment per-turn ephemeral marker.

**Coordination handoff (no colmena change):**
- An ADP migration plan referenced from the commit message, telling the ADP team which files to update (`apps/service/ia/platform/{worker,api}/src/`).

---

## Task 1: Disable autoinject in initial user message

**Goal:** The first turn's user message stops carrying `resolved_files` — the LLM sees only the catalog (already prepended to the system message in Plan A Task 11). The autoinject is the change that delivers the actual cost saving.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

- [ ] **Step 1: Find the current autoinject site**

Open `llm.rs` and locate the block at approximately lines 1316-1323. It looks like:

```rust
if resume_answer.is_none() {
    let user_message = if resolved_files.is_empty() {
        LlmMessage::user(prompt.to_string())?
    } else {
        LlmMessage::user_with_files(prompt.to_string(), resolved_files)?
    };

    messages.push(user_message.clone());
}
```

Confirm by running:
```bash
grep -n "user_with_files\|resolved_files" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs | head -20
```

- [ ] **Step 2: Write the failing test**

Find the existing test that exercises `execute()` with input files. Search the file:
```bash
grep -n "fn.*files\|fn.*resolved\|.with_files\|resolved_files" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs | grep -v "let\|use\|//" | head -20
```

Find a test that confirms files are in the first user message. If one exists, that test will FAIL after this task. If none exists, add one BEFORE the change to lock in the current behavior, then INVERT it to assert the post-change behavior.

Add a new test (or modify the existing one):

```rust
    #[tokio::test]
    async fn first_turn_user_message_does_not_carry_files_after_plan_b() {
        // Build LlmNode with attachment_registry wired (so files get registered
        // and the catalog block is rendered into the system message). Provide
        // inputs.files[0] with an inline data: URI.
        //
        // Mock LlmRepository to capture the LlmCallRequest. Assert:
        //   - The first user message in request.messages is LlmMessage::user(...)
        //     (NOT user_with_files).
        //   - The first user message contains the prompt text but has no
        //     `files` / `parts` populated.
        //   - The system message contains "Documents available in this session:"
        //     (catalog from Plan A).
    }
```

Use the existing mocking patterns in this file (`MockLlmRepository`, `MockAttachmentRegistry`, real `SqliteAttachmentRegistry::new("sqlite::memory:")` — pick whichever the surrounding tests use).

- [ ] **Step 3:** Run the test. Expected: FAIL — the current code DOES use `user_with_files` when files are present.

- [ ] **Step 4: Apply the change**

Replace the block at lines ~1316-1323 with:

```rust
        // Plan B (D6): the LLM no longer receives file content in the initial
        // user message. The catalog block prepended to the system message
        // (Plan A Task 11) tells the model which documents are available; the
        // model calls load_attachment(document_id) to read content, or
        // references "$attachment:<document_id>" in tool args to forward
        // bytes without reading them. This trades a round-trip for cost
        // savings — see docs/developer_guide/31_load_attachment.md.
        if resume_answer.is_none() {
            let user_message = LlmMessage::user(prompt.to_string())?;
            messages.push(user_message.clone());
        }
```

Confirm `resolved_files` is still used elsewhere in the function (for attachment registration around line 1180-1289, for byte persistence, for catalog computation). The variable does NOT become dead — only this specific consumer changes.

- [ ] **Step 5:** Run the test. Expected: PASS.

- [ ] **Step 6: Find and update existing tests that depended on autoinject**

Search for tests that assert the first user message has files:

```bash
grep -n "user_with_files\|message\\.files\|files_in_first_message\|first.*user.*files" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs | head -20
```

Each such test now either:
- Asserts the new no-autoinject behavior (update the assertion).
- Tests a code path that's no longer reachable in production (delete or rewrite to test `load_attachment` instead).

Be conservative — if a test exercises a code path that still exists (e.g., the file registration loop), keep it but update only the user-message assertion.

If `LlmMessage::user_with_files` is no longer called from production code (only from `agent_service.rs` for the synthetic `load_attachment` injection), that's expected — it remains a supported message variant.

- [ ] **Step 7:** Run `cargo test --lib llm`. All pass.

- [ ] **Step 8:** Run `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo check --all-targets`. All clean.

- [ ] **Step 9: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(llm): D6 — stop autoinjecting file content in initial user message

The LLM no longer receives the bytes of attached files in turn 1. The
catalog block prepended to the system message (Plan A Task 11) tells the
model what documents are available; the model calls load_attachment to
read content or uses \$attachment:<id> placeholders to forward bytes
without reading them.

Trade-off: cost savings (no input tokens for unread docs) vs +1
round-trip when the model needs to read.

BREAKING for graphs that depended on autoinject. Update affected prompts
to instruct the model to call load_attachment when reading is required.

Plan B — Catalog-driven behavior."
```

---

## Task 2: Update `ATTACHMENTS_SYSTEM_PRELUDE` to explain the new contract

**Goal:** The LLM needs to know that (a) it must call `load_attachment` to read content and (b) the content is ephemeral — bytes won't survive past the current turn.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs` (or wherever `ATTACHMENTS_SYSTEM_PRELUDE` is defined — confirm via grep).

- [ ] **Step 1: Find the prelude**

```bash
grep -rn "ATTACHMENTS_SYSTEM_PRELUDE" src/libs/colmena/src/
```

Read the current value.

- [ ] **Step 2: Write a failing test**

In the same file (or wherever the prelude const is defined), add a unit test asserting the prelude mentions the two facts:

```rust
#[cfg(test)]
mod prelude_tests {
    use super::*;

    #[test]
    fn prelude_explains_no_autoinject_behavior() {
        assert!(
            ATTACHMENTS_SYSTEM_PRELUDE.contains("call load_attachment")
                || ATTACHMENTS_SYSTEM_PRELUDE.contains("load_attachment("),
            "prelude should instruct the model to call load_attachment"
        );
        assert!(
            ATTACHMENTS_SYSTEM_PRELUDE.contains("ephemeral")
                || ATTACHMENTS_SYSTEM_PRELUDE.contains("only for this turn")
                || ATTACHMENTS_SYSTEM_PRELUDE.contains("not retained"),
            "prelude should warn that load_attachment results are ephemeral"
        );
    }
}
```

- [ ] **Step 3:** Run the test. Expected: FAIL.

- [ ] **Step 4: Update the prelude**

Replace `ATTACHMENTS_SYSTEM_PRELUDE` with content like:

```rust
pub const ATTACHMENTS_SYSTEM_PRELUDE: &str = "Documents are attached to this conversation. \
You will NOT see their content automatically. The catalog below lists each document with \
its document_id. To read a document, call load_attachment(document_id). To forward a \
document to a downstream tool (e.g., http_request multipart), use the string \
\"$attachment:<document_id>\" in the tool's args.\n\n\
load_attachment results are ephemeral: the document content is available only for the turn \
in which you invoked the tool. Future turns will see a marker confirming the call happened, \
but not the content itself. Call load_attachment again if you need to re-read the document.";
```

Match the existing prelude's tone and length — if it was very short, keep it short and break the new info into two paragraphs. Adjust wording but keep the two key facts.

- [ ] **Step 5:** Run the test. Expected: PASS.

- [ ] **Step 6:** Run the full llm test module. Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs
git commit -m "feat(llm): update ATTACHMENTS_SYSTEM_PRELUDE for D6 + D7 semantics

Prelude now explicitly states:
- Documents are not auto-injected; the model must call load_attachment
  to read content.
- load_attachment results are ephemeral — content available only for
  the current turn.

Plan B — Catalog-driven behavior."
```

---

## Task 3: Make `load_attachment` ephemeral (synthetic message not persisted)

**Goal:** When the model calls `load_attachment`, the synthetic `user_with_files` message stays in the in-memory `messages` vec for the rest of the turn's ReAct iterations. But when persisting to `llm_node_history`, the synthetic message is replaced by a short text marker.

**Files:**
- Modify: `src/libs/colmena/src/llm/application/agent_service.rs`

- [ ] **Step 1: Find the load_attachment branch**

In `agent_service.rs`, around lines 328-410, the code that handles the `LOAD_ATTACHMENT` sentinel. Read it carefully — note the structure:

```rust
// (around line 376-379)
let synth = LlmMessage::user_with_files(
    format!("[Attachment requested by the model: {}]", document_id),
    vec![file_data],
)?;

// (around lines 404-409)
if let Some(user_msg) = synthetic_user {
    messages.push(user_msg.clone());
    self.conversation_repository
        .add_message(session_id, user_msg)  // ← THIS persists. Must change.
        .await?;
}
```

The bug to fix: the `add_message` call writes the synthetic `user_with_files` to the conversation repository. We want to push the synthetic to in-memory `messages` but persist a MARKER instead.

- [ ] **Step 2: Write a failing test**

Find or extend `load_attachment_sentinel_injects_synthetic_user_message_and_continues` (line ~912 of agent_service.rs).

Add a new test:

```rust
    #[tokio::test]
    async fn load_attachment_synthetic_message_is_not_persisted_to_history() {
        // Build AgentService with:
        //   - MockLlmRepository that on turn 1 emits a load_attachment tool call,
        //     then on turn 2 emits a plain assistant response.
        //   - Mock AttachmentResolver returning a known file_data.
        //   - Real InMemoryConversationRepository so we can inspect persisted msgs.
        // Execute the turn.
        //
        // Assert:
        //   - The model's in-memory `messages` (passed to LlmRepository on
        //     turn 2) contains the synthetic user_with_files (so the model
        //     reads the doc).
        //   - The conversation repository's persisted history does NOT
        //     contain a user_with_files message — instead it has a
        //     plain user-text message with a marker like
        //     "[load_attachment(\"doc-1\") was invoked. Content available
        //      only for this turn; call again to re-read.]"
    }
```

This is the test that proves the dual behavior: in-memory has the file, persisted history has the marker.

- [ ] **Step 3:** Run the test. Expected: FAIL (current code persists the synthetic with files).

- [ ] **Step 4: Make the change**

Replace the lines around 404-409:

```rust
                            if let Some(user_msg) = synthetic_user {
                                // Plan B (D7): the synthetic user_with_files
                                // message stays in the in-memory `messages`
                                // vec so the model has the doc content for
                                // the rest of this turn's ReAct iterations.
                                // But we persist a MARKER to llm_node_history
                                // — not the doc content — so future turns
                                // don't keep paying input-token cost for it.
                                // See docs/developer_guide/31_load_attachment.md.
                                messages.push(user_msg.clone());

                                let marker_text = format!(
                                    "[load_attachment(\"{}\") was invoked. Document \
                                     content was available for this turn only. Call \
                                     load_attachment again if you need to re-read it.]",
                                    document_id
                                );
                                let marker_msg = LlmMessage::user(marker_text)?;
                                self.conversation_repository
                                    .add_message(session_id, marker_msg)
                                    .await?;
                            }
```

(Adjust formatting to match the file's existing style.)

- [ ] **Step 5:** Run the new test. Expected: PASS.

- [ ] **Step 6:** Run the existing test `load_attachment_sentinel_injects_synthetic_user_message_and_continues`. It may now fail because it likely asserts persistence of the synthetic. Update its assertion to match: it should confirm the in-memory messages contain user_with_files but the persisted history contains a marker.

- [ ] **Step 7:** Run `cargo test --lib agent_service`. Expected: all pass.

- [ ] **Step 8:** Run `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo check --all-targets`. Clean.

- [ ] **Step 9: Commit**

```bash
git add src/libs/colmena/src/llm/application/agent_service.rs
git commit -m "feat(agent): D7 — load_attachment results ephemeral per turn

When the model calls load_attachment, the synthetic user_with_files
message that carries the doc content stays in the in-memory ReAct
iteration stream for the rest of the current turn. But it is NOT
persisted to llm_node_history — a short text marker takes its place.

Effect: the model reads the doc and reasons about it normally within
the turn; future turns see only the marker, saving input-token cost
on long sessions. The model can call load_attachment again to re-read.

Plan B — Cost optimization."
```

---

## Task 4: Remove `attachment_id` alias from `image_generation` tool result

**Goal:** The legacy `attachment_id` field disappears from the tool result. Only `document_id` remains. ADP frontend that parses `attachment_id` will break — that's the intended trigger for the ADP migration.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs`

- [ ] **Step 1: Find the tool result construction**

```bash
grep -n "attachment_id\|document_id\|\"images\":" src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs | head -15
```

Find the JSON construction around lines 380-390 (added in Plan A Task 4).

- [ ] **Step 2: Update the test FIRST**

The existing test `image_generation_auto_registers_artifact_in_registry` asserts both fields. Update the assertion:

```rust
        // Plan B (D8): attachment_id alias removed; only document_id remains.
        assert!(out_obj["document_id"].as_str().unwrap().starts_with("img_"));
        assert!(
            out_obj.get("attachment_id").is_none(),
            "Plan B removed the attachment_id legacy alias"
        );
        assert!(
            out_obj.get("url").is_none(),
            "Plan B removed the url field"
        );
```

The test will now FAIL because the current code still emits both.

- [ ] **Step 3:** Run the test. Expected: FAIL.

- [ ] **Step 4: Remove the fields from the tool result**

Find the tool result JSON construction. Currently:

```rust
"images": [{
    "document_id": document_id,
    "attachment_id": stored.storage_key,  // deprecated alias
    "url": stored.read_url,
    "mime_type": stored.mime_type,
    "size_bytes": stored.size_bytes
}]
```

Change to:

```rust
"images": [{
    "document_id": document_id,
    "mime_type": stored.mime_type,
    "size_bytes": stored.size_bytes
}]
```

`stored.read_url` and `stored.storage_key` are no longer surfaced in the tool result — they remain in the auto-registered conversation_attachments row for the resolver to use internally.

- [ ] **Step 5: Update `description()` text**

The tool's description currently mentions both `document_id` (canonical) and `attachment_id` (deprecated). Remove the `attachment_id` mention. Match this pattern:

```rust
fn description(&self) -> Option<&str> {
    Some("Generate an image. Returns { document_id, mime_type, size_bytes }. \
          Use \"$attachment:<document_id>\" in downstream tool args to forward \
          the image, or call load_attachment(document_id) to read it.")
}
```

(Keep prose aligned with what was there pre-Plan-B.)

- [ ] **Step 6:** Run the test. Expected: PASS.

- [ ] **Step 7:** Run the full module:
```bash
cargo test --lib image_generation
```
Expected: all pass. If any test still asserts on `attachment_id` or `url`, update it.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs
git commit -m "feat(image_generation): D8 — remove legacy attachment_id alias + url

Tool result now emits only { document_id, mime_type, size_bytes }.
BREAKING for ADP frontend that parses attachment_id or url — coordinate
with ADP team (apps/service/ia/platform/{worker,api}/src/) before
pushing to colmena develop.

The storage_key and read_url are still recorded internally on the
auto-registered conversation_attachments row for the resolver.

Plan B — Tool result schema cleanup."
```

---

## Task 5: Remove `attachment_id` alias from `image_edit`

**Goal:** Same as Task 4, applied to `image_edit`.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`

- [ ] **Step 1:** Apply the same pattern as Task 4. Update existing test assertions, remove `attachment_id` and `url` from tool result JSON, update `description()` text.

- [ ] **Step 2:** Run `cargo test --lib image_edit`. All pass.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs
git commit -m "feat(image_edit): D8 — remove legacy attachment_id alias + url

Mirrors Task 4 for image_edit node.
Plan B — Tool result schema cleanup."
```

---

## Task 6: Remove `attachment_id` alias from `tts`

**Goal:** Same as Task 4, applied to `tts`.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`

- [ ] **Step 1:** Apply the same pattern. The tts tool result uses `"audio"` (not `"images"`) as the outer key, but the inner object follows the same shape — remove `attachment_id` and `url`. Preserve fields specific to tts (`duration_ms` if present).

- [ ] **Step 2:** Run `cargo test --lib tts`. All pass.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs
git commit -m "feat(tts): D8 — remove legacy attachment_id alias + url

Mirrors Tasks 4-5 for tts node.
Plan B — Tool result schema cleanup."
```

---

## Task 7: Integration tests for Plan B behaviors

**Goal:** End-to-end test driver for the two behavioral changes. Verifies:
- A turn with input files[] produces a system message containing the catalog but a user message WITHOUT file content.
- After a load_attachment invocation in turn N, turn N+1's persisted history contains a marker, not the file content.

**Files:**
- Create: `src/libs/colmena/tests/plan_b_behavioral_test.rs`

- [ ] **Step 1: Author the test driver**

Use the established pattern from `attachment_uniform_resolution_test.rs` (Plan A Task 12). Reuse the seeding helpers if exported; otherwise inline.

Structure:

```rust
//! Plan B end-to-end tests: confirm no-autoinject + ephemeral load_attachment.

#[tokio::test]
async fn first_turn_user_message_has_no_files_only_catalog_in_system() {
    // Seed an attachment in the registry.
    // Build LlmNode with a MockLlmRepository that captures the
    // LlmCallRequest on its first invocation.
    // Drive execute() with inputs.files[] (inline data: URI).
    // Assert:
    //   - The captured request's system_message contains "Documents available"
    //     and the document_id.
    //   - The captured request's first user message is LlmMessage::user(...),
    //     no files attached.
}

#[tokio::test]
async fn load_attachment_persists_marker_not_content_to_history() {
    // Seed an attachment.
    // Build AgentService with:
    //   - MockLlmRepository: turn 1 emits load_attachment tool call;
    //                       turn 2 emits a plain "ok" assistant response.
    //   - Real InMemoryConversationRepository.
    //   - Resolver that returns the seeded file_data on resolve.
    // Execute the turn.
    // Inspect the conversation repository's persisted history:
    //   - It contains a `user` message with the marker text
    //     "[load_attachment(\"...\") was invoked. ...]"
    //   - It does NOT contain a `user_with_files` message.
    //   - It contains the assistant's final response.
}
```

- [ ] **Step 2:** Run the tests. Expected: PASS.

- [ ] **Step 3:** Run the full suite to confirm no regressions:
```bash
cargo test --lib
cargo test --test plan_b_behavioral_test
```

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/tests/plan_b_behavioral_test.rs
git commit -m "test(plan-b): end-to-end tests for no-autoinject + ephemeral load_attachment

Two integration tests confirm:
1. First-turn user message no longer carries file content; catalog is
   in the system message instead.
2. load_attachment synthetic user_with_files is in-memory only —
   conversation_repository receives a marker.

Plan B — Behavioral coverage."
```

---

## Task 8: Update docs (developer guides + CLAUDE.md)

**Goal:** Reflect the new contract in human-readable docs.

**Files:**
- Modify: `docs/developer_guide/31_load_attachment.md` — describe no-autoinject + ephemeral semantics + the marker that appears in history.
- Modify: `docs/developer_guide/32_multimedia_generation.md` (or wherever tool result schemas are documented) — note that `attachment_id` and `url` were removed from tool results.
- Modify: `CLAUDE.md` — add a "Plan B shipped" bullet.

- [ ] **Step 1: Update `31_load_attachment.md`** — add a section under the Plan A entry:

```markdown
## Plan B — Catalog-driven behavior + ephemeral content (2026-05-25)

Plan B activates the cost-optimization behaviors that Plan A laid the foundation for.

### No autoinject in the first turn

When a user attaches a document via `inputs.files[]`, the LLM does **not** receive
the document bytes in the first user message. The catalog block prepended to the
system message (Plan A Task 11) tells the model what documents are available with
their `document_id`. The model decides per-turn whether to:
- Read content — call `load_attachment(document_id)`.
- Forward the doc to a downstream tool — use `"$attachment:<document_id>"` in the
  tool's args.
- Ignore the doc — no cost incurred.

This trades a round-trip (when the model needs to read) for cost savings (no input
tokens for unread docs).

### Ephemeral load_attachment results

When the model calls `load_attachment(document_id)`, the doc content is injected
into the **in-memory ReAct iteration stream** for the rest of the current turn.
The model can reason about the content normally within the turn.

But the synthetic message carrying the content is **not persisted to
`llm_node_history`**. Instead, a short marker message is saved:

```
user: [load_attachment("Q3_report") was invoked. Document content was available
       for this turn only. Call load_attachment again if you need to re-read it.]
```

Future turns see the marker, not the content. The model retains the analysis it
produced from the doc (assistant messages stay intact), but stops paying input-token
cost for the doc on every subsequent turn.

If the model needs to re-read the doc, it calls `load_attachment` again — the
resolver re-streams the content from `OutputStorageRepository`.
```

- [ ] **Step 2: Update `32_multimedia_generation.md`** — find the section that describes the tool result format. Update to reflect the schema change:

```markdown
### Tool result schema (Plan B, 2026-05-25)

`image_generation`, `image_edit`, and `tts` tool results emit:

```json
{
  "images": [{
    "document_id": "img_revenue_chart_a1b2c3",
    "mime_type": "image/png",
    "size_bytes": 348000
  }]
}
```

(`tts` uses `"audio"` as the outer key, structure identical.)

The legacy `attachment_id` and `url` fields were removed in Plan B. Downstream
consumers (e.g., ADP frontend) that need to render the artifact must look up
the storage URL via `conversation_attachments` (joined by `document_id`) or
fetch it through a dedicated endpoint.
```

- [ ] **Step 3: Update `CLAUDE.md`** — add a bullet under "Current Status":

```markdown
- **Attachment uniform resolution Plan B shipped 2026-05-25** — LLM no longer
  auto-receives attached doc content; catalog-driven via system message.
  `load_attachment` results are ephemeral (marker in history, not content).
  `image_generation`/`image_edit`/`tts` tool results dropped legacy
  `attachment_id` and `url`; only `document_id` remains. BREAKING for ADP
  frontend. See
  [`docs/superpowers/specs/2026-05-25-attachment-uniform-resolution-design.md`](docs/superpowers/specs/2026-05-25-attachment-uniform-resolution-design.md).
```

- [ ] **Step 4: Commit**

```bash
git add docs/developer_guide/31_load_attachment.md \
        docs/developer_guide/32_multimedia_generation.md \
        CLAUDE.md
git commit -m "docs: reflect Plan B (no-autoinject + ephemeral + schema cleanup)

Documents the catalog-driven behavior, the ephemeral load_attachment
semantics with marker in history, and the tool result schema change
(removed attachment_id + url from image_generation/image_edit/tts).

Plan B — Documentation."
```

---

## Task 9: ADP coordination handoff

**Goal:** Communicate the breaking changes to the ADP team and provide the migration recipe. NO code change in colmena.

**Files:**
- Create: `docs/superpowers/specs/2026-05-25-plan-b-adp-migration-notes.md` — handoff document.

- [ ] **Step 1: Write the migration notes**

Create the file with content like:

```markdown
# Plan B — ADP Migration Notes

**Audience:** ADP team (apps/service/ia/platform/{worker,api}/src/, frontend).
**Source of truth:** colmena's `workingbranch/upload_documents_with_inline` branch (or its merge into `develop`).
**Breaking changes:** two distinct surfaces.

## 1. Tool result schema for image_generation / image_edit / tts

### Before (Plan A)

```json
{ "images": [{
  "document_id": "img_revenue_chart_a1b2c3",
  "attachment_id": "<storage-key-uuid>",
  "url": "https://storage.googleapis.com/.../signed-url",
  "mime_type": "image/png",
  "size_bytes": 348000
}]}
```

### After (Plan B)

```json
{ "images": [{
  "document_id": "img_revenue_chart_a1b2c3",
  "mime_type": "image/png",
  "size_bytes": 348000
}]}
```

### What ADP needs to change

- **Frontend rendering** — pages that render generated images today consume `url` directly. Replace with:
  - A new ADP API endpoint, e.g. `GET /api/attachments/:document_id/url` that:
    - Authenticates the user against the `agent_session_id` that owns the document.
    - Queries `conversation_attachments` for the row matching `(agent_session_id, document_id)`.
    - Returns a signed URL to the storage_key the row references, or proxies the bytes.
  - Frontend fetches the URL from this endpoint instead of taking it from the tool result.

- **Any code that joins by `attachment_id`** — switch to joining by `document_id`. The `conversation_attachments` table has a unique index on `(agent_session_id, document_id, provider)`.

### Rollout order

1. ADP frontend ships the new "fetch URL by document_id" path behind a feature flag, reading `document_id` while still falling back to `url` when present.
2. Colmena Plan B merges to `develop`. ADP worker auto-pulls colmena develop on next Cloud Build.
3. After ADP's Cloud Build completes, frontend feature flag is flipped on for canary users.
4. After canary validation, flag rolls out to 100%. Frontend code removes the `url` fallback path.
5. Future colmena change can drop the `description()`-level mention of the legacy fields.

## 2. LLM behavior — no-autoinject + ephemeral load_attachment

### Before (Plan A)

When a graph received `inputs.files[]`, the LLM's first user message included the
doc bytes via `LlmMessage::user_with_files`. The model could analyze the doc
immediately on turn 1 without explicit action.

### After (Plan B)

The model sees only the catalog (in the system message). To read content, the
model must call `load_attachment(document_id)`. The doc content is then injected
into that turn's iteration stream — but disappears from history after the turn
completes (a marker replaces it).

### What ADP needs to change

- **Graph prompts** — any graph that assumed the model would automatically see
  attached docs needs its system_prompt updated. The model should be instructed:
  *"To analyze attached documents, call load_attachment(document_id) — the
  document IDs are listed in the catalog below."*

- **Frontend UX** — if ADP shows "the agent is processing your document..."
  during turn 1, that flow may now show an extra round-trip (turn 1: model
  calls load_attachment; turn 2: model responds). Adjust spinners or add UI
  states for the multi-turn path.

- **Long-lived session cost monitoring** — Plan B reduces input-token cost on
  sessions where the same doc is referenced across many turns. ADP's cost
  dashboards may show a step-down. This is expected.

## 3. Database migration

Plan A introduced the migration `20260525000001_attachment_uniform_resolution.sql`
(additive columns on `conversation_attachments`). Plan B adds NO new migration —
schema unchanged from Plan A.

Confirm the Plan A migration ran cleanly in ADP's environments before deploying
Plan B colmena:

```bash
psql $DATABASE_URL -c "\d conversation_attachments" | grep -E "storage_key|origin|last_used_at"
```

All three columns should be present. If not, apply manually:

```bash
psql $DATABASE_URL < src/libs/colmena/migrations/postgres/20260525000001_attachment_uniform_resolution.sql
```

## 4. Validation checklist (ADP team)

Before flipping the feature flag in production:

- [ ] ADP frontend reads `document_id` from `image_generation`/`image_edit`/`tts`
      tool results.
- [ ] ADP API exposes the `GET /api/attachments/:document_id/url` endpoint (or
      equivalent).
- [ ] At least one canary graph runs `image_generation` + downstream
      `http_request` multipart with `$attachment:<document_id>` and confirms
      the image arrives at the destination intact.
- [ ] Cost dashboards show no unexpected token-usage spike. (Expected: a small
      one for catalog tokens in turn 1, offset by savings on subsequent turns.)
- [ ] Long-running agent sessions (>5 turns with the same doc loaded) show
      reduced per-turn input token cost.

## 5. Rollback plan

If Plan B breaks production:

1. ADP frontend flips the feature flag off → falls back to reading `url` from
   the tool result. **This won't work because `url` is gone.** So:
2. The actual rollback is reverting colmena's `develop` to the pre-Plan-B SHA
   and force-pushing. ADP Cloud Build re-runs against the older colmena.
3. Plan A foundation remains intact during rollback — no data loss.
```

- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/specs/2026-05-25-plan-b-adp-migration-notes.md
git commit -m "docs: ADP migration notes for Plan B breaking changes

Self-contained handoff doc for the ADP team covering:
- Tool result schema change (removed attachment_id + url)
- Required ADP frontend + API changes
- LLM behavior change (no-autoinject, ephemeral load_attachment)
- Required graph prompt updates
- Rollout order, validation checklist, rollback plan

Plan B — Coordination."
```

---

## Verification checklist

After all tasks land, run:

- [ ] `cargo fmt --check` — clean
- [ ] `cargo clippy --all-targets -- -D warnings` — clean
- [ ] `cargo test --verbose` — all pass (unit + integration + doctests)
- [ ] `source .env && cargo test -- --ignored` — env-gated tests pass
- [ ] Manual smoke: run `cargo run --bin dag_engine -- run tests/graphs/agents/upload_inline_to_endpoint.json --agent-session-id smoke_b_$(date +%s)` with `.env` sourced. Confirm:
  - The LLM gets the catalog in its system prompt (look for "Documents available" in the log output).
  - The LLM does NOT receive the doc content in its first user message.
  - When the LLM calls `load_attachment`, the doc is loaded for the turn, then the persisted history shows the marker.
- [ ] ADP worker sweep against `apps/service/ia/platform/{worker,api}/src/` confirming no `attachment_id` or `url` parsing remains in tool-result handling code (this is informational — the actual ADP work is the team's, but flag if you find anything trivial).

---

## Self-review

After the implementation completes, fresh-eyes pass on the diff:

**Spec coverage:**
- D6 (no autoinject in turn 1) → Task 1.
- D7 (ephemeral load_attachment with marker) → Task 3, supporting prelude in Task 2.
- D8 full (remove attachment_id alias + url) → Tasks 4-6.
- ADP coordination → Task 9.
- Tests → Task 7.
- Docs → Task 8.

**Placeholder scan:** none expected.

**Type consistency:**
- `LlmMessage::user_with_files` constructor still exists (used in `agent_service.rs` for the in-memory synthetic injection). Don't accidentally remove it.
- Tool result JSON keys must match across `image_generation`, `image_edit`, and `tts` post-cleanup. After Plan B, all three emit `{document_id, mime_type, size_bytes}` shape (with `tts` adding `duration_ms` etc.).

**Ambiguity check:**
- "Marker" wording in the persisted history — consistent string across all paths so future code can detect/parse it if needed. Pin a constant like `LOAD_ATTACHMENT_MARKER_PREFIX = "[load_attachment("` if helpful.
- ADP migration order is suggested but the actual sequencing depends on the ADP team's release cadence; the notes acknowledge this.

**Scope check:** Plan B is focused on the three Plan-A-deferred items (D6, D7, full D8). TTL cleanup (D10) and any new feature work is correctly out of scope.

---

## Risks

1. **Graph regressions in production.** Any graph relying on the autoinject behavior will return "no veo el archivo"-style responses until its prompt is updated. Mitigation: run a sweep of ADP graphs before push, list the ones that pass files[] and don't have a load_attachment instruction, surface them in the migration notes.

2. **ADP frontend break.** Will lose generated-image rendering until the URL-by-document_id endpoint ships. Mitigation: coordinate timing via Plan-B-ADP-migration-notes; feature-flag the frontend change behind a flag the ADP team controls.

3. **Marker confusion for the model.** If the model is poorly instructed about the ephemeral semantics, it may try to re-reason about the doc content in turn N+1 without realizing the bytes are gone. Mitigation: ATTACHMENTS_SYSTEM_PRELUDE update in Task 2 explicitly warns about this; the marker text itself reminds the model to "Call load_attachment again if you need to re-read."

4. **Cost regressions in cold sessions.** Turns where the model would have used the autoinjected content now pay an extra round-trip. For one-shot Q&A flows ("here's a PDF, summarize it"), turn-1 latency increases by one round-trip. Mitigation: graph authors can mitigate by adding an explicit "first action: call load_attachment for the user's uploaded file" instruction in the system_prompt.

5. **Test regressions outside the touched modules.** Some end-to-end tests in ADP may assume autoinject. Mitigation: the ADP sweep step in Task 9 surfaces these so the ADP team can update.
