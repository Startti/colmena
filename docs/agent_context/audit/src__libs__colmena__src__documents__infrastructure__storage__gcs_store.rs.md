# src/libs/colmena/src/documents/infrastructure/storage/gcs_store.rs

**Layer:** infrastructure  
**Purpose:** GCS-backed implementation of the `ArtifactStore` port for persisting document artifacts (versions, metadata, binary renders, and patches). Uses `google-cloud-storage` v1.x with CAS (`set_if_generation_match`) for HEAD pointer coordination and GCS lifecycle policies for cleanup.

## Symbols

### Types
- `VersionManifest` (struct, private) — Holds an ordered list of version IDs; deserialized from `_manifest.json` in GCS.

### Public Types
- `GcsArtifactStore` (struct, pub) — Wraps `Storage` client, bucket path, and logical prefix; implements `ArtifactStore` trait.

### Impl methods (GcsArtifactStore)
- `new` (fn, pub async) — Builds a GCS `Storage` client and returns a configured store; bucket is bare name, prefix is trimmed of slashes.
- `art_prefix` (fn, private) — Constructs `{prefix}/artifacts/{id}` or `artifacts/{id}` depending on prefix; base for all key derivations.
- `meta_key` (fn, private) — Returns `{art_prefix}/meta.json` key.
- `head_key` (fn, private) — Returns `{art_prefix}/HEAD` key (stores current version ID as text).
- `manifest_key` (fn, private) — Returns `{art_prefix}/._manifest.json` key (version list).
- `ir_key` (fn, private) — Returns `{art_prefix}/versions/{vid}/ir.json` key.
- `render_key` (fn, private) — Returns `{art_prefix}/versions/{vid}/render.{ext}` key (Word/Excel/HTML binary).
- `patch_key` (fn, private) — Returns `{art_prefix}/versions/{vid}/patch_applied.json` key.
- `blob_key` (fn, private) — Returns `{art_prefix}/versions/{vid}/blobs/{name}` key.
- `write` (fn, private async) — Writes bytes to GCS with content-type; returns generation number.
- `write_create_only` (fn, private async) — Writes bytes only if object does not exist (generation=0 precondition); returns 412 Precondition Failed if already exists.
- `write_cas` (fn, private async) — Writes bytes with generation-match precondition (compare-and-swap); used to coordinate HEAD updates.
- `read_bytes` (fn, private async) — Reads bytes from GCS streaming into buffer; maps 404 to `NotFound` error.
- `read_bytes_with_gen` (fn, private async) — Reads bytes and returns generation number alongside (needed for CAS operations).
- `read_manifest` (fn, private async) — Deserializes `VersionManifest` from GCS; returns empty manifest if file does not exist (idempotent).
- `write_manifest` (fn, private async) — Serializes and writes `VersionManifest` to GCS.

### ArtifactStore trait impl
- `create_artifact` (fn, pub async) — Writes artifact metadata and an empty version manifest to GCS.
- `write_version` (fn, pub async) — Writes IR (JSON), rendered binary (docx/xlsx/html), patch metadata, and all blobs; then appends version ID to manifest (last-writer-wins on manifest update).
- `read_version` (fn, pub async) — Reads IR, rendered binary, and patch metadata; **does NOT read back blobs** (returns empty vec). [FLAG: unfinished]
- `read_current` (fn, pub async) — Reads HEAD pointer and delegates to `read_version`.
- `list_versions` (fn, pub async) — Deserializes manifest and sorts versions by semantic number.
- `set_head` (fn, pub async) — Atomically sets HEAD pointer with CAS: first-time write uses generation=0 (create-only); subsequent writes read current generation and use it as precondition.
- `delete_version` (fn, pub async) — Removes version ID from manifest; GCS objects left in place for lifecycle policy cleanup.
- `read_meta` (fn, pub async) — Deserializes artifact metadata from GCS.
- `update_meta` (fn, pub async) — Overwrites artifact metadata in GCS.
- `delete_artifact` (fn, pub async) — Writes tombstone to HEAD and clears manifest; GCS objects left for lifecycle policy cleanup.

## File-level notes

### Unfinished Implementation
**Blob Reading Gap:** The `read_version` method (line 345) hard-codes `blobs: Vec::new()` instead of reading the blob files that were written during `write_version` (lines 299–306). This is a data-loss gap: any blobs stored for a version cannot be retrieved. The infrastructure to read them (`read_bytes`, `blob_key`) exists but is not used.

### Naming Clarity
Parameters `_id` and `_version` in `read_version` (line 319–320) are prefixed with underscore despite being actively used in the function body (line 322+). The underscore prefix conventionally signals unused variables; here they are used, so the names should be `id` and `version` for clarity.

### Resilience by Silent Fallback
Multiple locations (lines 351, 382) use `std::str::from_utf8(&bytes).unwrap_or("").trim()` to parse HEAD content (expecting a version ID as text). UTF-8 decode failure silently defaults to empty string, which may mask data corruption but provides graceful degradation.

### Concurrency Semantics
- Manifest update at line 308 is documented as "best-effort; overwrite is last-writer-wins" — concurrent calls to `write_version` can race on the manifest, but GCS object writes themselves are atomic per key.
- HEAD pointer uses CAS (`set_if_generation_match`) to ensure safe coordination of version switches, but only if the caller reads current generation first (see line 381–389).

### Storage Design
- Version IDs are stored as plain text in HEAD for human readability.
- Manifest is a simple JSON list, not ordered/versioned — no ordering guarantees on concurrent appends.
- Deletion is soft (tombstone + manifest clear); GCS lifecycle rules (configured externally) handle hard cleanup after a configured age threshold.
