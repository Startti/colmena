# Inject Secrets in Config Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend the DAG engine so that `SecureValueService::inject_secrets` runs on each node's `config` (in addition to `inputs`) before execution. This unblocks the canvas-builder pattern where the meta-agent generates nodes whose config contains `<sv_NAME>` handles directly.

**Architecture:** A small change in `run_use_case.rs` — call the existing `inject_secrets` a second time on the node's config object, right after the existing call on inputs. No new files in src/. Plus an integration test against a live Postgres.

**Tech Stack:** Rust 1.95.0. No new deps.

**Spec:** [docs/superpowers/specs/2026-05-07-inject-secrets-in-config-design.md](../specs/2026-05-07-inject-secrets-in-config-design.md)

---

## File Structure

| Path | Change |
|---|---|
| `src/libs/colmena/src/dag_engine/application/run_use_case.rs` | Add a second `inject_secrets` call covering the node's config, just after the existing inputs call. |
| `tests/graphs/basic/secure_value_in_config_smoke.json` | NEW — minimal graph for integration testing. |
| `src/libs/colmena/tests/secure_value_in_config_integration.rs` | NEW — `#[ignore]`d integration test. |

---

## Task 1: Locate the inject_secrets call site and add a regression-style integration test

**Files:**
- Locate: existing `inject_secrets` call in `src/libs/colmena/src/dag_engine/application/run_use_case.rs`.
- Create: `tests/graphs/basic/secure_value_in_config_smoke.json`
- Create: `src/libs/colmena/tests/secure_value_in_config_integration.rs`

The integration test will manually pre-populate a secure value, run a graph whose `log` node has the corresponding handle as a config field, and assert the log node's input contains the real value (not the handle).

- [ ] **Step 1: Find the call site**

Run: `grep -n "inject_secrets" /home/daniel-garcia4/startti/colmena/src/libs/colmena/src/dag_engine/application/run_use_case.rs`
Expected: at least one match around line 381 inside the per-node execution block.

Read 30 lines around that line to understand the surrounding code (variable names, error handling pattern, where `node_config.config` is in scope).

- [ ] **Step 2: Write the smoke graph**

Create `tests/graphs/basic/secure_value_in_config_smoke.json`:

```json
{
  "comment": "Smoke test for inject_secrets on config. The log node's config contains a handle <sv_smoke>; if the engine inject_secrets covers config, the log node will see the real value 'smoke-value-xyz' (pre-populated by the test setup). If not, it sees the handle literal.",
  "metadata": {
    "category": "basic",
    "features": ["secure_values", "inject_in_config"],
    "requires_env": ["DATABASE_URL"]
  },
  "nodes": {
    "show": {
      "type": "log",
      "config": {
        "marker_field": "<sv_smoke>"
      }
    }
  },
  "edges": []
}
```

- [ ] **Step 3: Inspect existing integration test patterns**

Run: `ls /home/daniel-garcia4/startti/colmena/src/libs/colmena/tests/` and find `secure_suspend_integration.rs`. Read it as the canonical template — same crate, same engine setup, same DATABASE_URL convention.

- [ ] **Step 4: Write the integration test** at `src/libs/colmena/tests/secure_value_in_config_integration.rs`. Mirror the engine setup of `secure_suspend_integration.rs`. Outline:

```rust
//! Integration test for inject_secrets covering the node's config.
//! Run with: `source .env && cargo test --test secure_value_in_config_integration -- --ignored`.

use colmena_dag_engine::dag_engine::application::secure_value_service::SecureValueService;
use colmena_dag_engine::dag_engine::infrastructure::persistence::postgres_secure_value_repository::PostgresSecureValueRepository;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;

// (Imports for the engine constructor — copy from secure_suspend_integration.rs.)
// Adjust the use paths if necessary based on what's exported.

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn inject_secrets_replaces_handle_in_config() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPoolOptions::new().connect(&url).await.unwrap();

    // 1. Pre-populate a secure value for our session id.
    let session_id = format!(
        "inject_in_config_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let repo = Arc::new(PostgresSecureValueRepository::new(pool.clone()));
    let svc = Arc::new(SecureValueService::new(repo.clone() as Arc<_>));
    svc
        .persist_secret(&session_id, "test_setup", "smoke", "smoke-value-xyz")
        .await
        .expect("persist_secret must succeed");

    // 2. Run the graph with that session_id (so inject_secrets has a hit).
    //    Use the same engine harness as secure_suspend_integration.rs.
    //    Capture the events, find the `node-start` for `show`, and
    //    inspect its `config.marker_field`.
    //
    //    EXPECTED (after fix): config.marker_field == "smoke-value-xyz"
    //    BEFORE fix: config.marker_field == "<sv_smoke>"

    // ... follow the pattern from secure_suspend_integration.rs ...

    // Cleanup
    repo.cleanup(&session_id).await.unwrap();
}
```

For the engine harness, copy the relevant `engine()` helper and event-streaming bits verbatim from `secure_suspend_integration.rs`. Match the assertion shape (look for `NodeStart` events).

- [ ] **Step 5: Run the test FIRST to prove the bug**

Run: `source .env && cargo test --test secure_value_in_config_integration -- --ignored 2>&1 | tail -20`
Expected: the test FAILS — config.marker_field is still `<sv_smoke>`, proving the bug exists.

If the test passes here, something is wrong with your harness — investigate before proceeding.

- [ ] **Step 6: Commit the test (TDD: failing first)**

```bash
git add tests/graphs/basic/secure_value_in_config_smoke.json \
        src/libs/colmena/tests/secure_value_in_config_integration.rs
git commit -m "test(secure-values): regression test exposes inject_secrets gap on config"
```

