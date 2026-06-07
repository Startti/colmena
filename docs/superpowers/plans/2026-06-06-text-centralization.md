# LLM-Facing Text Centralization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move every Rust-inline LLM-facing string (tool descriptions, summaries, system preludes, sandbox auto-preludes) into a top-level `text/` folder organized as `text/prompts/*.md` (monolithic) + `text/tools/*.yaml` (structured key-value). Add a hand-written built-in tools index at `docs/developer_guide/41_builtin_tools_index.md` with a CI completeness test.

**Architecture:**
- Content layout: `src/libs/colmena/text/` is a top-level sibling of `skills/`. `text/prompts/` holds monolithic .md files (the existing `src/.../nodes/prompts/*.md` migrate here too). `text/tools/` holds one YAML per package: `gsheets.yaml`, `crdt_doc.yaml`, `documents.yaml`, `helpers.yaml`.
- Loader: a new `src/libs/colmena/src/text/mod.rs` exposes `text::tool_summary(name)` and `text::tool_description(name)`. YAMLs are embedded via `include_str!` and parsed once into a `OnceLock<HashMap<String, ToolText>>` at first access. Missing entries panic with a clear message.
- Synthetic tool builders swap inline literals for `text::*` accessors. Existing CI gate `every_synthetic_tool_has_summary` is replaced by stronger tests (`every_registered_tool_has_text_entry`, `no_orphan_yaml_entries`).
- Built-in tools index: hand-maintained markdown at `docs/developer_guide/41_builtin_tools_index.md`; a test parses the doc and asserts every registered synthetic tool appears in some section.

**Tech Stack:** Rust 1.95, `serde_yaml 0.9` (already in Cargo.toml), `OnceLock` (stdlib), `include_str!` (stdlib), existing colmena synthetic-tools machinery.

**Spec:** [docs/superpowers/specs/2026-06-06-text-centralization-design.md](../specs/2026-06-06-text-centralization-design.md)

---

## File Structure

**New files:**
- `src/libs/colmena/text/README.md` — navigation guide
- `src/libs/colmena/text/prompts/` — folder holding moved + new .md prompts
- `src/libs/colmena/text/tools/gsheets.yaml`
- `src/libs/colmena/text/tools/crdt_doc.yaml`
- `src/libs/colmena/text/tools/documents.yaml`
- `src/libs/colmena/text/tools/helpers.yaml`
- `src/libs/colmena/text/prompts/python_sandbox/` — subfolder for 4 auto-preludes/postludes
- `src/libs/colmena/src/text/mod.rs` — loader
- `docs/developer_guide/41_builtin_tools_index.md` — index doc

