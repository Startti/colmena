# Interaction Boundary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the token-budget boundary between the memory zone and the recent window with a structural one — the start of the current interaction — so no part of an open interaction can be summarised away.

**Architecture:** `build_compacted_messages` currently asks `recent_boundary_by_tokens` where to cut, which walks backwards accumulating tokens. This plan replaces that call with `current_interaction_start`, which scans backwards for the last `assistant` message carrying no tool calls and returns the index just after it. That message is, by construction, the ReAct loop's exit condition, so everything after it belongs to the interaction still in flight. The token budget stops governing the cut, the pair guard becomes unnecessary, and `recent_boundary_by_tokens` loses its only caller.

**Tech Stack:** Rust 1.95 (pinned in `rust-toolchain.toml`), `cargo test`, `tokio::test` for async tests, `mockall` where doubles are needed. Package name is `colmena_dag_engine`, **not** `colmena`.

## Global Constraints

- `[lints.rust] warnings = "deny"` in `Cargo.toml` — any rustc warning fails the build. An unused import, an unused variable, or a dead function is a build failure, not a warning.
- Run `cargo fmt` before every commit. `cargo clippy --all-targets` must be clean.
- `cargo test --lib <module>` is the fast loop. `cargo test --verbose` is the gate before push — it is what CI runs, and `--lib` alone hides integration and doctest failures.
- Documentation ships in the same change as the code. A `PreToolUse` hook blocks `git push` when the outgoing diff touches repo files with no documentation.
- Prose in `docs/` is Spanish. Code comments and API docs are English.
- Conventional commits only. Allowed prefixes: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`. **Never** add `Co-Authored-By` or any AI attribution.
- The design this implements: [`docs/superpowers/specs/2026-08-22-interaction-scoped-memory-and-result-maps-design.md`](../specs/2026-08-22-interaction-scoped-memory-and-result-maps-design.md).

---

## File Structure

| File | Responsibility after this plan |
|---|---|
| `src/libs/colmena/src/llm/application/history_compaction.rs` | Gains `current_interaction_start` (pure, testable in isolation). `build_compacted_messages` loses its `recent_token_budget` parameter and its pair guard. `recent_boundary_by_tokens`, `RECENT_TOKEN_BUDGET` and their tests are removed. |
| `src/libs/colmena/src/llm/application/agent_service.rs` | The single call site drops the budget argument. Nothing else changes. |
| `docs/developer_guide/15_memory_guide.md` | §Compactación describes a structural boundary; the two "Limitación conocida" outcomes are replaced by the new guarantee. |
| `docs/CHANGELOG_2026-08.md` | New section. |
| `docs/agent_context/audit/src__libs__colmena__src__llm__application__history_compaction.rs.md` | Re-derived from the file on disk. |
| `tests/graphs/agents/interaction_boundary_e2e_turn{1,2}.json` | New two-run E2E proving a user question survives verbatim next to an oversized tool result. |

---

### Task 1: `current_interaction_start`

A pure function over a message slice. No I/O, no async — testable on its own.

**Files:**
- Modify: `src/libs/colmena/src/llm/application/history_compaction.rs` (add the function next to `classify_value_class`, and its tests inside the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `LlmMessage`, `MessageRole` — already imported at the top of the file.
- Produces: `pub fn current_interaction_start(messages: &[LlmMessage]) -> usize`. Task 2 calls it. Returns an index in `0..=messages.len()`; `messages.len()` only when the newest message itself closes an interaction (nothing is open).

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests`. `tc(id, name)` already exists in that module and builds a `ToolCall`.

