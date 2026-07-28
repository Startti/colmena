# src/libs/colmena/src/llm/infrastructure/persistence/mod.rs

**Layer:** infrastructure  **Purpose:** Aggregates multiple persistence implementations for conversation history and attachment storage (PostgreSQL, SQLite, in-memory). Provides a factory pattern and re-exports the concrete adapters as a unified interface.

## Symbols

- `in_memory_conversation_repository` (mod, pub) — Module containing in-memory conversation repository implementation
- `postgres_attachment_registry` (mod, pub) — Module containing PostgreSQL-backed attachment registry adapter
- `postgres_conversation_repository` (mod, pub) — Module containing PostgreSQL-backed conversation repository adapter
- `repository_factory` (mod, pub) — Module containing factory for instantiating appropriate repositories
- `sqlite_attachment_registry` (mod, pub) — Module containing SQLite-backed attachment registry adapter
- `sqlite_conversation_repository` (mod, pub) — Module containing SQLite-backed conversation repository adapter
- `InMemoryConversationRepository` (pub use) — Re-export of in-memory conversation repository implementation
- `PostgresAttachmentRegistry` (pub use) — Re-export of PostgreSQL attachment registry implementation
- `PostgresConversationRepository` (pub use) — Re-export of PostgreSQL conversation repository implementation
- `ConversationRepositoryFactory` (pub use) — Re-export of factory for creating conversation repositories
- `SqliteAttachmentRegistry` (pub use) — Re-export of SQLite attachment registry implementation
- `SqliteConversationRepository` (pub use) — Re-export of SQLite conversation repository implementation

## File-level notes

- Clean facade module that aggregates persistence implementations with proper re-exports
- No TODOs, unimplemented!(), panics, or error handling gaps
- No duplication or unclear naming; straightforward module aggregation pattern
- All implementations are always built (no conditional compilation or feature gates)
- Module organization (separate file per implementation) supports clean separation of concerns
