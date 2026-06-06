# LLM-facing text centralization — design

**Status**: Approved (2026-06-06)
**Author**: Daniel García + colmena agent
**Tracks**: E-T17 (text centralization) + E-T18 (built-in tools index doc)
**Related**:
[2026-06-06 toolkit packages + summaries spec](2026-06-06-toolkit-packages-design.md),
the existing `src/.../nodes/prompts/*.md` precedent, the `skills/` top-level folder.

---

## 1. Goals

Two interlocked deliverables for the LLM-facing surface:

1. **Text centralization.** Every Rust-native string that the LLM reads —
   tool descriptions, tool summaries, system preludes, sandbox auto-preludes —
   moves out of inline source code into a dedicated top-level `text/` folder.
   Authors edit a single YAML or Markdown file to change what the model sees;
   they no longer hunt for an `r#"..."#` literal across the codebase.

2. **Built-in tools index.** A single Markdown reference at
   `docs/developer_guide/41_builtin_tools_index.md` lists every Rust-native
   LLM tool colmena ships with, its one-line summary, and a link to its
   detailed documentation. A CI test guarantees completeness — adding a new
   synthetic tool without an index entry fails the build.

Both deliverables share the same source of truth (the `text/tools/*.yaml`
registry) so the index never drifts from the actual tool surface.

## 2. Non-goals

- Replacing or extending `Cargo.toml` deps — `serde_yaml 0.9` is already
  present, `include_str!` / `OnceLock` are stdlib.
- Moving error messages, log lines, validation messages, code comments, or
  any internal string out of source. Scope is **A — LLM-facing prose only**.
- Restructuring the `skills/` top-level folder. Skills already live in
  markdown bundles and stay untouched.
- Auto-generating the built-in tools index from YAML. It is hand-maintained
  with a completeness test — generation can come later if drift becomes a
  problem.
- Versioning prompts or A/B-ing prompts at runtime. Single source per file;
  changes flow through git.

## 3. Open-source / ADP rule

Colmena is an open-source library. The `text/` folder MUST NOT contain
ADP-specific prompts, business-domain language, or naming. ADP, as a
downstream consumer, can ship its own prompts via its own configuration
path. The colmena registry covers only generic primitives.

## 4. Folder layout

```
src/libs/colmena/text/                       ← top-level, sibling of skills/
├── README.md                                 ← navigation guide
├── prompts/                                  ← monolithic .md files (free prose)
│   ├── llm_default_system.md                 # MOVE from src/.../nodes/prompts/
│   ├── extraction_system.md                  # MOVE
│   ├── reactor_system.md                     # MOVE
│   ├── planner_system.md                     # MOVE
│   ├── critic_system.md                      # MOVE
│   ├── orchestrator_phase_reactor.md         # MOVE
│   ├── orchestrator_grounding.md             # MOVE
│   ├── sql_llm_critic.md                     # NEW (from sql_llm_critic.rs:32)
│   ├── crdt_spreadsheet_protocol.md          # NEW (CRDT_SPREADSHEET_PROTOCOL_PRELUDE)
│   ├── documents_system_prelude.md           # NEW
│   ├── attachments_system_prelude.md         # NEW
│   ├── secure_suspend_tool_description.md    # NEW (Spanish block)
│   └── python_sandbox/
│       ├── crdt_doc_run_python_prelude.md    # NEW
│       ├── crdt_doc_run_python_postlude.md   # NEW
│       ├── gsheets_run_python_prelude.md     # NEW
│       └── gsheets_run_python_postlude.md    # NEW
│
└── tools/                                    ← structured YAML key-value
    ├── gsheets.yaml                          # 10 tools
    ├── crdt_doc.yaml                         # 11 tools
    ├── documents.yaml                        # 7 tools
    └── helpers.yaml                          # load_skill, load_attachment, recall_history
```

`text/README.md` lists which file holds which prompt and how to add a new one.

## 5. YAML schema for `tools/*.yaml`

Each top-level key is the tool's registered `name` constant. Two required
sub-keys, no others permitted (validator rejects unknown keys).

```yaml
gsheets_list_sheets:
  summary: List every tab (sheet) inside a spreadsheet by ID
  description: |
    List every sheet (tab) in a Google Sheets spreadsheet.

    Returns a list of {sheet_id, title, index, row_count, col_count} for
    each tab. Common use: discover the actual tab name before reading data,
    especially when the workbook may have a non-English locale.
```