```rust
    #[test]
    fn interaction_start_is_after_the_last_assistant_without_tool_calls() {
        let closing = LlmMessage::assistant("listo".to_string()).unwrap();
        let msgs = vec![
            LlmMessage::user("vieja".into()).unwrap(),
            closing.clone(),
            LlmMessage::user("actual".into()).unwrap(),
            LlmMessage::assistant_with_tool_calls(String::new(), vec![tc("c1", "sql_query")])
                .unwrap(),
            LlmMessage::tool("c1".into(), "filas".into()).unwrap(),
        ];
        assert_eq!(current_interaction_start(&msgs), 2);
    }

    #[test]
    fn an_assistant_with_an_empty_tool_call_vec_also_closes() {
        // The ReAct loop returns on BOTH `Some(vec![])` and `None`. Detecting
        // the close with `is_none()` alone would miss the non-streaming path.
        let msgs = vec![
            LlmMessage::user("x".into()).unwrap(),
            LlmMessage::assistant_with_tool_calls("listo".to_string(), vec![]).unwrap(),
            LlmMessage::user("actual".into()).unwrap(),
        ];
        assert_eq!(current_interaction_start(&msgs), 2);
    }

    #[test]
    fn several_unanswered_user_messages_all_belong_to_the_open_interaction() {
        let msgs = vec![
            LlmMessage::assistant("listo".to_string()).unwrap(),
            LlmMessage::user("uno".into()).unwrap(),
            LlmMessage::user("dos".into()).unwrap(),
            LlmMessage::user("tres".into()).unwrap(),
        ];
        assert_eq!(current_interaction_start(&msgs), 1);
    }

    #[test]
    fn a_closing_assistant_as_the_newest_message_leaves_nothing_open() {
        // Reachable on a resume with no new prompt: the newest stored message is
        // the previous turn's final answer. Task 2 must not let this empty the
        // recent window.
        let msgs = vec![
            LlmMessage::user("x".into()).unwrap(),
            LlmMessage::assistant("listo".to_string()).unwrap(),
        ];
        assert_eq!(current_interaction_start(&msgs), msgs.len());
    }

    #[test]
    fn without_a_closed_interaction_everything_is_current() {
        let msgs = vec![
            LlmMessage::user("x".into()).unwrap(),
            LlmMessage::assistant_with_tool_calls(String::new(), vec![tc("c1", "sql_query")])
                .unwrap(),
            LlmMessage::tool("c1".into(), "filas".into()).unwrap(),
        ];
        assert_eq!(current_interaction_start(&msgs), 0);
        assert_eq!(current_interaction_start(&[]), 0);
    }
```

- [ ] **Step 2: Run the tests and confirm they fail**

```bash
cargo test --lib history_compaction
```

Expected: four failures, `cannot find function 'current_interaction_start' in this scope`.

- [ ] **Step 3: Write the implementation**

Add above `use crate::llm::application::tool_digest::digest_tool_result;`:

```rust
/// Index where the open interaction starts: right after the last `assistant`
/// message that carried no tool calls.
///
/// The ReAct loop in `agent_service` terminates **if and only if** the assistant
/// returned no tool calls — condition at `agent_service.rs:354`, `return` at
/// `:360` for `Some(empty)`, `return` at `:677` for `None`. A persisted
/// `assistant` with no tool calls is therefore, by construction, the close of an
/// interaction, and everything after it is still in flight.
///
/// Returns `0` when no interaction has closed yet: the whole history belongs to
/// the current one.
pub fn current_interaction_start(messages: &[LlmMessage]) -> usize {
    for i in (0..messages.len()).rev() {
        let closes = matches!(messages[i].role(), MessageRole::Assistant)
            && messages[i].tool_calls().is_none_or(|tcs| tcs.is_empty());
        if closes {
            return i + 1;
        }
    }
    0
}
```

`is_none_or` covers both `None` and `Some(empty)`. If clippy prefers a different form on this toolchain, take clippy's suggestion — `warnings = "deny"` makes its opinion binding.

- [ ] **Step 4: Run the tests and confirm they pass**

```bash
cargo test --lib history_compaction
```

