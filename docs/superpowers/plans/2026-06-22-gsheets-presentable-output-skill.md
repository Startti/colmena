# Rich Formatting By Default — `gsheets-presentable-output` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Google Sheets agents produce presentable formatting by default — via an always-on nudge in the `gsheets_format_range` tool description plus a deep, auto-enrolled built-in skill `gsheets-presentable-output`.

**Architecture:** Two prongs. (1) Enrich the always-in-prompt tool description (moves the default with zero model discretion). (2) A built-in skill (compiled via `include_dir!`, same shape as `gdocs-surgical-edits`) auto-enrolled into the load-on-demand catalog whenever `gsheets_format_range` is in the agent's catalog (mirrors the `agent_has_gdocs_edit_tools` gate). No change to the `gsheets_format_range` code.

**Tech Stack:** Rust (`colmena_dag_engine`), `include_dir!` built-in skills, the Skills infra (`BuiltinSkillRepository`, frontmatter parser), YAML text registry, `serde_json` for the enroll gate.

**Spec:** `docs/superpowers/specs/2026-06-22-gsheets-presentable-output-skill-design.md`

---

## File Structure

- `src/libs/colmena/skills/gsheets-presentable-output/SKILL.md` — **new** (overview + frontmatter with 5 references).
- `src/libs/colmena/skills/gsheets-presentable-output/references/01-recipe.md` … `05-layout.md` — **new** (5 reference files).
- `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs` — add a `gsheets_presentable_output_is_loadable` test (mirror `gdocs_surgical_edits_is_loadable`).
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` — add `agent_has_gsheets_format_tool` helper + auto-enroll block + gate tests.
- `src/libs/colmena/text/tools/gsheets.yaml` — enrich the `gsheets_format_range` description (always-on nudge).
- `docs/developer_guide/39_gsheets.md`, `docs/CHANGELOG_2026-06.md`, `docs/BACKLOG.md` — docs.
- `tests/graphs/agents/gsheets_presentable_report_e2e.json` — **new** open-prompt E2E graph.

**Note on include_dir:** built-in skills are auto-compiled from `skills/` via `include_dir!("$CARGO_MANIFEST_DIR/skills")` — a new dir needs NO registration, but it only appears in an agent's catalog when its name is in `skills_config.builtin` (Task 2 wires the gate).

---

## Task 1: The built-in skill (SKILL.md + 5 references)

**Files:**
- Create: `src/libs/colmena/skills/gsheets-presentable-output/SKILL.md`
- Create: `src/libs/colmena/skills/gsheets-presentable-output/references/{01-recipe,02-palettes,03-number-formats,04-multi-op-template,05-layout}.md`
- Test: `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs` (add a loadable test)

- [ ] **Step 1: Write `SKILL.md`** (frontmatter `references` use the `name:`/`description:` object shape, like `gdocs-surgical-edits`):

```markdown
---
name: gsheets-presentable-output
description: Use when calling gsheets_format_range to make a sheet presentable. Covers the data→formulas→format order, ready-to-use color palettes, number-format patterns (currency/%/date), a full multi-op template, and layout rules. Load the reference for your scenario.
references:
  - name: recipe
    description: Step-by-step recipe for a professional report. The correct order is data → formulas → format (format LAST, over the populated range). Read this first.
  - name: palettes
    description: Ready-to-use hex palettes that look good — header blue/dark-gray, white header text, subtle zebra, light-gray totals row. Copy these instead of inventing colors.
  - name: number_formats
    description: numberFormat patterns — currency ($#,##0), percent (0.0%), date, thousands. When to use each, with exact pattern strings.
  - name: multi_op_template
    description: A COMPLETE ops:[...] JSON for a typical report (title, header, currency on amounts, totals row, table borders, column widths). Copy and adapt the ranges.
  - name: layout
    description: Layout rules — text left / numbers right, column widths, table borders, separating the totals row, optional zebra striping.
---

# gsheets — Presentable output best practices

Quick rules:

1. **Format is separate from values.** Write data + formulas first with
   `gsheets_set_range` / `gsheets_set_cell` (formulas start with `=`), THEN
   style with `gsheets_format_range`. Never expect format to change values.
2. **One multi-op call.** Send all the formatting as a single
   `gsheets_format_range` with several `ops` — header, currency, borders,
   totals row, widths — not one call per attribute.
3. **Always make human-facing output presentable by default**, even when the
   user did not ask for formatting explicitly. A bare grid of numbers is not
   an acceptable deliverable.

Load the reference that matches your task: start with `recipe` for the full
flow, `multi_op_template` to copy a ready ops payload, `number_formats` for the
exact pattern strings, `palettes` for colors, `layout` for alignment/widths.
```

- [ ] **Step 2: Write `references/01-recipe.md`**:

```markdown
# Recipe — a professional report, end to end

Order matters. Do these in sequence:

1. **Plan the layout.** Decide the columns and where the header row, data
   rows, and totals row will land (e.g. header row 1, data rows 2..N, totals
   row N+1).
2. **Write values + formulas** with `gsheets_set_range` (USER_ENTERED — a
   string starting with `=` becomes a formula). Use `=SUM(...)` for totals,
   not hardcoded numbers.
3. **Format LAST**, in ONE `gsheets_format_range` multi-op call, over the now-
   populated ranges:
   - Header row: bold, white text, dark background, centered, bottom border.
   - Numeric columns: a `number_format` (currency / percent / date as fits).
   - Whole table: thin borders on all cells.
   - Totals row: bold, light-gray background, top border to separate it.
   - Column widths: label column wider, numeric columns even.
4. **Report** the spreadsheet URL.

See `multi_op_template` for a copy-paste ops payload, `palettes` for colors,
`number_formats` for pattern strings.
```

- [ ] **Step 3: Write `references/02-palettes.md`**:

```markdown
# Palettes — colors that look good

Use these instead of inventing colors. All hex `#RRGGBB`.

| Role | Hex | Notes |
|---|---|---|
| Header background | `#1F4E78` | dark blue; pair with white text `#FFFFFF` |
| Header background (alt) | `#434343` | dark gray; also pairs with white text |
| Header text | `#FFFFFF` | always use a light text on the dark header |
| Totals row background | `#D9D9D9` | light gray; keep text default (dark) |
| Zebra stripe (odd rows) | `#F3F3F3` | very subtle gray; optional, for long tables |
| Accent / positive | `#1E8E3E` | green, e.g. for highlighting a key total |

Rule of thumb: ONE dark header color + white header text + (optional) a subtle
zebra + a light totals row. Don't use more than ~3 colors in one table.
```

- [ ] **Step 4: Write `references/03-number-formats.md`**:

```markdown
# Number formats — exact pattern strings

Set via `format.number_format: { type, pattern }` in a `gsheets_format_range` op.

| Data | type | pattern | Renders |
|---|---|---|---|
| Money (whole) | `CURRENCY` | `$#,##0` | `$1,234` |
| Money (cents) | `CURRENCY` | `$#,##0.00` | `$1,234.56` |
| Percent | `PERCENT` | `0.0%` | `12.3%` (value `0.123`) |
| Thousands | `NUMBER` | `#,##0` | `1,234` |
| Date | `DATE` | `yyyy-mm-dd` | `2026-06-22` |
| Plain integer | `NUMBER` | `0` | `1234` |

Apply to the NUMERIC range only (e.g. the amounts block `B2:F8`), not to label
columns. Percent expects the underlying value as a fraction (0.123 → 12.3%).
```

- [ ] **Step 5: Write `references/04-multi-op-template.md`** (a complete, copyable ops payload):

````markdown
# Multi-op template — copy and adapt the ranges

A typical report: title row 1, header row 3, data rows 4-7, totals row 8,
columns A..F. Adapt the ranges to your sheet, then send as ONE call:

```json
{
  "spreadsheet_id": "<id>",
  "ops": [
    { "sheet": "<tab>", "range": "A1:F1",
      "format": { "text": { "bold": true, "font_size": 16 }, "horizontal_alignment": "CENTER" } },
    { "sheet": "<tab>", "range": "A3:F3",
      "format": { "text": { "bold": true, "color": "#FFFFFF" }, "background_color": "#1F4E78",
                  "horizontal_alignment": "CENTER",
                  "borders": { "bottom": { "style": "SOLID_THICK" } } } },
    { "sheet": "<tab>", "range": "B4:F8",
      "format": { "number_format": { "type": "CURRENCY", "pattern": "$#,##0" } } },
    { "sheet": "<tab>", "range": "A3:F8",
      "format": { "borders": { "top": {"style":"SOLID"}, "bottom": {"style":"SOLID"},
                  "left": {"style":"SOLID"}, "right": {"style":"SOLID"},
                  "inner_horizontal": {"style":"SOLID"}, "inner_vertical": {"style":"SOLID"} } } },
    { "sheet": "<tab>", "range": "A8:F8",
      "format": { "text": { "bold": true }, "background_color": "#D9D9D9",
                  "borders": { "top": { "style": "SOLID_THICK" } } } },
    { "sheet": "<tab>", "range": "A3:A8", "format": { "column_width_px": 150 } },
    { "sheet": "<tab>", "range": "B3:F8", "format": { "column_width_px": 100 } }
  ]
}
```

This is non-destructive: it does not touch values/formulas, and each op's
`fields` mask only changes the attributes you set.
````

- [ ] **Step 6: Write `references/05-layout.md`**:

```markdown
# Layout rules

- **Alignment:** text/labels left, numbers right (`horizontal_alignment`).
  Center only the header row.
- **Column widths:** the label column wider (~140-160px), numeric columns
  even (~90-110px). Set via `column_width_px` over the column's range.
- **Borders:** thin `SOLID` on the whole data block; use a `SOLID_THICK`
  bottom border under the header and a `SOLID_THICK` top border above the
  totals row to separate sections.
- **Totals row:** bold + light-gray background + top border. Keep numbers
  right-aligned and in the same number format as the data.
- **Zebra (optional, long tables):** subtle `#F3F3F3` background on odd data
  rows — only when the table is long enough to need row tracking.
- Don't over-format: one header color, optional zebra, one totals highlight.
```

- [ ] **Step 7: Add the loadable test** in `builtin_skill_repository.rs` (mirror `gdocs_surgical_edits_is_loadable` ~line 366):

```rust
    #[tokio::test]
    async fn gsheets_presentable_output_is_loadable() {
        let repo = BuiltinSkillRepository::new(&["gsheets-presentable-output".to_string()]).unwrap();
        let skill = repo.load_skill("gsheets-presentable-output").await.unwrap();
        assert_eq!(skill.name, "gsheets-presentable-output");
        assert!(skill.body.contains("Quick rules"), "body should contain the quick-rules section");
        assert_eq!(skill.references.len(), 5, "expected 5 references, got {}", skill.references.len());
        let names: Vec<String> = skill.references.iter().map(|r| r.name.clone()).collect();
        for expected in &["recipe", "palettes", "number_formats", "multi_op_template", "layout"] {
            assert!(names.contains(&expected.to_string()), "missing reference {expected}; got {names:?}");
        }
        // each reference resolves to a body
        for r in &skill.references {
            let body = repo.load_skill_reference("gsheets-presentable-output", &r.name).await.unwrap();
            assert!(!body.trim().is_empty(), "reference {} empty", r.name);
        }
    }
```

(Confirm the exact reference-loading method name by reading the sibling test — if it's `load_skill("name", Some("ref"))` rather than `load_skill_reference`, use that form. Match reality.)

- [ ] **Step 8: Run the test**

Run: `cargo test --lib gsheets_presentable_output_is_loadable 2>&1 | tail -15`
Expected: PASS (skill compiles via include_dir, frontmatter parses, 5 refs resolve).

- [ ] **Step 9: Check the builtin-names enumeration test.** Some repos have a test like `available_builtin_names_includes_authored_skills` (~builtin_skill_repository.rs:252) that asserts the set of compiled skill names. Run it; if it hardcodes the expected list, add `gsheets-presentable-output`:

Run: `cargo test --lib builtin_skill 2>&1 | tail -15`
Expected: PASS (update the expected-names list if that test fails on the new dir).

- [ ] **Step 10: Commit**

```bash
git add src/libs/colmena/skills/gsheets-presentable-output src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs
git commit -m "feat(gsheets): gsheets-presentable-output built-in skill (recipe + 5 references)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Auto-enroll gate in `llm.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (add helper + enroll block + tests)

- [ ] **Step 1: Write failing gate tests** in the enrollment test module (next to `gdocs_alias_in_enabled_tools_triggers_enrollment` ~line 5533). They use the same `empty_inputs()` helper:

```rust
    #[tokio::test]
    async fn gsheets_alias_triggers_format_skill_enrollment() {
        let cfg = json!({ "enabled_tools": ["gsheets"] });
        assert!(LlmNode::agent_has_gsheets_format_tool(&cfg, &empty_inputs()));
    }
    #[tokio::test]
    async fn explicit_format_tool_triggers_enrollment() {
        let cfg = json!({ "enabled_tools": ["gsheets_format_range"] });
        assert!(LlmNode::agent_has_gsheets_format_tool(&cfg, &empty_inputs()));
    }
    #[tokio::test]
    async fn wildcard_triggers_format_skill_enrollment() {
        assert!(LlmNode::agent_has_gsheets_format_tool(&json!({ "enabled_tools": "*" }), &empty_inputs()));
    }
    #[tokio::test]
    async fn gsheets_read_only_does_not_trigger_format_skill() {
        let cfg = json!({ "enabled_tools": ["gsheets_read", "gsheets_run_python"] });
        assert!(!LlmNode::agent_has_gsheets_format_tool(&cfg, &empty_inputs()));
    }
    #[tokio::test]
    async fn excluded_format_tool_does_not_trigger() {
        // gsheets alias present but format tool explicitly excluded
        let cfg = json!({ "enabled_tools": ["gsheets", "!gsheets_format_range"] });
        assert!(!LlmNode::agent_has_gsheets_format_tool(&cfg, &empty_inputs()));
    }
    #[tokio::test]
    async fn tool_configurations_format_entry_triggers_enrollment() {
        let cfg = json!({ "tool_configurations": { "gsheets_format_range": {} } });
        assert!(LlmNode::agent_has_gsheets_format_tool(&cfg, &empty_inputs()));
    }
```

- [ ] **Step 2: Run — expect FAIL** (`agent_has_gsheets_format_tool` undefined):

Run: `cargo test --lib gsheets_alias_triggers_format 2>&1 | tail -10`
Expected: FAIL (cannot find function).

- [ ] **Step 3: Implement the gate helper** next to `agent_has_gdocs_edit_tools` (~line 456). It checks for `gsheets_format_range` availability, honoring the `!gsheets_format_range` exclusion (a refinement over the gdocs gate):

```rust
    /// True when the agent's resolved tool catalog will contain
    /// `gsheets_format_range` — used to auto-enroll the
    /// `gsheets-presentable-output` skill. Honors `!gsheets_format_range`
    /// exclusions (so an agent that opts the tool out does NOT get the skill).
    pub(super) fn agent_has_gsheets_format_tool(config: &Value, inputs: &NodeInputs) -> bool {
        const FORMAT_TOOL: &str = "gsheets_format_range";
        let enabled = inputs
            .get("enabled_tools")
            .or_else(|| config.get("enabled_tools"));
        let raw_names: Vec<&str> = match enabled {
            Some(Value::String(s)) => vec![s.as_str()],
            Some(Value::Array(arr)) => arr.iter().filter_map(|v| v.as_str()).collect(),
            _ => Vec::new(),
        };
        // Explicit exclusion wins over any alias/wildcard.
        if raw_names.iter().any(|n| *n == "!gsheets_format_range") {
            return false;
        }
        for n in &raw_names {
            if n.starts_with('!') {
                continue;
            }
            if *n == "*" || *n == "gsheets" || *n == FORMAT_TOOL {
                return true;
            }
        }
        if let Some(Value::Object(tc)) = config.get("tool_configurations") {
            if tc.keys().any(|k| k == FORMAT_TOOL) {
                return true;
            }
        }
        false
    }
```

- [ ] **Step 4: Wire the auto-enroll block** right after the existing `gdocs-surgical-edits` enroll block (~line 618):

```rust
        // Auto-enroll the `gsheets-presentable-output` builtin skill whenever
        // the agent can call `gsheets_format_range`. Pairs with the always-on
        // nudge in the tool description: the nudge shifts the default, the
        // skill teaches the full presentable-report recipe on demand.
        if Self::agent_has_gsheets_format_tool(config, inputs)
            && !skills_config
                .builtin
                .iter()
                .any(|n| n == "gsheets-presentable-output")
        {
            skills_config
                .builtin
                .push("gsheets-presentable-output".to_string());
        }
```

- [ ] **Step 5: Run — expect PASS** (gate tests + no regressions):

Run: `cargo test --lib agent_has_gsheets_format_tool 2>&1 | tail; cargo test --lib triggers_format 2>&1 | tail -12`
Expected: all PASS.

- [ ] **Step 6: Run the broader enrollment suite + lib**

Run: `cargo test --lib 2>&1 | tail -12`
Expected: green (no regression in the gdocs enrollment tests or elsewhere).

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(gsheets): auto-enroll gsheets-presentable-output when format tool is in catalog

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: Always-on nudge in the tool description

**Files:**
- Modify: `src/libs/colmena/text/tools/gsheets.yaml` (the `gsheets_format_range` entry)

- [ ] **Step 1: Enrich the `description`.** Read the current `gsheets_format_range` entry (added in the §47 work) and APPEND a presentable-by-default block + compact example. Keep the existing content; add (Spanish, matching the file):

```yaml
    Por DEFAULT, cuando generes una hoja para que la vea una persona, aplicá
    formato presentable en UNA llamada multi-op (sin esperar a que te lo pidan):
    encabezado en negrita con fondo y texto contrastante + centrado, formato de
    número en las columnas numéricas (moneda `$#,##0`, `%`, o fecha según
    corresponda), bordes finos en la tabla, fila de totales destacada (negrita +
    fondo + borde superior) y anchos de columna razonables. Una grilla de números
    sin formato no es una entrega aceptable. (Para la receta completa + paletas +
    plantilla de ops, cargá la skill `gsheets-presentable-output` vía load_skill.)
    Ejemplo de ops presentables:
    ops: [
      { sheet:"Hoja1", range:"A1:F1", format:{ text:{bold:true,color:"#FFFFFF"}, background_color:"#1F4E78", horizontal_alignment:"CENTER" } },
      { sheet:"Hoja1", range:"B2:F8", format:{ number_format:{ type:"CURRENCY", pattern:"$#,##0" } } },
      { sheet:"Hoja1", range:"A8:F8", format:{ text:{bold:true}, background_color:"#D9D9D9", borders:{ top:{style:"SOLID_THICK"} } } }
    ]
