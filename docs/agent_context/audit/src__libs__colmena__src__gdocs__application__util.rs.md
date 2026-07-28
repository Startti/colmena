# src/libs/colmena/src/gdocs/application/util.rs

**Layer:** application  **Purpose:** Provides JSON builder utilities for constructing Google Docs API request bodies, specifically `Location` and `Range` objects with conditional `tabId` inclusion to prevent silent cross-tab writes in multi-tab documents.

## Symbols

- `location` (pub fn) — Builds a JSON object `{ index, tabId? }` for Docs API location references, conditionally including tabId only when set
- `range` (pub fn) — Builds a JSON object `{ startIndex, endIndex, tabId? }` for Docs API range references, conditionally including tabId only when set
- `tests` (private mod) — Test module for the util functions
- `location_omits_tab_id_when_none` (test fn) — Verifies location builder omits tabId when passed None
- `location_includes_tab_id_when_set` (test fn) — Verifies location builder includes tabId when passed Some(TabId)
- `range_omits_tab_id_when_none` (test fn) — Verifies range builder omits tabId when passed None
- `range_includes_tab_id_when_set` (test fn) — Verifies range builder includes tabId when passed Some(TabId)

## File-level notes

- Module documentation is excellent: clearly explains the critical issue (Google Docs API interprets indices against the FIRST tab when tabId is absent, causing silent cross-tab writes) and mandates use of these helpers at all emission sites.
- All public functions are deterministic; no failure paths exist (serde_json operations are infallible).
- Test coverage is comprehensive: all branches tested (Some/None cases for both functions).
- Minor code duplication: the conditional `if let Some(t) = tab_id { m.insert("tabId"...) }` pattern repeats across both functions, but the functions are short enough that extracting to a helper would not significantly improve clarity.
- No unsafe code, panics, or error-prone patterns.
