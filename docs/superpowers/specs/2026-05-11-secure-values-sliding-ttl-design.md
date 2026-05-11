# Secure Values — Sliding 24h TTL + Per-Run Sweep + Leakage Prevention

**Date:** 2026-05-11
**Status:** Design
**Scope:** `secure_value_mappings` lifetime management AND leakage prevention via random-suffix handles, min-length values, and outbound response masking in `DagToolExecutor`.

## Problem

Two contradictory behaviors collide in the current design:

1. **Spec 6** (`agent_session_id`-first lookup) was added precisely so secure values survive across runs of the same agent conversation (multi-turn flows where turn N uses a handle persisted on turn M).
2. **`run_use_case.rs:687`** invokes `secure_value_service.cleanup(session_id)` at the end of every successful DAG run. That call deletes ALL rows for the run's ephemeral `session_id`, including ones just persisted by `secure_suspend` during the run.

In practice this manifests two ways:

- **Single-run flow** (e.g. meta-agent persists a token then a downstream `http_request` uses it): WORKS because injection happens before cleanup. Test `secure_suspend_login_e2e.json` validated this.
- **Multi-turn / cross-run flow** (the canvas-builder pair, any conversation that re-uses a token in a later turn): BROKEN. The handle in conversation history references a row that was deleted at the end of the persist run.

The new integration test `llm_tool_suspend_integration::multiple_secrets_resolved_via_qa_format` surfaced this: persist log shows `OK` and visibility probe on the engine pool confirms the row is in the DB, but a cross-pool query immediately after run completion returns 0 rows — because the engine wiped them at end-of-run.

## Decision

Adopt a **sliding 24-hour TTL** keyed by the existing `expires_at` column, plus a **bounded per-run sweep of expired rows** (B3) that replaces the current full-session cleanup.

- TTL is set on persist (24h from `NOW()`) and extended by 24h on every successful `decrypt` (lookup-as-write).
- `exists` filters expired rows but does NOT extend the TTL (it is a precondition check, not a use).
- At end of each completed DAG run, the engine calls `cleanup_expired_for_run(session_id, agent_session_id)` which deletes only rows whose `expires_at < NOW()` and whose `session_id` OR `agent_session_id` matches the run. Live rows survive.

24h is hardcoded as a constant in the postgres repository. Future configurability via env var is out of scope for this spec.

### Why sliding, not fixed

A user in an active conversation that spans more than 24h should not see their credentials abruptly invalidate. The cost of sliding is one `UPDATE` per `decrypt` — atomic with the lookup via `UPDATE ... RETURNING` — which is negligible. The effective cap shifts from "24h since persist" to "24h since last use", which still bounds zombie data.

### Why B3, not background sweep or lazy-on-access