```

(Place it inside the existing `description: |` block, after the current attribute list, before any closing. Preserve YAML indentation exactly — it's a block scalar.)

- [ ] **Step 2: Verify the YAML parses**

Run: `cargo test --lib text 2>&1 | tail -8`
Expected: PASS (text-registry validation; the enriched description loads).

- [ ] **Step 3: Build (the description is `include_str!`/registry-loaded — confirm no break)**

Run: `cargo build --lib 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/text/tools/gsheets.yaml
git commit -m "feat(gsheets): presentable-by-default nudge in gsheets_format_range description

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: Docs + sweep + live E2E (success criterion) + PR — controller-run

**Files:**
- Create: `tests/graphs/agents/gsheets_presentable_report_e2e.json`
- Modify: `docs/developer_guide/39_gsheets.md`, `docs/CHANGELOG_2026-06.md`, `docs/BACKLOG.md`

- [ ] **Step 1: Write the open-prompt E2E graph** `tests/graphs/agents/gsheets_presentable_report_e2e.json` (nodes-as-map; default LLM stack; `enabled_tools: ["gsheets"]`). The prompt gives data + a business goal but **NO formatting instructions** — the success criterion is that rich formatting now happens by default:

```json
{
  "_comment": "E2E (manual, live): OPEN prompt — agent builds a sales report and should produce presentable formatting BY DEFAULT (currency, borders, totals row) without being told how, thanks to the always-on nudge + auto-enrolled gsheets-presentable-output skill. Needs GEMINI_API_KEY + DATABASE_URL + OAuth creds in-memory + --agent-session-id.",
  "nodes": {
    "trigger": { "type": "trigger_webhook", "config": { "path": "/gsheets-presentable", "method": "POST",
      "test_payload": { "prompt": "Armá una hoja de cálculo de Google con un reporte de ventas trimestrales por región y pasámela lista para mostrar. Datos (USD): Norte Q1 125000 Q2 138000 Q3 142500 Q4 165000; Sur Q1 98000 Q2 102000 Q3 110000 Q4 121000; Este Q1 156000 Q2 149000 Q3 168000 Q4 180000; Oeste Q1 87000 Q2 95000 Q3 99000 Q4 108000. Incluí total por región y total general." } } },
    "agent": { "type": "llm_call", "config": { "provider": "google", "api_key": "${GEMINI_API_KEY}", "model": "gemini-2.5-flash", "stream": false, "max_iterations": 18, "connection_url": "${DATABASE_URL}", "system_message": "Sos un analista que arma reportes en Google Sheets. Escribí valores/fórmulas y entregá la hoja lista para mostrar. Reportá la URL.", "enabled_tools": ["gsheets"] } },
    "log": { "type": "log" }
  },
  "edges": [ { "from": "trigger", "to": "agent" }, { "from": "agent", "to": "log" } ]
}
```

