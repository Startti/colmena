# Toolkit packages + tool summaries — design

**Status**: Approved (2026-06-06)
**Author**: Daniel García + colmena agent
**Tracks**: E-T15 (summaries) + E-T16 (toolkit packages)
**Related**: `developer_guide/29_lazy_tool_loading.md`, `developer_guide/39_gsheets.md`, the existing `filter_enabled_tools` machinery in `llm.rs`

---

## 1. Goals

Two related ergonomic improvements to the agent-tool integration:

1. **Toolkit packages** — replace verbose tool lists in `enabled_tools` with a single alias that expands to a curated set of tools. Generalize beyond today's `api_explorer__*` prefix-rule so any future toolkit (gsheets, slack, browser, github, …) follows one convention.
2. **Tool summaries for lazy loading** — every gsheets synthetic tool gains a one-line `summary` so that when an agent uses `lazy_tool_loading: true`, the LLM can choose tools competently before paying the token cost of full schemas.

Both are zero-break-change additions.

## 2. Non-goals

- Replacing or deprecating `enabled_tools: "*"` (wildcard) — stays.
- Replacing the existing `api_explorer` prefix-rule (`alias__subtool` double-underscore) — stays for back-compat.
- Versioned packages or remote package registries — out of scope; YAGNI.
- Auto-injection of a system-message paragraph describing the enabled package — future enhancement, not in this spec.
- Per-instance package configuration (auth, defaults, etc.) — when a toolkit needs per-instance config, the user falls back to listing tools + `tool_configurations`. Same model as `tavily_client` today.

## 3. Open-source / ADP rule

Colmena is an open-source library. The toolkit_packages registry MUST NOT contain any ADP-specific package names, auth assumptions, or business logic. ADP, as a downstream consumer, can either use the built-in packages or (future) extend the registry via its own configuration path.

## 4. Naming convention (the load-bearing decision)

**Rule (enforced in CI):** package aliases MUST NOT contain `_`. Individual tool names MUST contain `_` after the package namespace (e.g. `gsheets_read`).

This means readers of a graph JSON can disambiguate at a glance:

| Entry | Interpretation |
|---|---|
| `gsheets` | package alias (no `_`) → expands to all gsheets tools |
| `gsheets_read` | individual tool (`_` after namespace) → just that one |
| `api_explorer` | package alias (no `_`) → expands via existing `__` prefix-rule |
| `tavily_web` | individual tool (legacy naming, has `_` but no package called `tavily`) |

Enforcement: a unit test in `toolkit_packages.rs` asserts every `pkg.alias` is underscore-free. CI fails on violation.

## 5. Components

### 5.1 New module: `toolkit_packages.rs`

Path: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs`

```rust
/// A curated bundle of tools exposed under a single alias.
pub struct ToolkitPackage {
    /// Alias used in `enabled_tools`. Must not contain `_`.
    pub alias: &'static str,
    /// One-line human description shown in docs / future introspection tools.
    pub description: &'static str,
    /// Exact names of every tool this package activates. Order is preserved
    /// in the expansion.
    pub tools: &'static [&'static str],
}

pub static TOOLKIT_PACKAGES: &[ToolkitPackage] = &[
    ToolkitPackage {
        alias: "gsheets",
        description: "Read, write, and analyze Google Sheets workbooks (10 tools)",
        tools: &[
            "gsheets_create_spreadsheet",
            "gsheets_create_from_xlsx",
            "gsheets_export_xlsx",
            "gsheets_list_sheets",
            "gsheets_add_sheet",
            "gsheets_delete_sheet",
            "gsheets_read",
            "gsheets_set_cell",
            "gsheets_set_range",
            "gsheets_run_python",
        ],
    },
    // Future toolkits append here, one struct each.
];

