# src/libs/colmena/src/llm/domain/signed_url_fetcher.rs

**Layer:** domain  **Purpose:** Defines `SignedUrlFetcher` trait, a port for fetching signed URL bodies as byte streams. LlmCallUseCase depends only on this abstract interface; concrete implementations (HTTP via reqwest, test stubs) live in infrastructure.

## Symbols

- `SignedUrlFetcher` (trait, pub) — async trait that provides an abstract interface for downloading signed URLs as byte streams, respecting query-parameter credentials and rejecting authorization headers
- `stream` (async fn, pub) — downloads the body of a signed URL as a `BoxedByteStream`, returning `LlmError::NetworkError` on transport failures or `LlmError::SignedUrlFetchFailed` on non-2xx status

## File-level notes

- Clean port definition following hexagonal architecture discipline: zero infrastructure dependencies, abstract trait in domain, concrete adapters deferred to infrastructure layer.
- Documentation clearly explains signing semantics (credentials in query params, no auth headers) and error handling expectations.
- No dead code, unfinished work, or obvious improvements identified.
