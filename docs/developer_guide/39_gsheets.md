# 39. Google Sheets integration (Subsystem E)

> v1 ships 9 synthetic LLM tools mirroring `crdt_doc_*` shape — agents
> read, write, create, and analyse Google Sheets via the Sheets API v4 +
> Drive API. Auth via Service Account JSON or Application Default
> Credentials. No OAuth user-scoped flow in v1.

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