Expected: all four pass. The pre-existing tests still pass — nothing calls the new function yet.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets
git add src/libs/colmena/src/llm/application/history_compaction.rs
git commit -m "feat(llm): add current_interaction_start boundary helper"
```

---

### Task 2: Rewire `build_compacted_messages` onto the structural boundary

**Files:**
- Modify: `src/libs/colmena/src/llm/application/history_compaction.rs` (the body of `build_compacted_messages`, its signature, and the fixtures of the async tests that fed it a budget)
- Modify: `src/libs/colmena/src/llm/application/agent_service.rs` around line 205

**Interfaces:**
- Consumes: `current_interaction_start` from Task 1.
- Produces: `build_compacted_messages(stored, key, repo, summarizer) -> Vec<LlmMessage>` — **four** parameters. The `recent_token_budget: usize` parameter is gone. Task 3 verifies the resulting behaviour end to end; Plan 2 builds on this signature.

- [ ] **Step 1: Write the failing test**

This is the behaviour the whole plan exists for. Add inside `mod tests`:

```rust
    /// The defect this plan closes: with a budget-driven boundary, an oversized
    /// tool result pushed the cut back far enough to swallow the question that
    /// triggered it. The user's own message must stay verbatim.
    #[tokio::test]
    async fn the_current_question_survives_next_to_an_oversized_tool_result() {
        let repo = Arc::new(InMemoryConversationRepository::new());
        let k = ckey();
        let question = "según el contrato de arriba, qué pasa si el proveedor se demora";

        repo.add_message(&k, LlmMessage::user("hola".into()).unwrap())
            .await
            .unwrap();
        repo.add_message(&k, LlmMessage::user("otra vieja".into()).unwrap())
            .await
            .unwrap();
        repo.add_message(&k, LlmMessage::user("vieja".into()).unwrap())
            .await
            .unwrap();
        // Closes the previous interaction.
        repo.add_message(&k, LlmMessage::assistant("listo".to_string()).unwrap())
            .await
            .unwrap();
        // The open interaction starts here.
        repo.add_message(&k, LlmMessage::user(question.into()).unwrap())
            .await
            .unwrap();
        repo.add_message(
            &k,
            LlmMessage::assistant_with_tool_calls(String::new(), vec![tc("c1", "sql_query")])
                .unwrap(),
        )
        .await
        .unwrap();
        repo.add_message(&k, LlmMessage::tool("c1".into(), "z".repeat(40_000)).unwrap())
            .await
            .unwrap();

        let stored = repo.get_with_summaries(&k).await.unwrap();
        let out = build_compacted_messages(&stored, &k, repo.as_ref(), None).await;

        assert!(
            out.iter().any(|m| m.content() == question),
            "the open interaction's question must travel verbatim, not summarised"
        );
        assert!(
            out.iter().any(|m| m.content().chars().count() == 40_000),
            "the tool result of the open interaction must travel verbatim too"
        );
    }

    /// The recent window must never be empty, even when nothing is open.
    #[tokio::test]
    async fn a_closed_newest_interaction_still_leaves_a_recent_message() {
        let repo = Arc::new(InMemoryConversationRepository::new());
        let k = ckey();
        for i in 0..4 {
            repo.add_message(&k, LlmMessage::user(format!("vieja {i}")).unwrap())
                .await
                .unwrap();
        }
        repo.add_message(&k, LlmMessage::assistant("la respuesta final".to_string()).unwrap())
            .await
            .unwrap();

        let stored = repo.get_with_summaries(&k).await.unwrap();
        let out = build_compacted_messages(&stored, &k, repo.as_ref(), None).await;

        let last = out.last().expect("output is never empty");
        assert_ne!(
            last.role(),
            &MessageRole::System,
            "the summary block must not be the last message on the wire"
        );
        assert_eq!(last.content(), "la respuesta final");
    }
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
cargo test --lib the_current_question_survives
```

Expected: a compile error — `build_compacted_messages` still takes five arguments. That compile failure **is** the red state; do not add the budget argument to silence it.

- [ ] **Step 3: Change the signature and the body**

In `build_compacted_messages`, delete the `recent_token_budget: usize` parameter. Then replace the boundary block. What is there today:

```rust
    let classes = classify_value_class(&messages);
    let mut b = recent_boundary_by_tokens(&messages, &classes, recent_token_budget);

    // Guard de pares: no cortar dejando un Tool sin su Assistant.
    while b > keep_first && matches!(messages[b].role(), MessageRole::Tool) {
        b -= 1;
    }
    if b <= keep_first {
        return messages;
    }
```

What it becomes:

```rust
    let classes = classify_value_class(&messages);
    // Structural boundary: everything from the open interaction's first message
    // onward travels verbatim, whatever it weighs. The pair guard that used to
    // live here is unnecessary now — the boundary lands on an interaction's
    // first message, which can never be a `Tool` orphaned from its `Assistant`.
    let mut b = current_interaction_start(&messages);
    // Nothing is open: the newest message closed its own interaction, which a
    // resume with no new prompt reaches. Keep that closing message in the recent
    // window instead of shipping a prompt whose only non-summary content is the
    // system block — Anthropic and Gemini hoist the summary out of the message
    // array, so an empty recent window leaves the model reading an old turn as
    // the newest thing anyone said.
    if b == messages.len() {
        b -= 1;
    }
    if b <= keep_first {
        return messages;
    }
