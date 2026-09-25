# src/libs/colmena/src/llm/infrastructure/attachments/mod.rs

**Layer:** infrastructure  
**Purpose:** Module organization for attachment stream resolution infrastructure. Declares the stream resolver implementation submodule and re-exports the primary concrete type for use by LLM and other nodes.

## Symbols

- `stream_resolver_impl` (mod, pub) — submodule containing the concrete `AttachmentStreamResolver` implementation (Plan A: registry lookup; a miss is `NotFound`, never a raw storage_key read)
- `AttachmentStreamResolverImpl` (re-export, pub) — re-exported concrete implementation that composes an `AttachmentRegistry` and `OutputStorageRepository` to resolve attachments by `(agent_session_id, document_id)`

## File-level notes

- Minimal and well-organized module file; no code smell detected.
- The implementation (`stream_resolver_impl.rs`) resolves only through the registry, with a `last_used_at` touch-up; there is no raw storage_key fallback.
- Test coverage in the submodule: registry hit, ids that are not the session's (NotFound, storage untouched), storage_key absent (legacy row).
- No dependencies or flags.
