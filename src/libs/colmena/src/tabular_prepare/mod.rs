//! Large tabular preparation (dark behind `COLMENA_LARGE_TABULAR`).
//!
//! A large CSV/Excel attachment is prepared once into typed columnar tables,
//! tracked in a registry keyed by the source `storage_key`. This module holds
//! the registry ([`registry`], with a SQLite implementation so far) and will
//! hold the ports the host replaces, the `ensure_prepared` entry a tool calls,
//! and the cleanup pass `attachment_gc` runs. Nothing here converts a file yet
//! and nothing calls it.
//!
//! See `docs/developer_guide/54_tabular_prepare.md`.

pub mod registry;
pub mod sqlite_registry;

#[cfg(test)]
mod registry_contract;
