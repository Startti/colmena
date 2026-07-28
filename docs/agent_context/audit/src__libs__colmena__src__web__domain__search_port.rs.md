# src/libs/colmena/src/web/domain/search_port.rs

**Layer:** domain  
**Purpose:** Defines the search/fetch port trait and value objects (request/response types, enums) for abstracting search/extract providers (Tavily, future SearxNG/Exa/Serper/Brave adapters). Includes serde serialization and comprehensive unit tests.

## Symbols

- `SearchRequest` (struct, pub) — Input value object for search queries with query string, result count, content inclusion flag, depth, domain filters, and optional time range
- `SearchRequest::new()` (fn, pub) — Constructor taking query string, sets safe defaults (max_results=5, include_content=false, SearchDepth::Basic, no time range)
- `SearchDepth` (enum, pub) — Search depth level: Basic or Advanced; serializes/deserializes lowercase
- `SearchDepth::as_str()` (fn, pub) — Returns lowercase string representation ("basic" or "advanced")
- `TimeRange` (enum, pub) — Time filter: Day, Week, Month, Year; serializes/deserializes lowercase
- `TimeRange::as_str()` (fn, pub) — Returns lowercase string representation ("day", "week", "month", "year")
- `SearchResponse` (struct, pub) — Output value object containing query, results vector, optional answer string, and credits used counter
- `SearchResult` (struct, pub) — Individual search result with title, URL, snippet, score, and optional full extracted content (skipped when None in serialization)
- `FetchRequest` (struct, pub) — Input for fetch/extract operations: URL and extraction format
- `ExtractFormat` (enum, pub) — Extraction output format: Markdown or Text; serializes/deserializes lowercase
- `ExtractFormat::as_str()` (fn, pub) — Returns lowercase string representation ("markdown" or "text")
- `FetchResponse` (struct, pub) — Output of fetch/extract containing URL, optional title, extracted content string, content length, and credits used
- `SearchPort` (trait, pub) — Async port abstraction with `search()` and `fetch()` methods; implemented by provider adapters (Tavily, future SearxNG/Exa/Serper/Brave)
- `SearchPort::search()` (fn, pub async) — Search method taking SearchRequest, returning Result<SearchResponse>
- `SearchPort::fetch()` (fn, pub async) — Fetch/extract method taking FetchRequest, returning Result<FetchResponse>

## Tests (module-level)

- `search_request_new_sets_safe_defaults` — Verifies SearchRequest::new() applies correct default values
- `search_depth_serializes_lowercase` — Validates SearchDepth serializes to lowercase JSON
- `time_range_serializes_lowercase` — Validates TimeRange variants serialize to correct lowercase strings
- `extract_format_serializes_lowercase` — Validates ExtractFormat serializes to lowercase JSON
- `search_result_round_trips_json` — Round-trip serialization test for SearchResult with optional content field
- `search_result_content_is_skipped_when_none` — Verifies content field is omitted from JSON when None

## File-level notes

- **Pure domain layer**: No infrastructure dependencies, no use cases; defines contracts and value objects only
- **Hexagonal architecture adherence**: SearchPort trait is the port; implementations will live in infrastructure layer
- **Serde hygiene**: Uses `skip_serializing_if` on optional content field; enums use `rename_all = "lowercase"` for consistent API serialization
- **Well-tested**: 6 unit tests covering defaults, serialization round-tripping, and edge cases (None content omission)
- **Future-ready**: Module docs mention support for multiple adapter providers; trait abstraction allows clean addition of new adapters without domain changes
- **No flags**: Code is clean, complete, and follows established patterns; all symbols have clear purpose and usage; no dead code, TODOs, or missing error handling
