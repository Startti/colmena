# Built-in skills index

Every Rust-native LLM skill colmena ships with. Each row links to a detailed
guide. The descriptions below show what each skill teaches — when your agent
needs guidance on a topic, the LLM loads it via the `load_skill` tool.

For context on skills design and authoring, see
[§24 Skills](24_skills.md). For the tools equivalent, see
[§41 Built-in tools index](41_builtin_tools_index.md).

## The skills

| Skill | What it teaches | SKILL.md |
|---|---|---|
| `crdt-doc-cross-sheet-analysis` | Patterns for comparing two CRDT sheets, joining/enriching, transforming rows based on conditions from another sheet. | [SKILL.md](../../src/libs/colmena/skills/crdt-doc-cross-sheet-analysis/SKILL.md) |
| `crdt-doc-formulas` | Excel-style formulas in a CRDT spreadsheet — the `{v,f,fs}` cell schema, `include_formulas=true`, `needs_browser` warning handling. | [SKILL.md](../../src/libs/colmena/skills/crdt-doc-formulas/SKILL.md) |
| `crdt-doc-table-exploration` | Single-table patterns for CRDT documents — inspect schema first, top-N via nlargest, filters via query, type coercion, multi-tab output. | [link](../../src/libs/colmena/skills/crdt-doc-table-exploration/SKILL.md) |
| `crdt-doc-run-python` | Calling the `crdt_doc_run_python` tool — DataFrame shape rules, output protocol, type quirks, debugging. | [SKILL.md](../../src/libs/colmena/skills/crdt-doc-run-python/SKILL.md) |
| `data-run-python-recipes` | Calling the `data_run_python` tool — bindings + `output_tables`/`output_sheets`/`output_attachments` sinks, the four canonical recipes (spreadsheet→DB upsert, DB→file, cross-source join, sink modes/anti-patterns). 4 references on-demand (spreadsheet_to_db, db_to_file, cross_source_join, sinks_and_modes). | [SKILL.md](../../src/libs/colmena/skills/data-run-python-recipes/SKILL.md) |
| `expense-analysis` | Analyzing expense data — categories, vendor rollups, period comparisons. | [SKILL.md](../../src/libs/colmena/skills/expense-analysis/SKILL.md) |
| `gdocs-surgical-edits` | Calling any `gdocs_*` edit tool — scope/anchor discipline, multi-edit composition via `apply_edits`, what the surgical-edit errors mean, canonical style-change recipe. 5 references on-demand (replace_text_scoping, apply_edits_patterns, error_recovery, style_changes_pattern, before_after_examples). Auto-enrolled when any `gdocs_*` edit tool is in the catalog. | [SKILL.md](../../src/libs/colmena/skills/gdocs-surgical-edits/SKILL.md) |
| `gsheets-cross-sheet-analysis` | Same cross-sheet patterns as `crdt-doc-cross-sheet-analysis`, but via Google Sheets `gsheets_*` tools. | [SKILL.md](../../src/libs/colmena/skills/gsheets-cross-sheet-analysis/SKILL.md) |
| `gsheets-presentable-output` | Turning a written Google Sheet into a presentable report via `gsheets_format_range` — header bands, number/currency formats, palettes, multi-op composition. 5 references on-demand (recipe, layout, palettes, number_formats, multi_op_template). Auto-enrolled when `gsheets_format_range` is in the catalog. | [SKILL.md](../../src/libs/colmena/skills/gsheets-presentable-output/SKILL.md) |
| `gsheets-table-exploration` | Single-table patterns for Google Sheets — inspect schema first, top-N via nlargest, filters via query, type coercion, multi-tab output. | [link](../../src/libs/colmena/skills/gsheets-table-exploration/SKILL.md) |
| `gsheets-editing` | Write/edit decision guide — pick the right mechanism (`gsheets_set_cell`/`gsheets_set_range` vs `gsheets_run_python` `update_by_position`/`update_in_place`/`overwrite`/new-tab), edit rows with no unique key via `update_by_position`, write live formulas by column name (`{{Column}}`), create + populate sheets. 3 references (edit-rows, create-and-populate, cell-and-range-writes). Auto-enrolled when the agent has gsheets write tools. | [SKILL.md](../../src/libs/colmena/skills/gsheets-editing/SKILL.md) |
| `python-expert` | Modern Python (3.11+) — typing, async, dataclasses, stdlib internals. Not for general programming questions. | [SKILL.md](../../src/libs/colmena/skills/python-expert/SKILL.md) |
| `sales-analysis` | Analyzing sales data — common tables, KPIs, pitfalls. | [SKILL.md](../../src/libs/colmena/skills/sales-analysis/SKILL.md) |
| `sql-optimizer` | Writing, reviewing, or optimizing SQL — performance, indexes, joins, query plans. Not for ORM-specific questions. | [SKILL.md](../../src/libs/colmena/skills/sql-optimizer/SKILL.md) |
| `sql-query-best-practices` | Calling the `sql_query` tool — multi-statement patterns, bulk loads, common pitfalls, what is blocked and why. 6 references on-demand (multi_statement, bulk_insert, select_after_mutation, anti_patterns, schema_discovery, error_recovery). | [SKILL.md](../../src/libs/colmena/skills/sql-query-best-practices/SKILL.md) |

## How an LLM loads a skill

The `load_skill` tool is available to every LLM inside Colmena. When your agent
reaches a topic covered by a built-in skill, it can pull in the guidance:

```json
{
  "tool_name": "load_skill",
  "parameters": {
    "skill_name": "sql-optimizer"
  }
}
```

The skill's content (markdown from `SKILL.md`) appears in the conversation history,
and the LLM can reference it in subsequent turns. See [§24 Skills](24_skills.md)
for the full loading protocol and layered-context semantics.

## How to add a new skill

1. **Create the skill folder:** Add a directory under
   `src/libs/colmena/skills/<skill-name>/` with a hyphenated name matching your
   topic (e.g., `my-optimization-tips`).

2. **Write SKILL.md:** Create `SKILL.md` in that folder with the skill content in
   markdown format. Include a frontmatter block at the top:
   ```yaml
   ---
   skill_name: my-optimization-tips
   description: Tips for optimizing X
   ---
   ```
   Then write the body as markdown — code examples, patterns, common pitfalls, etc.

3. **Register in the runtime:** Add the skill name to the built-in skills list in
   `src/libs/colmena/src/llm/application/skill_loader.rs` (or wherever the runtime
   loads skills). Ensure the skill is discoverable by the `load_skill` tool.

4. **Document coverage:** Once registered, the skill appears in this index and is
   available to all LLMs. Update this file if adding a new built-in skill.

5. **Test the load:** Verify the LLM can call `load_skill` with your skill name and
   receives the markdown content as expected. Use a simple test graph with
   `load_skill` to confirm.

For user-provided skills (not built-in), operators declare them via
`tool_configurations.<name>.skills` and point the LLM to load via `load_skill`.
See [§24 Skills](24_skills.md) for the full authoring guide.
