# src/libs/colmena/src/gdocs/application/_test_helpers.rs

**Layer:** application  
**Purpose:** Provides shared test fixtures and builder helpers for application-layer gdocs tests. Wires MockDocsClient, OutlineCache, and InMemoryRevisionStore; offers convenience builders (`snap`, `snap_multi_tab`) to construct DocumentSnapshot values without repetitive struct boilerplate.

## Symbols

- `TestRig` (struct, pub) — Container holding MockDocsClient, OutlineCache, and InMemoryRevisionStore for wiring application tests.
- `TestRig::default()` (fn, impl Default) — Initializes TestRig with MockDocsClient, OutlineCache (5-second TTL), and InMemoryRevisionStore.
- `TestRig::new()` (fn, impl) — Public constructor delegating to Self::default().
- `snap(rev: &str, paras: Vec<(u32, ParagraphKind, &str, u32, u32)>) -> DocumentSnapshot` (fn, pub) — Builds a single-tab DocumentSnapshot from revision ID and paragraph tuples, hardcoding doc_id="doc1" and title="T".
- `snap_multi_tab(rev: &str, tabs: Vec<(Option<&str>, Vec<(u32, ParagraphKind, &str, u32, u32)>)>) -> DocumentSnapshot` (fn, pub) — Builds a multi-tab DocumentSnapshot from revision ID and (tab_id, paragraphs) tuples, hardcoding doc_id="doc1" and title="T".  [FLAG: dead_candidate — marked with #[allow(dead_code)]; may be unused in some test configurations]
- `doc_id() -> DocumentId` (fn, pub) — Returns hardcoded DocumentId "doc1" for test scenarios.

## File-level notes

- File is test-only (`#![cfg(test)]`); all symbols are test fixtures and should not be referenced from production code.
- `snap_multi_tab` has explicit `#[allow(clippy::type_complexity, dead_code)]`, indicating it may be unused in the current test suite but is retained for flexibility or future use.
- Hardcoded test values ("doc1", "T") keep fixtures simple and predictable, appropriate for unit/integration test contexts.
- No error handling needed; test fixtures are intended to panic on invalid inputs rather than gracefully degrade.
