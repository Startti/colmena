# src/libs/colmena/src/gdocs/application/mod.rs

**Layer:** application  **Purpose:** Module index for the Google Docs application layer, re-exporting all submodules implementing document editing use cases (apply_edits, diff, co_edit_guard, insert, delete_text, replace_text, styling, tables, scope resolution, named ranges).

## Symbols

- `_test_helpers` (module, public, cfg test) — test utilities for Google Docs application layer
- `apply_edits` (module, public) — applies document edits to Google Docs
- `co_edit_guard` (module, public) — prevents concurrent editing conflicts via paragraph-level diff and human change detection
- `delete_text` (module, public) — deletes text in documents
- `diff` (module, public) — calculates diffs for document changes
- `insert` (module, public) — inserts content into documents
- `named_range` (module, public) — manages named ranges in Google Docs
- `replace_section` (module, public) — replaces sections in documents
- `replace_text` (module, public) — replaces text in documents
- `scope_resolver` (module, public) — resolves document scopes for edit targeting
- `style` (module, public) — handles text and paragraph styling operations
- `table` (module, public) — handles table insertion and manipulation
- `table_format` (module, public) — formats tables (cell alignment, borders, shading)
- `util` (module, public) — utility functions shared across application submodules

## File-level notes

- Pure module index (no implementations or logic)
- All modules follow consistent naming convention (verb/noun pattern)
- Logical grouping: editing operations (insert, delete, replace, apply), structural (table, table_format, named_range, replace_section), safety (co_edit_guard, scope_resolver), formatting (style, diff, util)
- Standard Rust convention for application layer organization
