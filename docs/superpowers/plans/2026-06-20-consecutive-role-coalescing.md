# Consecutive-role coalescing (failed-turn self-heal) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Stop a mid-turn failure from permanently poisoning a conversation. When the assembled request has adjacent messages of the same role (e.g. two `user` rows left by a turn that failed after persisting the user message but before saving the assistant reply), merge them into one instead of hard-failing at `LlmRequest::new`. Self-heals already-broken conversations; touches the wire format only, never persistence.

**Architecture:** Add a pure domain helper `coalesce_consecutive_same_role(Vec<LlmMessage>) -> Vec<LlmMessage>` in `llm/domain/llm_request.rs`, called at the very top of `LlmRequest::new` (the single choke point every provider request flows through). Consecutive `Tool` messages stay un-merged (legitimate parallel tool results). The existing consecutive-role validation is kept as a defensive net (now unreachable for user/assistant/system after coalescing). Persistence (`add_message`) is unchanged, so `recall_history` still returns every original verbatim.

**Tech Stack:** Rust, domain layer (zero infra deps), pure functions.

---

## Background (the bug this fixes)

Observed live against deployed dev (2026-06-20). Sequence:
1. `AgentService::run` persists the user message **before** the ReAct loop ([`agent_service.rs:181`](../../../src/libs/colmena/src/llm/application/agent_service.rs)).
2. The turn fails mid-flight (a transient `Redis error` in the ADP worker layer, a timeout, a tool crash, a process restart, …) **before** the first assistant message is saved ([`agent_service.rs:332`](../../../src/libs/colmena/src/llm/application/agent_service.rs)). A dangling `user` row remains in `llm_node_history`.
3. The next turn assembles `[…, user(dangling), user(new)]`; `LlmRequest::new` validates consecutive roles ([`llm_request.rs:30-47`](../../../src/libs/colmena/src/llm/domain/llm_request.rs)) and returns `LlmError::ConsecutiveRoles` → the turn fails **before reaching the provider**.
4. Every retry appends another `user` → permanently stuck. Only a brand-new chat recovers.

Providers (Gemini/Anthropic) require strictly alternating user/assistant turns anyway, so **normalizing to that (coalescing) is the correct wire shape** — not a patch. It also self-heals existing poisoned conversations (no DB surgery needed) because it runs at assembly time.

## Design decisions (locked)

- **Where:** at the top of `LlmRequest::new`, so ALL request paths (agent loop, single calls, streaming, summarizer) are protected by one change.
- **What merges:** adjacent messages with the **same role**, EXCEPT `Tool` (consecutive tool results are legitimate and map to distinct `tool_call_id`s — leave them).
- **Merge semantics:** `content` = the non-empty parts joined by `"\n\n"`; `tool_calls` = concatenation (assistant); `files` = concatenation (multimodal user). Role preserved.
- **No data loss:** storage is untouched; even if an exotic field combination isn't perfectly merged into the prompt, `recall_history` still has the originals losslessly.
- **Keep the validation** loop as a defensive net (so `LlmError::ConsecutiveRoles` stays referenced — no `dead_code` failure under `warnings = "deny"`). After coalescing it never triggers for user/assistant/system; `Tool` is still allowed.
- **Out of scope (separate, optional follow-up):** rolling back the dangling user message on turn failure in `agent_service` (prevents the cause). Coalescing alone closes the user-facing problem and self-heals; the rollback is a complementary hardening parked for later.

---

### Task 1: Pure coalescing helper

**Files:**
- Modify: `src/libs/colmena/src/llm/domain/llm_request.rs`

- [ ] **Step 1: Write the failing tests.** Add inside the existing `#[cfg(test)] mod tests` block (the module already imports `MessageRole`; add `ToolCall`/`FunctionCall` imports if a test needs them — see below):

