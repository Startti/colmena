# src/libs/colmena/src/documents/infrastructure/storage/mod.rs

**Layer:** infrastructure  **Purpose:** Module index that conditionally exposes local filesystem and GCS-based storage adapters for the documents subsystem via feature flags (`gcs` feature gates cloud storage).

## Symbols

- `gcs_asset_store` (mod, pub) — Feature-gated module declaring GCS asset store implementation.
- `gcs_store` (mod, pub) — Feature-gated module declaring GCS artifact store implementation.
- `local_fs_asset_store` (mod, pub) — Module declaring local filesystem asset store implementation.
- `local_fs_store` (mod, pub) — Module declaring local filesystem artifact store implementation.
- `GcsAssetStore` (re-export, pub) — Struct re-exported from `gcs_asset_store` module (feature-gated with `gcs`).
- `GcsArtifactStore` (re-export, pub) — Struct re-exported from `gcs_store` module (feature-gated with `gcs`).
- `LocalFsAssetStore` (re-export, pub) — Struct re-exported from `local_fs_asset_store` module (unconditional).
- `LocalFsStore` (re-export, pub) — Struct re-exported from `local_fs_store` module (unconditional).

## File-level notes

- Pure module index with no logic or trait definitions — follows hexagonal architecture by organizing storage infrastructure adapters.
- Feature gating (`gcs` feature) correctly guards cloud storage backends while local filesystem adapters remain always available.
- All re-exports are intentional and match their module declarations with no asymmetries.
