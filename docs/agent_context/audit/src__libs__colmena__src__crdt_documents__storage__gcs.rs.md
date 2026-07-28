# src/libs/colmena/src/crdt_documents/storage/gcs.rs

**Layer:** infrastructure  **Purpose:** GCS backend placeholder for CRDT document storage. Currently a stub; v1 ships localfs only. This file exists to satisfy the `#[cfg(feature = "gcs")]` arm when the feature is enabled.

## Symbols

- `GcsStorage` (struct, pub) — Placeholder struct for Google Cloud Storage backend implementation. [FLAG: unfinished — feature-gated stub awaiting implementation]
- `GcsStorage::new` (fn, pub) — Constructor that rejects all calls with "not implemented yet — coming in a follow-up task" error. [FLAG: unfinished — returns NotImplemented error instead of actual initialization]
- `ArtifactStorage for GcsStorage` (impl, async trait) — Trait implementation for async storage operations; all methods are stubs. [FLAG: unfinished — all 6 trait methods (list, load_state, load_meta, save_state, save_meta, delete) use unreachable!()]

## File-level notes

- **Entire file is feature-gated.** Code only compiles when `feature = "gcs"` is enabled.
- **All trait methods are unreachable stubs.** The `ArtifactStorage` impl provides no real behavior; any call to these methods will panic.
- **Intentional placeholder.** The comment at line 1–2 confirms this is scaffolding: "v1 ships localfs only; this file exists so the cfg arm compiles." Deferral is documented and expected.
- **No external dependencies imported.** Only uses `super::*` for the trait and error types.