```rust
    #[test]
    fn coalesces_two_consecutive_user_messages() {
        let msgs = vec![
            LlmMessage::user("primera pregunta".into()).unwrap(),
            LlmMessage::user("segunda pregunta".into()).unwrap(),
        ];
        let out = coalesce_consecutive_same_role(msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role(), &MessageRole::User);
        assert_eq!(out[0].content(), "primera pregunta\n\nsegunda pregunta");
    }

    #[test]
    fn coalesces_three_plus_consecutive_and_leaves_alternating_intact() {
        let msgs = vec![
            LlmMessage::user("u1".into()).unwrap(),
            LlmMessage::assistant("a1".into()).unwrap(),
            LlmMessage::user("u2".into()).unwrap(),
            LlmMessage::user("u3".into()).unwrap(),
            LlmMessage::user("u4".into()).unwrap(),
        ];
        let out = coalesce_consecutive_same_role(msgs);
        // u1 | a1 | (u2+u3+u4)
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].content(), "u2\n\nu3\n\nu4");
    }

    #[test]
    fn does_not_coalesce_consecutive_tool_messages() {
        let msgs = vec![
            LlmMessage::tool("call_a".into(), "result a".into()).unwrap(),
            LlmMessage::tool("call_b".into(), "result b".into()).unwrap(),
        ];
        let out = coalesce_consecutive_same_role(msgs);
        assert_eq!(out.len(), 2, "parallel tool results must stay separate");
    }

    #[test]
    fn merges_assistant_tool_calls_when_coalescing_assistants() {
        let tc = |id: &str| {
            ToolCall::new(
                id.to_string(),
                FunctionCall { name: "f".into(), arguments: "{}".into() },
            )
        };
        let msgs = vec![
            LlmMessage::assistant_with_tool_calls("".into(), vec![tc("c1")]).unwrap(),
            LlmMessage::assistant_with_tool_calls("texto".into(), vec![tc("c2")]).unwrap(),
        ];
        let out = coalesce_consecutive_same_role(msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tool_calls().map(|t| t.len()), Some(2));
        assert_eq!(out[0].content(), "texto");
    }

    #[test]
    fn empty_and_singleton_are_passthrough() {
        assert!(coalesce_consecutive_same_role(vec![]).is_empty());
        let one = vec![LlmMessage::user("hola".into()).unwrap()];
        assert_eq!(coalesce_consecutive_same_role(one).len(), 1);
    }
```

For the tool-calls test, ensure the test module imports exist near the top of `mod tests`:
```rust
    use crate::llm::domain::tools::{FunctionCall, ToolCall};
```

- [ ] **Step 2: Run to verify they fail.** `cargo test -p colmena_dag_engine --lib llm_request` — expect compile error: `coalesce_consecutive_same_role` not found.

- [ ] **Step 3: Implement the helper.** Add these free functions ABOVE `impl LlmRequest` in `llm_request.rs` (add imports at the top of the file as needed: `use crate::llm::domain::{MessageRole, ToolCall};` and `use crate::llm::domain::llm_message::FileData;` — verify the exact `FileData` path; it is defined in `llm/domain/llm_message.rs`):

