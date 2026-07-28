# src/libs/colmena/src/web/domain/errors.rs

**Layer:** domain  
**Purpose:** Defines domain errors shared across the three web-toolkit ports (search, api_spec, browser) with classification logic for DAG-crashing vs. LLM-recoverable failures.

## Symbols

### Enum & Variants
- `WebDomainError` (enum, pub) — Union of 20 error variants covering config/init crashes and LLM-recoverable web toolkit faults; thiserror-derived with stable Display messages for tool results
  - `InvalidConfig(String)` (variant, pub) — Config validation failure; crashes DAG
  - `AdapterInit(String)` (variant, pub) — Adapter initialization failure; crashes DAG
  - `RateLimit { calls_used: u32, cap: u32 }` (variant, pub) — Rate limit exceeded; recoverable by LLM
  - `SessionLost { last_known_url: Option<String> }` (variant, pub) — Browser session lost; recoverable
  - `SelectorNotFound { selector: String, page_url: String, hints: Vec<String> }` (variant, pub) — CSS selector not found on page; recoverable with hints
  - `NavigationFailed(String)` (variant, pub) — Browser navigation failed; recoverable
  - `Timeout { ms: u64 }` (variant, pub) — Operation timeout in milliseconds; recoverable
  - `SpecParseFailed { details: String }` (variant, pub) — OpenAPI/Swagger spec parsing failed; recoverable
  - `UnsupportedSpecFormat { detected: String }` (variant, pub) — Spec format not supported (e.g., AsyncAPI); recoverable
  - `EndpointNotFound { searched_for: String, did_you_mean: Vec<String> }` (variant, pub) — Endpoint not found in loaded spec; recoverable with suggestions
  - `Upstream { status: u16, body: String }` (variant, pub) — Upstream HTTP error response; recoverable
  - `SessionCapReached { active: u32, cap: u32 }` (variant, pub) — Session count at capacity; recoverable
  - `UnexpectedHtmlResponse { url: String, resolved_url: String }` (variant, pub) — Got HTML response when expecting JSON from URL; recoverable
  - `SpecTooLarge { size_bytes: u64, limit_bytes: u64 }` (variant, pub) — Spec file exceeds size limit; crashes DAG
  - `Swagger2ConversionFailed { reason: String, unsupported_feature: Option<String> }` (variant, pub) — Swagger 2.0 to OpenAPI 3.0.3 conversion failed with optional feature details; recoverable
  - `MissingRequiredParams { missing: Vec<String>, hints: Option<String> }` (variant, pub) — Required HTTP parameters not provided; recoverable with hints
  - `InvalidParamType { param: String, expected_type: String, got: String }` (variant, pub) — HTTP parameter type mismatch; recoverable
  - `MissingAuth { scheme: String, message: String }` (variant, pub) — Authentication credentials missing for scheme; recoverable
  - `SpecNotLoaded { spec_url: String }` (variant, pub) — Spec URL not found in per-conversation cache; recoverable

### Methods
- `WebDomainError::is_llm_recoverable()` (method, pub) — Classifies error as LLM-recoverable (true) or DAG-crashing (false); returns false only for `InvalidConfig`, `AdapterInit`, and `SpecTooLarge`

### Tests
- `tests` (module, private) — 12 unit tests covering is_llm_recoverable classification for each error category and Display message formatting

## File-level notes

- Well-structured domain error type with clear semantic partitioning: 3 variants cause DAG crash, 17 variants surface to LLM as recoverable tool results
- All variants have stable `#[error(...)]` messages suitable for LLM consumption per design spec comment
- Classification logic in `is_llm_recoverable()` is exhaustive — all variants covered
- Test coverage is thorough: each non-trivial variant has a dedicated test validating its classification or message format
- Doc comments on variants are present where non-obvious (e.g., `Swagger2ConversionFailed`, `SpecNotLoaded`)
- No error-handling gaps, dead code, or unfinished stubs
