# src/libs/colmena/src/llm/infrastructure/attachments/mod.rs

**Layer:** infrastructure  
**Purpose:** Module organization for attachment stream resolution infrastructure. Declares the stream resolver implementation submodule and re-exports the primary concrete type for use by LLM and other nodes.

## Symbols

- `stream_resolver_impl` (mod, pub) — submodule containing the concrete `AttachmentStreamResolver` implementation (Plan A: registry lookup + fallback to raw storage_key)
- `AttachmentStreamResolverImpl` (re-export, pub) — re-exported concrete implementation that composes an `AttachmentRegistry` and `OutputStorageRepository` to resolve attachments by `(agent_session_id, document_id)`

## File-level notes

- Minimal and well-organized module file; no code smell detected.
- The implementation (`stream_resolver_impl.rs`) follows the canonical two-path resolution strategy: (1) registry lookup with `last_used_at` touch-up, (2) backward-compat fallback to raw storage_key.
- Test coverage in the submodule is comprehensive: registry hit, registry miss with fallback, both miss (NotFound), storage_key absent (legacy row).
- No dependencies or flags.
