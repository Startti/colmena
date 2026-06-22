# 40. Toolkit packages

> Status: shipped 2026-06-06 (E-T16). See [spec](../superpowers/specs/2026-06-06-toolkit-packages-design.md) and [plan](../superpowers/plans/2026-06-06-toolkit-packages-and-summaries.md).

## Concept

A toolkit package is a curated bundle of synthetic tools exposed under a single
alias. Instead of writing `enabled_tools: ["gsheets_list_sheets", "gsheets_read",
...]` for every Google Sheets tool, you write `enabled_tools: ["gsheets"]` and
the engine expands the alias to its 10 tools at runtime.

## Syntax

### Basic activation: single package

```json
"enabled_tools": ["gsheets"]
```

Expands to all 11 gsheets tools (`gsheets_create_spreadsheet`, `gsheets_read`, `gsheets_format_range`, etc.).

### Package plus individual tools

```json
"enabled_tools": ["gsheets", "current_time"]
```

Expands to all gsheets tools + `current_time` (a built-in individual tool).

### Tool exclusion

```json
"enabled_tools": ["gsheets", "!gsheets_delete_sheet", "!gsheets_add_sheet"]
```

Expands to all gsheets tools except the two deleted.

### Wildcard with exclusion

```json
"enabled_tools": ["*", "!gsheets_create_from_xlsx"]
```

Expands to every registered tool except `gsheets_create_from_xlsx`.

### Back-compat: `api_explorer` prefix-rule

```json
"enabled_tools": ["api_explorer__load_spec", "api_explorer__search_endpoint"]
```

The `__` prefix-rule for `api_explorer` (and similar multi-subtool packages)
still works for backward-compatibility. New toolkits should prefer the explicit
`TOOLKIT_PACKAGES` registry — it's more explicit and supports exclusion.

### Individual tool unchanged

```json
"enabled_tools": ["tavily_web", "sql_query_prod"]
```

Both still work: any entry not in the package registry is treated as a direct
tool name.

## Naming convention

Package aliases MUST NOT contain `_`. Tool names MUST contain `_` after the
package namespace (e.g. `gsheets_read`, `gsheets_create_spreadsheet`). This is
the visual disambiguation rule and is enforced by the
`package_aliases_have_no_underscore` test in
`llm_synthetic_tools/toolkit_packages.rs`.

**Why:** At a glance, `enabled_tools: ["gsheets", "tavily_web"]` shows which
are packages (no `_`) and which are individual tools (contains `_`), with no
ambiguity.

## Exclusion semantics

Any entry in `enabled_tools` that starts with `!` is an exclusion. Exclusions
are applied AFTER inclusions resolve (set difference). Order is irrelevant.
Excluding a package alias removes every tool in that package; excluding a
single tool name removes only that tool.

**Example:**

```json
"enabled_tools": ["gsheets", "crdt_doc", "!gsheets_delete_sheet", "!crdt_doc_add_sheet"]
```

1. Inclusions: `gsheets` (10 tools) + `crdt_doc` (6 tools) = 16 tools.
2. Exclusions: remove `gsheets_delete_sheet` and `crdt_doc_add_sheet`.
3. Result: 14 tools.

## Edge cases

| Case | Behavior |
|---|---|
| Unknown alias (typo) | Silent — returns 0 tools for that entry |
| Exclude tool not in includes | No-op |
| Exclude alone (no inclusions) | Empty result, no panic |
| `!` alone (empty exclusion) | Logged warning, ignored |
| Tool name collides with package alias | Exact tool match wins |

## Registering a new package

Append a `ToolkitPackage` struct literal to `TOOLKIT_PACKAGES` in
`src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs`:

```rust
ToolkitPackage {
    alias: "your_alias",  // must not contain '_'
    description: "Short description shown in catalog",
    tools: &["your_tool_1", "your_tool_2"],
},
```

The engine will automatically:
- Validate that `alias` contains no `_` (CI test enforces this).
- Expand `alias` in any `enabled_tools` list.
- Apply exclusions after expansion.
- Emit the tools in the `lazy_tool_loading` catalog.

## Comparison with `api_explorer`'s `__` prefix-rule

`api_explorer` (and similar toolkits with sub-tools named `alias__subtool`)
still work via the existing prefix-rule for back-compat. New toolkits should
prefer the explicit `TOOLKIT_PACKAGES` registry — it's more explicit,
supports exclusion, and the naming-convention rule prevents accidental
matches.
