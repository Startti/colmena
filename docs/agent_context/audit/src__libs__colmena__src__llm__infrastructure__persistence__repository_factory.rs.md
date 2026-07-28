# src/libs/colmena/src/llm/infrastructure/persistence/repository_factory.rs

**Layer:** infrastructure  **Purpose:** Factory that caches and returns `ConversationRepository` instances based on connection URL, supporting both Postgres (via shared `PgPoolRegistry`) and SQLite backends with automatic migrations.

## Symbols

- `ConversationRepositoryFactory` (struct, pub) — Cache-backed factory for `ConversationRepository` instances keyed by connection URL; shares Postgres pools via `PgPoolRegistry` to align with state persistence, secure values, and SQL nodes.
- `registry` (field, private) — Shared `PgPoolRegistry` for obtaining Postgres connection pools.
- `repositories` (field, private) — Thread-safe cache (Arc<Mutex<HashMap>>) of instantiated repositories by URL.
- `new` (fn, pub) — Constructor initializing the factory with a `PgPoolRegistry` and empty repository cache.
- `get_repository` (fn, pub async) — Returns cached repository for the URL, or creates and caches a new one after pool initialization and migrations; supports `postgres://`, `postgresql://`, and `sqlite://` protocols.

## File-level notes

- Postgres and SQLite both run migrations with `set_ignore_missing(true)` to tolerate removed migrations from schema consolidations.
- SQLite pools are capped at 1 connection; Postgres pools come from the shared registry, enabling coordination with state persistence and SQL nodes.
- Protocol detection via `starts_with` prefix matching; unsupported protocols return a descriptive error.
- All database errors are wrapped in `LlmError::RequestFailed` with descriptive messages.
