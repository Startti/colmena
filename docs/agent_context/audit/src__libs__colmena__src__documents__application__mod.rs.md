# src/libs/colmena/src/documents/application/mod.rs

**Layer:** application  **Purpose:** Public interface aggregator for the documents application layer, re-exporting all use-case modules (Excel/HTML/Word operations, document CRUD, asset management, versioning) and runtime types.

## Symbols

- `apply_excel_ops` (mod, pub) — Re-exports Excel document modification use cases
- `apply_html_ops` (mod, pub) — Re-exports HTML document modification use cases
- `apply_patch` (mod, pub) — Re-exports document patch application use cases
- `apply_word_ops` (mod, pub) — Re-exports Word document modification use cases
- `create_document` (mod, pub) — Re-exports document creation use case
- `delete_asset` (mod, pub) — Re-exports asset deletion use case
- `get_head` (mod, pub) — Re-exports document head retrieval use case
- `list_assets` (mod, pub) — Re-exports asset enumeration use case
- `list_versions` (mod, pub) — Re-exports version history enumeration use case
- `read_document` (mod, pub) — Re-exports document read use case
- `rollback` (mod, pub) — Re-exports document version rollback use case
- `runtime` (mod, pub) — Re-exports document runtime module
- `DocumentRuntime` (pub use from runtime) — Runtime coordinator for document processing
- `DEFAULT_RETENTION` (pub use from runtime) — Default asset retention period constant
- `DEFAULT_STORAGE_ROOT` (pub use from runtime) — Default filesystem storage root path constant

## File-level notes

- Clean module facade with no executable code, only declarations and re-exports
- All symbols exported with full visibility; application layer API is comprehensively exposed
- No abstract complexity or conditional logic — pure organizational structure
