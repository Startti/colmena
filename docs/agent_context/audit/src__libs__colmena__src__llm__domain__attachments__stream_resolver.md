# src/libs/colmena/src/llm/domain/attachments/stream_resolver.rs

**Layer:** domain  
**Purpose:** Defines a domain port (`AttachmentStreamResolver`) for resolving `$attachment:<document_id>` references to readable streams. Composes `AttachmentRegistry` (document_id → storage_key) and `OutputStorageRepository` (storage_key → StoredStream) to enable downstream consumers (e.g., `http_request` multipart) to access attachment bytes. Plan A of the attachment uniform resolution subsystem.

## Symbols

- `AttachmentResolveError` (enum, public) — discriminated error type distinguishing catalog-level failures (4xx) from infrastructure-level failures (5xx) for call-site routing
- `AttachmentResolveError::NotFound` (variant, public) — indicates document_id not in `conversation_attachments` (hallucinated LLM id or post-GC row)
- `AttachmentResolveError::StorageKeyMissing` (variant, public) — indicates row exists but `storage_key` is NULL (legacy pre-Plan A row with no local copy)
- `AttachmentResolveError::Expired` (variant, public) — indicates catalog has revoked access (TTL elapsed or explicit revocation); backing blob may exist but will not be streamed
- `AttachmentResolveError::StorageError` (variant, public) — propagated `StorageError` from the underlying adapter (network, permission, or missing blob)
- `AttachmentResolveError::RegistryError` (variant, public) — propagated `AttachmentError` from registry query (connection, query error)
- `AttachmentStreamResolver` (trait, public) — async port for resolving attachments to streams; consumers implement this trait with concrete registry + storage adapters
- `AttachmentStreamResolver::resolve` (async method, public) — given `agent_session_id` and `document_id`, returns a `StoredStream` and updates `last_used_at` as a side effect
- `tests` (module, private) — unit test module
- `error_variants_are_distinct` (fn, private) — smoke test verifying error variants format differently in debug output

## File-level notes

- Clean, well-documented domain port definition with clear error semantics
- No infrastructure dependencies; all external integrations (registry, storage) are abstracted behind traits
- Good javadoc comments explaining error variants and their use cases (4xx vs 5xx routing)
- Minimal test coverage (smoke test only); full coverage is in adapter implementations and integration tests
- Part of the larger attachment subsystem (Plan A–C): registration, resolution, and GC lifecycle