/// Linear-scan lookup. The registry is small (≪ 50 entries) so a HashMap
/// would be over-engineering.
pub fn find_package(alias: &str) -> Option<&'static ToolkitPackage> {
    TOOLKIT_PACKAGES.iter().find(|p| p.alias == alias)
}
```

The module exposes only `ToolkitPackage`, `TOOLKIT_PACKAGES`, and `find_package`. Mod-level tests live alongside.

### 5.2 Extended `filter_enabled_tools`

Same function in `llm.rs:51`, extended with two new behaviors:

1. **Inclusion expansion**: any entry in `enabled_tools` that matches a package alias is replaced by the package's `tools` list.
2. **Exclusion**: any entry starting with `!` is collected separately and removed from the final include set after all inclusions resolve.

The existing `__` prefix-rule and exact-name match continue to work as before. The `configured_aliases` set (from `tool_configurations`) still auto-enables matching tools — unchanged.

### 5.3 Tool summaries

Every gsheets tool builder (`tool_create_spreadsheet`, `tool_read`, etc. in `gsheets_tools.rs`, plus `tool_gsheets_run_python` in `gsheets_run_python.rs`) gains a one-line `summary` passed alongside the long description.

**Precondition check during implementation**: confirm `ToolDefinition` already supports a `summary` field. If not, add one (`Option<String>`, default None) and wire it through to the lazy-loading catalog. The lazy-loading reader (`llm_synthetic_tools/lazy_tools_catalog.rs` per CLAUDE.md) already expects this metadata.

Summaries follow the canonical 10-20 word format used by other lazy-aware tools:

| Tool | Summary |
|---|---|
| `gsheets_create_spreadsheet` | Create a new Google Sheets workbook and return its URL |
| `gsheets_create_from_xlsx` | Upload a local .xlsx attachment and convert it into a new Google Sheet |
| `gsheets_export_xlsx` | Download an existing Google Sheet as .xlsx bytes attachment |
| `gsheets_list_sheets` | List every tab (sheet) inside a spreadsheet by ID |
| `gsheets_add_sheet` | Create a new tab inside an existing spreadsheet |
| `gsheets_delete_sheet` | Permanently delete a tab from a spreadsheet |
| `gsheets_read` | Read a cell range from a tab; supports formatted, unformatted, and formula render modes |
| `gsheets_set_cell` | Write one value or formula into a single cell |
| `gsheets_set_range` | Write a 2-D values array starting at a given address |
| `gsheets_run_python` | Run sandboxed pandas analysis over sheet ranges loaded directly by the dispatcher (rows never pass through the LLM) |

## 6. Syntax for `enabled_tools`

```jsonc
// Package activation
"enabled_tools": ["gsheets"]                            // → 10 tools

// Mix of package and individual tool
"enabled_tools": ["gsheets", "tavily_web"]              // → 11 tools

// Exclusion — read-only-style agent
"enabled_tools": ["gsheets", "!gsheets_delete_sheet"]   // → 9 tools

// Exclude an entire package from wildcard
"enabled_tools": ["*", "!gsheets_delete_sheet"]         // → all minus that one

// Exclude an entire package by alias from wildcard
"enabled_tools": ["*", "!gsheets"]                      // → all minus the 10 gsheets tools

// Individual tool still works (zero-break)
"enabled_tools": ["gsheets_read"]                       // → 1 tool

// api_explorer prefix-rule untouched
"enabled_tools": ["api_explorer"]                       // → all api_explorer__* sub-tools

// Wildcard untouched
"enabled_tools": "*"                                    // → every tool
```

## 7. Resolution algorithm

```text
input:
  all_tools:               Vec<ToolDefinition>
  enabled_tools_config:    Option<&Value>             // user's enabled_tools entry
  configured_aliases:      HashSet<String>            // from tool_configurations

step 1 — parse:
  raw_includes  = []
  raw_excludes  = []
  wildcard      = false

  for each entry in enabled_tools_config (string or array):
    if entry == "*":
      wildcard = true
      continue
    if entry.starts_with('!'):
      name = entry[1:]
      if name.is_empty():
        log warning "empty exclusion entry ignored"
        continue
      raw_excludes.push(name)
    else:
      raw_includes.push(entry)

  raw_includes.extend(configured_aliases)   // tool_configurations entries auto-enable

step 2 — expand:
  fn expand(name) =
    if let Some(pkg) = find_package(name):
      return pkg.tools.iter().cloned()
    else:
      return [name]

  final_includes = set()
  for name in raw_includes:
    final_includes.extend(expand(name))

  final_excludes = set()
  for name in raw_excludes:
    final_excludes.extend(expand(name))

