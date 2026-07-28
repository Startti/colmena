# src/libs/colmena/src/documents/application/runtime.rs

**Layer:** application  
**Purpose:** Composes pre-built use cases for the documents feature from JSON config, supporting pluggable storage backends (localfs, GCS). Provides a single entry point for DAG nodes and synthetic tools to access document operations.

## Symbols

- `DEFAULT_RETENTION` (const, pub) — Default retention window of 20 versions per artifact when caller doesn't specify one
- `DEFAULT_STORAGE_ROOT` (const, pub) — Default storage path for localfs backend: `.colmena/documents`
- `DEFAULT_ASSET_STORAGE_ROOT` (const, pub) — Default storage path for assets on localfs backend: `.colmena/documents/assets` [FLAG: dead_candidate — defined but never used in code (not referenced in asset_root derivation at line 113–120)]
- `DEFAULT_ASSET_GCS_PREFIX` (const, pub) — Default GCS object prefix for assets: `colmena/documents/assets`
- `DEFAULT_MAX_ASSET_SIZE_BYTES` (const, pub) — Default maximum asset upload size: 10 MiB
- `default_allowed_mimes()` (fn, private) — Returns HashSet of allowed asset MIME types: png, jpeg, gif, webp
- `DocumentRuntime` (struct, pub) — Container holding Arc-wrapped artifact store, asset store, and all 10 pre-built use cases (create, apply, read, get_head, list_versions, rollback, upload_asset, list_assets, delete_asset)
- `DocumentRuntime::from_config()` (method, pub async) — Constructs runtime from JSON config; auto-detects storage backend (localfs or gcs, feature-gated), creates directories, provisions use case instances with renderer/validator/id-generator traits
- `DocumentRuntime::with_store()` (method, pub) — Builds runtime around existing stores; wires all 10 use cases with injected renderers, validators, and id generator (primarily for testing and shared-store scenarios)
- `tests` (mod, private) — 4 unit tests: from_config_defaults_to_localfs, from_config_rejects_gcs_without_feature (feature-gated), from_config_rejects_unknown_backend, runtime_creates_and_reads_document, runtime_creates_html_and_serves_asset_use_cases

## File-level notes

- **Module purpose well-established**: Top-level doc comment clearly explains this is the runtime/service-locator for the documents feature, used by DAG nodes and synthetic tools via OnceCell sharing.
- **Backend abstraction clean**: Storage implementation (localfs vs. GCS) properly gated behind feature flags and trait boundaries. Unknown backends rejected with clear error message.
- **Configuration-driven**: All constants have defaults, all are overridable via config JSON. Unknown config fields silently ignored for forward compatibility.
- **Asset root derivation**: When `asset_storage_root` is not in config, the logic derives a sibling directory named `<artifacts_dir>_assets` (e.g., `.colmena/documents_assets`). The `DEFAULT_ASSET_STORAGE_ROOT` constant is a static suggestion but never used as the actual default.
- **Test coverage adequate**: 4 tests cover localfs default path, gcs rejection (without feature), unknown backend rejection, document lifecycle (create + read), and asset upload + listing; all use tempdir for isolation.
- **No breaking risks**: The struct and both builder methods are stable and backward-compatible; no use of todo!(), unimplemented!(), or deprecated APIs.