Note: the system_message does NOT mention formatting — the nudge/skill must drive it. Validate JSON: `python3 -c "import json;json.load(open('tests/graphs/agents/gsheets_presentable_report_e2e.json'));print('ok')"`.

- [ ] **Step 2: Dev guide §39.** Add a short note in the `gsheets_format_range` / formatting section: rich formatting is encouraged by default (the tool description nudges it and the `gsheets-presentable-output` skill auto-enrolls when the format tool is available; load it via `load_skill` for the full recipe).

- [ ] **Step 3: CHANGELOG.** Add the next section (check the last `## N.` — as of now §47 → use §48). Spanish. Cover: the presentable-by-default nudge in the tool description + the new `gsheets-presentable-output` built-in skill (5 references) auto-enrolled when `gsheets_format_range` is in the catalog (mirrors the gdocs-surgical-edits gate); no tool code change; additive/no ADP impact. Reference spec + plan + E2E graph.

- [ ] **Step 4: BACKLOG.** Mark the "Formato rico por default + mini-skill de presentable output" item (Subsystem E v1.1) `[x]` SHIPPED with date + §48 pointer.

- [ ] **Step 5: Commit docs**

```bash
git add tests/graphs/agents/gsheets_presentable_report_e2e.json docs/developer_guide/39_gsheets.md docs/CHANGELOG_2026-06.md docs/BACKLOG.md
git commit -m "docs(gsheets): presentable-output skill — dev guide, CHANGELOG §48, BACKLOG, E2E graph

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Step 6: Full sweep (CI parity — clippy IS a separate gate, run it):**

```bash
cargo fmt --check && cargo clippy --all-targets 2>&1 | grep -E "^error|^warning:" ; cargo test --verbose 2>&1 | tail -25
```
Expected: fmt clean, clippy 0 errors/warnings, tests exit 0.

- [ ] **Step 7: ADP additive sweep.** Confirm additive-only — only `skills/`, `gsheets.yaml`, a new helper + enroll block in `llm.rs`, docs. No `EngineConfig`/`ColmenaEngine`/exported-trait change → ADP unaffected. (Quick grep of the diff vs the base.)

- [ ] **Step 8: Live E2E — THE SUCCESS CRITERION (controller, secrets in-memory).** Per memory `feedback_always_real_e2e` + `ref_local_gsheets_e2e_runbook`: rebuild the release binary (`cargo build --release --bin dag_engine`), inject OAuth creds from Secret Manager in-memory (no echo/file/commit), run `tests/graphs/agents/gsheets_presentable_report_e2e.json` with `--agent-session-id`, save SSE to `/tmp/colmena_e2e/gsheets_presentable.sse`. **Read back the resulting sheet** and assert rich formatting landed WITHOUT the prompt asking for it: currency number format on amounts, header bold+bg, totals row highlighted, borders. Present a friendly report. **If formatting is still minimal**, iterate the nudge/skill wording (this is part of the E2E acceptance, not a new task) and re-run until an open prompt yields presentable output. Clean up the demo spreadsheet after.

- [ ] **Step 9: Push + open PR + watch CI**

```bash
git push -u origin <branch>
gh pr create --base develop --title "feat(gsheets): rich formatting by default (presentable-output skill + nudge)" --body "..."
gh pr checks <n> --watch
```

---

## Self-Review Notes (author)

- **Spec coverage:** Component 1 (always-on nudge) → Task 3. Component 2 (skill + 5 references) → Task 1. Component 3 (auto-enroll gate keyed on `gsheets_format_range`, honoring `!` exclusion) → Task 2. Testing (skill loadable, gate unit tests, text valid, live E2E open-prompt) → Tasks 1/2/3/4. No-objetivos (no gdocs, no tool-code change) respected — no task touches `gsheets_tools.rs`/`format.rs`. Cross-repo additive → Task 4 step 7.
- **Type/name consistency:** skill name `gsheets-presentable-output` and reference names (`recipe`, `palettes`, `number_formats`, `multi_op_template`, `layout`) identical in SKILL.md frontmatter (Task 1), the loadable test (Task 1 step 7), and the enroll push (Task 2 step 4). Gate fn `agent_has_gsheets_format_tool` defined in Task 2 step 3, called in step 4 + tests step 1. Tool name `gsheets_format_range` consistent across gate + nudge.
- **Verify-during-impl flags (use existing names):** confirm the reference-loading method in the loadable test (`load_skill(name, Some(ref))` vs `load_skill_reference`) against the sibling `gdocs_surgical_edits_is_loadable`; confirm whether a builtin-names enumeration test needs the new name added (Task 1 step 9); confirm `empty_inputs()`/`NodeInputs`/`Value` are in scope in the enrollment test module (they are — used by the gdocs gate tests). Confirm the latest CHANGELOG number before writing §48 (Task 4 step 3).
- **Success criterion is behavioral (Task 4 step 8):** unlike prior features, "done" requires the open-prompt E2E to actually produce rich formatting; the plan explicitly allows wording iteration within that step.