```rust
/// Merge adjacent messages that share the same role — EXCEPT `Tool`, where
/// consecutive entries are legitimate (parallel tool results keyed by distinct
/// `tool_call_id`). Providers require strictly alternating user/assistant
/// turns; a turn that fails after persisting the user message leaves a dangling
/// `user` row that would otherwise make every later turn fail at
/// `LlmRequest::new`. Coalescing normalizes the wire shape and self-heals such
/// conversations. Pure; never touches persistence (recall_history keeps the
/// originals verbatim).
pub fn coalesce_consecutive_same_role(messages: Vec<LlmMessage>) -> Vec<LlmMessage> {
    let mut out: Vec<LlmMessage> = Vec::with_capacity(messages.len());
    for msg in messages {
        let mergeable = matches!(out.last(), Some(last)
            if last.role() == msg.role() && *msg.role() != MessageRole::Tool);
        if mergeable {
            let prev = out.pop().expect("checked non-empty");
            out.push(merge_same_role(prev, msg));
        } else {
            out.push(msg);
        }
    }
    out
}

/// Merge two same-role messages: join non-empty contents, concat tool_calls and
/// files. Construction is infallible for valid inputs (the only `new` failure is
/// empty content for a non-assistant role, and the joined content of two valid
/// non-assistant messages is non-empty).
fn merge_same_role(a: LlmMessage, b: LlmMessage) -> LlmMessage {
    let role = *a.role();
    let content = [a.content(), b.content()]
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut tool_calls: Vec<ToolCall> = Vec::new();
    if let Some(tc) = a.tool_calls() {
        tool_calls.extend_from_slice(tc);
    }
    if let Some(tc) = b.tool_calls() {
        tool_calls.extend_from_slice(tc);
    }

    let mut files: Vec<FileData> = Vec::new();
    if let Some(f) = a.files() {
        files.extend_from_slice(f);
    }
    if let Some(f) = b.files() {
        files.extend_from_slice(f);
    }

    let built = if role == MessageRole::Assistant && !tool_calls.is_empty() {
        LlmMessage::assistant_with_tool_calls(content, tool_calls)
    } else if role == MessageRole::User && !files.is_empty() {
        LlmMessage::user_with_files(content, files)
    } else {
        LlmMessage::new(role, content)
    };

    built.unwrap_or_else(|_| {
        // Unreachable for valid inputs; keep total + panic-free.
        LlmMessage::new(role, " ".to_string())
            .unwrap_or_else(|_| LlmMessage::assistant(String::new()).expect("assistant allows empty"))
    })
}
```

> Note: `FileData` must be `Clone` for `extend_from_slice` — it is (it derives Clone in `llm_message.rs`). If the exact import path differs, adapt it; the type is the one returned by `LlmMessage::files()`.

- [ ] **Step 4: Run the helper tests.** `cargo test -p colmena_dag_engine --lib llm_request` — the 5 new tests pass. (The pre-existing `test_request_creation_fails_on_consecutive_roles` still fails — fixed in Task 2.)

- [ ] **Step 5: fmt + clippy.** `cargo fmt && cargo clippy -p colmena_dag_engine --lib 2>&1 | tail -20` — no warnings.

- [ ] **Step 6: Commit.**
```bash
git add src/libs/colmena/src/llm/domain/llm_request.rs
git commit -m "$(cat <<'EOF'
feat(llm): add pure coalesce_consecutive_same_role helper

Merges adjacent same-role messages (except Tool) — content joined, tool_calls
and files concatenated. Pure domain helper; foundation for self-healing
conversations poisoned by a mid-turn failure that left a dangling user message.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Wire coalescing into `LlmRequest::new`

**Files:**
- Modify: `src/libs/colmena/src/llm/domain/llm_request.rs` (the `new` fn + the existing consecutive-role test).

- [ ] **Step 1: Update the existing test to reflect the new behavior.** Replace `test_request_creation_fails_on_consecutive_roles` (it currently asserts an error) with a test that asserts coalescing now makes it succeed:

```rust
    #[test]
    fn consecutive_user_messages_are_coalesced_into_one() {
        let config = create_test_config();
        let messages = vec![
            LlmMessage::new(MessageRole::User, "Hello".to_string()).unwrap(),
            LlmMessage::new(MessageRole::User, "How are you?".to_string()).unwrap(),
        ];
        let request = LlmRequest::new(messages, config, false).unwrap();
        assert_eq!(request.message_count(), 1);
        assert_eq!(request.messages()[0].content(), "Hello\n\nHow are you?");
    }
