# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs

**Layer:** infrastructure  
**Purpose:** Static registry of curated tool bundles (gsheets, gdocs, gdocsread) exposed via package aliases in `enabled_tools`. Provides linear-scan lookup and validation that aliases and tool names follow the naming convention (packages have no `_`, individual tools have `_` after namespace).

## Symbols

- `ToolkitPackage` (struct, public) — Data structure holding a package's static alias, human description, and ordered list of tool names.
- `TOOLKIT_PACKAGES` (static, public) — Curated registry of three toolkit packages: gsheets (12 tools), gdocs (36 tools), gdocsread (10 read-only tools).
- `find_package` (fn, public) — Linear-scan lookup by alias string; returns `Option<&'static ToolkitPackage>`.
- `tests` (mod, cfg(test)) — Test module validating naming conventions and package completeness.
- `package_aliases_have_no_underscore` (test fn, private) — Validates that all package aliases contain no `_` (reserved for tool names).
- `gsheets_package_has_all_twelve_tools` (test fn, private) — Validates gsheets package contains exactly 12 tools including deprecated `gsheets_run_python` and unified `data_run_python`.
- `find_package_returns_some_for_known_alias` (test fn, private) — Validates `find_package("gsheets")` returns Some.
- `find_package_returns_none_for_unknown` (test fn, private) — Validates `find_package` returns None for non-existent aliases and tool names.
- `gdocs_package_has_all_tools` (test fn, private) — Validates gdocs package contains exactly 36 tools across all subsystems (v1, v1.1, table-edit bundle).
- `gdocsread_readonly_package_subset` (test fn, private) — Validates gdocsread is a read-only subset of gdocs (10 tools, no write/mutation keywords, all present in full gdocs package).

## File-level notes

- **Naming convention:** Module documentation (lines 1-8) enforces the package-vs-tool disambiguation rule via underscore boundaries, tested in `package_aliases_have_no_underscore`.
- **Soft-deprecation bridge:** gsheets package includes both deprecated `gsheets_run_python` and new unified `data_run_python` during transition (lines 37-42); comment notes Phase 2 hard-delete is pending.
- **Subsystem layering:** gdocs and gdocsread packages document their subsystem origins (v1, v1.1, Bundle 2A/2B/4A, etc.) via inline comments (lines 72-88, 106-110), easing future maintenance.
- **Defensive hardcoding:** Test assertions hardcode tool counts (12 for gsheets, 36 for gdocs, 10 for gdocsread) as intentional guards forcing explicit test updates when registry changes; this pattern prevents silent tool additions/removals.
- **Ordering guarantee:** Module doc (lines 15-17) states "Order is preserved in the expansion," but no consumer code in-file exploits this; the guarantee exists but appears unused within this module's scope.
- **No error handling:** `find_package` returns `Option` (caller handles None), and all test assertions use `expect()` — appropriate for initialization-time failures.
