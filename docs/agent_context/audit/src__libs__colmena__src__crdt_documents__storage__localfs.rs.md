# src/libs/colmena/src/crdt_documents/storage/localfs.rs

**Layer:** infrastructure  
**Purpose:** Filesystem-backed storage adapter implementing `ArtifactStorage` trait for CRDT document artifacts. Provides atomic read/write operations for artifact metadata and state via temporary files and atomic renames.

## Symbols

- `LocalFsStorage` (pub struct) — holds root directory path for artifact storage hierarchy
- `LocalFsStorage::new` (pub fn) — initializes storage with root directory creation
- `LocalFsStorage::artifact_dir` (fn, private) — computes artifact subdirectory path
- `LocalFsStorage::meta_path` (fn, private) — computes meta.json file path for an artifact
- `LocalFsStorage::state_path` (fn, private) — computes state.yjs file path for an artifact
- `ArtifactStorage::list` (async trait method) — enumerates artifacts, reads meta.json files, skips malformed entries with warning
- `ArtifactStorage::load_state` (async trait method) — loads artifact state bytes; returns None if file not found
- `ArtifactStorage::load_meta` (async trait method) — loads and deserializes artifact metadata; returns None if file not found
- `ArtifactStorage::save_state` (async trait method) — atomically writes state via temp file with ULID suffix
- `ArtifactStorage::save_meta` (async trait method) — atomically writes metadata via temp file with ULID suffix
- `ArtifactStorage::delete` (async trait method) — removes artifact directory if it exists (idempotent)
- `tests::temp_root` (fn, private) — creates temp directory for test isolation
- `tests::save_then_load_state_round_trip` (async test) — verifies state persistence roundtrip
- `tests::list_returns_all_with_meta` (async test) — verifies artifact enumeration
- `tests::delete_removes_dir` (async test) — verifies artifact deletion

## File-level notes

- No flags identified. Code is well-formed infrastructure implementing a trait adapter with proper error handling, atomic writes, and test coverage. Directory existence check in `delete()` is idempotent design (succeeds even if artifact already deleted). Malformed meta.json entries logged as warnings in `list()` rather than failing hard, allowing partial recovery on corruption.