```

Also add a test mirroring the real poisoned history:

```rust
    #[test]
    fn poisoned_history_with_dangling_user_self_heals() {
        let config = create_test_config();
        // user, assistant, [dangling user from a failed turn], [new user]
        let messages = vec![
            LlmMessage::new(MessageRole::User, "q1".to_string()).unwrap(),
            LlmMessage::new(MessageRole::Assistant, "a1".to_string()).unwrap(),
            LlmMessage::new(MessageRole::User, "dangling".to_string()).unwrap(),
            LlmMessage::new(MessageRole::User, "nueva".to_string()).unwrap(),
        ];
        let request = LlmRequest::new(messages, config, false).unwrap();
        // user | assistant | (dangling + nueva)
        assert_eq!(request.message_count(), 3);
        assert_eq!(request.messages()[2].content(), "dangling\n\nnueva");
    }
```

- [ ] **Step 2: Run to verify they fail.** `cargo test -p colmena_dag_engine --lib llm_request` — the two tests above fail (`new` still errors on consecutive user).

- [ ] **Step 3: Coalesce at the top of `new`.** In `LlmRequest::new`, insert the coalescing call as the FIRST statement, before the empty check. The current head is:
```rust
    pub fn new(
        messages: Vec<LlmMessage>,
        config: LlmConfig,
        stream: bool,
    ) -> Result<Self, LlmError> {
        if messages.is_empty() {
            return Err(LlmError::EmptyMessages);
        }
```
Change to:
```rust
    pub fn new(
        messages: Vec<LlmMessage>,
        config: LlmConfig,
        stream: bool,
    ) -> Result<Self, LlmError> {
        // Normalize the wire shape: providers require alternating roles. Merge
        // any adjacent same-role messages (e.g. a dangling user left by a failed
        // turn) so a poisoned conversation self-heals instead of erroring here.
        // Persistence is untouched — recall_history keeps the originals.
        let messages = coalesce_consecutive_same_role(messages);

        if messages.is_empty() {
            return Err(LlmError::EmptyMessages);
        }
```
Leave the existing consecutive-role validation loop below it unchanged (defensive net; keeps `LlmError::ConsecutiveRoles` referenced). Update its leading comment to: `// Defensive: after coalescing only consecutive Tool messages can remain (allowed).`

- [ ] **Step 4: Run the full module.** `cargo test -p colmena_dag_engine --lib llm_request` — ALL pass (the helper tests, the two updated/new tests, and the still-valid `test_request_creation_succeeds_with_interspersed_system_messages` / empty-messages / getters tests).

- [ ] **Step 5: fmt + clippy.** `cargo fmt && cargo clippy -p colmena_dag_engine --lib 2>&1 | tail -20` — no warnings.

- [ ] **Step 6: Commit.**
```bash
git add src/libs/colmena/src/llm/domain/llm_request.rs
git commit -m "$(cat <<'EOF'
fix(llm): coalesce consecutive same-role messages in LlmRequest::new

A turn that fails after persisting the user message left a dangling user row,
making every later turn fail with ConsecutiveRoles (provider rejects two user
turns in a row). Coalescing at request assembly normalizes the wire shape and
self-heals already-poisoned conversations, without touching persistence.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Documentation

**Files:** `docs/CHANGELOG_2026-06.md`, `docs/developer_guide/18_troubleshooting.md` (if present).

- [ ] **Step 1: CHANGELOG.** Append a new section at the bottom of `docs/CHANGELOG_2026-06.md` (newest-at-bottom convention), numbered as the next integer after the current highest `## N.` (verify; likely `## 42.`):

```markdown
## 42. LlmRequest — coalescing de roles consecutivos (auto-cura turnos fallidos) — 2026-06-20

**Qué cambió.** `LlmRequest::new` ahora fusiona mensajes adyacentes del mismo rol
(excepto `Tool`, que admite consecutivos por tool calls paralelos) en vez de
fallar con `ConsecutiveRoles`. Nuevo helper puro
`coalesce_consecutive_same_role` en `llm/domain/llm_request.rs`.

**Por qué.** Un turno que falla *después* de persistir el mensaje del usuario
(blip de Redis, timeout, tool que crashea, restart) dejaba un `user` colgado en
`llm_node_history`; el turno siguiente armaba `[…, user, user]` y fallaba en la
validación **antes de llamar al provider** → la conversación quedaba trabada
permanentemente (solo se recuperaba abriendo un chat nuevo). Los providers
exigen roles alternados, así que normalizar a eso es la forma correcta.

- **Auto-cura** conversaciones ya envenenadas (corre en el ensamblado, sin tocar la DB).
- **Sin pérdida:** la persistencia no cambia; `recall_history` sigue devolviendo los originales verbatim.
- Merge: `content` unido por `\n\n`; `tool_calls` y `files` concatenados.
- La validación de roles consecutivos queda como red defensiva (ya no se dispara para user/assistant/system).
- Pendiente complementario (opcional): rollback del user message ante turno fallido en `agent_service` (ataca la causa). Coalescing solo ya cierra el problema user-facing.

**Tests.** 7 unit en `llm_request` (helper + `new` + caso historia envenenada).

**Estado.** done.
```

- [ ] **Step 2: Troubleshooting note (only if the file exists).** If `docs/developer_guide/18_troubleshooting.md` exists, add a short entry under its error list:
```markdown
### `Consecutive messages with the same role` (resuelto 2026-06-20)
Antes, un turno que fallaba tras persistir el mensaje del usuario dejaba un
`user` colgado y trababa la conversación. `LlmRequest::new` ahora fusiona roles
consecutivos (coalescing) y auto-cura. Si lo ves en una versión vieja: abrí un
chat nuevo o actualizá colmena. Ver CHANGELOG §42.
```
If the file does not exist, skip this step (do not create it).

- [ ] **Step 3: Commit.**
```bash
git add docs/CHANGELOG_2026-06.md docs/developer_guide/18_troubleshooting.md 2>/dev/null; git add docs/CHANGELOG_2026-06.md
git commit -m "$(cat <<'EOF'
docs(llm): document consecutive-role coalescing self-heal

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Full verification

- [ ] **Step 1: Full suite (CI parity, no DATABASE_URL).** `cargo test -p colmena_dag_engine --verbose 2>&1 | tail -30` → exit 0, all unit + integration + doctests pass.
- [ ] **Step 2: fmt + clippy across the crate.** `cargo fmt --check && cargo clippy -p colmena_dag_engine --all-targets 2>&1 | tail -20` → no diff, no warnings.
- [ ] **Step 3: Real-world self-heal check (post-deploy, manual — document, don't block).** After this ships to dev, the poisoned cloud session `cmqmb12af` should self-heal: sending a new message no longer errors with `Consecutive messages with the same role` and the agent answers. Record the result; this is the live confirmation of the fix.
- [ ] **Step 4: Push (only when the user asks).** Branch first if on `develop`; open a PR against `develop`.

---

## Self-Review

**Spec coverage:** The bug (dangling user → permanent poison) is fixed by coalescing at the single request choke point (`LlmRequest::new`), which both prevents the error and self-heals existing conversations (Task 2). The helper is pure and tested (Task 1). `Tool` consecutiveness is preserved. Persistence/recall untouched. Docs + CHANGELOG (Task 3). Verified (Task 4).

**Placeholder scan:** No TBD/TODO; every step shows complete code/commands.

**Type consistency:** `coalesce_consecutive_same_role(Vec<LlmMessage>) -> Vec<LlmMessage>` defined in Task 1 and called identically in Task 2. `merge_same_role` uses `LlmMessage` constructors verified to exist (`user`/`user_with_files`/`assistant`/`assistant_with_tool_calls`/`new`) and accessors (`role`/`content`/`tool_calls`/`files`). `ToolCall`/`FunctionCall`/`FileData` imports flagged.

**Architecture:** Pure domain change, zero infra deps — consistent with the hexagonal rule. No DB/API change → ADP unaffected beyond picking up the new behavior on its next worker build.

**Risk:** Low. Coalescing only fires on adjacent same-role (a malformed/edge shape that providers reject anyway); well-formed alternating conversations are unchanged. The kept validation guards against any un-coalesced residue.