```

`b - 1` cannot underflow: the early return above guarantees `total > keep_first + 1`, so `messages.len() >= 4`.

- [ ] **Step 4: Fix the single external call site**

`agent_service.rs` around line 205 currently ends the call with the budget argument. Remove that line so the call passes four arguments:

```rust
        let base_compacted = crate::llm::application::history_compaction::build_compacted_messages(
            &stored_now,
            session_id,
            self.conversation_repository.as_ref(),
            self.message_summarizer.as_ref(),
        )
        .await;
```

- [ ] **Step 5: Delete the tests that only ever tested budget arithmetic**

These pin a boundary that no longer exists. They pass today only because the code they describe is still there — repairing their fixtures would be work thrown away, since Step 6 deletes the code underneath them.

Remove from `mod tests`:

| Test | Why it goes |
|---|---|
| `recent_boundary_is_always_a_valid_index` | the invariant sweep of a function being deleted |
| `recent_boundary_counts_only_content_tokens` | asserts budget accumulation |
| `oversized_message_leaves_the_recent_window_on_the_next_turn` | the one-turn cost property was a consequence of the budget walk |
| `repro_adp_panic_last_content_message_alone_exceeds_budget` | the panic needed the leaked index; the function is gone |
| `repro_panic_also_fires_on_a_large_user_prompt` | same |
| `parallel_tool_calls_with_oversized_last_result_ship_the_history_raw` | pins the pair guard's raw return, and the pair guard is gone |

Also delete the `Shape` enum and the `build(n, size, shape)` helper — used only by the invariant sweep.

- [ ] **Step 6: Delete the budget boundary itself**

Now that nothing references them:

```bash
grep -rn "recent_boundary_by_tokens\|RECENT_TOKEN_BUDGET" src/ --include='*.rs'
```

Expected: no hits outside the definitions you are about to remove. **If anything else appears, stop and report it** — a caller exists that this plan did not account for.

Remove `pub const RECENT_TOKEN_BUDGET` and `pub fn recent_boundary_by_tokens` in full, doc comments included. `est_tokens` may become unused; `warnings = "deny"` will tell you. Delete it if so, leave it if the digest path still uses it.

- [ ] **Step 7: Repair the fixtures of the tests that survive**

These test summarisation and digest behaviour — still valid — but their fixtures used a small budget to force an old zone. A budget no longer creates one, so each needs a **closing assistant** (`LlmMessage::assistant("cierre".to_string()).unwrap()`) inserted right after the messages meant to be summarised, otherwise `current_interaction_start` returns `0`, `b <= keep_first` fires, and they receive the raw history.

| Test | What it needs |
|---|---|
| `short_messages_pass_verbatim_no_summary_block` | only drop the final argument — it asserts the short-history early return, which is unchanged |
| `old_long_nl_gets_summarized_and_cached_recent_stays_full` | a closing assistant after the messages it expects summarised, then drop the argument |
| `structured_tool_result_becomes_digest_without_calling_summarizer` | a closing assistant after the structured tool result at idx 2, so the digest path still runs on it |
| `oversized_newest_user_prompt_stays_verbatim` | a closing assistant before the oversized message, so a summary zone exists |
| `oversized_newest_assistant_stays_verbatim` | same |
| `recent_window_is_never_empty` | same; `build_oversized_newest_tool_fixture` needs the closing assistant added inside it |

Do **not** weaken an assertion to make a test pass. If one can no longer express its intent, delete it and say so in your report rather than leaving a test that asserts nothing.

- [ ] **Step 8: Run the full suite**

```bash
cargo fmt && cargo clippy --all-targets && cargo test --verbose
```

Expected: 0 failures, no dead-code warnings. Report the real counts.

- [ ] **Step 9: Re-derive the audit doc**

`CLAUDE.md` declares `docs/agent_context/audit/…history_compaction.rs.md` ground truth for agents and humans, and it cites line numbers for every symbol and test. This task moves nearly all of them.

Re-derive **every** citation from the file on disk. Do not shift them by arithmetic — deletions do not move things by a constant. List the current positions first:

```bash
grep -n "^pub fn \|^fn \|^pub const \|^pub async fn \|^    fn \|^    async fn " \
  src/libs/colmena/src/llm/application/history_compaction.rs