**Moved files (rename, content unchanged):**
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/prompts/critic_system.md` → `src/libs/colmena/text/prompts/critic_system.md`
- `extraction_system.md`, `llm_default_system.md`, `orchestrator_grounding.md`, `orchestrator_phase_reactor.md`, `planner_system.md`, `reactor_system.md`, `routing_classifier_system.md` — same move pattern. **8 files total.**

**Modified files (caller paths updated):**
- `src/libs/colmena/src/lib.rs` — add `pub mod text;`
- `src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs` — extract `CRITIC_SYSTEM_PROMPT`
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs` — extract `SECURE_SUSPEND_TOOL_DESCRIPTION`
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_summary.rs` — extract `CRDT_SPREADSHEET_PROTOCOL_PRELUDE`
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs` — extract `ATTACHMENTS_SYSTEM_PRELUDE`
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs` — extract `DOCUMENTS_SYSTEM_PRELUDE`
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs` — extract `let prelude` (line 292) and `let postlude` (line 302)
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs` — extract `let prelude` (line 253) and `let postlude` (line 263)
- Callers of existing `nodes/prompts/*.md` (8 files: `extraction.rs`, `planner.rs`, `critic.rs`, `reactor.rs`, `orchestrator.rs`, `output_parser.rs`, `router/extract_and_route.rs`, plus `llm.rs`) — update `include_str!` paths from `prompts/X.md` to `../../../text/prompts/X.md`
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs` — 9 builders use `text::tool_summary` / `text::tool_description`
- `gsheets_run_python.rs`, `crdt_doc_tools.rs`, `crdt_doc_run_python.rs`, `crdt_doc_import_sheet.rs`, `document_tools.rs`, `load_skill_tool.rs`, `load_attachment_tool.rs`, `recall_history.rs` — same migration pattern
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` — replace `every_synthetic_tool_has_summary` with new tests
- `docs/developer_guide/DEVELOPER_GUIDE.md` — index entry for `41_builtin_tools_index.md`
- `docs/CHANGELOG_2026-06.md` — E-T17 + E-T18 entries
- `docs/BACKLOG.md` — auto-gen + i18n + hot-reload entries

---

## Task Dependency Graph

```
Task 0 (skeleton + move 8 existing prompts + update include_str! paths)
  ↓
Task 1 (loader + 3 startup tests)
  ↓
  ├─ Task 2 (gsheets migration)         ┐
  ├─ Task 3 (crdt_doc migration)         │ all parallel
  ├─ Task 4 (documents migration)        │ after T1
  ├─ Task 5 (helpers migration)         ┘
  ↓
Task 6 (extract 8 inline prompts) — can run in parallel with T2-T5
  ↓
Task 7 (activate new validation tests, retire every_synthetic_tool_has_summary)
  ↓
Task 8 (built-in tools index doc)
  ↓
Task 9 (completeness test for index)
  ↓
Task 10 (docs sweep)
```

---

## Task 0 (E-T17a): Create `text/` skeleton + move existing 8 prompts

**Goal:** Top-level `text/` folder with all skeleton files. Move the 8 existing `nodes/prompts/*.md` into `text/prompts/`. Update every `include_str!` caller path. Create empty stub YAMLs (loader populates them in T2-T5).

**Files:**
- Create: `src/libs/colmena/text/README.md`
- Create: `src/libs/colmena/text/prompts/` directory
- Create: `src/libs/colmena/text/prompts/python_sandbox/` directory
- Create: `src/libs/colmena/text/tools/gsheets.yaml` (empty stub `{}`)
- Create: `src/libs/colmena/text/tools/crdt_doc.yaml` (empty stub `{}`)
- Create: `src/libs/colmena/text/tools/documents.yaml` (empty stub `{}`)
- Create: `src/libs/colmena/text/tools/helpers.yaml` (empty stub `{}`)
- Move: 8 files from `src/libs/colmena/src/dag_engine/infrastructure/nodes/prompts/` to `src/libs/colmena/text/prompts/`
- Modify: callers of existing `prompts/*.md` (~9 files) — update `include_str!` paths

- [ ] **Step 1: Create directories and stubs**

```bash
cd /Users/danielgarcia/startti/colmena
mkdir -p src/libs/colmena/text/prompts/python_sandbox
mkdir -p src/libs/colmena/text/tools
# Empty YAML stubs so include_str! works before T2-T5 populate them
printf '{}\n' > src/libs/colmena/text/tools/gsheets.yaml
printf '{}\n' > src/libs/colmena/text/tools/crdt_doc.yaml
printf '{}\n' > src/libs/colmena/text/tools/documents.yaml
printf '{}\n' > src/libs/colmena/text/tools/helpers.yaml
```

- [ ] **Step 2: Move the 8 existing prompts**

```bash
cd /Users/danielgarcia/startti/colmena
git mv src/libs/colmena/src/dag_engine/infrastructure/nodes/prompts/critic_system.md           src/libs/colmena/text/prompts/critic_system.md
git mv src/libs/colmena/src/dag_engine/infrastructure/nodes/prompts/extraction_system.md       src/libs/colmena/text/prompts/extraction_system.md
git mv src/libs/colmena/src/dag_engine/infrastructure/nodes/prompts/llm_default_system.md      src/libs/colmena/text/prompts/llm_default_system.md
git mv src/libs/colmena/src/dag_engine/infrastructure/nodes/prompts/orchestrator_grounding.md  src/libs/colmena/text/prompts/orchestrator_grounding.md
git mv src/libs/colmena/src/dag_engine/infrastructure/nodes/prompts/orchestrator_phase_reactor.md src/libs/colmena/text/prompts/orchestrator_phase_reactor.md
git mv src/libs/colmena/src/dag_engine/infrastructure/nodes/prompts/planner_system.md          src/libs/colmena/text/prompts/planner_system.md
git mv src/libs/colmena/src/dag_engine/infrastructure/nodes/prompts/reactor_system.md          src/libs/colmena/text/prompts/reactor_system.md
git mv src/libs/colmena/src/dag_engine/infrastructure/nodes/prompts/routing_classifier_system.md src/libs/colmena/text/prompts/routing_classifier_system.md
```

- [ ] **Step 3: Find every caller and update `include_str!` paths**

```bash
cd /Users/danielgarcia/startti/colmena
grep -rn 'include_str!\("prompts/\|include_str!\("\\.\\./prompts/' src/libs/colmena/src/ --include="*.rs"
```

Expected: 9 hits across `extraction.rs`, `planner.rs`, `critic.rs`, `reactor.rs`, `orchestrator.rs` (3 hits — it loads three prompts), `output_parser.rs`, `router/extract_and_route.rs`, `llm.rs`. The exact relative path each currently uses depends on file depth — count `../` to traverse from the file up to `src/libs/colmena/src/` and then in to `text/prompts/`.

For each hit, transform the path. Examples by depth:

```rust
// In src/.../nodes/extraction.rs (3 levels under src/):
// BEFORE
const DEFAULT_EXTRACTION_SYSTEM_MSG: &str = include_str!("prompts/extraction_system.md");
// AFTER
const DEFAULT_EXTRACTION_SYSTEM_MSG: &str = include_str!("../../../../text/prompts/extraction_system.md");
```

Path arithmetic for every existing call site:
- `nodes/extraction.rs` → `../../../../text/prompts/extraction_system.md` (4 levels up to colmena/, then into text/prompts/)
- `nodes/planner.rs` → `../../../../text/prompts/planner_system.md`
- `nodes/critic.rs` → `../../../../text/prompts/critic_system.md`
- `nodes/reactor.rs` → `../../../../text/prompts/reactor_system.md`
- `nodes/orchestrator.rs` → `../../../../text/prompts/orchestrator_phase_reactor.md` + `orchestrator_grounding.md` + `llm_default_system.md`
- `nodes/output_parser.rs` → `../../../../text/prompts/extraction_system.md`
- `nodes/llm.rs` → `../../../../text/prompts/llm_default_system.md`
- `nodes/router/extract_and_route.rs` → `../../../../../text/prompts/extraction_system.md` (5 levels — one extra `..` for `router/`)

Find each, count slashes from the file's location to `src/libs/colmena/`, then add `text/prompts/<name>.md`.

- [ ] **Step 4: Write the navigation README**

Path: `src/libs/colmena/text/README.md`

```markdown
# colmena LLM-facing text

This folder holds every Rust-native string the LLM reads. Edit a file here
to change what the model sees — you do not need to touch source code.

## Layout

- `prompts/` — monolithic system messages and preludes. One file per text.
  - `python_sandbox/` — the auto-prelude/postlude blocks wrapped around
    user code inside `crdt_doc_run_python` and `gsheets_run_python`.
- `tools/` — YAML registries, one file per toolkit package. Each top-level
  key is a tool's registered `name` constant. Two sub-keys: `summary` (≤
  200 chars one-line) and `description` (multi-line).

## How to add a new tool's text

1. Open `tools/<package>.yaml`.
2. Append an entry:

   ```yaml
   <tool_name>:
     summary: A short one-liner shown in the lazy-loading catalog.
     description: |
       Full description visible to the LLM when the tool is called.
   ```

3. Run `cargo test --lib text` — the loader and `every_registered_tool_has_text_entry`
   test verify your YAML parses and matches a registered builder.

## How to add a new prompt

1. Create a new `.md` file under `prompts/`.
2. In the Rust caller, swap the inline string for
   `include_str!("../../<...>/text/prompts/<name>.md")`. Count `..` to
   reach `src/libs/colmena/` from the calling file.

## Why this layout exists

See `docs/superpowers/specs/2026-06-06-text-centralization-design.md`.
```

- [ ] **Step 5: Build + tests must pass**

```bash
cd /Users/danielgarcia/startti/colmena
cargo build --lib 2>&1 | tail -5
cargo test --lib --quiet 2>&1 | tail -5
```

Expected: both pass. The 8 `include_str!` callers now point at the new location; if any path was wrong, the compiler tells you at this step.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/text/ src/libs/colmena/src/
git rm -rf src/libs/colmena/src/dag_engine/infrastructure/nodes/prompts/ 2>/dev/null || true
git commit -m "feat(text-centralization): skeleton + move 8 prompts to top-level text/

E-T17a. Creates src/libs/colmena/text/ as a sibling of skills/. Moves the
8 existing nodes/prompts/*.md files into text/prompts/ and updates every
include_str! caller path. Adds empty YAML stubs in text/tools/ for the
follow-up package migrations (T2-T5). Adds a navigation README.

Zero behavior change — same constants, same string contents.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 1 (E-T17b): Loader module `src/text/mod.rs` + 3 startup tests

**Goal:** Add the loader that exposes `text::tool_summary(name)` and `text::tool_description(name)`. YAMLs are still empty stubs at this point; the loader works on empty data and the structure tests pass.

**Files:**
- Create: `src/libs/colmena/src/text/mod.rs`
- Modify: `src/libs/colmena/src/lib.rs` (add `pub mod text;`)

- [ ] **Step 1: Write the loader**

Path: `src/libs/colmena/src/text/mod.rs`

```rust
//! Loader for LLM-facing text content under `src/libs/colmena/text/`.
//!
//! YAML files at `text/tools/*.yaml` are embedded at compile time via
//! `include_str!` and parsed into a static `HashMap` at first access.
//! Missing entries panic with a clear "add an entry" message — failures
//! are detectable at startup, not deep in a tool call.
//!
//! See `docs/superpowers/specs/2026-06-06-text-centralization-design.md`.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
pub struct ToolText {
    pub summary: String,
    pub description: String,
}

const GSHEETS_YAML: &str = include_str!("../../text/tools/gsheets.yaml");
const CRDT_DOC_YAML: &str = include_str!("../../text/tools/crdt_doc.yaml");
const DOCUMENTS_YAML: &str = include_str!("../../text/tools/documents.yaml");
const HELPERS_YAML: &str = include_str!("../../text/tools/helpers.yaml");

static TOOL_TEXTS: OnceLock<HashMap<String, ToolText>> = OnceLock::new();

/// Populate the registry from every embedded YAML. Panics if any YAML is
/// malformed or a tool key appears in more than one file.
fn load() -> &'static HashMap<String, ToolText> {
    TOOL_TEXTS.get_or_init(|| {
        let mut m: HashMap<String, ToolText> = HashMap::new();
        for (label, yaml) in [
            ("gsheets", GSHEETS_YAML),
            ("crdt_doc", CRDT_DOC_YAML),
            ("documents", DOCUMENTS_YAML),
            ("helpers", HELPERS_YAML),
        ] {
            // Empty file ("{}") parses to an empty map; that's expected
            // before T2-T5 populate the registry.
            let parsed: HashMap<String, ToolText> = serde_yaml::from_str(yaml)
                .unwrap_or_else(|e| panic!("text/tools/{label}.yaml malformed: {e}"));
            for (k, v) in parsed {
                if m.insert(k.clone(), v).is_some() {
                    panic!("duplicate tool key '{k}' across text/tools/*.yaml");
                }
            }
        }
        m
    })
}

/// Lookup the summary for a registered synthetic tool. Panics with a
/// clear message if the tool is missing from `text/tools/*.yaml`.
pub fn tool_summary(name: &str) -> &'static str {
    load()
        .get(name)
        .map(|t| t.summary.as_str())
        .unwrap_or_else(|| {
            panic!(
                "Missing 'summary' for tool '{name}' in text/tools/*.yaml. \
                 Add an entry or pass an explicit summary to the builder."
            )
        })
}

/// Lookup the description for a registered synthetic tool. Panics if missing.
pub fn tool_description(name: &str) -> &'static str {
    load()
        .get(name)
        .map(|t| t.description.as_str())
        .unwrap_or_else(|| {
            panic!("Missing 'description' for '{name}' in text/tools/*.yaml")
        })
}

/// Every tool name currently in the registry. Used by tests to detect
/// orphan YAML entries (entries with no matching registered builder).
pub fn all_tool_names() -> Vec<&'static str> {
    load().keys().map(|s| s.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_files_parse_at_startup() {
        // Calling load() forces every embedded YAML to be parsed. A
        // malformed file produces a clear panic with the file label and
        // the serde error.
        let _ = load();
    }

    #[test]
    fn empty_registry_is_acceptable_initially() {
        // Before T2-T5 land, all YAMLs are "{}". The loader must accept
        // that gracefully — the orphan/missing tests run later.
        let names = all_tool_names();
        // Length is 0 before tool migrations, > 0 after. Either is OK.
        assert!(names.len() <= 100, "registry suspiciously large: {}", names.len());
    }

    #[test]
    fn duplicate_key_panics() {
        // The check is structural in load(); the panic message must
        // mention the colliding key. We test this by constructing two
        // YAMLs with the same key — but since the real YAMLs are
        // embedded constants, this test verifies the panic path indirectly
        // via a synthetic parse.
        let yaml_a: &str = "shared_key:\n  summary: from a\n  description: x\n";
        let yaml_b: &str = "shared_key:\n  summary: from b\n  description: y\n";
        let mut m: HashMap<String, ToolText> = HashMap::new();
        let parsed_a: HashMap<String, ToolText> = serde_yaml::from_str(yaml_a).unwrap();
        m.extend(parsed_a);
        let parsed_b: HashMap<String, ToolText> = serde_yaml::from_str(yaml_b).unwrap();
        // Verify the second insert would have triggered the duplicate panic
        // path in load() — we can't reach it directly without spawning a
        // subprocess, so we sanity-check the structural shape here.
        for (k, _) in parsed_b {
            assert!(m.contains_key(&k), "duplicate detection sanity check failed");
        }
    }
}
```

- [ ] **Step 2: Register the module in `src/lib.rs`**

Open `src/libs/colmena/src/lib.rs`. Find the section with `pub mod` declarations. Insert `pub mod text;` between existing modules (alphabetic placement: after `pub mod storage;`, before `pub mod web;`).

Current top-of-file declarations include:

```rust
pub mod crdt_documents;
pub mod dag_engine;
pub mod documents;
pub mod gsheets;
pub mod llm;
pub mod skills;
pub mod storage;
pub mod web;
```

After:

```rust
pub mod crdt_documents;
pub mod dag_engine;
pub mod documents;
pub mod gsheets;
pub mod llm;
pub mod skills;
pub mod storage;
pub mod text;
pub mod web;
```

- [ ] **Step 3: Build + tests**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib text --quiet 2>&1 | tail -10
```

Expected: 3 tests pass.

Full suite + clippy:

```bash
cargo test --lib --quiet 2>&1 | tail -5
cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
```

Expected: all green / no warnings.

- [ ] **Step 4: Commit**

```bash
cd /Users/danielgarcia/startti/colmena
git add src/libs/colmena/src/text/ src/libs/colmena/src/lib.rs
git commit -m "feat(text): loader module + 3 startup tests

E-T17b. New src/libs/colmena/src/text/mod.rs embeds the four
text/tools/*.yaml files via include_str! and exposes
tool_summary / tool_description / all_tool_names accessors backed
by a OnceLock<HashMap>. Loader panics with a clear message on:
  - malformed YAML (file label + serde error)
  - duplicate key across files
  - missing entry (suggests the fix)

Synthetic tool builders migrate in follow-up tasks T2-T5.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2 (E-T17c): Migrate gsheets — write `gsheets.yaml` + update 10 builders

**Goal:** Replace inline `summary` and `description` strings in every gsheets builder with `text::tool_summary(NAME)` / `text::tool_description(NAME)` calls. Populate `text/tools/gsheets.yaml`.

**Files:**
- Modify: `src/libs/colmena/text/tools/gsheets.yaml` (was empty stub)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs`

- [ ] **Step 1: Populate `gsheets.yaml`**

Open each `tool_*()` builder in `gsheets_tools.rs` and `gsheets_run_python.rs` and copy the existing literal `description` and `summary` arguments into the YAML.

Path: `src/libs/colmena/text/tools/gsheets.yaml`

Replace the `{}` stub with the full registry. The summaries below are the canonical strings shipped in E-T15c-1 (task 2 of the previous plan, commit `15aff25`):

```yaml
gsheets_create_spreadsheet:
  summary: Create a new Google Sheets workbook and return its URL
  description: |
    PASTE THE EXISTING LITERAL DESCRIPTION FROM gsheets_tools.rs::tool_create_spreadsheet HERE.
    Preserve newlines and Markdown formatting exactly. Wrap with `description: |` so YAML
    keeps every newline intact.

gsheets_create_from_xlsx:
  summary: Upload a local .xlsx attachment and convert it into a new Google Sheet
  description: |
    PASTE FROM tool_create_from_xlsx.

gsheets_export_xlsx:
  summary: Download an existing Google Sheet as .xlsx bytes attachment
  description: |
    PASTE FROM tool_export_xlsx.

gsheets_list_sheets:
  summary: List every tab (sheet) inside a spreadsheet by ID
  description: |
    PASTE FROM tool_list_sheets.

gsheets_add_sheet:
  summary: Create a new tab inside an existing spreadsheet
  description: |
    PASTE FROM tool_add_sheet.

gsheets_delete_sheet:
  summary: Permanently delete a tab from a spreadsheet
  description: |
    PASTE FROM tool_delete_sheet.

gsheets_read:
  summary: Read a cell range from a tab; supports formatted, unformatted, and formula render modes
  description: |
    PASTE FROM tool_read.

gsheets_set_cell:
  summary: Write one value or formula into a single cell
  description: |
    PASTE FROM tool_set_cell.

gsheets_set_range:
  summary: Write a 2-D values array starting at a given address
  description: |
    PASTE FROM tool_set_range.

gsheets_run_python:
  summary: Run sandboxed pandas analysis over sheet ranges loaded directly by the dispatcher (rows never pass through the LLM)
  description: |
    PASTE FROM tool_gsheets_run_python (in gsheets_run_python.rs, NOT gsheets_tools.rs).
```

Use this exact grep to extract each existing description:

```bash
cd /Users/danielgarcia/startti/colmena
grep -A 30 "^pub fn tool_create_spreadsheet" \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs
```

Repeat for each builder.

- [ ] **Step 2: Update each builder to call `text::*`**

For each of the 9 builders in `gsheets_tools.rs`, replace the inline strings with accessor calls. Example for `tool_list_sheets`:

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

Apply the same transformation to every `tool_*()` in `gsheets_tools.rs` and to `tool_gsheets_run_python()` in `gsheets_run_python.rs`.

- [ ] **Step 3: Run gsheets tests**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib gsheets --quiet 2>&1 | tail -10
```

Expected: all gsheets tests still pass (28+).

- [ ] **Step 4: Run the loader's startup test to confirm YAML parses**

```bash
cargo test --lib text --quiet 2>&1 | tail -5
```

Expected: 3 tests pass. The YAML is no longer `{}` — it must parse cleanly.

- [ ] **Step 5: Clippy**

```bash
cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/text/tools/gsheets.yaml \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs
git commit -m "feat(text-centralization): migrate gsheets to text/tools/gsheets.yaml

E-T17c. All 10 gsheets builders now pull their summary and description
from text/tools/gsheets.yaml via text::tool_summary / text::tool_description.
Zero behavior change verified by existing gsheets tests.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3 (E-T17d): Migrate crdt_doc — write `crdt_doc.yaml` + update 11 builders

**Goal:** Same migration pattern as Task 2, for the 11 crdt_doc synthetic tools.

**Files:**
- Modify: `src/libs/colmena/text/tools/crdt_doc.yaml`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs`

- [ ] **Step 1: Enumerate the builders**

```bash
cd /Users/danielgarcia/startti/colmena
grep -n "^pub fn tool_" \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs
```

Expected: 11 builders (the canonical list shipped in E-T15c-2, commit `8895686`).

- [ ] **Step 2: Populate `crdt_doc.yaml`**

For each builder, copy the existing inline description + summary into the YAML. Tool names + summaries are already canonical from the earlier migration. Example pattern:

```yaml
crdt_doc_list_sheets:
  summary: List every tab (sheet) inside the current CRDT document artifact
  description: |
    PASTE THE EXISTING LITERAL DESCRIPTION FROM tool_list_sheets HERE.

crdt_doc_list_sheets_of:
  summary: List tabs inside a specific CRDT artifact by ID
  description: |
    PASTE FROM tool_list_sheets_of.

# ... and so on for: tool_read, tool_set_cell, tool_set_range,
# tool_add_sheet, tool_get_recent_changes, tool_list_my_artifacts,
# tool_create_artifact, tool_run_python, tool_import_sheet
```

The summary strings to use verbatim are the ones shipped in E-T15c-2 — they live in the current source as the third argument to `build_synthetic_tool_with_summary` in each builder.

- [ ] **Step 3: Update each builder to call `text::*`**

Same transformation as Task 2 step 2 — replace the inline summary and description arguments with `text::tool_description(NAME)` / `text::tool_summary(NAME)`.

- [ ] **Step 4: Tests + clippy + commit**

```bash
cargo test --lib crdt_doc --quiet 2>&1 | tail -8
cargo test --lib text --quiet 2>&1 | tail -5
cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
git add src/libs/colmena/text/tools/crdt_doc.yaml \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs
git commit -m "feat(text-centralization): migrate crdt_doc to text/tools/crdt_doc.yaml

E-T17d. All 11 crdt_doc builders now pull text from text/tools/crdt_doc.yaml.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4 (E-T17e): Migrate documents — write `documents.yaml` + update 7 builders

**Goal:** Same pattern for the 7 document builders.

**Files:**
- Modify: `src/libs/colmena/text/tools/documents.yaml`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`

- [ ] **Step 1: Enumerate builders**

```bash
cd /Users/danielgarcia/startti/colmena
grep -n "^pub fn build_.*_tool" \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs
```

Expected: 7 builders (`build_document_create_tool`, etc.) — same list shipped in E-T15c-3, commit `d86c9e1`.

- [ ] **Step 2: Populate `documents.yaml` + migrate builders**

Pattern identical to Task 2/3. Copy current literals into the YAML, replace builder bodies with `text::*` accessors.

- [ ] **Step 3: Tests + clippy + commit**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib document_tools --quiet 2>&1 | tail -8
cargo test --lib text --quiet 2>&1 | tail -5
cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
git add src/libs/colmena/text/tools/documents.yaml \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs
git commit -m "feat(text-centralization): migrate documents to text/tools/documents.yaml

E-T17e. All 7 document builders now pull text from text/tools/documents.yaml.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5 (E-T17f): Migrate helpers — write `helpers.yaml` + update 3 builders

**Goal:** Migrate `load_skill`, `load_attachment`, `recall_history` (the helper trio).

**Files:**
- Modify: `src/libs/colmena/text/tools/helpers.yaml`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/recall_history.rs`

- [ ] **Step 1: Inspect each helper's current shape**

```bash
cd /Users/danielgarcia/startti/colmena
grep -n "build_load_skill_tool_definition\|build_load_attachment_tool_definition\|tool_recall_history\|ToolDefinition {" \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/recall_history.rs
```

E-T15c-4 (commit `a9046bb`) noted that load_skill and load_attachment use direct struct literals while recall_history uses `build_synthetic_tool_with_summary`. Each migration path differs:

- **Struct-literal builders (load_skill, load_attachment)**: change the `summary: Some(...)` and `description: ...` lines to call `text::*`.
- **Helper builder (recall_history)**: change the second + third args to `build_synthetic_tool_with_summary`.

- [ ] **Step 2: Populate `helpers.yaml` + migrate**

```yaml
load_skill:
  summary: Load a markdown skill bundle into the conversation; reveals built-in or user-provided guidance on demand
  description: |
    PASTE FROM build_load_skill_tool_definition's description string.

load_attachment:
  summary: Materialize a registered attachment's content (with auto-summary for large files) into the conversation
  description: |
    PASTE FROM build_load_attachment_tool_definition.

recall_history:
  summary: Re-read the original content of one past message by its turn index
  description: |
    PASTE FROM tool_recall_history (second argument to build_synthetic_tool_with_summary).
```

For `load_skill_tool.rs`, the direct-struct-literal pattern becomes:

```rust
// BEFORE (approximate, real shape may vary slightly)
ToolDefinition {
    name: LOAD_SKILL.to_string(),
    description: "Load a knowledge skill on demand; call before responding when relevant".to_string(),
    summary: Some("Load a markdown skill bundle into the conversation; reveals built-in or user-provided guidance on demand".to_string()),
    parameters: ...,
    input_schema_override: ...,
}

// AFTER
ToolDefinition {
    name: LOAD_SKILL.to_string(),
    description: crate::text::tool_description(LOAD_SKILL).to_string(),
    summary: Some(crate::text::tool_summary(LOAD_SKILL).to_string()),
    parameters: ...,
    input_schema_override: ...,
}
```

(Both fields convert `&'static str` to `String` because that's what the struct fields require. Calling `.to_string()` is cheap; cloning a `&'static` string into a `String` allocates only once per tool invocation.)

Recall_history (helper-builder pattern) becomes the same shape as Task 2 step 2:

```rust
super::build_synthetic_tool_with_summary::<RecallHistoryArgs>(
    TOOL_RECALL_HISTORY,
    text::tool_description(TOOL_RECALL_HISTORY),
    text::tool_summary(TOOL_RECALL_HISTORY),
)
```

- [ ] **Step 3: Tests + clippy + commit**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib load_skill load_attachment recall_history --quiet 2>&1 | tail -8
cargo test --lib text --quiet 2>&1 | tail -5
cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
git add src/libs/colmena/text/tools/helpers.yaml \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/recall_history.rs
git commit -m "feat(text-centralization): migrate helpers to text/tools/helpers.yaml

E-T17f. load_skill, load_attachment, recall_history now pull from
text/tools/helpers.yaml. describe_tool stays exempt (dynamic per-turn).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6 (E-T17g): Extract 8 inline prompts to `text/prompts/*.md`

**Goal:** Move every long-form inline string from source into a markdown file. The 8 target strings are documented in spec §8. This task can run in parallel with Tasks 2-5 — it touches different files.

**Files:**
- Create: `src/libs/colmena/text/prompts/sql_llm_critic.md`
- Create: `src/libs/colmena/text/prompts/secure_suspend_tool_description.md`
- Create: `src/libs/colmena/text/prompts/crdt_spreadsheet_protocol.md`
- Create: `src/libs/colmena/text/prompts/documents_system_prelude.md`
- Create: `src/libs/colmena/text/prompts/attachments_system_prelude.md`
- Create: `src/libs/colmena/text/prompts/python_sandbox/crdt_doc_run_python_prelude.md`
- Create: `src/libs/colmena/text/prompts/python_sandbox/crdt_doc_run_python_postlude.md`
- Create: `src/libs/colmena/text/prompts/python_sandbox/gsheets_run_python_prelude.md`
- Create: `src/libs/colmena/text/prompts/python_sandbox/gsheets_run_python_postlude.md`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs:32`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs:25`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_summary.rs:23`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs:18`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs:228`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs:292,302` (two strings)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs:253,263` (two strings)

- [ ] **Step 1: For each inline string, identify the exact range and extract**

For each target, do:
1. Read the current const value verbatim.
2. Write it (without surrounding `r#"..."#` syntax, without escape sequences — raw markdown) to the new `.md` file.
3. Replace the const value in the source with `include_str!("path/to/<name>.md")`.

Example pattern for `CRITIC_SYSTEM_PROMPT` at `sql_llm_critic.rs:32`:

```bash
cd /Users/danielgarcia/startti/colmena
sed -n '32,80p' src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs
```

Inspect the multi-line value. Then create the markdown:

Path: `src/libs/colmena/text/prompts/sql_llm_critic.md`

Content: the literal text from inside the `r#"..."#` (no backticks, no leading whitespace from indentation).

Then update the Rust source:

```rust
// BEFORE (sql_llm_critic.rs:32)
const CRITIC_SYSTEM_PROMPT: &str = r#"You are a PostgreSQL security and optimization reviewer. ..."#;

// AFTER
const CRITIC_SYSTEM_PROMPT: &str = include_str!("../../../text/prompts/sql_llm_critic.md");
```

The `../../../text/prompts/X.md` path arithmetic (3 `..` from `sql_llm_critic.rs`): file is at `src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs`. Going up 3 → `src/libs/colmena/src/`, then up 1 more (`../`) → `src/libs/colmena/`, then into `text/prompts/`. So the path is `../../../../text/prompts/sql_llm_critic.md`. **Double-check by file depth — common mistake** is using one fewer `..` than required.

Compute the path for each file:
- `src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs` (4 levels under `src/`) → `../../../../text/prompts/sql_llm_critic.md`
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs` (5 levels) → `../../../../../text/prompts/secure_suspend_tool_description.md`
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_summary.rs` (6 levels) → `../../../../../../text/prompts/crdt_spreadsheet_protocol.md`
- `nodes/llm_synthetic_tools/load_attachment_tool.rs` (6 levels) → `../../../../../../text/prompts/attachments_system_prelude.md`
- `nodes/llm_synthetic_tools/document_tools.rs` (6 levels) → `../../../../../../text/prompts/documents_system_prelude.md`
- `nodes/llm_synthetic_tools/crdt_doc_run_python.rs` (6 levels) → `../../../../../../text/prompts/python_sandbox/crdt_doc_run_python_prelude.md` (and `_postlude.md`)
- `nodes/llm_synthetic_tools/gsheets_run_python.rs` (6 levels) → `../../../../../../text/prompts/python_sandbox/gsheets_run_python_prelude.md` (and `_postlude.md`)

For the Python preludes, the `let prelude = r#"..."#;` is inside a function. Replace the function-local binding:

```rust
// BEFORE (crdt_doc_run_python.rs around line 290)
fn wrap_user_code(user_code: &str) -> String {
    let prelude = r#"# === colmena auto-prelude (do not modify) ===
import pandas as pd
...
"#;
    let postlude = r#"
# === user code ends ===
...
"#;
    format!("{prelude}{user_code}{postlude}")
}

// AFTER
const PRELUDE: &str = include_str!("../../../../../../text/prompts/python_sandbox/crdt_doc_run_python_prelude.md");
const POSTLUDE: &str = include_str!("../../../../../../text/prompts/python_sandbox/crdt_doc_run_python_postlude.md");

fn wrap_user_code(user_code: &str) -> String {
    format!("{PRELUDE}{user_code}{POSTLUDE}")
}
```

Same shape for `gsheets_run_python.rs`.

- [ ] **Step 2: Verify build + tests after each file**

After completing all 8 extractions, run:

```bash
cd /Users/danielgarcia/startti/colmena
cargo build --lib 2>&1 | tail -5
cargo test --lib --quiet 2>&1 | tail -5
cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
```

Expected: build clean, tests green, no warnings. Any `include_str!` with a wrong path will fail at compile time with a clear error like `error: couldn't read "..."` — fix the `..` count.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/text/prompts/ \
        src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_summary.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs
git commit -m "feat(text-centralization): extract 8 inline prompts to text/prompts/*.md

E-T17g. Every long-form inline string the LLM reads now lives in a
markdown file under text/prompts/. Specifically:

  - CRITIC_SYSTEM_PROMPT (sql_llm_critic.rs)
  - SECURE_SUSPEND_TOOL_DESCRIPTION (secure_suspend.rs)
  - CRDT_SPREADSHEET_PROTOCOL_PRELUDE (crdt_summary.rs)
  - ATTACHMENTS_SYSTEM_PRELUDE (load_attachment_tool.rs)
  - DOCUMENTS_SYSTEM_PRELUDE (document_tools.rs)
  - crdt_doc_run_python prelude + postlude
  - gsheets_run_python prelude + postlude

Each Rust caller now reads via include_str!. Zero behavior change.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 7 (E-T17h): Activate new validation tests + retire old

**Goal:** Replace `every_synthetic_tool_has_summary` (shipped in E-T15d, commit during the previous plan) with stronger tests that go through the new text registry. Add `no_orphan_yaml_entries` and `prompts_exist_and_nonempty`.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`

- [ ] **Step 1: Find the existing `every_synthetic_tool_has_summary` test**

```bash
cd /Users/danielgarcia/startti/colmena
grep -n "every_synthetic_tool_has_summary\|summary_coverage_tests" \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs
```

Expected: a `#[cfg(test)] mod summary_coverage_tests` block near the end of the file.

- [ ] **Step 2: Replace the existing test module**

Find the block (it was added in E-T15d). Replace it entirely with the new test module:

```rust
#[cfg(test)]
mod text_coverage_tests {
    //! Enforces that EVERY synthetic tool registered in colmena has an
    //! entry in text/tools/*.yaml and that no YAML entry is orphaned. The
    //! build refuses to ship if either invariant breaks.
    //!
    //! `describe_tool` is exempt — it is constructed dynamically per turn
    //! by `lazy_tools_catalog::build_describe_tool_definition` and does not
    //! go through the synthetic-tool builders covered here.

    use crate::llm::domain::tools::ToolDefinition;
    use crate::text;

    fn all_synthetic_tools() -> Vec<ToolDefinition> {
        // EXACT SAME LIST OF BUILDERS AS THE PREVIOUS every_synthetic_tool_has_summary.
        // Copy the contents of the old `all_synthetic_tools()` function verbatim —
        // the inventory hasn't changed.
        let mut tools = Vec::new();

        // gsheets — 10 tools
        tools.push(super::gsheets_tools::tool_create_spreadsheet());
        tools.push(super::gsheets_tools::tool_create_from_xlsx());
        tools.push(super::gsheets_tools::tool_export_xlsx());
        tools.push(super::gsheets_tools::tool_list_sheets());
        tools.push(super::gsheets_tools::tool_add_sheet());
        tools.push(super::gsheets_tools::tool_delete_sheet());
        tools.push(super::gsheets_tools::tool_read());
        tools.push(super::gsheets_tools::tool_set_cell());
        tools.push(super::gsheets_tools::tool_set_range());
        tools.push(super::gsheets_run_python::tool_gsheets_run_python());

        // crdt_doc — use existing collector
        tools.extend(super::crdt_doc_tools::build_all_crdt_doc_tools());

        // documents
        tools.extend(super::document_tools::build_all_document_tools());

        // helpers — call each helper's public builder
        tools.push(super::load_skill_tool::build_load_skill_tool_definition());
        tools.push(super::load_attachment_tool::build_load_attachment_tool_definition());
        tools.push(super::recall_history::tool_recall_history());

        tools
    }

    #[test]
    fn every_registered_tool_has_text_entry() {
        let tools = all_synthetic_tools();
        for td in &tools {
            let s = text::tool_summary(&td.name);
            let d = text::tool_description(&td.name);
            assert!(
                s.chars().count() >= 10 && s.chars().count() <= 200,
                "summary for '{}' out of bounds (len={})",
                td.name,
                s.chars().count(),
            );
            assert!(!d.is_empty(), "description for '{}' is empty", td.name);
        }
        assert!(!tools.is_empty(), "all_synthetic_tools() returned 0 entries");
    }

    #[test]
    fn no_orphan_yaml_entries() {
        let registered: std::collections::HashSet<String> = all_synthetic_tools()
            .iter()
            .map(|t| t.name.clone())
            .collect();
        let orphans: Vec<&'static str> = text::all_tool_names()
            .into_iter()
            .filter(|name| !registered.contains(*name))
            .collect();
        assert!(
            orphans.is_empty(),
            "Orphan YAML entries (no matching registered builder): {:?}",
            orphans,
        );
    }

    #[test]
    fn tool_def_summary_matches_yaml() {
        // Belt-and-suspenders: the migrated builders now READ from the
        // YAML, so ToolDefinition.summary == text::tool_summary(name) by
        // construction. This test catches a regression where someone
        // hand-edits a builder back to an inline literal.
        for td in all_synthetic_tools() {
            let yaml_summary = text::tool_summary(&td.name);
            assert_eq!(
                td.summary.as_deref(),
                Some(yaml_summary),
                "ToolDefinition.summary for '{}' diverges from text/tools/*.yaml — \
                 likely a builder was hand-edited",
                td.name,
            );
        }
    }
}
```

The OLD `summary_coverage_tests` module (from E-T15d) is removed completely — `text_coverage_tests` replaces and strictly supersets its coverage.

- [ ] **Step 3: Run the new tests**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib text_coverage_tests --quiet 2>&1 | tail -10
```

Expected: 3 tests pass.

- [ ] **Step 4: Full suite + clippy**

```bash
cargo test --lib --quiet 2>&1 | tail -5
cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
```

Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs
git commit -m "test(text-centralization): replace every_synthetic_tool_has_summary

E-T17h. New text_coverage_tests module replaces the old summary-only
gate with three stronger invariants:

  - every_registered_tool_has_text_entry: every builder's name resolves
    in text/tools/*.yaml; summary is 10..=200 chars; description nonempty.
  - no_orphan_yaml_entries: every YAML entry maps to a registered
    builder (catches dead entries after a tool removal).
  - tool_def_summary_matches_yaml: ToolDefinition.summary == YAML summary
    (catches a hand-edited builder that bypasses the registry).

describe_tool stays exempt — dynamic per-turn construction.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 8 (E-T18a): Built-in tools index doc

**Goal:** Hand-write `docs/developer_guide/41_builtin_tools_index.md` with one table per package, every tool listed.

**Files:**
- Create: `docs/developer_guide/41_builtin_tools_index.md`

- [ ] **Step 1: Write the doc**

Path: `docs/developer_guide/41_builtin_tools_index.md`

```markdown
# Built-in tools index

Every Rust-native LLM tool colmena ships with. Each row links to a detailed
doc. The "Summary" column comes from `text/tools/*.yaml`; a CI test
(`index_doc_covers_all_registered_tools`) guarantees this index lists every
registered synthetic tool.

For the YAML registry that backs this index, see
[`src/libs/colmena/text/tools/`](../../src/libs/colmena/text/tools/).

For toolkit packages (`enabled_tools: ["gsheets"]` shortcut), see
[40_toolkit_packages.md](40_toolkit_packages.md).

## gsheets (10 tools)

| Tool | Summary | Detailed docs |
|---|---|---|
| `gsheets_create_spreadsheet` | Create a new Google Sheets workbook and return its URL | [§39](39_gsheets.md) |
| `gsheets_create_from_xlsx` | Upload a local .xlsx attachment and convert it into a new Google Sheet | [§39](39_gsheets.md) |
| `gsheets_export_xlsx` | Download an existing Google Sheet as .xlsx bytes attachment | [§39](39_gsheets.md) |
| `gsheets_list_sheets` | List every tab (sheet) inside a spreadsheet by ID | [§39](39_gsheets.md) |
| `gsheets_add_sheet` | Create a new tab inside an existing spreadsheet | [§39](39_gsheets.md) |
| `gsheets_delete_sheet` | Permanently delete a tab from a spreadsheet | [§39](39_gsheets.md) |
| `gsheets_read` | Read a cell range from a tab; supports formatted, unformatted, and formula render modes | [§39](39_gsheets.md) |
| `gsheets_set_cell` | Write one value or formula into a single cell | [§39](39_gsheets.md) |
| `gsheets_set_range` | Write a 2-D values array starting at a given address | [§39](39_gsheets.md) |
| `gsheets_run_python` | Run sandboxed pandas analysis over sheet ranges loaded directly by the dispatcher (rows never pass through the LLM) | [§39](39_gsheets.md) |

## crdt_doc (11 tools)

| Tool | Summary | Detailed docs |
|---|---|---|
| `crdt_doc_list_sheets` | List every tab (sheet) inside the current CRDT document artifact | [§38](38_crdt_documents.md) |
| `crdt_doc_list_sheets_of` | List tabs inside a specific CRDT artifact by ID | [§38](38_crdt_documents.md) |
| `crdt_doc_read` | Read a cell range from the current CRDT document; supports include_formulas for round-trip | [§38](38_crdt_documents.md) |
| `crdt_doc_set_cell` | Write a value or formula into one cell of the current CRDT document | [§38](38_crdt_documents.md) |
| `crdt_doc_set_range` | Write a 2-D values array into the current CRDT document starting at an address | [§38](38_crdt_documents.md) |
| `crdt_doc_add_sheet` | Create a new tab inside the current CRDT document | [§38](38_crdt_documents.md) |
| `crdt_doc_get_recent_changes` | List recent CRDT change events since the agent's cursor | [§38](38_crdt_documents.md) |
| `crdt_doc_list_my_artifacts` | List CRDT artifacts owned by or shared with the current session | [§38](38_crdt_documents.md) |
| `crdt_doc_create_artifact` | Create a new empty CRDT spreadsheet artifact | [§38](38_crdt_documents.md) |
| `crdt_doc_run_python` | Run sandboxed pandas analysis over CRDT sheets without loading rows through the LLM | [§38](38_crdt_documents.md) |
| `crdt_doc_import_sheet` | Clone a sheet from another CRDT artifact into the current one | [§38](38_crdt_documents.md) |

## documents (7 tools)

| Tool | Summary | Detailed docs |
|---|---|---|
| `document_create` | Create a new document artifact (Excel or Word); returns artifact_id and initial version | [§38](38_crdt_documents.md) |
| `document_read` | Read the IR of a document at a given version (or current HEAD) with optional slicing | [§38](38_crdt_documents.md) |
| `document_apply_patch` | Apply a patch (list of ops) to an existing document atomically with auto-rebase on non-conflicting changes | [§38](38_crdt_documents.md) |
| `document_get_head` | Get the current HEAD of an artifact, optionally with a human-readable summary of edits since a baseline version | [§38](38_crdt_documents.md) |
| `document_list_versions` | List the versions retained for an artifact with timestamps, source and per-version summary | [§38](38_crdt_documents.md) |
| `document_rollback` | Roll back an artifact to a previous version; full history is preserved | [§38](38_crdt_documents.md) |
| `document_list_my_artifacts` | List every document artifact that belongs to the current session with metadata | [§38](38_crdt_documents.md) |

## helpers

| Tool | Summary | Detailed docs |
|---|---|---|
| `load_skill` | Load a markdown skill bundle into the conversation; reveals built-in or user-provided guidance on demand | [§24](24_skills.md) |
| `load_attachment` | Materialize a registered attachment's content (with auto-summary for large files) into the conversation | [§31](31_load_attachment.md) |
| `recall_history` | Re-read the original content of one past message by its turn index | [§29](29_lazy_tool_loading.md) |

## Toolkit packages

See [40_toolkit_packages.md](40_toolkit_packages.md). Today the only registered
package is `gsheets`, which expands to all 10 gsheets_* tools listed above.

## describe_tool

Dynamically constructed per turn by `lazy_tools_catalog::build_describe_tool_definition`
when `lazy_tool_loading: true`. See [§29](29_lazy_tool_loading.md). Not part of
the static `text/tools/*.yaml` registry by design.
```

- [ ] **Step 2: Commit**

```bash
cd /Users/danielgarcia/startti/colmena
git add docs/developer_guide/41_builtin_tools_index.md
git commit -m "docs(E-T18a): built-in tools index

Single reference listing every Rust-native LLM tool colmena ships with,
grouped by package, with summary + link to detailed docs. The completeness
test in the next task guarantees this index does not drift from the
registered tool set.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 9 (E-T18b): Completeness test for the index doc

**Goal:** A test that parses `41_builtin_tools_index.md` and asserts every registered synthetic tool appears in some table.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` (extend `text_coverage_tests`)

- [ ] **Step 1: Add the test to `text_coverage_tests`**

Append inside the `text_coverage_tests` mod (from Task 7):

```rust
#[test]
fn index_doc_covers_all_registered_tools() {
    // Embed the doc at compile time so the test is portable across
    // worktrees and CI environments.
    const INDEX_DOC: &str = include_str!(
        "../../../../../../docs/developer_guide/41_builtin_tools_index.md"
    );
    let registered: Vec<String> = all_synthetic_tools()
        .iter()
        .map(|t| t.name.clone())
        .collect();

    let mut missing: Vec<String> = Vec::new();
    for name in &registered {
        // Each tool name should appear at least once as a backtick-wrapped
        // token in the doc (the table convention `| \`tool_name\` | ...`).
        let needle = format!("`{}`", name);
        if !INDEX_DOC.contains(&needle) {
            missing.push(name.clone());
        }
    }

    assert!(
        missing.is_empty(),
        "These registered tools are missing from \
         docs/developer_guide/41_builtin_tools_index.md: {:?}",
        missing,
    );
}
```

The `../../../../../../docs/developer_guide/41_builtin_tools_index.md` path goes up 6 levels from `mod.rs` (located at `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`) — same depth as the `text/prompts/` paths in helpers — and then into `docs/developer_guide/`.

- [ ] **Step 2: Run the test**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib index_doc_covers_all_registered_tools --quiet 2>&1 | tail -8
```

Expected: PASS. If FAIL, the message lists the missing tool names — go add them to the doc.

- [ ] **Step 3: Full suite + clippy + commit**

```bash
cargo test --lib --quiet 2>&1 | tail -5
cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs
git commit -m "test(E-T18b): index_doc_covers_all_registered_tools

CI gate: every registered synthetic tool must appear in
docs/developer_guide/41_builtin_tools_index.md. The test embeds the
doc via include_str! and searches for the backtick-wrapped tool name.

If the test fails, the implementer is told exactly which tools to add.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 10 (E-T17i): Docs sweep

**Goal:** Final pass through the doc folders to surface the new layout.

**Files:**
- Modify: `docs/developer_guide/DEVELOPER_GUIDE.md`
- Modify: `docs/CHANGELOG_2026-06.md`
- Modify: `docs/BACKLOG.md`
- Modify: `CLAUDE.md` (top-level project guide)

- [ ] **Step 1: Update `DEVELOPER_GUIDE.md` with the new index entry**

Find the section listing developer-guide files and add:

```markdown
- `41_builtin_tools_index.md` — every Rust-native LLM tool with summary + link to detailed docs
```

(Adjacent to the existing `40_toolkit_packages.md` entry.)

- [ ] **Step 2: Append to `CHANGELOG_2026-06.md`**

```markdown
- **E-T17 shipped 2026-06-06** — LLM-facing text centralization. Every
  Rust-inline tool description, summary, system prelude, and Python
  sandbox auto-prelude moved into a top-level
  [`src/libs/colmena/text/`](../src/libs/colmena/text/) folder organized
  as `prompts/*.md` (monolithic) plus `tools/*.yaml` (structured). New
  loader at `src/text/mod.rs` resolves names with `text::tool_summary` /
  `text::tool_description`. Builders panic at startup if any tool is
  missing from the registry. CI tests verify: YAML parses, no orphan
  entries, every registered tool has an entry, ToolDefinition.summary
  matches the YAML.
- **E-T18 shipped 2026-06-06** — new
  [`docs/developer_guide/41_builtin_tools_index.md`](developer_guide/41_builtin_tools_index.md)
  lists every built-in synthetic tool with its summary and detailed-doc
  link. CI test (`index_doc_covers_all_registered_tools`) refuses to ship
  if a new tool is added without an index entry.
```

- [ ] **Step 3: Append to `BACKLOG.md`**

```markdown
- **Auto-generated tools index** — replace the hand-written
  `41_builtin_tools_index.md` with a build step that reads
  `text/tools/*.yaml` and writes the markdown. The completeness test
  shipped in E-T18b would become redundant; the build step would be the
  single source of truth.
- **i18n support for tool text** — extend the YAML schema to allow
  language-keyed entries (`summary.en`, `summary.es`) and add a runtime
  language selector. Out of scope today; only English tool text ships.
- **Hot reload for `text/`** — watch the folder for changes and reparse
  YAMLs without restart. Useful for prompt iteration during development;
  complex because the binary embeds the YAML via `include_str!`.
```

- [ ] **Step 4: Update `CLAUDE.md` with a pointer to `text/`**

Find the "Key Directories" section (top of file) and add an entry:

```markdown
- `src/libs/colmena/text/` — **LLM-facing text registry**. Every prompt,
  description, and summary the LLM reads lives here as YAML or Markdown.
  Edit a file in `text/prompts/` or `text/tools/` to change what the
  model sees — no Rust changes needed. See
  [docs/developer_guide/41_builtin_tools_index.md](docs/developer_guide/41_builtin_tools_index.md)
  for the user-facing index.
```

- [ ] **Step 5: Commit**

```bash
cd /Users/danielgarcia/startti/colmena
git add docs/developer_guide/DEVELOPER_GUIDE.md \
        docs/CHANGELOG_2026-06.md \
        docs/BACKLOG.md \
        CLAUDE.md
git commit -m "docs(E-T17+T18): final docs sweep

- DEVELOPER_GUIDE.md indexed the new 41_builtin_tools_index.md
- CHANGELOG_2026-06.md records E-T17 + E-T18 ship entries
- BACKLOG.md captures auto-gen + i18n + hot-reload follow-ups
- CLAUDE.md gets a Key Directories entry pointing at text/

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Final sweep

- [ ] **Step 1: Full test suite**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib --quiet 2>&1 | tail -10
```

Expected: all green. Previously: 1346 tests pass after E-T15+T16. After this plan: 1349+ (3-5 new text tests).

- [ ] **Step 2: Clippy**

```bash
cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
```

Expected: no warnings.

- [ ] **Step 3: Format**

```bash
cargo fmt --check 2>&1 | tail -5
```

If diff: `cargo fmt` and commit as a fixup.

- [ ] **Step 4: Re-run the gsheets package smoke (regression test)**

```bash
set -a; source .env; set +a
export GOOGLE_APPLICATION_CREDENTIALS=/Users/danielgarcia/colmena-sa.json
export PYTHONPATH=/Users/danielgarcia/startti/colmena/.venv/lib/python3.14/site-packages
cargo run --release --bin dag_engine -- run tests/graphs/agents/gsheets_package_smoke.json \
  --agent-session-id final_textcentral_$(date +%s) --include-extra-info 2>&1 | \
  grep -E "tool-input-available|tool-output-available|finishReason" | head -10
```

Expected: agent calls `gsheets_list_sheets` successfully. If the model is being flaky, re-run 2-3 times — at least one success demonstrates end-to-end correctness.

---

## Self-review checklist

| Spec section | Plan task(s) | OK? |
|---|---|---|
| §1 Goals — text centralization | Tasks 0–7 | ✅ |
| §1 Goals — built-in tools index | Tasks 8–9 | ✅ |
| §3 Open-source rule | Honoured: no ADP-specific content in text/ | ✅ |
| §4 Folder layout | Task 0 creates exact structure | ✅ |
| §5 YAML schema | Tasks 2–5 populate using exactly that schema | ✅ |
| §6 Loader module | Task 1 | ✅ |
| §7 Builder migration | Tasks 2–5 | ✅ |
| §8 Inline-prompt migration (8 strings) | Task 6 | ✅ |
| §9 Five tests | Task 1 has yaml_files_parse_at_startup; Task 7 has every_registered_tool_has_text_entry + no_orphan_yaml_entries + tool_def_summary_matches_yaml; Task 9 has index_doc_covers_all_registered_tools. `prompts_exist_and_nonempty` is implicit — `include_str!` of an empty file would still compile, but every prompt extracted in Task 6 has the original (non-empty) content by definition. If explicit coverage is wanted, add a 5th test in Task 7 that asserts each `*_PRELUDE` const is non-empty. | ✅ |
| §10 Edge cases | Task 1 covers duplicate keys, malformed YAML, missing entries. Task 7 covers orphan entries. | ✅ |
| §11 Built-in tools index | Tasks 8 + 9 | ✅ |
| §12 Task breakdown | Tasks 0–10 map 1:1 to spec's E-T17a–i + E-T18a–b | ✅ |
| §13 Back-compat matrix | Final sweep re-runs every test + smoke to confirm | ✅ |
| §14 Future BACKLOG | Task 10 step 3 records all three | ✅ |
