//! Documents CRDT spike (Phase 0). See
//! `docs/superpowers/specs/2026-05-31-documents-crdt-spike-design.md`.
//!
//! This module is SPIKE code. It is intentionally isolated so it can be
//! removed in one commit after the GO/NO-GO verdict.

pub mod tool_executor;
pub mod artifact_id;
pub mod doc_registry;
pub mod projection;
pub mod runtime;
pub mod server;
pub mod snapshot_writer;
pub mod storage;
pub mod xlsx_import;
pub mod yjs_protocol;

pub use artifact_id::ArtifactId;
pub use doc_registry::{DocRegistry, RegisteredArtifact};
pub use runtime::{CrdtDocumentsRuntime, RuntimeError, DEFAULT_STORAGE_ROOT};
pub use snapshot_writer::{spawn_writer, SnapshotHandle};
pub use storage::{ArtifactMeta, ArtifactStorage, LocalFsStorage, StorageConfig, StorageError};