```

Then rewrite the doc's Symbols and Tests sections to match: drop the entries for everything deleted, add one for `current_interaction_start`.

- [ ] **Step 10: Commit**

```bash
git add src/libs/colmena/src/llm/application/history_compaction.rs src/libs/colmena/src/llm/application/agent_service.rs docs/agent_context/audit/
git commit -m "feat(llm): anchor the compaction boundary to the current interaction"
```

---

### Task 3: Documentation and live E2E

The project rule is explicit: a unit test does not substitute for exercising the change through the real DAG engine.

**Files:**
- Create: `tests/graphs/agents/interaction_boundary_e2e_turn1.json`
- Create: `tests/graphs/agents/interaction_boundary_e2e_turn2.json`
- Modify: `docs/developer_guide/15_memory_guide.md`
- Modify: `docs/CHANGELOG_2026-08.md`

**Interfaces:**
- Consumes: the behaviour delivered by Tasks 1–3. Produces no code surface.

- [ ] **Step 1: Write the two E2E graphs**

Model them on `tests/graphs/agents/history_compaction_oversized_prompt_turn{1,2}.json`, already in the repo — same shape, same conventions, real registered node types only (`trigger_webhook`, `llm_call`, `python_script`, `output`; verify against `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`). Secrets only as `${GEMINI_API_KEY}` / `${DATABASE_URL}` placeholders, never literals.

Turn 1 asks something the agent answers with a tool call and a final reply — that final reply is what closes the interaction. Turn 2 asks a question whose answer requires a tool returning a large result, so the question and the result land in the same open interaction.

- [ ] **Step 2: Run them live**

```bash
set -a; source /Users/danielgarcia/startti/colmena/.env; set +a
unset COLMENA_LOCAL
export COLMENA_DUMP_PROMPT_FULL=1
cargo run --bin dag_engine -- run tests/graphs/agents/interaction_boundary_e2e_turn1.json --agent-session-id ib_e2e_001
cargo run --bin dag_engine -- run tests/graphs/agents/interaction_boundary_e2e_turn2.json --agent-session-id ib_e2e_001
```

Save the SSE under `/tmp/colmena_e2e/`. Never print or commit key values.

- [ ] **Step 3: Assert against the captured wire**

In the turn-2 dump, the question from turn 2 **and** the large tool result must both appear verbatim, and neither may appear as a `[Tn]` line inside the `## Conversation summary` block. If either is summarised, the boundary is not anchored where this plan says — stop and report rather than adjusting the assertion.

If the E2E cannot run for an environmental reason (no `DATABASE_URL`, no API key, network blocked), **stop and say so precisely.** Do not claim verification you did not perform.

- [ ] **Step 4: Update the guide**

In `docs/developer_guide/15_memory_guide.md` §Compactación, replace the section describing the budget-driven recent window and both "Limitación conocida" outcomes. The new text states: the boundary is the start of the current interaction, detected structurally as the position after the last `assistant` with no tool calls; everything from there travels verbatim regardless of size; the token budget no longer decides where the conversation is cut.

Then grep the whole `docs/` tree for `RECENT_TOKEN_BUDGET`, "ventana de recientes", "presupuesto" and "2.500 tokens" **outside** the files you touched. That sweep is the step that gets skipped.

- [ ] **Step 5: Write the CHANGELOG entry**

A new section in `docs/CHANGELOG_2026-08.md` following the format of the existing ones: what changed, what was measured (real numbers from Step 2 and from `cargo test --verbose`), reference documentation, and status.

State plainly what this costs: an open interaction now travels verbatim whatever it weighs, so a long one with several large tool results can push the request toward the provider's context ceiling. That is a deliberate trade, not an oversight, and the guide should say so.

- [ ] **Step 6: Commit**

```bash
git add tests/graphs/agents/interaction_boundary_e2e_turn1.json tests/graphs/agents/interaction_boundary_e2e_turn2.json docs/
git commit -m "docs(llm): document the interaction boundary and verify it end to end"
```

---

## What this plan does NOT do

- **The map for oversized structured results** and the `from_tool_call` binding on `data_run_python` — Plan 2. Until it lands, an oversized tool result in an open interaction travels whole. That is the design's intent, and the cost is real and stated above.
- **Role-based pinning of `system`** (zone 0) — Plan 3, blocked on the Anthropic adapter collapsing multiple `system` messages by overwrite. `messages[..SUMMARY_KEEP_FIRST_MSGS]` stays positional here.
