# src/libs/colmena/src/text/mod.rs

**Layer:** infrastructure  
**Purpose:** Static registry loader for LLM-facing tool descriptions. Embeds YAML files at compile time and provides lookups for tool summaries/descriptions with startup validation.

## Symbols

- `ToolText` (struct, pub) — Value object holding `summary` and `description` fields deserialized from YAML
- `GSHEETS_YAML` (const, private) — Embedded YAML string for gsheets tool descriptions
- `CRDT_DOC_YAML` (const, private) — Embedded YAML string for crdt_doc tool descriptions
- `DOCUMENTS_YAML` (const, private) — Embedded YAML string for documents tool descriptions
- `HELPERS_YAML` (const, private) — Embedded YAML string for helpers tool descriptions
- `GDOCS_YAML` (const, private) — Embedded YAML string for gdocs tool descriptions
- `SQL_YAML` (const, private) — Embedded YAML string for sql tool descriptions
- `DATA_RUN_PYTHON_YAML` (const, private) — Embedded YAML string for data_run_python tool descriptions
- `TOOL_TEXTS` (static, private) — OnceLock-wrapped HashMap holding parsed tool registry, initialized on first access
- `load()` (fn, private) — Initializes registry from all embedded YAML files; panics on malformed YAML or duplicate tool keys
- `tool_summary()` (fn, pub) — Looks up summary string for a tool name; panics if missing
- `tool_description()` (fn, pub) — Looks up description string for a tool name; panics if missing
- `all_tool_names()` (fn, pub) — Returns vector of all registered tool names; used by tests for orphan detection
- `tests::yaml_files_parse_at_startup()` (test, private) — Verifies all embedded YAML files parse without error
- `tests::empty_registry_is_acceptable_initially()` (test, private) — Verifies empty/placeholder YAML entries are accepted during development
- `tests::duplicate_yaml_keys_would_panic_in_load()` (test, private) — Verifies duplicate-key detection logic via synthetic YAML parsing

## File-level notes

- **Panic-based error handling is intentional** — startup validation catches misconfigured tools before runtime, not during tool calls.
- **OnceLock guarantees thread-safe lazy initialization** — registry is parsed once and cached across all lookups.
- **7 YAML files embedded** — one per tool family (gsheets, crdt_doc, documents, helpers, gdocs, sql, data_run_python); additions require new const and a load() array entry.
- **Duplicate key check** — line 49-51 panics if any tool name appears in multiple YAML files, preventing silent shadowing.
- **Test sanity check at line 104-108** — upper bound of 100 tool entries is arbitrary but reasonable; catches proliferation without breaking development.
- **No intra-crate imports** — this module depends only on `serde`, `std`, and is used by 10 modules (registry lookups across dag_engine, llm, and other subsystems).
