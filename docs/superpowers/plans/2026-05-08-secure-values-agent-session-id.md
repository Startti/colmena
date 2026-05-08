# Secure Values agent_session_id Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Align `secure_value_mappings` keying with the existing pattern used by `llm_node_history`: agent_session_id-first lookup with session_id fallback. Closes the canvas-builder cross-session use case where a meta-agent persists secrets in one session and the agent-being-built consumes them in a later session with a different ephemeral session_id but the same agent_session_id.

**Architecture:** Aditive migration adds `agent_session_id` column. Trait/service methods accept `Option<&str>` for the agent. Postgres impl mirrors the conversation_repository pattern (if-let-Some agent → query by agent; else → fall back to session_id). Engine call sites propagate `agent_session_id` from `active_agent_session_id` (already in scope after the existing DAG state spec).

**Tech Stack:** Rust 1.95.0. SQL migration. No new deps.

**Spec:** [docs/superpowers/specs/2026-05-08-secure-values-agent-session-id-design.md](../specs/2026-05-08-secure-values-agent-session-id-design.md)

---

## Task 1: Migration + repo schema

**Files:**
- Create: `src/libs/colmena/migrations/postgres/20260508000001_secure_values_agent_session_id.sql`

- [ ] **Step 1:** Create the migration file with:

```sql
-- Spec: docs/superpowers/specs/2026-05-08-secure-values-agent-session-id-design.md
-- Adds stable-scope identifier so secure values can be looked up across runs
-- that share an agent context (canvas-builder pattern).

ALTER TABLE secure_value_mappings
    ADD COLUMN IF NOT EXISTS agent_session_id TEXT;

CREATE INDEX IF NOT EXISTS idx_secure_values_agent_hash
    ON secure_value_mappings(agent_session_id, hash_key);
```

- [ ] **Step 2:** Run `cargo sqlx prepare` if the project uses offline checks, or just `cargo build` to confirm migrations compile. If migrations are loaded at runtime via `MIGRATOR.run()`, they only need to be present in the directory.

- [ ] **Step 3:** Commit:

```bash
git add src/libs/colmena/migrations/postgres/20260508000001_secure_values_agent_session_id.sql
git commit -m "feat(secure-values): migration adds agent_session_id column + index"
```

---

## Task 2: Extend trait + Postgres impl

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/secure_value_repository.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs`

- [ ] **Step 1:** Update the trait. Add `agent_session_id: Option<&str>` to `persist`, `decrypt`, `exists`. Update the default impl of `exists` so it forwards the same parameter:

```rust
async fn persist(
    &self,
    session_id: &str,
    agent_session_id: Option<&str>,
    source_node_id: &str,
    hash_key: &str,
    real_value: &str,
    field_name: &str,
) -> Result<(), DagError>;

async fn decrypt(
    &self,
    session_id: &str,
    agent_session_id: Option<&str>,
    hash_key: &str,
) -> Result<Option<String>, DagError>;