- **Background sweeper (B1):** would require lifecycle management (spawn at engine startup, cancel on shutdown) and creates a class of timing bugs (test flakes when the sweep doesn't fire in time).
- **Lazy on lookup (B2):** every `decrypt`/`exists` call pays a small `DELETE` cost; agent sessions that are never touched again accumulate zombies indefinitely.
- **Per-run scope (B3):** piggy-backs on the existing run lifecycle that already calls `cleanup`. Deletes bounded by the run's own `session_id` and `agent_session_id` set. Self-healing as the conversation continues — every turn sweeps its own scope. Sessions truly abandoned can be handled by an external cron (out of scope here).

## Affected code

| Layer | File | Change |
|-------|------|--------|
| Migration | `migrations/postgres/20260511000001_secure_values_24h_ttl.sql` | NEW — `ALTER COLUMN expires_at SET DEFAULT NOW() + INTERVAL '24 hours'` |
| Domain | `dag_engine/domain/secure_value_repository.rs` | Add trait method `cleanup_expired_for_run(session_id, agent_session_id) -> Result<u64, DagError>` |
| Infrastructure | `dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs` | `decrypt` becomes `UPDATE ... RETURNING`; `exists` adds `AND expires_at > NOW()`; implement `cleanup_expired_for_run` |
| Application | `dag_engine/application/secure_value_service.rs` | Expose `cleanup_expired_for_run`. Change `inject_secrets` to return `HashMap<String,String>` (decrypted-value → handle). New `mask_outbound(&mut Value, &HashMap<String,String>)`. Update `persist_secret` to append random 8-hex suffix to handle. |
| Suspend node | `dag_engine/infrastructure/nodes/secure_suspend.rs` | After parsing values, validate `len ≥ 4` on each; reject too-short with clear error. |
| Tool executor | `dag_engine/infrastructure/dag_tool_executor.rs` | Capture map from `inject_secrets`; after node execution (both Ok and Err paths) apply `mask_outbound` before returning tool result. |
| Run lifecycle | `dag_engine/application/run_use_case.rs:687` | Replace `cleanup(session_id)` with `cleanup_expired_for_run(session_id, agent_session_id)`. Update callers of the changed `inject_secrets` signature (ignore return where unused). |
| Tests (repo) | `postgres_secure_value_repository.rs` `#[cfg(test)]` | 5 new `#[ignore]` cases for TTL behavior (see Tests section) + cases for random-suffix uniqueness. |
| Tests (cross-session) | `tests/secure_values_cross_session_integration.rs` | Add a case asserting secret survives end-of-run cleanup when `agent_session_id` is set. |
| Tests (suspend tool) | `tests/llm_tool_suspend_integration.rs` | Re-enable the `COUNT(*)` assertion that proves rows persist past run end. Update any hardcoded `<sv_user>` assertions to match the new `<sv_user_HEX8>` pattern. |
| Tests (outbound masking) | `tests/outbound_masking_integration.rs` | NEW — `#[ignore]` integration test for the masking pass via a synthetic node that echoes its inputs. |

## Behavior matrix

| Scenario | Behavior |
|----------|----------|
| Run with NO `agent_session_id`, secret persisted within run, used by downstream node | Works (decrypt within same run; cleanup at end deletes only IF expired — typically not yet) |
| Run with NO `agent_session_id`, fresh secret (TTL still in future) at end of run | Survives end-of-run sweep. Will eventually expire via another run's sweep that touches the same `session_id`, or via external cron. |
| Run with `agent_session_id`, secret persisted in turn 1, looked up in turn 2 | Works. Decrypt in turn 2 finds it and extends TTL by 24h. End-of-run sweep finds nothing expired in this agent's scope. |
| Run with `agent_session_id`, secret persisted in turn 1, no activity for 25h, looked up in turn 2 | Returns `None`. Row was expired; turn 2's sweep deletes it. The LLM must request a fresh suspension. |
| Two concurrent agent sessions, each persists `<sv_user>` | Both rows coexist (different `session_id`s in INSERT, different rows). Sweeps stay scoped. |
| Agent session abandoned (no further runs) | Rows linger past 24h until something else sweeps them or external cron runs. Acceptable for v1. |

## SQL details

### `decrypt` — atomic UPDATE+RETURNING

```sql
-- when agent_session_id is provided (preferred path)
UPDATE secure_value_mappings
SET expires_at = NOW() + INTERVAL '24 hours'
WHERE agent_session_id = $2
  AND hash_key = $3
  AND expires_at > NOW()
RETURNING pgp_sym_decrypt(encrypted_value, $1)::text AS decrypted;

-- when agent_session_id is NULL (legacy path)
UPDATE secure_value_mappings
SET expires_at = NOW() + INTERVAL '24 hours'
WHERE session_id = $2
  AND hash_key = $3
  AND expires_at > NOW()
RETURNING pgp_sym_decrypt(encrypted_value, $1)::text AS decrypted;
```

Returns 0 rows when the handle is missing OR expired — caller receives `Ok(None)`. Already aligns with the current contract.

### `exists` — add expiration filter, no UPDATE

```sql
SELECT EXISTS(
  SELECT 1 FROM secure_value_mappings
  WHERE agent_session_id = $1 AND hash_key = $2 AND expires_at > NOW()
);
```

### `cleanup_expired_for_run` — scoped DELETE

```sql
DELETE FROM secure_value_mappings
WHERE expires_at < NOW()
  AND (
    session_id = $1
    OR ($2::text IS NOT NULL AND agent_session_id = $2)
  );
```

The count of deleted rows is read via `sqlx::query(...).execute().await?.rows_affected()` — no `RETURNING` needed.

The `$2::text IS NOT NULL` guard makes the OR-clause silently no-op when `agent_session_id` is None — the query collapses to `session_id = $1` semantics. No null-handling logic in the Rust layer.

## Migration

Single file: `migrations/postgres/20260511000001_secure_values_24h_ttl.sql`

```sql
-- Sliding TTL: extend default to 24h. Existing rows keep their original
-- expires_at and will be swept naturally by cleanup_expired_for_run as
-- their owning runs complete (or via external cron for abandoned sessions).
ALTER TABLE secure_value_mappings
  ALTER COLUMN expires_at SET DEFAULT NOW() + INTERVAL '24 hours';
```

Idempotent. No data backfill required.

---

## Extension — Leakage Prevention

Three pieces, applied together to harden the secure-values surface against three distinct leak vectors. Per user instruction, **historical handles do not need backward compatibility** — new persists use the new format, lookups still work for old handles too (string match is exact on `hash_key`).

### Piece 1 — Minimum length on values

**Problem.** Outbound masking (Piece 3) does substring matching on decrypted values inside response bodies. If a secret value is very short (e.g., a PIN `"1"`, a flag `"on"`), substring matching causes pathological over-masking of unrelated content.

**Fix.** Reject too-short values at persist time. `secure_suspend.rs`, after `parse_qa_response` populates the values vector, validate:

```rust
for (s, v) in secrets.iter().zip(values.iter()) {
    if v.chars().count() < 4 {
        return Err(Box::<dyn Error + Send + Sync>::from(format!(
            "secure_suspend: value for secret '{}' is too short (min 4 chars). \
             Short values cause unsafe outbound masking — please supply ≥4 chars.",
            s.name
        )));
    }
}
```

Threshold of **4 chars** chosen to:
- Permit standard 4-digit PINs.
- Reject all plausibly-collision-prone strings (`"on"`, `"ok"`, `"42"`, etc.).

Threshold is hardcoded as `const MIN_SECRET_VALUE_LEN: usize = 4` next to the validation. Future configurability is out of scope.

### Piece 2 — Random-suffix handles

**Problem.** Today `secure_suspend.persist_secret` builds `format!("<sv_{}>", name)`. With semantic names like `user`, `pass`, `token`, an LLM that has seen the format once could *guess* handles for other sessions (e.g., `<sv_admin>`) and try to inject them into tool args. The decrypt would either miss (safe) or, if a similarly-named secret existed in this session's scope, accidentally use it.

**Fix.** Append a per-persist random suffix to the handle:

```rust
fn new_handle(name: &str) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string(); // 32 hex chars
    let suffix: String = id.chars().take(8).collect();
    format!("<sv_{name}_{suffix}>")
}
```

Examples:
- Old: `<sv_user>`
- New: `<sv_user_4f3a2b9c>`

**Consequences.**
- `secure_value_service.persist_secret` generates the handle (formerly the node did `format!("<sv_{}>", name)`). The node receives the generated handle in the returned map and emits it in its `handles` output.
- `inject_secrets` performs the same `<sv_*>` pattern match it does today and looks up `hash_key` by exact string — no algorithmic change.
- The collision pre-check in `secure_suspend` already runs PER-`name` (not per-handle) — it asks "does `<sv_<name>>` already exist for this session?". With random suffix, the same `name` can be persisted multiple times in different runs without collision. That's actually a feature (multi-turn agents persisting a token under name `auth` across days). The existing collision check can stay scoped to the current session — duplicate `name` within the **same** persist call is still rejected by `parse_and_validate_secrets`.
- The conversation history that holds old `<sv_user>` handles continues to resolve via existing `hash_key` rows (their suffix is empty). No backfill, no version flag.

### Piece 3 — Outbound masking at `DagToolExecutor`

**Problem.** When a node consumes a secret (decrypted by `inject_secrets` before execution), the node's response may echo that secret back to the LLM. A login endpoint returns `{"token": "abc", "username": "alice"}` — `alice` was a secret going in and now it's about to be shown to the LLM verbatim. Multiplied across every node type, every error path, every subgraph, every tool result.

**Fix.** A single masking pass at the universal choke point: `DagToolExecutor::execute_inner`, just before the result returns to `agent_service`.

#### Changes to `SecureValueService::inject_secrets`

Currently the method mutates `Value` in place and returns `Result<()>`. Change to return the mapping it applied:

```rust
/// Replace every `<sv_*>` handle in `value` with its decrypted form, in
/// place. Returns a map of `decrypted_value → handle` for every replacement
/// performed, so callers can later remask the same values in outbound
/// content.
pub async fn inject_secrets(
    &self,
    value: &mut serde_json::Value,
    session_id: &str,
    agent_session_id: Option<&str>,
) -> Result<HashMap<String, String>, DagError>;
```

Callers that don't need the map (`run_use_case` for non-LLM nodes) simply ignore the return.

#### New `SecureValueService::mask_outbound`

```rust
/// Walk `value` recursively. For each JSON string, replace every substring
/// equal to any key in `mapping` with the corresponding handle value.
/// Replacements are applied longest-key-first to prevent partial leaks
/// when two secrets share a prefix.
pub fn mask_outbound(
    &self,
    value: &mut serde_json::Value,
    mapping: &HashMap<String, String>,
);
```

Implementation rules:
- Sort keys by `len` descending; iterate.
- For each key, walk JSON: replace in every string. JSON numbers/bools/nulls are not modified.
- Substring replacement uses `str::replace` (literal, not regex). Case-sensitive.
- Skip mapping entries with `key.chars().count() < 4` as a defensive belt-and-suspenders (Piece 1 already enforces this, but the masker remains safe even if invoked with shorter values).
- Recursion goes through objects and arrays alike.

#### Wiring in `DagToolExecutor::execute_inner`

```rust
let applied_secrets = if let (Some(svc), Some(sid)) = (&self.secure_value_service, &self.session_id) {
    let mut inputs_val = serde_json::to_value(&inputs)?;
    let map = svc.inject_secrets(&mut inputs_val, sid, self.agent_session_id.as_deref())
        .await
        .map_err(|e| ...)?;
    let inputs = serde_json::from_value::<HashMap<String, Value>>(inputs_val)?;
    map
} else {
    HashMap::new()
};

let result = node.execute(&inputs, &node_exec_config, &mut state, None).await;

// MASK BEFORE RETURN — applies to both Ok and Err paths.
match result {
    Ok(mut value) => {
        if let Some(svc) = &self.secure_value_service {
            svc.mask_outbound(&mut value, &applied_secrets);
        }
        // ...continue existing flow (e.g., hash_output for `secure: true` tools)
        Ok(into_tool_result(...))
    }
    Err(e) => {
        let mut err_value = serde_json::json!({ "error": e.to_string() });
        if let Some(svc) = &self.secure_value_service {
            svc.mask_outbound(&mut err_value, &applied_secrets);
        }
        let masked_msg = err_value["error"].as_str().unwrap_or("").to_string();
        Ok(into_tool_result_error(..., masked_msg))
    }
}
```

#### Why this covers every node type

The masking runs in the `DagToolExecutor`, which is the SINGLE path through which any tool result reaches the LLM. `http_request`, `websocket_request`, `python_script`, `sql_query`, `subgraph`, and any future tool are all dispatched here. One implementation, universal coverage.

For nodes invoked **outside** the tool path (top-level DAG nodes), masking is not applied because their output goes to downstream nodes (not the LLM). If a top-level node's output is later piped to an LLM via `inputs`, that's a separate concern (`inject_secrets` on the LLM node's inputs handles it — and if the LLM receives a literal secret value passed through input wiring, the operator already opted into that by hard-wiring the secret value rather than the handle).

#### Edge cases

| Case | Behavior |
|---|---|
| Response contains the secret value embedded in a longer string (`"Logged in as alice"`) | Substring matches → replaced with `"Logged in as <sv_user_XXX>"`. ✅ |
| Two secrets share a prefix (`"alice"`, `"alicezhang"`) | Longest-first ordering replaces `"alicezhang"` before `"alice"`. ✅ |
| Secret value contains regex special chars | `str::replace` is literal, not regex. Safe. ✅ |
| Response is binary / not JSON | Out of scope — masking applies to JSON `Value`. Tools returning binary should hash via `secure: true` mechanism. |
| Multiple inject_secrets passes (e.g., LLM tool input AND node config) | All decrypts within this tool call accumulate into the same map. Each is remasked on output. ✅ |
| Secret value is itself short (3 chars) | Rejected at persist time by Piece 1. The masker also guards with `len ≥ 4` for defense in depth. ✅ |
| Response is the literal handle string `<sv_user_XXX>` (no decryption needed) | No-op — the handle isn't in the `applied_secrets` map as a key. ✅ |
| `agent_session_id` not set; legacy fallback | `inject_secrets` returns an empty map → `mask_outbound` is a no-op. Behavior preserved for legacy flows. |

---

## Tests

### Repo postgres (`#[ignore]`)

1. `decrypt_extends_expires_at` — insert with `expires_at = NOW() + 30s`, call `decrypt`, assert `expires_at > NOW() + 23h`.
2. `decrypt_returns_none_for_expired_row` — insert with `expires_at = NOW() - 1s`, call `decrypt`, assert `Ok(None)`.
3. `exists_returns_false_for_expired_row` — analogous, asserts `Ok(false)`.
4. `cleanup_expired_for_run_deletes_only_expired_in_scope` — seed 3 rows: (a) expired in this run's session, (b) expired in another session, (c) not-yet-expired in this run's session. Call cleanup, assert only (a) is gone.
5. `cleanup_expired_for_run_respects_agent_session_id` — seed two expired rows, one with this run's `agent_session_id`, one with a different one. Call cleanup with `Some(this_agent)`, assert only the matching one is deleted.

### Cross-session integration (extend existing test)

In `tests/secure_values_cross_session_integration.rs`, add a case:

- Persist a secret in Run 1 with `agent_session_id = agent_X`.
- Let Run 1 reach `Completed` status (the cleanup fires; the row is NOT expired).
- Via the engine's secure_value_service (or a fresh sqlx connection), assert the row still exists.
- Run 2 with the same `agent_X` and a fresh ephemeral session: `decrypt` finds the value AND extends the TTL.

### LLM tool suspend integration (re-enable)

In `tests/llm_tool_suspend_integration.rs::multiple_secrets_resolved_via_qa_format`:

- Restore the `SELECT COUNT(*) FROM secure_value_mappings WHERE agent_session_id = $1` assertion at the end. Now passes: rows persist past `Completed` because they're not expired.
- Update the test answer payload: secret names stay `user` and `pass` (≥3 chars), values change from `alice` / `hunter2` (≥4) — already compliant. No payload changes needed for Piece 1.
- The returned handles are no longer `<sv_user>` / `<sv_pass>` — they are `<sv_user_HEX8>` / `<sv_pass_HEX8>`. Assertions that pattern-match on handles must use `starts_with("<sv_user_")` etc.

### Secret-value length validation (`secure_suspend.rs` unit tests)

1. `parse_validates_value_min_length` — feed a Q/A payload where one value is 3 chars; expect a node-execute error mentioning "too short (min 4 chars)".
2. `parse_accepts_min_length_value` — value is exactly 4 chars; persist succeeds.

### Random-suffix handle (`secure_value_service.rs` unit tests)

1. `persist_secret_handle_has_random_suffix` — call `persist_secret` twice with the same `name` and different `session_id`s; assert the two handles differ in their suffix portion and both match `^<sv_<name>_[0-9a-f]{8}>$`.
2. `persist_secret_handle_preserves_name_prefix` — assert the handle starts with `<sv_<name>_`.

### Outbound masking (new integration test)

`tests/outbound_masking_integration.rs` (`#[ignore]`):

- Build a minimal graph with `secure_suspend` (tool) → resume with values `"alice123"` and `"hunter456"` → a downstream synthetic test-only "echo" tool registered in `tool_configurations` that returns its inputs verbatim.
- Assert: the response surfaced to the agent (captured via observer or event stream) contains `<sv_user_HEX8>` and `<sv_pass_HEX8>`, NOT `"alice123"` / `"hunter456"`.
- Assert masking also applies on error: register a tool that returns `Err("authentication failed for user alice123")` — the surfaced error message must read `"authentication failed for user <sv_user_HEX8>"`.

If a registered "echo" node doesn't exist, the test can use `python_script` configured to return `output = inputs` — same semantics.

### Outbound masking (`secure_value_service.rs` unit tests)

1. `mask_outbound_replaces_string_literal` — value is `Value::String("token=alice")`, map is `{"alice" → "<sv_user_X>"}`. After mask, value is `"token=<sv_user_X>"`.
2. `mask_outbound_walks_nested_objects` — value is `{"user": {"name": "alice"}}`, mapping replaces; result `{"user": {"name": "<sv_user_X>"}}`.
3. `mask_outbound_walks_arrays` — value is `["alice", "bob"]`, mapping has `"alice"`; result `["<sv_user_X>", "bob"]`.
4. `mask_outbound_orders_longest_first` — mapping has `"alice"` AND `"alicezhang"`. value is `"alicezhang"`. Replacement uses the LONGER key first → result is `<sv_user_X>` (not `<sv_a_X>zhang`).
5. `mask_outbound_skips_short_values` — mapping has key `"ok"` (2 chars). value is `"okok"`. No replacement (length guard).
6. `mask_outbound_is_noop_on_empty_map` — empty mapping leaves value unchanged.
7. `mask_outbound_does_not_modify_numbers_or_booleans` — value is `{"count": 42, "active": true, "name": "alice"}`. Only `"alice"` gets replaced.

## Observability

- `tracing::info!` event when `cleanup_expired_for_run` deletes ≥1 rows: `target="colmena::run_use_case"`, fields `rows_deleted`, `session_id`.
- `tracing::warn!` event when `cleanup_expired_for_run` returns Err (non-fatal — log and continue).
- Existing diagnostic logs (`postgres_persist: post-insert visibility probe`, `secure_suspend: persisting secret`) are kept — they proved their worth during this investigation.

## Backward compatibility

- `SecureValueRepository::cleanup(session_id)` stays in the trait (legacy callers, test fixtures). The use case no longer invokes it. Marked with a doc comment referencing this spec.
- Existing rows in production with `expires_at = NOW() + 1h` will expire on their original schedule and be swept naturally. No backfill.
- API surface of `SecureValueService` adds `cleanup_expired_for_run`; no breaking changes.

## Out of scope

- Configurability of the 24h constant (env var). Hardcoded for v1.
- Configurability of the min-value-length constant (4). Hardcoded for v1.
- Configurability of the random-suffix length (8 hex chars). Hardcoded for v1.
- A background sweeper task in `ColmenaEngine` for truly abandoned sessions. Can be added later if cron-based hygiene proves insufficient.
- Renewing TTL on `exists` calls. Intentionally NOT done — `exists` is a precondition check, not a use.
- Changing the encryption scheme or `pgp_sym_encrypt` defaults.
- Audit logging of secure-value access.
- Masking of node outputs that flow node-to-node WITHOUT crossing the `DagToolExecutor` boundary (e.g., a top-level `secure_suspend` → `http_request` chain in a DAG). The masking only protects content that reaches the LLM via tool dispatch.
- Backward compatibility for old `<sv_user>` (no-suffix) handles in historical conversation memory. Per user instruction, conversations from before this change are not preserved across this migration; treat as a hard cutover.
- Masking inside binary / non-JSON tool responses.

## Open questions

None. All edge cases handled in the matrices above.