**Constraints (enforced by tests):**
- `summary`: 10 ≤ length ≤ 200 characters
- `description`: non-empty
- key matches the tool's registered `name` constant (e.g.
  `GSHEETS_LIST_SHEETS_TOOL = "gsheets_list_sheets"`)

## 6. Loader module

Path: `src/libs/colmena/src/text/mod.rs`

```rust
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Deserialize)]
pub struct ToolText {
    pub summary: String,
    pub description: String,
}

// Compile-time embedded YAMLs.
const GSHEETS_YAML:   &str = include_str!("../../text/tools/gsheets.yaml");
const CRDT_DOC_YAML:  &str = include_str!("../../text/tools/crdt_doc.yaml");
const DOCUMENTS_YAML: &str = include_str!("../../text/tools/documents.yaml");
const HELPERS_YAML:   &str = include_str!("../../text/tools/helpers.yaml");

static TOOL_TEXTS: OnceLock<HashMap<String, ToolText>> = OnceLock::new();

fn load() -> &'static HashMap<String, ToolText> {
    TOOL_TEXTS.get_or_init(|| {
        let mut m = HashMap::new();
        for (label, yaml) in [
            ("gsheets",   GSHEETS_YAML),
            ("crdt_doc",  CRDT_DOC_YAML),
            ("documents", DOCUMENTS_YAML),
            ("helpers",   HELPERS_YAML),
        ] {
            let parsed: HashMap<String, ToolText> = serde_yaml::from_str(yaml)
                .unwrap_or_else(|e| panic!("text/tools/{label}.yaml malformed: {e}"));
            for (k, v) in parsed {
                if let Some(_existing) = m.insert(k.clone(), v) {
                    panic!("duplicate tool key '{k}' across text/tools/*.yaml");
                }
            }
        }
        m
    })
}

/// Lookup the summary for a registered synthetic tool. Panics at first
/// access if the tool is missing — failures are detectable at startup,
/// not deep in a tool call.
pub fn tool_summary(name: &str) -> &'static str {
    load()
        .get(name)
        .map(|t| t.summary.as_str())
        .unwrap_or_else(|| panic!(
            "Missing 'summary' for tool '{name}' in text/tools/*.yaml. \
             Add an entry or pass an explicit summary to the builder."
        ))
}

pub fn tool_description(name: &str) -> &'static str {
    load()
        .get(name)
        .map(|t| t.description.as_str())
        .unwrap_or_else(|| panic!("Missing 'description' for '{name}' in text/tools/*.yaml"))
}

pub fn all_tool_names() -> Vec<&'static str> {
    load().keys().map(|s| s.as_str()).collect()
}
```

The 'static lifetime works because `TOOL_TEXTS` lives in the binary's data
segment; references to its content remain valid for the entire process.

## 7. Synthetic tool builder migration

Each `tool_*()` builder swaps inline strings for `text::` accessors. Example:

```rust
// BEFORE
pub fn tool_list_sheets() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ListSheetsArgs>(
        GSHEETS_LIST_SHEETS_TOOL,
        "List every sheet (tab) in a Google Sheets spreadsheet. ...",
        "List every tab (sheet) inside a spreadsheet by ID",
    )
}

// AFTER
pub fn tool_list_sheets() -> ToolDefinition {
    use crate::text;
    super::build_synthetic_tool_with_summary::<ListSheetsArgs>(
        GSHEETS_LIST_SHEETS_TOOL,
        text::tool_description(GSHEETS_LIST_SHEETS_TOOL),
        text::tool_summary(GSHEETS_LIST_SHEETS_TOOL),
    )
}
```

The tool's `name` constant becomes the single lookup key — there is no
possibility of summary and description referring to different tools.

## 8. Inline-prompt migration

For every long-form const in source (CRITIC_SYSTEM_PROMPT,
SECURE_SUSPEND_TOOL_DESCRIPTION, CRDT_SPREADSHEET_PROTOCOL_PRELUDE,
DOCUMENTS_SYSTEM_PRELUDE, ATTACHMENTS_SYSTEM_PRELUDE, the four Python
auto-prelude/postlude blocks):

