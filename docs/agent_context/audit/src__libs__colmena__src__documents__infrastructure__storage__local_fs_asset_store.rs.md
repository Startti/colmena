# src/libs/colmena/src/documents/infrastructure/storage/local_fs_asset_store.rs

**Layer:** infrastructure  **Purpose:** Filesystem-based adapter for the AssetStore port, storing asset bytes and metadata in a directory tree organized by session/asset ID with atomic file operations.

## Symbols

- `LocalFsAssetStore` (struct, pub) — Holds a root PathBuf and implements the AssetStore trait via filesystem operations
- `StoredMeta` (struct, private) — Serde-serializable metadata (session_id, mime, size_bytes, label, created_at) persisted to meta.json
- `LocalFsAssetStore::new` (fn, pub) — Constructs a LocalFsAssetStore from a root path
- `LocalFsAssetStore::asset_dir` (fn, private) — Computes the standard directory path for an asset: root/sessions/{session_id}/{asset_id}
- `LocalFsAssetStore::find_asset_dir` (fn, private) — Searches all session directories to locate an asset's directory by asset_id alone, returns NotFound if not found
- `AssetStore::upload` (async fn, impl) — Atomically writes asset bytes and metadata using tmp→rename pattern to ensure consistency
- `AssetStore::read` (async fn, impl) — Loads bytes and mime type from asset directory, requires asset to be found via find_asset_dir
- `AssetStore::list_by_session` (async fn, impl) — Enumerates all AssetSummary for a session directory, silently skips incomplete entries missing meta.json
- `AssetStore::delete` (async fn, impl) — Removes the entire asset directory via fs::remove_dir_all
- `AssetStore::head` (async fn, impl) — Reads metadata only without loading bytes, reconstructs AssetSummary with session_id extracted from directory path [FLAG: improvement — session_id should be taken from StoredMeta instead of extracting from path, inconsistent with list_by_session and fragile to directory layout changes]
- `tests::upload_then_read_roundtrip` (test) — Verifies roundtrip: upload with label, then read returns correct bytes and mime
- `tests::list_returns_summaries_with_label` (test) — Verifies list_by_session includes all assets with correct labels
- `tests::delete_removes_asset` (test) — Verifies deleted asset cannot be read, returns NotFound
- `tests::list_empty_session_returns_empty_vec` (test) — Verifies list_by_session returns empty vec for nonexistent session
- `tests::head_without_loading_bytes` (test) — Verifies head returns size and mime without reading asset bytes

## File-level notes

- **Atomic writes**: upload uses tmp→rename pattern for both bytes.bin and meta.json to prevent partial/corrupt writes.
- **Session-scoped retrieval**: find_asset_dir searches all sessions when only asset_id is provided; caller (read, delete, head) must provide or find the session. This is intentional for the AssetStore port design.
- **IO error handling**: read_dir operations map IO errors to AssetError::Storage with descriptive messages. try_exists errors are silently treated as "not found" (line 54, 122) — could hide real IO problems but acceptable for search operations.
- **Metadata consistency**: StoredMeta::session_id is redundantly stored but not used in head() — lines 171–175 extract session_id from directory path instead, creating inconsistency with list_by_session (line 146) which uses the stored session_id.