---

## Task 2: Implement the fix

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`

- [ ] **Step 1: Locate the existing inject_secrets call**

Run: `grep -n "inject_secrets" /home/daniel-garcia4/startti/colmena/src/libs/colmena/src/dag_engine/application/run_use_case.rs`

Read ~30 lines around the relevant line (~381). Identify:
- The variable name holding the inputs that's being mutated.
- The variable name holding the node's config.
- The error handling pattern (likely `if let Err(e) = ... { eprintln!(...) }` or similar).

- [ ] **Step 2: Add the second inject call**

Right AFTER the existing `inject_secrets` call on inputs, add an analogous call on the node's config. Mirror the error handling pattern. The conceptual change:

```rust
// Existing:
if let Err(e) = svc.inject_secrets(&mut inputs_value, &session_id).await {
    eprintln!("⚠️ Failed to inject secrets in inputs: {}", e);
}

// NEW — add immediately after:
if let Err(e) = svc.inject_secrets(&mut node_config_value, &session_id).await {
    eprintln!("⚠️ Failed to inject secrets in config: {}", e);
}
```

The exact variable name for the config object depends on the surrounding code — read it carefully. The node's config is whatever ends up being passed to `node.execute(&inputs, &CONFIG, ...)`.

If the config is currently passed by value or constructed at the last moment, you may need to:
- Capture it as a mutable `Value`.
- Inject into it.
- Pass that mutated value to `execute`.

Whatever the structural change is, keep it minimal — a few lines.

- [ ] **Step 3: Run cargo check**

Run: `cargo check --all-targets 2>&1 | tail -5`
Expected: clean (deny-warnings on).

- [ ] **Step 4: Run the regression test from Task 1**

Run: `source .env && cargo test --test secure_value_in_config_integration -- --ignored 2>&1 | tail -10`
Expected: now PASSES (config.marker_field == "smoke-value-xyz").

- [ ] **Step 5: Run the full crate unit tests**

Run: `cargo test --lib -p colmena_dag_engine 2>&1 | tail -10`
Expected: no regressions.

- [ ] **Step 6: Run all `#[ignore]`d integration tests**

Run: `source .env && cargo test -- --ignored 2>&1 | tail -20`
Expected: all pass (especially the existing `secure_suspend_integration` and the new one).

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/application/run_use_case.rs
git commit -m "feat(secure-values): inject_secrets now covers node config in addition to inputs"
```

---

## Task 3: Re-validate the canonical canvas-builder pattern end-to-end

Re-run the existing direct e2e graph (`tests/graphs/advanced/secure_suspend_login_direct.json` — the v2 with handles in config). After the fix, httpbin must echo back the REAL values in the JSON body, not the handles, and the response body should NOT contain `<sv_demo_user>` strings anywhere meaningful.

**Files:** none modified — pure validation step. The output of this run gets attached to the commit as documentation evidence in the message.

- [ ] **Step 1: Build the binary**

Run: `cargo build --bin dag_engine 2>&1 | tail -3`
Expected: clean build.

- [ ] **Step 2: First invocation — suspend phase**

Run:
```bash
SESSION_ID="gap1_validate_$(date +%s)"
echo "SESSION: $SESSION_ID"
source .env
./target/debug/dag_engine run tests/graphs/advanced/secure_suspend_login_direct.json --session-id "$SESSION_ID" 2>&1 | tail -10
```
Expected: SUSPENDED with two questions. Note the SESSION_ID for step 3.

- [ ] **Step 3: Resume phase**

Run:
```bash
./target/debug/dag_engine run tests/graphs/advanced/secure_suspend_login_direct.json \
  --session-id "$SESSION_ID" \
  --answer "Pega tu usuario:
juan@example.com
Pega tu contraseña:
my-Real-PWD-987" 2>&1 | tail -50
```

Capture the output. **Verify:**
- The `node-end` for `login` shows `body.json: {"user": "<value_X>", "password": "<value_Y>"}` (hashed because secure: true) — but the underlying httpbin RESPONSE has the real values that got hashed AFTER the call.
- The `[LogNode]` block (with secure values restored on input to log) shows `json: {"user": "juan@example.com", "password": "my-Real-PWD-987"}` — proving the real values reached the wire.
- The `args` section (httpbin's echo of query params) is now EMPTY or contains only `__colmena_node_id_path` and `session_id` — proving the values went into the BODY, not into query params.

If the output still shows handles in `json` field of the echoed body, the fix didn't take effect — investigate.

- [ ] **Step 4: No commit needed for this task** (it's pure validation). Just confirm the success in the run output.

---

## Final Verification

- [ ] Run `cargo test --verbose -p colmena_dag_engine 2>&1 | tail -10` — no regressions.
- [ ] Run `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5` — clean.
- [ ] `git log --oneline` shows ~2 new commits since the start of this plan.

---

## Self-Review Notes

**Spec coverage:**

| Spec section | Task |
|---|---|
| Diseño — segundo `inject_secrets` call | Task 2 |
| Test de integración pre-populating + log node config | Task 1 |
| Re-validación end-to-end | Task 3 |
| No tocar dispatcher de tools | (no task — explicitly NOT touched) |

**Risk:** the fix is a 4-line change in production code. The risk surface is small. Nodes whose config contains literal `<...>`-shaped strings that happen to coincide with stored handles would get unexpectedly rewritten — but this is the same risk that already exists for inputs, and the placeholder format is intentionally distinctive.

**No placeholders in this plan** — every step has the actual code or command.