```rust
// BEFORE
const CRITIC_SYSTEM_PROMPT: &str = r#"You are a PostgreSQL security..."#;

// AFTER
const CRITIC_SYSTEM_PROMPT: &str = include_str!("../../../text/prompts/sql_llm_critic.md");
```

The seven existing files at `src/.../nodes/prompts/*.md` move to
`text/prompts/`, and their `include_str!` paths in callers are updated.

## 9. Tests

| Test | What it asserts |
|---|---|
| `yaml_files_parse_at_startup` | Calls `text::load()` to surface any YAML-syntax failure as a unit-test failure (not at first runtime tool call) |
| `every_registered_tool_has_text_entry` | Iterates `all_synthetic_tools()` (from E-T15d) and calls `text::tool_summary` + `text::tool_description` for each — panics if any is missing. Also enforces 10–200 char summary bound. |
| `no_orphan_yaml_entries` | Iterates `text::all_tool_names()` and asserts each maps to a registered builder — catches dead entries the registry no longer uses |
| `prompts_exist_and_nonempty` | For every `include_str!("../../../text/prompts/X.md")` site, the loaded string is non-empty (rules out accidental empty files) |
| `index_doc_covers_all_registered_tools` | Parses `docs/developer_guide/41_builtin_tools_index.md` and asserts each registered tool appears in some section's table — see §11 |

`every_synthetic_tool_has_summary` from E-T15d is **replaced** by
`every_registered_tool_has_text_entry`. The new test is strictly stronger.

## 10. Edge cases / decisions

| Case | Decision |
|---|---|
| YAML file fails to parse | Panic at startup with the YAML path and serde error — fail fast |
| Tool registered in code but missing in YAML | Panic at first `text::tool_summary` call with a clear "add an entry" message |
| YAML entry has no matching tool | `no_orphan_yaml_entries` test fails |
| Same tool key in two YAMLs | Panic at startup ("duplicate tool key 'X' across text/tools/*.yaml") |
| Summary outside [10, 200] chars | `every_registered_tool_has_text_entry` test fails |
| `describe_tool` | Exempt — built dynamically per turn (same exemption as E-T15d) |
| DAG nodes used as tools (`python_script`, `http_request`, etc.) | Out of scope — their text is user-supplied via `tool_configurations` |

## 11. Built-in tools index doc

Path: `docs/developer_guide/41_builtin_tools_index.md`

Sections, in order:

1. **Intro paragraph** explaining the doc's purpose and the relationship to `text/tools/*.yaml`.
2. **By package** subsections, one per `TOOLKIT_PACKAGES` entry plus an "orphan" section for non-packaged synthetic tools (helpers, etc.):
   - `gsheets` (10 tools)
   - `crdt_doc` (11 tools)
   - `documents` (7 tools)
   - `helpers` (load_skill, load_attachment, recall_history)
3. Each subsection has a table:
   ```
   | Tool | Summary | Detailed docs |
   |---|---|---|
   | `gsheets_list_sheets` | List every tab… | [§39](39_gsheets.md) |
   ```
4. **Toolkit packages** section linking to §40.
5. **describe_tool** mentioned once at the bottom with the §29 link.

The "Summary" column is filled from `text/tools/*.yaml` (manually copied;
the completeness test verifies presence but not equality).

A future enhancement (BACKLOG) auto-generates this doc from the YAMLs;
v1 stays hand-maintained for simplicity.

## 12. Task breakdown (for writing-plans)

