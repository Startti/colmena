# src/libs/colmena/src/llm/domain/file_provider_repository.rs

**Layer:** domain  **Purpose:** Defines the port trait for uploading files to LLM provider APIs (Files API). Implements streaming upload abstraction consumed by `LlmCallUseCase` for materializing files from signed URLs.

## Symbols

- `BoxedByteStream` (type alias, pub) — Pinned boxed stream of bytes with error handling for streaming file uploads
- `FileProviderRepository` (trait, pub) — Port trait defining the interface for file upload operations to LLM providers
- `FileProviderRepository::upload_streaming` (async method, pub) — Uploads a file to the provider's Files API by consuming a byte stream; returns a `ProviderFileRef` with ID and metadata
- `FileProviderRepository::ttl` (method, pub) — Returns the TTL (time-to-live) duration for uploaded files on this provider, or `None` if they never expire
- `FileProviderRepository::provider` (method, pub) — Returns the `ProviderKind` identifier for provider-specific cache keying
- `MockProvider` (struct, private) — Mock implementation of `FileProviderRepository` used in tests
- `MockProvider` (impl block) — Implements all three `FileProviderRepository` methods, returning fixed mock values for testing
- `_dyn_safe` (const fn pointer, private in test) — Compile-time object-safety verification; ensures `FileProviderRepository` can be used as `dyn FileProviderRepository`
- `mock_provider_returns_ref` (test fn) — Tests that `MockProvider` correctly uploads a stream and returns a `ProviderFileRef` with expected fields

## File-level notes

- Minimal, focused trait definition with no extraneous code
- Well-commented in Spanish (doc comments) and English (inline notes)
- Test suite includes both a functional mock test and an object-safety compile-time check
- No infrastructure dependencies; pure domain abstraction
- Stream type uses `futures::Stream` and `bytes::Bytes`, standard streaming abstractions for Rust
