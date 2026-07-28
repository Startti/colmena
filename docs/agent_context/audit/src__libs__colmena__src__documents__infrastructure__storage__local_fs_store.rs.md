# src/libs/colmena/src/documents/infrastructure/storage/local_fs_store.rs

**Layer:** infrastructure  
**Purpose:** Local filesystem-based implementation of the ArtifactStore trait (domain port). Manages version-controlled artifact storage with metadata, rendered binaries, patches, and blobs, using atomic write-then-rename for crash safety.

## Symbols

- `LocalFsStore` (struct, pub) — file-system artifact store with root directory path
- `LocalFsStore::new(root)` (fn, pub) — constructs a new LocalFsStore from a root path
- `LocalFsStore::art_dir()` (fn, private) — returns artifact directory path for a given ID
- `LocalFsStore::meta_path()` (fn, private) — returns path to meta.json for an artifact
- `LocalFsStore::head_path()` (fn, private) — returns path to HEAD file tracking current version
- `LocalFsStore::version_dir()` (fn, private) — returns path to version directory for artifact+version pair
- `LocalFsStore::atomic_write()` (fn, private, async) — atomically writes bytes to path via temp file + rename
- `ArtifactStore::create_artifact()` (fn, async) — creates artifact directory and writes metadata.json
- `ArtifactStore::write_version()` (fn, async) — writes version data: ir.json, render binary, patch_applied.json, and blobs to disk
- `ArtifactStore::read_version()` (fn, async) — reads version data from disk (IR, rendered binary, patch applied) [FLAG: unfinished — blobs hardcoded to empty Vec, never loaded]
- `ArtifactStore::read_current()` (fn, async) — reads current version by parsing HEAD pointer and delegating to read_version()
- `ArtifactStore::list_versions()` (fn, async) — lists version directories, parses names as VersionIds, sorts numerically
- `ArtifactStore::set_head()` (fn, async) — updates HEAD pointer file with optional precondition check against expected_current
- `ArtifactStore::delete_version()` (fn, async) — removes version directory if it exists
- `ArtifactStore::read_meta()` (fn, async) — reads and parses artifact meta.json
- `ArtifactStore::update_meta()` (fn, async) — overwrites artifact meta.json atomically
- `ArtifactStore::delete_artifact()` (fn, async) — removes entire artifact directory tree
- `tests::sample_version_data()` (fn, private) — factory for test VersionData with mock IR, binary, and empty blobs
- `tests::create_write_read_cycle()` (test, async) — integration test: create artifact → write version → set HEAD → read current
- `tests::set_head_precondition_mismatch_fails()` (test, async) — verifies PreconditionFailed error when HEAD doesn't match expected_current
- `tests::list_versions_sorted()` (test, async) — verifies versions are sorted numerically (v1 < v2 < v10)

## File-level notes

- **Unfinished blob loading** (line 130): `read_version()` returns `blobs: Vec::new()` unconditionally, while `write_version()` writes blobs to disk (lines 82–90). The read path never loads them back. Either the feature is incomplete or the write is dead code.
- **Silent default on missing HEAD** (line 172): `set_head()` uses `unwrap_or_default()` when reading the current HEAD file, treating "file not found" and "read error" the same as "empty string". This masks errors during precondition checks; a missing HEAD should probably be distinguished from one that exists but is empty.
- **No blob directory validation during read** (line 94–132): The method doesn't check for or attempt to read the `blobs/` subdirectory that `write_version()` creates, leaving a silent consistency gap.
- **No tests for blob round-trip**: Tests use `sample_version_data()` with empty blobs; no test validates blob write→read cycle.