| ID | Title | Estimate | Depends on |
|---|---|---:|---|
| **E-T17a** | Create `text/` skeleton — folder, README, 4 empty YAMLs, move existing 7 `.md` prompts from `nodes/prompts/` to `text/prompts/` and update `include_str!` paths in callers | 40 min | — |
| **E-T17b** | New loader module `src/libs/colmena/src/text/mod.rs` + 3 tests (`yaml_files_parse_at_startup`, scaffolding for the other two) | 1 h | E-T17a |
| **E-T17c** | Migrate 10 gsheets tools — write `text/tools/gsheets.yaml`, update 10 builders to call `text::*` accessors | 30 min | E-T17b |
| **E-T17d** | Migrate 11 crdt_doc tools | 30 min | E-T17b |
| **E-T17e** | Migrate 7 document tools | 25 min | E-T17b |
| **E-T17f** | Migrate 3 helper tools (load_skill, load_attachment, recall_history) | 25 min | E-T17b |
| **E-T17g** | Extract 8 inline prompts — CRITIC_SYSTEM_PROMPT, SECURE_SUSPEND_TOOL_DESCRIPTION, CRDT_SPREADSHEET_PROTOCOL_PRELUDE, DOCUMENTS_SYSTEM_PRELUDE, ATTACHMENTS_SYSTEM_PRELUDE, the 4 Python preludes/postludes — into `text/prompts/*.md` and switch callers to `include_str!` | 1.5 h | E-T17a |
| **E-T17h** | Activate `every_registered_tool_has_text_entry` + `no_orphan_yaml_entries` + retire `every_synthetic_tool_has_summary` | 30 min | E-T17c-f |
| **E-T18a** | Create `docs/developer_guide/41_builtin_tools_index.md` (hand-written, complete) | 45 min | E-T17c-f |
| **E-T18b** | Add `index_doc_covers_all_registered_tools` completeness test | 30 min | E-T18a |
| **E-T17i** | Docs sweep — `text/README.md` navigation guide, `DEVELOPER_GUIDE.md` index, `CHANGELOG_2026-06.md`, `BACKLOG.md` follow-ups | 30 min | E-T17a-h, E-T18a-b |

Total estimate: **~7.5 h** via subagent-driven development.

**Parallelization**: E-T17c/d/e/f are independent of each other (different files). E-T17g is independent of c/d/e/f. After E-T17b ships, all of c, d, e, f, g can run in parallel. E-T17h waits for c/d/e/f. E-T18a waits for c/d/e/f (needs the final summary list). E-T18b depends on E-T18a.

Suggested order:
1. E-T17a (skeleton + move existing prompts)
2. E-T17b (loader)
3. Parallel: c, d, e, f, g
4. E-T17h (test activation)
5. E-T18a → E-T18b
6. E-T17i (docs sweep)

## 13. Back-compat matrix

| Existing usage | After change | Status |
|---|---|---|
| `include_str!("prompts/<name>.md")` in source | path updated to `../../../text/prompts/<name>.md`; same constant name; same string content | ✅ Same const, same string |
| `build_synthetic_tool_with_summary(name, descr, summ)` call sites | Now pass `text::tool_description(name)` + `text::tool_summary(name)` instead of literal | ✅ Same `ToolDefinition`, identical content (verified by tests) |
| Downstream consumers that read `td.summary` / `td.description` | Same fields, same string content | ✅ Zero break |
| Inline `CRITIC_SYSTEM_PROMPT` etc. | Same constant name, now backed by `include_str!` | ✅ Same const visible |

No public API changes. No downstream consumer in ADP needs to change.

## 14. Future enhancements (BACKLOG)

- **Auto-generated index** — replace E-T18a's hand-written tables with a
  build step that reads `text/tools/*.yaml` and writes the .md. Once the
  human-maintained version is stable.
- **i18n** — add language-keyed YAML entries (`summary.en`, `summary.es`)
  with a runtime language selector. Out of scope today; the user has not
  asked for multi-language tool surfaces.
- **DAG-node default summaries** — `ExecutableNode` trait gains an
  optional `default_summary()` method; per-agent `tool_configurations`
  override. Pre-existing BACKLOG item.
- **Hot reload** — watch `text/` for changes and reparse without restart.
  Helpful for prompt iteration; complex to implement (the binary embeds
  the YAML via `include_str!`). Defer until a real DX pain point appears.

## 15. Self-review checklist

- ✅ Placeholders: none.
- ✅ Internal consistency: the algorithm in §6 matches the syntax in §5 and the edge cases in §10.
- ✅ Scope: focused on Option A (LLM-facing prose only); DAG-node tool text explicitly out of scope.
- ✅ Ambiguity: §10 disambiguates every edge case. The duplicate-key rule is explicit.
- ✅ Naming convention: YAML key MUST equal the tool's registered `name`. Enforced by tests.
- ✅ Back-compat: §13 — no consumer breaks.
- ✅ Open-source rule: §3 — no ADP-specific text allowed.
- ✅ Coverage: every inline LLM-facing string identified in §1 audit has a target in the new layout (§4 + §7 + §8).
- ✅ Test coverage: 5 tests in §9 cover load, presence, orphans, prompt files, doc index.
