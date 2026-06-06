# 39. Google Sheets integration (Subsystem E)

> v1 ships 9 synthetic LLM tools mirroring `crdt_doc_*` shape — agents
> read, write, create, and analyse Google Sheets via the Sheets API v4 +
> Drive API. Auth via Service Account JSON or Application Default
> Credentials. No OAuth user-scoped flow in v1.

## Recommended activation

Enable the whole gsheets surface with one alias:

```json
"enabled_tools": ["gsheets"]
```

This expands to all 10 gsheets tools via the toolkit-packages registry.
For a read-only-style agent, exclude write tools:

```json
"enabled_tools": [
  "gsheets",
  "!gsheets_delete_sheet",
  "!gsheets_add_sheet",
  "!gsheets_create_spreadsheet",
  "!gsheets_create_from_xlsx"
]
```

See [40_toolkit_packages.md](40_toolkit_packages.md) for the full syntax and exclusion semantics.

## Tool surface

| Tool | What it does |
|---|---|
| `gsheets_create_spreadsheet` | Create a new empty spreadsheet. Returns `{spreadsheet_id, url}`. |
| `gsheets_create_from_xlsx` | Upload an `.xlsx` attachment as a native Google Sheet (auto-conversion). **Deferred to E-T7b** (attachment plumbing pending); tool definition is published but calls error at runtime. |
| `gsheets_export_xlsx` | Download a Google Sheet as `.xlsx`, register as attachment. **Deferred to E-T7b** for the same reason. |
| `gsheets_list_sheets` | List tabs in a spreadsheet. |
| `gsheets_add_sheet` | Add a new tab. |
| `gsheets_delete_sheet` | Delete a tab by title or numeric id. |
| `gsheets_read` | Read cells; `value_render` controls formula vs evaluated; `as_records` controls 2D-array vs records shape. |
| `gsheets_run_python` | **Preferred for analysis.** Run sandboxed pandas/numpy/scipy code against one or more sheet ranges loaded server-side — rows NEVER pass through the LLM context. See section below. |
| `gsheets_set_cell` | Write one cell. Strings starting with `=` are evaluated by Google server-side. |
| `gsheets_set_range` | Bulk-write a rectangular block. Same formula semantics. |

UX aliases (per D-T16 lessons): `address` ↔ `addr`, `start` ↔ `start_addr`,
`values` ↔ `values_2d`, `name` ↔ `sheet`. Single-A1 ranges
auto-expanded (`"C1"` → `"C1:C1"`).

## Auth

Two paths, no app config required in colmena itself:

1. **Service Account JSON** — set `GOOGLE_APPLICATION_CREDENTIALS=/path/to/sa.json`.
   The SA email must be shared (Edit access) on each spreadsheet the
   agent touches. Best for unattended/automation use.
2. **Application Default Credentials** — when the env var is unset,
   `yup-oauth2` falls back to ADC: GCP metadata server in cloud
   environments, or `gcloud auth application-default login` for local
   dev.

Scopes: defaults to `spreadsheets` + `drive.file`. Override via
`COLMENA_GSHEETS_SCOPES=<comma-sep>` (short names or full URLs).

## Formulas — Google evaluates them

Unlike subsystem D (where colmena's `formula_engine` evaluates
spreadsheet formulas), Google Sheets evaluates `=...` formulas
server-side. Write a string starting with `=` to `gsheets_set_cell` /
`gsheets_set_range` and Google parses, evaluates, and cascades it. Read
back via `gsheets_read(..., value_render="UNFORMATTED_VALUE")` to get
the computed number, or `value_render="FORMULA"` to get the text.

## Pandas analysis flow

Same shape as `crdt_doc_*` analysis (subsystem F). Skill
`gsheets-cross-sheet-analysis` documents 6 patterns
(`pattern-a-cell-diff` through `pattern-f-conditional-transform`).

## Bulk analysis without LLM cost: `gsheets_run_python` (E-T14)

`gsheets_read` is fine for inspection (< 50 rows) but burns context
tokens at scale — 5000 rows ≈ 150k tokens just to feed pandas. The
`gsheets_run_python` tool reverses the flow: the LLM describes the
analysis as Python code, the dispatcher fetches every binding
**in parallel** server-side, runs the code in the existing sandbox
(same `execute_sandboxed_helper` used by `crdt_doc_run_python` and
`python_script`), and returns only `output` to the LLM.

```json
{
  "bindings": [
    {"var": "products", "spreadsheet_id": "<id>", "sheet": "Products"},
    {"var": "sales",    "spreadsheet_id": "<id>", "sheet": "Sales", "range": "A1:H5001"}
  ],
  "code": "import pandas as pd\nprods = pd.DataFrame(products)\nsold  = pd.DataFrame(sales)\nmerged = prods.merge(sold, on='sku', how='left')\noutput = merged.nlargest(5, 'qty')[['sku','name','qty']].to_dict('records')"
}
```

Mirrors `crdt_doc_run_python` (subsystem C, §5.6) — same prelude,
postlude, output / stdout / error caps (10 KB each), and 30-second
timeout. Differences vs the CRDT cousin:

- No `write_to_sheet` mode in v1 — write-back is still `gsheets_set_range`
  invoked separately.
- Each binding's records list is bound directly under the user-chosen
  `var` (the LLM calls `pd.DataFrame(<var>)` itself). The CRDT tool
  auto-builds `dfs[<sheet_id>]` because there's no per-binding name.
- Errors carry `loaded_columns: {<var>: [...]}` so the LLM can fix a
  `KeyError` without re-fetching.

Source: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs`.

## Hexagonal layout

- `src/libs/colmena/src/gsheets/domain/` — `SheetsClient` trait,
  value types, errors.
- `src/libs/colmena/src/gsheets/infrastructure/` — REST adapter
  (`http_client.rs`), auth (`auth.rs`), config (`config.rs`).
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs` —
  9 dispatchers.

## Out of scope for v1 (BACKLOG)

See "Subsystem E v1.1" in `docs/BACKLOG.md`: list_spreadsheets discovery,
OAuth user-scoped auth, cell formatting, charts, conditional formatting,
permissions / sharing, revisions, webhooks, plus the E-T7b xlsx
attachment plumbing.

## Spec + plan

- Spec: `docs/superpowers/specs/2026-06-05-google-sheets-design.md`
- Plan: `docs/superpowers/plans/2026-06-05-google-sheets.md`
