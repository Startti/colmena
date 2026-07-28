# src/libs/colmena/src/dag_engine/application/secure_value_service.rs

**Layer:** application  
**Purpose:** Business logic for encrypting sensitive HTTP response values, injecting them back before tool execution, and masking them in outbound responses. Core service for the secure-values subsystem (agent-first cross-session lookup, handles of form `<sv_<name>_<8-hex>>`).

## Symbols

- `SecureValueService` (struct, pub) — Main service orchestrating secure value management with repository dependency
- `new(repo: Arc<dyn SecureValueRepository>) -> Self` — Constructor
- `hash_output(output: &Value, config: &Value, session_id: &str, agent_session_id: Option<&str>, source_node_id: &str)` (async fn, pub) — Hash all sensitive string/number values in HTTP response body (not status); returns output with placeholders `<value_N>` and persists encrypted mappings
- `collect_values_to_hash(value: &mut Value, counter: &mut u32, to_persist: &mut Vec<...>)` (fn, private) — Recursively traverse JSON tree and collect string/number values for hashing; skips status field in HTTP responses
- `inject_secrets(inputs: &mut Value, session_id: &str, agent_session_id: Option<&str>)` (async fn, pub) — Detect placeholders (`<value_N>`, `<sv_*>`) and replace with decrypted real values; returns (decrypted → handle) map for outbound masking
- `collect_placeholders(value: &Value, placeholders: &mut Vec<String>)` (fn, private) — Collect all angle-bracket-wrapped strings (`<...>`) from JSON tree
- `replace_placeholder(value: &mut Value, placeholder: &str, real: String)` (fn, private) — Recursively replace placeholder string with real value throughout JSON tree
- `persist_secret(session_id: &str, agent_session_id: Option<&str>, source_node_id: &str, name: &str, real_value: &str)` (async fn, pub) — Persist a named secret with random 8-hex suffix; returns unique handle `<sv_<name>_<8hex>>`
- `new_handle(name: &str) -> String` (fn, private) — Build handle from v4 UUID suffix (8 hex chars) to prevent LLM from guessing/forging names
- `handle_exists(session_id: &str, agent_session_id: Option<&str>, handle: &str)` (async fn, pub) — Check handle registration (agent-first lookup with session fallback)
- `mask_outbound(value: &mut Value, mapping: &HashMap<String, String>)` (fn, pub) — Replace decrypted-value substrings with their handles in JSON tree; processes longest-key-first to avoid partial leaks; skips keys shorter than 4 chars
- `mask_walk(value: &mut Value, ordered: &[(&String, &String)])` (fn, private) — Recursive walker for mask_outbound; applies replacements in sorted order to string values
- `cleanup(session_id: &str)` (async fn, pub) — Delete all secure values for a session
- `cleanup_expired_for_run(session_id: &str, agent_session_id: Option<&str>)` (async fn, pub) — Per-run sweep: delete expired rows matching this session or agent; returns count of deleted rows

## Test module (lines 302–734)

- `MockEntry` (struct) — Test record: session, optional agent, hash key, and decrypted value
- `MockSecureValueRepository` (struct) — Mock repository implementing `SecureValueRepository` trait; stores all persist/decrypt/exists/cleanup operations in a Vec
- `build_service()` (fn) — Helper to instantiate mock repo + service for tests
- `test_hash_output_with_secure_flag` (async test) — Verify HTTP response body hashing when `secure: true`; status remains unchanged
- `test_hash_output_without_secure_flag` (async test) — Verify no hashing when `secure: false`
- `test_repo_exists_true_after_persist` (async test) — Verify `exists()` returns true after persist, false for non-existent keys
- `test_handle_exists_after_persist_secret` (async test) — Verify handle format and existence check
- `test_persist_secret_can_be_decrypted_via_inject` (async test) — Verify round-trip: persist → inject restores original value
- `test_inject_secrets_agent_first_cross_session` (async test) — Verify agent-first lookup allows cross-session secret access
- `test_inject_secrets_no_agent_session_isolated` (async test) — Verify session-scoped isolation when no agent is provided
- `mask_outbound_replaces_string_literal` (test) — Verify string replacement on scalar value
- `mask_outbound_walks_nested_objects` (test) — Verify recursion into nested objects
- `mask_outbound_walks_arrays` (test) — Verify recursion into arrays
- `mask_outbound_orders_longest_first` (test) — Verify longest-key-first prevents partial leaks
- `mask_outbound_skips_short_values` (test) — Verify keys < 4 chars are skipped
- `mask_outbound_is_noop_on_empty_map` (test) — Verify empty mapping is no-op
- `mask_outbound_does_not_modify_numbers_or_booleans` (test) — Verify only strings are masked
- `test_handle_exists_agent_first_cross_session` (async test) — Verify `handle_exists` follows agent-first lookup rule

## File-level notes

- **Consistent agent-first semantics**: `inject_secrets`, `persist_secret`, `handle_exists` all follow the same rule: look up by agent first, fall back to session-only if no agent provided. This enables resume chains where the same agent persists a secret in one ephemeral session and retrieves it in another.
- **HTTP-response special case** (line 71–75): Only the `body` field is hashed; `status` code is left unchanged. Well-intentioned — status codes are not secrets.
- **Placeholder format**: Generated handles use `<value_N>` for HTTP responses and `<sv_<name>_<8hex>>` for `secure_suspend` secrets. Detection logic is simple substring match (starts/ends with angle brackets, length > 2).
- **Outbound masking strategy**: `mask_outbound` sorts keys by length descending to avoid a longer secret being mistakenly masked by a shorter prefix (e.g., if both `alice123` and `alice` were keys, process `alice123` first).
- **Test coverage**: 16 tests including agent-first cross-session scenarios, round-trip persist/inject, masking edge cases. Mock repository faithfully implements the agent-first lookup rule.
- **No breaking issues detected**: All public methods are async, properly propagate errors, and follow the repository pattern. The service is dependency-injected with a trait object, enabling easy mocking.