async fn exists(
    &self,
    session_id: &str,
    agent_session_id: Option<&str>,
    hash_key: &str,
) -> Result<bool, DagError> {
    Ok(self.decrypt(session_id, agent_session_id, hash_key).await?.is_some())
}
```

- [ ] **Step 2:** Update `PostgresSecureValueRepository`:

`persist`:
```rust
sqlx::query(r#"
    INSERT INTO secure_value_mappings
        (session_id, agent_session_id, source_node_id, hash_key, encrypted_value, field_name)
    VALUES ($1, $2, $3, $4, pgp_sym_encrypt($5::text, $6), $7)
    ON CONFLICT (session_id, hash_key) DO UPDATE SET
        encrypted_value = EXCLUDED.encrypted_value,
        agent_session_id = EXCLUDED.agent_session_id,
        expires_at = NOW() + INTERVAL '1 hour'
"#)
.bind(session_id)
.bind(agent_session_id)
.bind(source_node_id)
.bind(hash_key)
.bind(real_value)
.bind(&encryption_key)
.bind(field_name)
.execute(&self.pool).await?;
```

`decrypt`: branch on `agent_session_id`. When `Some(agent)`: WHERE agent_session_id = $2 AND hash_key = $3. Else: WHERE session_id = $2 AND hash_key = $3. Mirror the conversation_repository pattern (`postgres_conversation_repository.rs:22-44`).

`exists`: same branching pattern.

- [ ] **Step 3:** Add unit-style integration tests (they need DATABASE_URL):

```rust
#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn cross_session_lookup_via_agent_id() {
    // Persist with session_id=run1, agent_session_id=Some("A1"), hash_key=<sv_demo>.
    // Decrypt with session_id=run2 (DIFFERENT), agent_session_id=Some("A1") → must Some(value).
    // Decrypt with session_id=run3, agent_session_id=Some("A2") → None.
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn legacy_session_only_lookup_still_works() {
    // Persist with session_id=run1, agent_session_id=None, hash_key=<sv_legacy>.
    // Decrypt with session_id=run1, agent_session_id=None → Some.
    // Decrypt with session_id=run2, agent_session_id=None → None.
}
```

Each test must clean up at the end (`cleanup(session_id)`).

- [ ] **Step 4:** Build + check + run unit tests + integration tests against live DB:

```bash
cargo check --all-targets 2>&1 | tail -5
source .env && cargo test --lib -p colmena_dag_engine -- --ignored postgres_secure_value 2>&1 | tail -15
```

The compile will fail at all call sites of `persist`/`decrypt`/`exists` until we update them in subsequent tasks. **Workaround for THIS task only:** before committing, update each compile-error site with a minimal `None` placeholder argument so the project compiles. The full propagation happens in Tasks 3-5. The mock in `secure_value_service.rs::tests` and any other impl of `SecureValueRepository` need updating.

Specifically: search for `impl SecureValueRepository for` and update each impl's method signatures + bodies to accept the new arg (StubRepo in `secure_suspend.rs::tests` and NoopRepo in `registry.rs::tests` may also need updating).

- [ ] **Step 5:** Commit:

```bash
git add src/libs/colmena/src/dag_engine/domain/secure_value_repository.rs \
        src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs \
        src/libs/colmena/src/dag_engine/application/secure_value_service.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs \
        src/libs/colmena/src/dag_engine/infrastructure/registry.rs
git commit -m "feat(secure-values): repo trait + postgres impl accept agent_session_id"
```

(Add any other files updated for compile errors.)

---

## Task 3: Update `SecureValueService` API

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/secure_value_service.rs`

- [ ] **Step 1:** Add `agent_session_id: Option<&str>` to: `hash_output`, `inject_secrets`, `persist_secret`, `handle_exists`. All call internally `self.repo.persist/decrypt/exists` with the new arg.

- [ ] **Step 2:** Update existing tests to pass `None` for backwards compat. Add 1-2 new tests:

```rust
#[tokio::test]
async fn handle_exists_uses_agent_session_id_when_provided() {
    let repo = Arc::new(MockSecureValueRepository { ... });
    let svc = SecureValueService::new(repo.clone());
    // persist_secret(session_id="run1", agent=Some("A1"), name="x", value="real").
    // handle_exists(session_id="run2", agent=Some("A1"), "<sv_x>") must be true (agent path).
    // handle_exists(session_id="run3", agent=None, "<sv_x>") must be false (no fallback match).
    // The inline mock might need a small update — track agent_session_id in the storage tuple.
}
```

The MockSecureValueRepository needs to be updated to actually USE the agent_session_id in lookup so this test means something. Update its storage from `HashMap<hash_key, value>` to `HashMap<(Option<String>, String /*hash*/), value>` keyed by (agent or session, hash_key). Pick the first key that exists matching the lookup.

- [ ] **Step 3:** Update all call sites of `inject_secrets`, `hash_output`, etc. with the new arg.

Find them: `grep -rn "inject_secrets\|hash_output\|persist_secret\|handle_exists" src/libs/colmena/src/`

Each call site temporarily passes `None` for agent until Task 4-5 plumb it through.

- [ ] **Step 4:** Run unit tests + cargo check.

```bash
cargo check --all-targets && cargo test --lib -p colmena_dag_engine 2>&1 | tail -10
```

Clean.

- [ ] **Step 5:** Commit:

```bash
git add src/libs/colmena/src/dag_engine/application/secure_value_service.rs <other touched files>
git commit -m "feat(secure-values): service API accepts agent_session_id"
```

---

## Task 4: Plumb `agent_session_id` through `run_use_case` and `dag_tool_executor`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`

- [ ] **Step 1:** In `run_use_case.rs:~398`, where `__colmena_session_id` is injected into inputs, ALSO inject `__colmena_agent_session_id` when `active_agent_session_id` is Some:

```rust
inputs.insert("__colmena_session_id".to_string(), Value::String(session_id.clone()));
if let Some(asid) = active_agent_session_id.as_deref() {
    inputs.insert("__colmena_agent_session_id".to_string(), Value::String(asid.to_string()));
}
```

- [ ] **Step 2:** Update both `inject_secrets` calls in `run_use_case.rs` (~line 380 inputs, ~line 397 config) to pass `active_agent_session_id.as_deref()` as the new arg.

- [ ] **Step 3:** In `dag_tool_executor.rs`, add a field `agent_session_id: Option<String>` and a builder `with_agent_session_id`. Inside `execute_inner`, where `__colmena_session_id` is now being injected (Task 4 of Gap 2, commit `99ba16c`), also inject `__colmena_agent_session_id`. Pass it to `inject_secrets`.

- [ ] **Step 4:** Update `llm.rs` to call `with_agent_session_id` when constructing `DagToolExecutor`. The `agent_session_id` is available in the input via `__colmena_agent_session_id` (the engine just put it there).

```rust
let agent_session_id = inputs
    .get("__colmena_agent_session_id")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string());
let tool_executor = DagToolExecutor::new(...)
    .with_session_id(session_id.clone())
    .with_agent_session_id(agent_session_id);
```

(Adapt to the actual constructor pattern — `with_secure_values` and the existing builder style.)

- [ ] **Step 5:** Build + tests:

```bash
cargo check --all-targets && cargo test --lib -p colmena_dag_engine 2>&1 | tail -10
```

- [ ] **Step 6:** Commit:

```bash
git add src/libs/colmena/src/dag_engine/application/run_use_case.rs \
        src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(secure-values): engine plumbs agent_session_id to inject_secrets and tool inputs"
```

---

## Task 5: `secure_suspend` resume reads agent_session_id from inputs

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`

- [ ] **Step 1:** In the resume-path of `execute`, read `__colmena_agent_session_id` from inputs:

```rust
let session_id = inputs.get("__colmena_session_id").and_then(|v| v.as_str()).ok_or(...)?;
let agent_session_id = inputs.get("__colmena_agent_session_id").and_then(|v| v.as_str());
```

- [ ] **Step 2:** Pass `agent_session_id` to `handle_exists` and `persist_secret`:

```rust
self.secure_value_service.handle_exists(session_id, agent_session_id, &handle).await?;
self.secure_value_service.persist_secret(session_id, agent_session_id, node_id, &s.name, v).await?;
```

- [ ] **Step 3:** Update unit tests in `secure_suspend.rs::tests` — those that call `inputs_with` should also accept an optional agent_session_id, and add tests:

```rust
#[tokio::test]
async fn resume_persists_with_agent_session_id_when_available() {
    let (node, repo) = build_node();
    // inputs include __colmena_agent_session_id = "A1".
    // After resume, the StubRepo received persist with agent_session_id = Some("A1").
    // (Update StubRepo to track agent_session_id in its captured calls.)
}
```

- [ ] **Step 4:** Build + tests.

- [ ] **Step 5:** Commit:

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs
git commit -m "feat(secure-suspend): resume-path persists secrets with agent_session_id when set"
```

---

## Task 6: HTTP node passes agent_session_id to hash_output

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`

- [ ] **Step 1:** Find where `hash_output` is invoked from `http.rs`. It probably reads `session_id` from `inputs.get("__colmena_session_id")`. Update to also read `__colmena_agent_session_id` and pass it to `hash_output`.

- [ ] **Step 2:** Add `__colmena_agent_session_id` to the `reserved_keys` array (`http.rs:~230-243`) so it's not sent as an extra query param to external APIs.

- [ ] **Step 3:** Build + tests.

- [ ] **Step 4:** Commit:

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs
git commit -m "feat(http-node): exclude __colmena_agent_session_id from query params and propagate to hash_output"
```

---

## Task 7: End-to-end cross-session validation

**Files:**
- Create: `src/libs/colmena/tests/secure_values_cross_session_integration.rs`

- [ ] **Step 1:** Write an integration test that:

1. Builds the engine with a Postgres-backed SecureValueService.
2. Manually persists a secret via `service.persist_secret(session_id="run_S1", agent_session_id=Some("agent_A1"), node_id="setup", name="cross", value="cross-value")`.
3. Runs a graph (`tests/graphs/basic/secure_value_in_config_smoke.json` or a new one) under `session_id="run_S2"` (DIFFERENT) with `agent_session_id="agent_A1"` (SAME).
4. Asserts the log node's `marker_field` ends up as `"cross-value"` — which would NOT happen if lookup were by session_id (different across runs).
5. Cleanup at the end.

Mark `#[ignore]`.

- [ ] **Step 2:** Run it against live DB:

```bash
source .env && cargo test --test secure_values_cross_session_integration -- --ignored 2>&1 | tail -20
```

Expected: pass.

- [ ] **Step 3:** Run the original LLM-driven e2e graph manually with `--agent-session-id` only (no `--session-id` — let the engine generate one) and verify resume works across the two CLI invocations.

```bash
SESSION_ID="agent_e2e_$(date +%s)"
echo "AGENT_SESSION: $SESSION_ID"
source .env

# Run 1: suspend
./target/debug/dag_engine run tests/graphs/advanced/secure_suspend_login_e2e.json \
  --agent-session-id "$SESSION_ID" 2>&1 | tail -5

# Run 2: resume (DIFFERENT session_id auto-generated by engine, same agent)
./target/debug/dag_engine run tests/graphs/advanced/secure_suspend_login_e2e.json \
  --agent-session-id "$SESSION_ID" \
  --answer "usuario
juan@example.com
contraseña
my-Real-PWD-987" 2>&1 | tail -25
```

Expected: the LLM completes successfully in the resume run, httpbin echoes back the real values, the LLM's final summary mentions success — even though the two runs had different ephemeral session_ids.

If the CLI doesn't support resume by `--agent-session-id` alone (engine doesn't know which paused chain to resume), document this as a separate spec ("CLI: resume-by-agent-id"), but the underlying secure_values fix is still validated by Step 1's integration test.

- [ ] **Step 4:** Commit:

```bash
git add src/libs/colmena/tests/secure_values_cross_session_integration.rs
git commit -m "test(secure-values): integration test confirms cross-session lookup via agent_session_id"
```

---

## Final Verification

- [ ] `cargo test --verbose -p colmena_dag_engine 2>&1 | tail -10` — no regressions.
- [ ] `source .env && cargo test -- --ignored 2>&1 | tail -25` — all integration tests pass.
- [ ] `cargo clippy --all-targets -- -D warnings` — clean.

---

## Self-Review Notes

**Spec coverage check:**

| Spec section | Task |
|---|---|
| Migration aditiva | Task 1 |
| Trait extension + Postgres impl | Task 2 |
| Service API extension | Task 3 |
| Engine plumbing (run_use_case + dag_tool_executor) | Task 4 |
| secure_suspend resume reads agent_session_id | Task 5 |
| HTTP node propagation | Task 6 |
| End-to-end validation | Task 7 |

**Risk:** the largest blast radius is Task 2/3 (signature changes). Compile errors will show every uncovered call site, which is good — no silent corruption.

**No placeholders.** Each step shows the actual code or command.
