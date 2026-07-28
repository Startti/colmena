# src/libs/colmena/src/gdocs/domain/traits.rs

**Layer:** domain  
**Purpose:** Hexagonal port defining the abstract interface for Google Docs and Drive REST operations. Production implementations live in infrastructure; tests use `MockDocsClient` via `mockall::automock`.

## Symbols

- `DocsClient` (trait, pub) — Hexagonal port defining all methods the application layer needs from Google Docs + Drive REST surface; implementations in infrastructure layer

### Lifecycle methods
- `create` (async fn, pub) — Create a blank doc with the given title, optionally placing it in a Drive parent folder
- `create_from_markdown` (async fn, pub) — Create a doc from a markdown string with native Drive conversion; may re-export to detect lossy elements
- `create_from_docx` (async fn, pub) — Upload and convert a .docx byte buffer to a native Google Doc
- `share` (async fn, pub) — Grant access to an email address with the requested role
- `list_permissions` (async fn, pub) — List every Drive permission on a doc (Bundle 2B, 2026-06-11)
- `delete_permission` (async fn, pub) — Revoke a permission by its permission_id from Drive (Bundle 2B)
- `list_documents` (async fn, pub) — Discover documents visible to the OAuth user, filtered by criteria; used by `gdocs_list_documents`
- `export` (async fn, pub) — Export the doc as bytes in the requested format

### Comments and collaboration (Bundle 4A, 2026-06-11)
- `add_comment` (async fn, pub) — Post a new Drive comment on the doc with opaque Google JSON anchor (doc-wide if None)
- `list_comments` (async fn, pub) — List Drive comments on the doc filtered by criteria
- `resolve_comment` (async fn, pub) — Mark a comment resolved by posting a reply with action:resolve

### Read methods
- `get` (async fn, pub) — Fetch a full document tree as DocumentSnapshot with tab content included
- `read_as_markdown` (async fn, pub) — Export the doc to text/markdown (v1 always returns full export regardless of tab_id)
- `read_outline` (async fn, pub) — Extract headings and first ~80 chars per paragraph from the doc
- `list_named_ranges` (async fn, pub) — List all namedRanges declared in the doc
- `list_tabs` (async fn, pub) — List all tabs including nested childTabs
- `list_revisions_since` (async fn, pub) — List Drive revisions after a given revision id; used by co-edit guard
- `get_at_revision` (async fn, pub) — Fetch a historical snapshot as plain text for co-edit diff computation

### Batch update methods
- `batch_update` (async fn, pub) — Apply a sequence of Request JSON objects atomically with optional optimistic concurrency via required_revision

### Tab management
- `add_tab` (async fn, pub) — Create a new tab with optional placement; returns DocsError::TabExists on duplicate if policy is fail

### Named range management
- `create_named_range` (async fn, pub) — Create a named range scoped to a paragraph or anchor (v1 supports Paragraph directly; others must be resolved by application layer)

### Drive image hosting
- `upload_image_to_drive` (async fn, pub) — Upload raw image bytes to Drive as a standalone file; returns file_id for insertInlineImage server-side hosting
- `set_anyone_reader` (async fn, pub) — Grant "anyone with the link" reader access to a Drive file
- `delete_drive_file` (async fn, pub) — Delete a Drive file by id; used to clean up temp images after Docs copies them

## File-level notes

- **Trait-level doc comment** explains the lifetime workaround: `Option<&'a Foo>` parameters use explicit `'a` lifetimes so that `mockall::automock` (gated by `#[cfg(test)]`) can generate working mock implementations.
- All methods return `Result<T, DocsError>` for consistent error handling.
- Methods logically grouped by section: Lifecycle, Reads, batchUpdate, Tabs, Named ranges, Drive image hosting.
- All public methods have clear doc comments with usage context.
- Bundles 2B (2026-06-11) and 4A (2026-06-11) are referenced in comments to indicate feature grouping.
- No dead code, unfinished stubs, or todos detected.
