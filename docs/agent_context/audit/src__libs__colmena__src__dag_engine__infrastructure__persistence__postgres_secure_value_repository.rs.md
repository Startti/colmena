# src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs

**Layer:** infrastructure  **Purpose:** PostgreSQL implementation of the SecureValueRepository trait using pgcrypto symmetric encryption (OpenPGP CFB), keyed by SECURE_VALUES_KEY environment variable. Manages encrypted persistent storage of sensitive values across DAG executions.

## Symbols

- `PostgresSecureValueRepository` (struct, pub) — PostgreSQL backend for secure value repository; holds connection pool and encryption key
- `PostgresSecureValueRepository::new` (fn, pub) — Constructs repository by loading SECURE_VALUES_KEY from environment; panics if unset or empty (fail-fast safety mechanism)
- `PostgresSecureValueRepository::new_with_key` (fn, pub) — Constructs repository with explicit encryption key; intended for tests or callers retrieving key from secret manager
- `PostgresSecureValueRepository::migrate` (async fn, pub) — Ensures pgcrypto extension exists in database (safety net for environments where migrations ran without superuser)
- `SecureValueRepository::persist` (async fn) — Encrypts and persists secret value with pgp_sym_encrypt; handles upsert with TTL extension; includes post-insert visibility diagnostic [FLAG: improvement — post-insert diagnostic SELECT adds latency without production necessity; consider conditional or removal]
- `SecureValueRepository::decrypt` (async fn) — Retrieves and decrypts value using agent_session_id as primary key with fallback to session_id; extends TTL to 24h on successful read
- `SecureValueRepository::exists` (async fn) — Checks existence of non-expired encrypted value using agent_session_id or session_id
- `SecureValueRepository::cleanup` (async fn) — Deletes all secure values for a given session
- `SecureValueRepository::cleanup_expired` (async fn) — Removes all expired rows across all sessions
- `SecureValueRepository::cleanup_expired_for_run` (async fn) — Scoped cleanup of expired values for a specific session/agent combination; respects both session_id and agent_session_id boundaries
- `test_postgres_exists_returns_false_for_unknown_key` (async test) — Verifies exists returns false for non-existent hash_key
- `test_postgres_exists_returns_true_after_persist` (async test) — Verifies exists returns true after successful persist and cleanup
- `cross_session_lookup_via_agent_id` (async test) — Validates agent_session_id enables cross-session value retrieval; confirms other agent IDs cannot decrypt
- `legacy_session_only_lookup_still_works` (async test) — Ensures backward compatibility with session_id-only (no agent_session_id) lookups
- `decrypt_extends_expires_at` (async test) — Confirms decrypt operation extends TTL to 24 hours from now
- `exists_returns_false_for_expired_row` (async test) — Validates exists ignores rows past expiration threshold
- `decrypt_returns_none_for_expired_row` (async test) — Confirms decrypt returns None for rows past expiration threshold
- `cleanup_expired_for_run_deletes_only_expired_in_scope` (async test) — Validates scoped cleanup removes only expired rows in target session; preserves unexpired rows and unrelated sessions
- `cleanup_expired_for_run_respects_agent_session_id` (async test) — Confirms cleanup respects agent_session_id as primary key even with unrelated session_id argument

## File-level notes

- All public methods implement the `SecureValueRepository` trait defined in domain layer
- Uses `async_trait` for async trait implementations
- Error handling is consistent: all database errors mapped to `DagError::StateError`
- All SQL queries use parameterized bindings (safe against injection)
- Dual-path SQL logic in decrypt/exists/cleanup_expired_for_run methods handles both session_id (legacy) and agent_session_id (primary) scopes; OR logic correctly prioritizes agent_session_id for lookups
- TTL extension (24 hours) happens on every successful decrypt; cleanup tracks expired rows via `expires_at` column
- Post-insert visibility probe (lines 107-123) is informational diagnostic logging; adds one SELECT per persist call
- All tests marked `#[ignore]` with DATABASE_URL requirement; use `cargo test -- --ignored` to run
- Test coverage is comprehensive: existence checks, persistence/retrieval, cross-session semantics, TTL expiry, scoped cleanup, both lookup paths
