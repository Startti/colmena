# src/libs/colmena/src/llm/domain/file_cache_repository.rs

**Layer:** domain  
**Purpose:** Port for persisting and retrieving cached file references uploaded to LLM provider File APIs. Defines a value object for cache entries and a repository trait for lookup/upsert/invalidate operations.

## Symbols

- `CachedFileEntry` (struct, pub) — Value object holding provider file metadata: document_id, provider, file ID, MIME type, filename, size, upload/expiry/last-used timestamps
- `CachedFileEntry::is_likely_alive` (fn, pub) — Heuristic check: returns true if file is likely still accessible (no expiry or expiry is >5 minutes in future), to avoid unnecessary provider calls
- `CachedFileEntry::into_ref` (fn, pub) — Converts self into a ProviderFileRef by extracting provider, file ID, MIME type, filename, and expires_at
- `FileCacheRepository` (trait, pub) — Port for file cache operations; async, Send + Sync
- `FileCacheRepository::lookup` (async fn, trait method) — Look up a cached file entry by document_id and provider; returns Option
- `FileCacheRepository::upsert` (async fn, trait method) — Insert or replace a cache entry
- `FileCacheRepository::invalidate` (async fn, trait method) — Remove a cache entry by document_id and provider

## Test Items

- `_` (const fn, dyn-safety guard) — Verifies FileCacheRepository can be used as Arc<dyn FileCacheRepository>
- `entry_with_expiry` (fn) — Test helper creating a CachedFileEntry with parameterized expiry
- `alive_when_expires_at_is_none` (test) — Verifies is_likely_alive returns true when expires_at is None
- `alive_when_expires_at_in_future_beyond_margin` (test) — Verifies is_likely_alive returns true for future expiry >5 minutes out
- `expired_when_within_5min_margin` (test) — Verifies is_likely_alive returns false when expiry is within 5-minute safety margin
- `expired_when_in_past` (test) — Verifies is_likely_alive returns false for past expiry
- `into_ref_preserves_fields` (test) — Verifies into_ref conversion preserves key fields
- `InMemoryCache` (struct, test) — Mock FileCacheRepository implementation using a Vec behind a Mutex
- `InMemoryCache::lookup` (impl) — Finds entry by document_id and provider
- `InMemoryCache::upsert` (impl) — Removes old matching entry, pushes new one
- `InMemoryCache::invalidate` (impl) — Removes entry by document_id and provider
- `in_memory_cache_lookup_upsert_invalidate_round_trip` (tokio::test) — End-to-end test: upsert, lookup, verify, invalidate, verify gone

## File-level Notes

- Clean domain port with zero infrastructure dependencies
- Derives Clone on CachedFileEntry to support test cloning; necessary and appropriate
- 5-minute safety margin in is_likely_alive is intentional anti-pattern guard (documented inline)
- Test coverage is thorough: expiry boundary cases, conversion, and round-trip mock integration
- No dead code, todos, or unfinished patterns
- Comment on line 1–2 is in Spanish; rest of file follows English convention for code
