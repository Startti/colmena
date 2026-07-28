# src/libs/colmena/src/gdocs/application/scope_resolver.rs

**Layer:** application  **Purpose:** Translates LLM-facing `Scope` directives (All, Tab, Paragraph, UnderHeading, BetweenHeadings) into resolved paragraph ranges against a document snapshot for content-addressed edit operations.

## Symbols

- `ResolvedScope` (struct, pub) — Data structure holding resolved scope: optional tab_id and 1-based inclusive paragraph range
- `ResolvedScope::contains_paragraph` (fn, pub) — Checks whether a (tab, paragraph_number) pair falls within this scope; handles doc-wide (tab_id=None) and tab-specific cases
- `resolve` (fn, pub) — Main entry point; translates a Scope enum against a DocumentSnapshot into a ResolvedScope by matching on scope kind (All, Tab, Paragraph, UnderHeading, BetweenHeadings) [FLAG: improvement — performs multiple min/max passes over paragraph collections (lines 41–59, 67–73, 119–125) where single-pass fold would be more efficient]
- `matches_heading` (fn, private) — Checks if a paragraph matches a heading target by stripping leading `#` and comparing trimmed text
- `paragraph_heading_level` (fn, private) — Returns heading level 1–6 for a ParagraphKind, or None for non-headings; excludes Title/Subtitle from nesting
- `heading_level` (fn, private) — Parses leading `#` count from a string and clamps to 1–6
- `find_heading_paragraph` (fn, private) — Searches a snapshot's paragraphs for one matching a heading spec; returns its paragraph number
- `tests` (mod, private) — 10 test cases covering All, Paragraph, Tab, UnderHeading (single and multi-section), BetweenHeadings (with/without before), error cases, heading level parsing, and scope containment logic

## File-level notes

- All scope variants are fully tested with expected semantics: All → doc range; Paragraph → single; Tab → tab range; UnderHeading → from heading to next same-level or EOF; BetweenHeadings → exclusive boundaries
- Comment at line 131–132 notes that empty scopes (start == last paragraph) can occur but are returned harmlessly; callers validate
- No async, no I/O, no external state — pure translation logic
