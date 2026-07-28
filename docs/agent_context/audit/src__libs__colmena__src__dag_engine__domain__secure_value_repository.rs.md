# src/libs/colmena/src/dag_engine/domain/secure_value_repository.rs

**Layer:** domain  **Purpose:** Port trait defining the contract for encrypted secure value storage and retrieval using Postgres pgcrypto (OpenPGP CFB symmetric encryption), with session-scoped and cross-session (agent_session_id) keying.

## Symbols

- `SecureValueRepository` (trait, pub) — Async trait for managing encrypted sensitive values with persistence, lookup, existence checks, and cleanup operations; implementations handle pgcrypto pgp_sym_encrypt/pgp_sym_decrypt keyed by SECURE_VALUES_KEY
- `persist` (method, async) — Encrypts and stores a sensitive value, keyed by session_id + optional agent_session_id; tagged with source node ID, hash_key placeholder, and field name for auditing
- `decrypt` (method, async) — Retrieves and decrypts a value by hash_key; falls back to session_id if agent_session_id not provided; returns None if key not found
- `exists` (method, async) — Checks presence of a hash_key without necessarily materializing plaintext; default impl calls decrypt and discards value (correct but suboptimal per docs — production impls should override with direct SQL EXISTS check)
- `cleanup` (method, async) — Deletes all secure values for a session after DAG completion
- `cleanup_expired` (method, async) — Periodic safety net that deletes expired values across all sessions
- `cleanup_expired_for_run` (method, async) — Deletes only expired rows within a run's scope (rows matching session_id OR agent_session_id); called at end of every Completed DAG run to preserve unexpired cross-session values for conversation continuity

## File-level notes

- Clean domain-layer port definition with no infrastructure coupling
- All methods correctly documented for cross-session behavior (agent_session_id fallback pattern is consistent)
- Comment correctly documents pgcrypto cipher as OpenPGP CFB (not AES-256-GCM)
- Uses `#[async_trait]` for async method support in trait
- All methods return `Result<T, DagError>` following hexagonal error propagation
- No dead code, unfinished implementations, or obvious improvements identified