step 3 — filter:
  if wildcard:
    enabled_set = all_tools.map(|t| t.name)
  else:
    enabled_set = final_includes

  enabled_set = enabled_set - final_excludes

  // back-compat: also include any tool whose name matches the existing
  // `{alias}__` prefix-rule for any alias in raw_includes (covers
  // api_explorer-style toolkits already shipped).
  for alias in raw_includes:
    for tool in all_tools:
      if tool.name.starts_with(&format!("{}__", alias)):
        enabled_set.insert(tool.name.clone())

  return all_tools.into_iter().filter(|t| enabled_set.contains(&t.name)).collect()
```

## 8. Edge-case decisions

| Case | Decision |
|---|---|
| Unknown alias (typo `gsheetz`) | Silent, returns 0 tools for that entry (back-compat with current behavior). Future: surface as warning. |
| Exclude tool not in includes | No-op, no error. |
| Exclude alone (no inclusions) | Result is empty set. Likely user error, but no special-case panic. |
| `!` alone (empty exclusion name) | Logged warning, entry ignored. |
| Tool name collides with package alias | **Exact tool match wins.** A tool literally named `gsheets` (single, not a package member) takes precedence over the package expansion when present in `all_tools`. The package members still expand from their own names. |
| Order of entries in `enabled_tools` | Irrelevant. Set semantics. |
| Same tool included by multiple paths (package + explicit + `tool_configurations`) | Deduplicated. |
| `enabled_tools: ["*"]` with `!` exclusions | Wildcard first, then exclusions remove. |

## 9. Back-compat matrix

| Existing usage | After change | Status |
|---|---|---|
| `"enabled_tools": "*"` | Same — every tool | ✅ Unchanged |
| `"enabled_tools": ["gsheets_read"]` | Same — one tool | ✅ Unchanged |
| `"enabled_tools": ["api_explorer"]` | Same — expands via `__` prefix | ✅ Unchanged |
| `"tool_configurations": {"api_explorer": {...}}` | Same — auto-enables via configured_aliases | ✅ Unchanged |
| Graph JSON without `enabled_tools` | Same — tools come only from `tool_configurations` | ✅ Unchanged |

No existing graph in `tests/graphs/` or in the ADP repo needs to change.

## 10. Testing

### 10.1 Unit tests (in `toolkit_packages.rs`)

1. `package_aliases_have_no_underscore` — iterates `TOOLKIT_PACKAGES`, asserts `!alias.contains('_')`. Blocks the convention violation at CI time.
2. `gsheets_package_has_all_ten_tools` — pins the contents so a future renaming of a tool also requires updating the package list (catches drift).
3. `find_package_returns_some_for_known_alias` / `find_package_returns_none_for_unknown` — sanity checks.

### 10.2 Unit tests in `filter_enabled_tools_tests` (extending the existing module)

4. `package_alias_expands_to_all_tools` — `["gsheets"]` → 10 tools.
5. `package_plus_individual_tool` — `["gsheets", "tavily_web"]` → 11 tools.
6. `exclusion_removes_tool_from_package` — `["gsheets", "!gsheets_delete_sheet"]` → 9 tools, no `delete_sheet`.
7. `exclusion_order_independent` — `["!gsheets_read", "gsheets"]` → 9 tools (same set as `["gsheets", "!gsheets_read"]`).
8. `exclusion_of_package_removes_all_its_tools` — `["*", "!gsheets"]` → all tools except the 10 gsheets ones.
9. `unknown_alias_silently_ignored` — `["gsheetz"]` → 0 tools, no panic.
10. `exact_tool_match_beats_package_collision` — synthetic test: when a tool exists named identically to a package alias, the tool wins.
11. `api_explorer_prefix_rule_still_works` — back-compat: `["api_explorer"]` enables every `api_explorer__*`.
12. `wildcard_plus_exclusion_works` — `["*", "!python_script"]`.
13. `package_via_tool_configurations_still_works` — entry in `configured_aliases` matching a package alias also expands.
14. `empty_exclusion_logged_and_ignored` — `["!"]` produces no panic.

### 10.3 End-to-end smoke

15. `tests/graphs/agents/gsheets_package_smoke.json` — minimal graph with `"enabled_tools": ["gsheets"]` (no `tool_configurations`). Agent calls `gsheets_list_sheets` to confirm activation works end-to-end.

### 10.4 Summary metadata tests

16. `every_gsheets_tool_has_summary` — iterate registered gsheets tool defs, assert `summary.is_some()` and length is between 10 and 200 chars.

## 11. Docs

| File | Change |
|---|---|
| `docs/developer_guide/39_gsheets.md` | Add "Recommended activation" subsection showing `enabled_tools: ["gsheets"]` as the canonical pattern. Note the exclusion syntax for read-only-style agents. |
| `docs/developer_guide/40_toolkit_packages.md` *(new)* | The canonical reference: concept, syntax, exclusion semantics, how to register a new package, naming-convention rule, edge cases, comparison with `api_explorer`'s `__` prefix-rule. |
| `docs/developer_guide/29_lazy_tool_loading.md` | Add a paragraph confirming `summary` is required for clean lazy operation; reference the gsheets package as the first fully-summarized package. |
| `docs/developer_guide/DEVELOPER_GUIDE.md` | Add `40_toolkit_packages.md` to the index. |
| `docs/node_as_tools_reference.json` | New top-level `toolkit_packages` section listing each package's alias, description, and tools array. |
| `docs/CHANGELOG_2026-06.md` | Entries for E-T15 and E-T16. |
| `docs/BACKLOG.md` | Add: "Toolkit packages v1.1 — auto-inject package description into system message" and "Surface unknown-alias warnings to the user". |

## 12. Task breakdown (for writing-plans)

| ID | Title | Estimate | Notes |
|---|---|---:|---|
| **E-T15** | Add `summary` to the 10 gsheets tool builders | 30 min | Standalone, no dependencies on E-T16. Includes the `every_gsheets_tool_has_summary` test. |
| **E-T16a** | New `toolkit_packages.rs` module + registry + unit tests (1, 2, 3) | 1 h | No changes to `llm.rs`; purely additive module. |
| **E-T16b** | Extend `filter_enabled_tools` with package expansion + exclusion + tests 4–14 | 1.5 h | Touches `llm.rs`. Keeps existing `__` prefix-rule. |
| **E-T16c** | E2E smoke graph + smoke test 15 | 30 min | Lives in `tests/graphs/agents/gsheets_package_smoke.json`. |
| **E-T16d** | Docs sweep (developer guide, node_as_tools_reference, CHANGELOG, BACKLOG, dev guide index) | 1 h | All edits are additive. |

Total estimate: **~4.5h** via subagent-driven development. E-T15 can run in parallel with E-T16a/b/c.

## 13. Future enhancements (BACKLOG)

- **System-message auto-injection**: when a package is enabled, optionally prepend its description to the agent's system message so the LLM gets a quick mental model ("You have access to the gsheets toolkit: …"). Useful but opinionated; deferred.
- **Unknown-alias warning surfaced to the user**: today the silent failure is back-compat; v1.1 could emit a warning into the `extra_info` block or a structured log.
- **Toolkit version pin**: `["gsheets@v2"]` syntax for future versioned packages. Not needed today.
- **CLI introspection**: `cargo run --bin dag_engine -- list-packages` printing every registered package + description + tools. Quality-of-life.
- **Per-package authn metadata**: declarative hints about what env vars each package needs. Today this lives in human docs only.

## 14. Self-review checklist

- ✅ Placeholders: none.
- ✅ Internal consistency: the algorithm in §7 matches the syntax in §6 and the edge cases in §8.
- ✅ Scope: focused — one spec, one implementation cycle (E-T15 + E-T16).
- ✅ Ambiguity: §8 disambiguates every edge case explicitly. The "exact tool match beats package" rule is called out.
- ✅ Naming convention: the only load-bearing rule (no `_` in package aliases) is enforced by test in §10.1.
- ✅ Back-compat: §9 is the contract — no existing graph or downstream consumer breaks.
- ✅ Open-source rule: §3 explicitly forbids ADP-specific package entries.
