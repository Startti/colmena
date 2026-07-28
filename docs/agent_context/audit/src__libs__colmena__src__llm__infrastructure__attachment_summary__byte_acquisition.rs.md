# src/libs/colmena/src/llm/infrastructure/attachment_summary/byte_acquisition.rs

**Layer:** infrastructure  
**Purpose:** Acquires attachment bytes from multiple sources (inline memory, filesystem paths, or signed URLs) for attachment summary generation. Handles stream-based byte fetching and error cases.

## Symbols

- `AcquireError` (enum, pub) — Error type for byte acquisition with Download, Read, and NoBytes variants
- `AcquireError::Download` (variant) — Download failure with error message
- `AcquireError::Read` (variant) — File read failure with error message
- `AcquireError::NoBytes` (variant) — Error when inline source lacks retained bytes
- `acquire_bytes` (fn, pub async) — Acquires attachment bytes from an AttachmentSource using provided inline data, file path reading, or signed URL streaming
- `tests` (mod) — Test module for byte acquisition
- `MockFetcher` (struct) — Mock SignedUrlFetcher implementation for testing with configurable body and chunk size
- `MockFetcher::new` (fn) — Creates a MockFetcher with specified body and default 8-byte chunk size
- `SignedUrlFetcher for MockFetcher` (impl) — Stream method that chunks mock body into a byte stream for testing
- `inline_returns_provided_bytes` (test) — Verifies inline bytes are returned unchanged when present
- `inline_without_bytes_errors` (test) — Verifies NoBytes error when inline source provided without bytes
- `signed_url_returns_fetched_bytes` (test) — Verifies signed URL fetching reconstructs full downloaded content from chunks

## File-level notes

- Module design rationale documented at top: v1 re-downloads (SignedUrl) or re-reads (Path) because inline sources are consumed during upload and not retained elsewhere
- Size cap enforcement deliberately omitted — frontend already enforces 100 MB limit; adding backend check would be redundant
- Stream-based chunked reading for SignedUrl is appropriate for potentially large files
- Test coverage includes inline with/without bytes and SignedUrl streaming; Path case (filesystem I/O) omitted but not required for unit tests
- All error cases properly wrapped with context messages
- No unfinished, dead, or improvement-worthy code identified
