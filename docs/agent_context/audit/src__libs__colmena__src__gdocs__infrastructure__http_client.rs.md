# src/libs/colmena/src/gdocs/infrastructure/http_client.rs

**Layer:** infrastructure  **Purpose:** Google Docs REST HTTP adapter implementing the `DocsClient` trait; handles OAuth token caching, retry logic with exponential backoff, and parsing of snapshot/outline/table data from Google's API responses.

## Symbols

- `PROD_BASE_DOCS` (const) — Hardcoded production URL for Google Docs API v1
- `PROD_BASE_DRIVE` (const) — Hardcoded production URL for Google Drive API v3
- `PROD_BASE_DRIVE_UPLD` (const) — Hardcoded production URL for Drive upload endpoint
- `GoogleDocsHttpClient` (pub struct) — REST adapter holding reqwest client, shared token cache, and rebindable base URLs for testing
- `GoogleDocsHttpClient::from_config` (pub fn) — Builds production client from operator config; reads OAuth credentials from env with comprehensive missing-variable error reporting
- `GoogleDocsHttpClient::with_base_urls` (pub fn, test-only cfg) — Constructs client with user-supplied base URLs for wiremock test interception
- `GoogleDocsHttpClient::bearer` (async fn) — Retrieves current bearer token from token cache
- `GoogleDocsHttpClient::send_with_retry` (async fn) — Sends HTTP request with exponential backoff retry on 429 and 5xx; rebuilds request per attempt for token refresh
- `GoogleDocsHttpClient::map_status` (pub async fn) — Converts non-success HTTP response to typed DocsError; reads response body for context; handles 401, 403, 404, 429, and BAD_REQUEST variants
- `parse_comment` (fn) — Parses Drive comment JSON into domain CommentEntry, tolerant of missing optional fields
- `is_retryable` (fn) — Determines if status code qualifies for retry (429 or 5xx)
- `parse_snapshot` (pub fn) — Parses full document JSON response into DocumentSnapshot; handles both legacy single-tab and multi-tab structures; assigns global 1-based paragraph numbering across tabs
- `parse_paragraphs` (fn) — Walks body content array extracting paragraphs; increments counter for 1-based numbering; skips non-paragraph elements
- `parse_tables` (fn) — Extracts table structural elements from body content; maintains per-tab table indexing; builds cell grid with content text
- `parse_cell` (fn) — Parses table cell JSON into CellSnapshot; concatenates text from nested paragraph elements; captures row/col span and index ranges
- `paragraph_kind` (fn) — Determines ParagraphKind from namedStyleType and bullet presence; maps Docs styles (HEADING_1-6, TITLE, SUBTITLE) to enum
- `paragraph_text_and_range` (fn) — Extracts text content from paragraph elements, trims trailing newline, returns text + start/end indices
- `walk_tabs` (fn) — Recursively flattens Google's nested `tabs[].childTabs[]` hierarchy into flat Vec<TabMeta> in pre-order; assigns index reflecting flat position
- `map_index_range_to_paragraphs` (fn) — Maps UTF-16 segment-relative index range to 1-based paragraph range in first tab; fallback to first paragraph if not found
- `diff_markdown_for_lossy` (fn) — Compares input markdown vs Drive re-export; detects and flags lossy conversions (footnotes, images, math) with truncated original text
- `DocsClient::get` (async fn) — Fetches document snapshot with full content and tabs via documents.get API
- `DocsClient::create` (async fn) — Creates blank doc via Docs API, moves to parent folder via Drive API, fetches fresh snapshot for metadata
- `DocsClient::create_from_markdown` (async fn) — Creates doc from markdown via multipart upload to Drive; re-exports and diffs to detect lossy conversions
- `DocsClient::create_from_docx` (async fn) — Creates doc from .docx bytes via multipart upload to Drive
- `DocsClient::share` (async fn) — Shares document with user email and role via Drive permissions.create
- `DocsClient::list_permissions` (async fn) — Lists file permissions with id, type, role, email, displayName; returns complete PermissionList
- `DocsClient::delete_permission` (async fn) — Deletes specific permission by ID via Drive permissions.delete
- `DocsClient::list_documents` (async fn) — Lists documents with filtering by name/query, parent folder, modified_after; returns paginated results
- `DocsClient::export` (async fn) — Exports document in specified format (PDF, DOCX, etc.) and returns bytes
- `DocsClient::add_comment` (async fn) — Adds comment to document; optional anchor for position; returns parsed CommentEntry
- `DocsClient::list_comments` (async fn) — Lists comments with filtering (include_resolved flag); client-side re-filter to prevent accidental inclusion
- `DocsClient::resolve_comment` (async fn) — Resolves comment via Drive reply with action=resolve (Drive flips parent comment's resolved flag)
- `DocsClient::read_as_markdown` (async fn) — Exports document as markdown via Drive export API; tab_id param ignored in v1 (Drive joins all tabs)
- `DocsClient::read_outline` (async fn) — Returns outline (heading hierarchy) from snapshot; filters by tab_id if provided; includes paragraph number and preview
- `DocsClient::list_named_ranges` (async fn) — Lists named ranges mapped to paragraph numbers; fetches snapshot for index mapping; handles multi-object namedRanges structure
- `DocsClient::list_tabs` (async fn) — Lists document tabs with full hierarchy; walks raw JSON via walk_tabs helper; requests full doc with includeTabsContent=true
- `DocsClient::list_revisions_since` (async fn) — Lists file revisions after a given revision ID; skips everything up to and including the marker
- `DocsClient::get_at_revision` (async fn) — Retrieves document text at specific revision via revisions.get with media export
- `DocsClient::batch_update` (async fn) — Sends batch update request to batchUpdate endpoint; includes required_revision in writeControl if provided; recovers new revisionId from response or falls back to fresh GET
- `DocsClient::add_tab` (async fn) — Adds new tab with pre-check for title uniqueness; sends batch request then re-lists to find the new tab by title
- `DocsClient::create_named_range` (async fn) — Creates named range at paragraph scope; fetches snapshot to resolve paragraph index to character range
- `DocsClient::upload_image_to_drive` (async fn) — Uploads image bytes to Drive via multipart; returns file ID
- `DocsClient::set_anyone_reader` (async fn) — Sets file permission to anyone:reader via Drive permissions.create
- `DocsClient::delete_drive_file` (async fn) — Deletes file from Drive via DELETE request
- `tests` (mod, cfg(test)) — Unit and integration test suite with wiremock mocking; covers snapshot parsing, tab flattening, batch updates, revisions, comments, and permissions

## File-level notes

- **Outdated comment (lines 12–14)**: The file header states "Task 9 adds `get` + snapshot parsing; Tasks 10/11/12 fill in the rest of the trait via `todo!()` stubs that compile until then." This is now FALSE — ALL trait methods are fully implemented with real REST logic, not stubs. The comment should be removed or updated to reflect current completion status.
- Error handling is comprehensive: `map_status` covers 401, 403, 404, 429, and conflict detection; all JSON parsing uses `.unwrap_or_default()` or `.ok_or_else()` for safety.
- Retry logic is sound: exponential backoff (1s, 2s, 4s) on 429 and 5xx; configurable max_retries; requests are rebuilt per attempt so token refreshes apply.
- Parsing is defensive: `and_then()` chains tolerate missing optional fields; paragraph numbering is global across tabs; table cell text concatenates and trims newlines consistently.
- Test suite is comprehensive: 16 test cases covering snapshot parsing (single/multi-tab, bullets, headings), table parsing (merge spans, empty cells), batch updates, revisions, comments, sharing, exports, and tab management.
- No dead code or unfinished stubs detected; all trait methods are production-ready.
