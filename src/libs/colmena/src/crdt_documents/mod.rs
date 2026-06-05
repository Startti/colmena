//! Documents CRDT spike (Phase 0). See
//! `docs/superpowers/specs/2026-05-31-documents-crdt-spike-design.md`.
//!
//! This module is SPIKE code. It is intentionally isolated so it can be
//! removed in one commit after the GO/NO-GO verdict.

pub mod artifact_id;
pub mod change_tracker;
pub mod change_tracker_store;
pub mod crdt_backend;
pub mod df_records;
pub mod df_writer;
pub mod doc_registry;
pub mod formula_engine;
pub mod formula_engine_yrs_resolver;
pub mod narration;
pub mod process_runtime;
pub mod projection;
pub mod runtime;
pub mod server;
pub mod snapshot_writer;
pub mod storage;
pub mod tool_executor;
pub mod ws_peer;
pub mod xlsx_export;
pub mod xlsx_import;
pub mod yjs_protocol;

pub use artifact_id::ArtifactId;
pub use change_tracker::{ChangeEvent, ChangeTracker};
pub use change_tracker_store::{
    ChangeTrackerStore, ChangeTrackerStoreRef, InMemoryChangeTrackerStore, NewEvent,
    StoreError as ChangeTrackerStoreError, StoredArtifact, StoredEvent,
};
pub use crdt_backend::{BackendError, CrdtBackend, DirectBackend, RestBackend};
pub use df_records::{
    build_records_for_sheets, build_sheet_records, RecordsError, SheetRecords,
    COMBINED_RECORDS_SIZE_CAP_BYTES,
};
pub use df_writer::{
    resolve_unique_sheet_name, write_records_as_new_sheet, WriteResult, WriterError,
    MAX_OUTPUT_SHEET_ROWS, MAX_SHEET_NAME_LEN,
};
pub use doc_registry::{DocRegistry, RegisteredArtifact};
pub use runtime::{CrdtDocumentsRuntime, RuntimeError, DEFAULT_STORAGE_ROOT};
pub use snapshot_writer::{spawn_writer, SnapshotHandle};
pub use storage::{ArtifactMeta, ArtifactStorage, LocalFsStorage, StorageConfig, StorageError};
pub use ws_peer::{WsPeerArtifact, WsPeerError};
