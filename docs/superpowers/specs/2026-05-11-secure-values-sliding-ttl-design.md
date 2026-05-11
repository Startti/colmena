# Secure Values — Sliding 24h TTL + Per-Run Expired Sweep

**Date:** 2026-05-11
**Status:** Design
**Scope:** `secure_value_mappings` lifetime management

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
| Application | `dag_engine/application/secure_value_service.rs` | Expose `cleanup_expired_for_run` delegating to repo |
| Run lifecycle | `dag_engine/application/run_use_case.rs:687` | Replace `cleanup(session_id)` with `cleanup_expired_for_run(session_id, agent_session_id)` |
| Tests (repo) | `postgres_secure_value_repository.rs` `#[cfg(test)]` | 5 new `#[ignore]` cases (see Tests section) |
| Tests (cross-session) | `tests/secure_values_cross_session_integration.rs` | Add a case asserting secret survives end-of-run cleanup when `agent_session_id` is set |
| Tests (suspend tool) | `tests/llm_tool_suspend_integration.rs` | Re-enable the `COUNT(*)` assertion that proves rows persist past run end |

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
- A background sweeper task in `ColmenaEngine` for truly abandoned sessions. Can be added later if cron-based hygiene proves insufficient.
- Renewing TTL on `exists` calls. Intentionally NOT done — `exists` is a precondition check, not a use.
- Changing the encryption scheme or `pgp_sym_encrypt` defaults.
- Audit logging of secure-value access.

## Open questions

None. All edge cases handled in the matrix above.
