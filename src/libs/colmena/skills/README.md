# Built-in skills

Each subdirectory here compiles into the `colmena_dag_engine` crate at
build time via the `include_dir!` macro and becomes available to LLM
nodes as a built-in skill. Skills load on demand via the `load_skill`
synthetic tool — see [`docs/developer_guide/24_skills.md`](../../../docs/developer_guide/24_skills.md)
for the runtime contract.

## Quick navigation

| Skill folder | What it teaches |
|---|---|
| `crdt-doc-cross-sheet-analysis` | Cross-sheet CRDT analysis (joins, enrichment) |
| `crdt-doc-formulas` | Excel-style formulas in CRDT sheets |
| `crdt-doc-run-python` | Using `crdt_doc_run_python` correctly |
| `expense-analysis` | Expense data patterns |
| `gsheets-cross-sheet-analysis` | Cross-sheet Google Sheets analysis |
| `python-expert` | Modern Python typing/async/stdlib |
| `sales-analysis` | Sales data analysis |
| `sql-optimizer` | SQL performance + optimization |

The user-facing version of this list, with descriptions taken from each
skill's frontmatter, lives at
[`docs/developer_guide/42_builtin_skills_index.md`](../../../docs/developer_guide/42_builtin_skills_index.md).

## How to add a new skill

1. Create a directory named after the skill (kebab-case, e.g.
   `python-expert/`). The directory name is the skill's canonical name.
2. Add a `SKILL.md` file with YAML frontmatter:

   ```yaml
   ---
   name: python-expert          # MUST match the directory name
   description: One-line description suitable for the user-facing index.
   when_to_load: When to invoke this skill (helps the LLM auto-discover).
   references:
     - typing
     - async
   ---
   ```
3. For each entry in `references`, add `references/<name>.md` next to
   `SKILL.md`.
4. Add a row to the user-facing index doc at
   `docs/developer_guide/42_builtin_skills_index.md` and to the quick-nav
   table above.
5. Run `cargo test --lib index_doc_covers_all_registered_skills` — the
   build refuses to ship if your skill is in the registry but not in the
   user-facing index.
6. Keep each file under 64 KB (the `include_dir!` size cap).

## Naming convention

Skill folder names are kebab-case. Uniqueness is enforced at build time
by the `include_dir!` macro — two folders with the same name produce a
compile error.

## The `_placeholder` folder

The `_placeholder/` directory exists only to give `include_dir!` a
non-empty contract when no real skills are present in CI. It is exempt
from the user-facing index and the completeness test.

## Full contract

See [`docs/developer_guide/24_skills.md`](../../../docs/developer_guide/24_skills.md).
