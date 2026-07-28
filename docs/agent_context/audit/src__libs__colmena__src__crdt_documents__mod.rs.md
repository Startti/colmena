# src/libs/colmena/src/crdt_documents/mod.rs

**Layer:** infrastructure  **Purpose:** Root module for CRDT documents subsystem (Phase 0 spike). Declares 21 submodules and re-exports 64 public types for document collaboration, change tracking, storage, formula evaluation, and WebSocket synchronization.

## Symbols

### Submodules
- `artifact_id` (mod, pub) — Artifact identifier types and utilities
- `change_tracker` (mod, pub) — Change event detection and tracking
- `change_tracker_store` (mod, pub) — Persistent and in-memory change event storage backends
- `crdt_backend` (mod, pub) — CRDT backend providers (Direct and REST)
- `df_records` (mod, pub) — DataFrame record building and validation for sheets
- `df_writer` (mod, pub) — Writing DataFrame records to documents and sheet creation
- `doc_registry` (mod, pub) — Document registry for artifact lifecycle management
- `formula_engine` (mod, pub) — Formula evaluation engine for spreadsheet calculations
- `formula_engine_yrs_resolver` (mod, pub) — Yrs-integrated formula resolver
- `narration` (mod, pub) — Narration and documentation helpers
- `process_runtime` (mod, pub) — Process-level runtime management
- `projection` (mod, pub) — Document state projections
- `recalc_observer` (mod, pub) — Recalculation observation and tracking
- `runtime` (mod, pub) — CRDT documents runtime orchestration
- `server` (mod, pub) — Server-side components for document synchronization
- `snapshot_writer` (mod, pub) — Snapshot persistence and writing
- `storage` (mod, pub) — Abstract and concrete artifact storage backends
- `tool_executor` (mod, pub) — Tool execution within CRDT documents
- `ws_peer` (mod, pub) — WebSocket peer protocol and connection handling
- `xlsx_export` (mod, pub) — Export documents to XLSX format
- `xlsx_import` (mod, pub) — Import XLSX files into documents
- `yjs_protocol` (mod, pub) — Yjs protocol message handling

### Re-exported Types (artifact_id)
- `ArtifactId` (type, pub) — Artifact identifier

### Re-exported Types (change_tracker)
- `ChangeEvent` (type, pub) — Change event record
- `ChangeTracker` (type, pub) — Change tracking controller

### Re-exported Types (change_tracker_store)
- `ChangeTrackerStore` (trait, pub) — Trait for change event storage backends
- `ChangeTrackerStoreRef` (type, pub) — Reference wrapper for change tracker store
- `InMemoryChangeTrackerStore` (struct, pub) — In-memory change event storage implementation
- `NewEvent` (type, pub) — New change event descriptor
- `ChangeTrackerStoreError` (type alias, pub) — Error type from change_tracker_store
- `StoredArtifact` (type, pub) — Persisted artifact metadata
- `StoredEvent` (type, pub) — Persisted change event

### Re-exported Types (crdt_backend)
- `BackendError` (type, pub) — CRDT backend error type
- `CrdtBackend` (trait, pub) — CRDT backend provider trait
- `DirectBackend` (struct, pub) — Direct/in-process CRDT backend implementation
- `RestBackend` (struct, pub) — REST-based CRDT backend implementation

### Re-exported Types (df_records)
- `build_records_for_sheets` (fn, pub) — Build DataFrame records from sheet data
- `build_sheet_records` (fn, pub) — Build sheet-specific records
- `RecordsError` (type, pub) — DataFrame records error type
- `SheetRecords` (type, pub) — Validated sheet records container
- `COMBINED_RECORDS_SIZE_CAP_BYTES` (const, pub) — Maximum combined record size in bytes

### Re-exported Types (df_writer)
- `apply_records_to_doc` (fn, pub) — Apply DataFrame records to an existing document
- `resolve_unique_sheet_name` (fn, pub) — Resolve a unique sheet name avoiding collisions
- `write_records_as_new_sheet` (fn, pub) — Write DataFrame records as a new sheet
- `DfWriterOutcome` (type, pub) — Outcome of DataFrame write operation
- `FormulaReplacement` (type, pub) — Formula replacement descriptor
- `WriteResult` (type, pub) — Write operation result
- `WriterError` (type, pub) — DataFrame writer error type
- `MAX_OUTPUT_SHEET_ROWS` (const, pub) — Maximum rows in output sheet
- `MAX_SHEET_NAME_LEN` (const, pub) — Maximum sheet name length

### Re-exported Types (doc_registry)
- `DocRegistry` (type, pub) — Document registry for lifecycle management
- `RegisteredArtifact` (type, pub) — Registered artifact descriptor

### Re-exported Types (runtime)
- `CrdtDocumentsRuntime` (type, pub) — CRDT documents runtime orchestrator
- `RuntimeError` (type, pub) — Runtime execution error type
- `DEFAULT_STORAGE_ROOT` (const, pub) — Default storage root directory path

### Re-exported Types (snapshot_writer)
- `spawn_writer` (fn, pub) — Spawn a snapshot writer task
- `SnapshotHandle` (type, pub) — Handle to snapshot writer

### Re-exported Types (storage)
- `ArtifactMeta` (type, pub) — Artifact metadata
- `ArtifactStorage` (trait, pub) — Abstract artifact storage trait
- `LocalFsStorage` (struct, pub) — Local filesystem storage implementation
- `StorageConfig` (type, pub) — Storage configuration
- `StorageError` (type, pub) — Storage operation error type

### Re-exported Types (ws_peer)
- `WsPeerArtifact` (type, pub) — WebSocket peer artifact connection
- `WsPeerError` (type, pub) — WebSocket peer error type

## File-level notes
- **Spike code warning**: Top comment explicitly marks this module as Phase 0 spike code intentionally isolated for removal after GO/NO-GO verdict. No internal implementation here; pure module aggregation and re-export of 21 submodule public APIs.
- **Re-export coverage**: Exports all major public types from submodules for consumer convenience; follows standard Rust module pattern.
- **No private symbols or implementations**: This is purely a facade — all logic lives in submodules.
