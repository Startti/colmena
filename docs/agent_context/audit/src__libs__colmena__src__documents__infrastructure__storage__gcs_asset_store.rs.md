# src/libs/colmena/src/documents/infrastructure/storage/gcs_asset_store.rs

**Layer:** infrastructure  **Purpose:** Google Cloud Storage adapter implementing the AssetStore port for document asset persistence, using manifest-based listing and reverse-indexing for O(1) session lookup since the GCS SDK lacks list/delete operations.

## Symbols

- `StoredMeta` (struct, private) — Serializable metadata sidecar for each stored asset (session_id, mime, size_bytes, label, created_at)
- `SessionManifest` (struct, private) — Wrapper for a Vec of asset IDs, enabling session-level listing without GCS prefix scan
- `GcsAssetStore` (struct, pub) — Main adapter holding Storage client, GCS bucket resource path, and logical prefix
- `new` (fn, pub async) — Constructor accepting bare bucket name and prefix, builds google_cloud_storage client, trims slashes from prefix
- `asset_dir` (fn, private) — Constructs GCS directory path as `{prefix}/assets/sessions/{session}/{asset}` (handles empty prefix)
- `bytes_key` (fn, private) — Returns `{asset_dir}/bytes.bin` for blob storage
- `meta_key` (fn, private) — Returns `{asset_dir}/meta.json` for metadata sidecar
- `manifest_key` (fn, private) — Returns `{prefix}/assets/sessions/{session}/_manifest.json` for session asset-ID list
- `write` (fn, private async) — Low-level write to GCS returning generation number, sets content_type, handles errors
- `read_bytes` (fn, private async) — Low-level streaming read from GCS, distinguishes 404 (NotFound) from other errors, buffers chunks
- `read_manifest` (fn, private async) — Reads session manifest JSON, gracefully returns empty manifest on 404
- `write_manifest` (fn, private async) — Serializes and writes SessionManifest to GCS
- `index_key` (fn, private) — Constructs global reverse-index key as `{prefix}/assets/index/{asset}` for O(1) session lookup
- `find_session_for` (fn, private async) — Reads global index to resolve session ID from asset ID, converts NotFound errors
- `write_index` (fn, private async) — Writes global index entry mapping asset ID to session ID at upload time
- `upload` (fn, pub async) — Implements AssetStore::upload; writes bytes, metadata, reverse-index, and adds to session manifest (best-effort)
- `read` (fn, pub async) — Implements AssetStore::read; finds session via index, reads meta and bytes, returns tuple with mime type
- `list_by_session` (fn, pub async) — Implements AssetStore::list_by_session; iterates manifest asset IDs, reads metadata, skips deleted entries silently
- `delete` (fn, pub async) — Implements AssetStore::delete; tombstones bytes as "DELETED", blanks meta, removes from session manifest
- `head` (fn, pub async) — Implements AssetStore::head; finds session, reads metadata only, returns AssetSummary without bytes

## File-level notes

- No unit tests — matches existing `GcsArtifactStore` policy (noted in file header).
- Deletion strategy deliberately uses tombstones + manifest removal with reliance on GCS lifecycle rules for physical cleanup (>90 days recommended).
- Global asset index (`{prefix}/assets/index/{asset}`) is the key workaround for lack of prefix-scan APIs — enables O(1) session resolution without StorageControl.
- All key-construction methods handle empty prefix edge case (no leading/trailing slash).
- `read_bytes` error handling distinguishes 404 (asset not found) from other transport errors with specific error wrapping.
- Manifest is last-writer-wins (concurrent uploads may briefly race, but convergence is guaranteed by overwrite semantics).
- No obvious dead code, unfinished patterns, or missing error boundaries.

