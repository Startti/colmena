# src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs

**Layer:** infrastructure  
**Purpose:** Implements `SecureSuspendNode`, an executable DAG node that pauses execution to collect user secrets via suspend/resume mechanism, encrypts them in the secure-values table, and returns opaque handles to the caller. Provides idempotent tool-defaults injection and synthetic tool construction utilities.

## Symbols

### Constants & Tool Utilities
- `SECURE_SUSPEND_TOOL_DESCRIPTION` (const) — canonical markdown tool description (auto-injected into LLM tool entries)
- `apply_secure_suspend_tool_defaults()` (pub fn) — idempotent auto-fill of description/node_schema on tool config entries (no-op for non-secure_suspend node_types)
- `secure_suspend_tool_node_schema()` (pub fn) — canonical `node_schema` builder: `secrets: [{question, name}]` array with validation rules
- `synthetic_secure_suspend_tool()` (pub fn) — factory to create minimal ToolConfiguration with empty description/schema (expects caller to run `apply_secure_suspend_tool_defaults`)
- `maybe_inject_secure_suspend_tool()` (pub fn) — idempotent injection of `ask_secret` tool into tool_configurations map (respects user overrides and key conflicts)

### Domain Logic
- `NAME_RE` (static Lazy<Regex>) — regex `^[a-z][a-z0-9_]{2,63}$` for validating secret names as lowercase slugs
- `Secret` (struct, private) — internal struct holding `(question: String, name: String)` pairs
- `parse_and_validate_secrets()` (fn, private) — parses config `secrets` array, validates each entry (question/name present), enforces lowercase-slug format, checks uniqueness of names and questions (O(n^2), acceptable for bounded lists)

### Node Implementation
- `SecureSuspendNode` (struct, pub) — holds `Arc<SecureValueService>` for encrypted persistence
- `SecureSuspendNode::new()` (impl fn) — constructor
- `SecureSuspendNode::execute()` (ExecutableNode impl, async) — main entry point; routes via presence of `__colmena_resume_answer`:
  - **Suspend path** (no resume answer): emits `{"__colmena_status": "SUSPENDED", "questions": [...]}` with `id: name`, `type: "secret"` per secret
  - **Resume path** (with resume answer): parses Q/A response, validates min-length (4 chars), checks collision on all handles, persists secrets via `SecureValueService`, returns `{"status": "resumed", "handles": {name: <sv_...>}}`
  - Accepts config from both `config` (DAG node) and `inputs` (LLM tool) via fallback merge
- `SecureSuspendNode::default_input()` (ExecutableNode impl) — returns `Some("secrets")`
- `SecureSuspendNode::default_output()` (ExecutableNode impl) — returns `Some("handles")`
- `SecureSuspendNode::schema()` (ExecutableNode impl) — returns empty JSON object (`{}`)

### Tests
- `tool_defaults_tests` (mod) — tests default injection (idempotency, user-override respect, no-op for other node_types)
- `tests` (mod, main) — 30+ test cases covering:
  - Validation (missing/empty secrets, invalid names, duplicates, duplicate questions)
  - Suspend path (question emission, ignore config.id in favor of secret.name)
  - Resume path (parsing, persistence, handle generation, collision detection, agent_session_id propagation)
  - Parser contract (multiline values, order-independence, missing/empty answers)
  - Security (4-char minimum, real values never leaked in tracing/output)
  - Injection helpers (flag handling, conflict safety)

## File-level notes

- **Security**: Minimum 4-character requirement on secret values (line 286–296) is intentional — shorter values cause "pathological over-masking" in outbound sanitization. Rejection happens pre-write.
- **Tracing safety**: Dedicated regression test (`resume_does_not_log_real_values`) at line 1159 verifies real secrets never leak into logs, even at TRACE level.
- **Contract integrity**: Resume branch re-uses suspend-emitted secret names as IDs (not `config.id` or `__node_id`) to maintain strict bidirectional parsing contract. Regression test at line 684.
- **Agent-session propagation**: When `__colmena_agent_session_id` present in inputs, it is forwarded to both `handle_exists()` and `persist_secret()` for cross-session lookup on resume. When absent, `None` is passed (legacy session-only behavior). Tested at lines 959 and 990.
- **Collision pre-check**: Before any persistence, ALL handles are checked for collisions (line 298–311); rollback is atomic (no partial writes).
- **Tool defaults design**: The separation of `synthetic_secure_suspend_tool()` (minimal, empty description/schema) and `apply_secure_suspend_tool_defaults()` (auto-fill) allows operators to declare terse tool entries (`{"name": "ask_secret", "node_type": "secure_suspend"}`) and have defaults injected downstream by the LLM node.
