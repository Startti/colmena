# CRDT Formulas — Design Spec (Subsystem D)

**Date:** 2026-06-04
**Status:** Draft → pending user review
**Subsystem:** D (Fórmulas) — part of CRDT collaborative editing MVP
**Prior art:** Subsystems B (recent changes), C (pandas/run_python), F (cross-sheet analysis) already shipped.

---

## 1. Problem statement

Today the backend stores cells as `{v, t}` only. Univer (frontend) loads
`UniverFormulaEnginePlugin` and evaluates formulas client-side, but the
backend has zero awareness of them:

- If an agent calls `crdt_doc_set_cell("B1", "=A1+5")`, the literal string
  `"=A1+5"` is persisted. Univer renders it as a formula on next paint
  (visually OK), but `crdt_doc_read("B1")` returns the **string** `"=A1+5"`
  to the agent (not the evaluated value `42`).
- If a user types `=SUM(A1:A10)` in Univer and the agent then reads that
  cell, it gets the formula text, not the sum.
- pandas (`run_python`) operating on the data gets the formula text,
  breaking any numeric analysis.

This blocks two real-world use cases:

- **Case A** — agent writes formulas a user can see evaluated (e.g. adds
  a "Total = =B2*C2" column).
- **Case B** — agent reads what a user has built with formulas
  (e.g. analyse a budget with `=SUM`, `=AVG`, `=IF`).

We need formulas to be a first-class data type in the backend, with
bidirectional read/write that's robust both for headless agent flows and
for human-driven Univer sessions.

---

## 2. Goals & non-goals

### Goals (v1)

- Detect formulas in `crdt_doc_set_cell` / `crdt_doc_set_range` writes.
- Evaluate them server-side via [`formualizer`](https://crates.io/crates/formualizer)
  (320+ Excel-compatible functions, MIT/Apache-2.0).
- Persist both the formula text (`f`) and the evaluated value (`v`).
- Recalculate dependent cells (same sheet) on every write.
- Let agents read evaluated values transparently (`crdt_doc_read` keeps
  returning scalars by default, preserving pandas compatibility).
- Let agents opt-in to seeing the formula text via
  `crdt_doc_read(include_formulas=true)`.
- Graceful fallback for functions outside formualizer's set
  (`fs="needs_browser"`).
- Excel-compatible error values (`#DIV/0!`, `#REF!`, `#CYCLE!`, `#NAME?`).
- Anti-divergence benchmark vs Univer to catch silent diffs early.

### Non-goals (deferred to v1.1 — listed in §11)

- Cross-artifact references `=[OtherWB]Sheet1!A1`.
- Cross-sheet eager recalc (intra-sheet only in v1).
- Array formulas (validate formualizer coverage in v1.1).
- Defined names (`=SalesTotal`).
- Explicit `crdt_doc_recalc(sheet?, all=true)` tool.

---

## 3. Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                     User / Agent                                  │
└────────┬──────────────────────────────────┬──────────────────────┘
         │ set_cell("A1", "=SUM(B:B)")     │ read(include_formulas)
         ▼                                  ▼
┌──────────────────────────────────────────────────────────────────┐
│  crdt_doc_set_cell / set_range / read   (synthetic LLM tools)    │
└────────┬─────────────────────────────────────────────────────────┘
         │
         ▼
┌──────────────────────────────────────────────────────────────────┐
│  crdt_documents::formula_engine   (NEW MODULE)                   │
│  ├─ parse(text) → AST | NeedsBrowser                             │
│  ├─ evaluate(ast, &impl CellResolver) → Value | EvalError        │
│  ├─ dependents_of(addr, sheet, doc) → Vec<Addr>                  │
│  └─ recalc_chain(changed_addr, sheet, doc) → ordered Vec<Addr>   │
└────────┬─────────────────────────────────────────────────────────┘
         │ uses
         ▼
┌──────────────────────────────────────────────────────────────────┐
│              formualizer crate (320+ fns, MIT/Apache)             │
└──────────────────────────────────────────────────────────────────┘
```

**Single new module:** `src/libs/colmena/src/crdt_documents/formula_engine.rs`
(~250-400 LOC). All other changes are surgical extensions to existing
files (`tool_executor.rs`, `df_writer.rs`, `projection.rs`, the synthetic
tool dispatchers in `crdt_doc_tools.rs`).

The `CellResolver` trait keeps `formula_engine` from leaking `yrs` types.
Production impl wraps a `&yrs::Doc + sheet_id`; tests use an in-memory
stub for unit-test isolation.

---

## 4. Data model

Cell schema in Y.Doc (extension is additive, fully back-compatible with
existing literal cells):

```
workbook.sheets[i].cells.<addr>:
  v   : Any         // always present — evaluated value or literal
  t   : u8          // always present — type tag: 1=str, 2=num, 3=bool, 4=error
  f   : String?     // present only if cell was written as a formula
  fs  : String?     // present only if f is present. Values:
                    //   "be"            → evaluated by formualizer (backend)
                    //   "fe"            → evaluated by Univer (frontend)
                    //   "needs_browser" → function outside formualizer's set;
                    //                     v holds the formula text as placeholder
```

### State transitions

| Trigger | f | fs | v |
|---|---|---|---|
| Literal write `set_cell("A1", 5)` | — (absent) | — | 5 |
| Formula write, backend-evaluable `set_cell("B1", "=SUM(A:A)")` | `"=SUM(A:A)"` | `"be"` | computed number |
| Formula write, browser-only fn `set_cell("C1", "=VLOOKUP(...)")` | `"=VLOOKUP(...)"` | `"needs_browser"` | `"=VLOOKUP(...)"` (placeholder) |
| Formula write, eval error `set_cell("D1", "=1/0")` | `"=1/0"` | `"be"` | `"#DIV/0!"`, t=4 |
| Browser user types formula | `"=..."` | `"fe"` | computed by Univer |
| pandas overwrites formula cell with literal | — (removed) | — (removed) | new literal |

### Why this design

- **Pandas compatibility default:** existing C subsystem reads
  `cells.<addr>.v` and gets a scalar. Adding `f`/`fs` keys to the cell
  doesn't change that — `df_records` already only reads `v`.
- **Univer compatibility:** `f` is the same key Univer uses internally
  for formula text in its cell schema. Round-trip works.
- **Honest opacity:** when we can't evaluate, the cell is explicitly
  marked `fs="needs_browser"` and `v` holds the formula text so any
  downstream that asks for `v` at least gets a non-null string they can
  inspect.

---

## 5. Component design

### 5.1 `formula_engine` module

```rust
pub struct ParsedFormula { /* wraps formualizer AST */ }

pub enum ParseOutcome {
    Ok(ParsedFormula),
    NeedsBrowser { unsupported_fns: Vec<String> },
    ParseError(String),
}

pub trait CellResolver {
    fn get(&self, sheet: &str, addr: &str) -> Option<CellSnapshot>;
    fn sheet_exists(&self, sheet: &str) -> bool;
}

pub struct CellSnapshot {
    pub v: serde_json::Value,
    pub t: u8,
}

pub enum EvalValue {
    Number(f64),
    String(String),
    Bool(bool),
    Error(ExcelError),  // "#DIV/0!", "#REF!", etc.
}

pub fn parse(text: &str) -> ParseOutcome;
pub fn evaluate(ast: &ParsedFormula, resolver: &impl CellResolver, current_sheet: &str)
    -> Result<EvalValue, EvalError>;
pub fn dependents_of(addr: &str, sheet: &str, resolver: &impl CellResolver) -> Vec<(String, String)>;
pub fn recalc_chain(changed: &str, sheet: &str, resolver: &impl CellResolver)
    -> Result<Vec<(String, String)>, CycleError>;
```

`recalc_chain` returns cells in **topological evaluation order** — caller
re-evaluates them sequentially and writes results back to the doc.
Only cells with a non-empty `f` are ever returned (literals are never in
the chain).

### 5.2 Yjs Doc extension (`tool_executor.rs`)

`apply_set_cell_in_proc` grows ~80 LOC of new logic in the formula path:

```rust
pub fn apply_set_cell_in_proc(doc: &Doc, sheet_id: &str, addr: &str, value: &Value)
    -> SetCellOutcome  // NEW: was unit return
```

`SetCellOutcome` carries `{cells_recalculated: usize, warnings: Vec<Warning>}`
so the tool dispatcher can surface it to the agent.

### 5.3 Tool dispatcher changes

| Tool | Change |
|---|---|
| `crdt_doc_set_cell` | Returns `{ok, cells_recalculated, warnings?}` instead of `{ok}`. Warnings include `{kind:"needs_browser", addr, function}` and `{kind:"eval_error", addr, error}`. |
| `crdt_doc_set_range` | Same plus aggregate `total_cells_recalculated` across all writes. Each cell in the range is parsed/evaluated individually (mixing literals and formulas is fine); the recalc-chain pass runs **once** after all cells in the batch are written, computing the union of dependents across every changed cell. |
| `crdt_doc_read` | New optional arg `include_formulas: bool = false`. When true, each cell value becomes `{v, f?, fs?}` instead of just the scalar `v`. Output shape is documented in the tool description. |
| `crdt_doc_list_sheets` | Each sheet entry adds `formula_count: usize` so the agent knows whether to bother with `include_formulas=true`. |

### 5.4 `df_writer` change

When writing back from pandas, `df_writer::apply_records_to_doc` checks
each target cell's prior state:

- Had `f` → emit CRDT event `{kind:"formula_replaced_by_literal", addr, prior_formula}` and remove `f`/`fs` from the cell map before writing the new `v`.
- Did not have `f` → no extra work.

Either way, `recalc_chain` runs at the end of the batch so any other
cells whose formulas referenced the overwritten addresses get refreshed.

---

## 6. Data flow

### 6.1 Formula write — happy path

```
1. agent → set_cell("Sheet1", "B5", "=SUM(B1:B4)")
2. tool dispatcher → apply_set_cell_in_proc
3. apply_set_cell_in_proc:
   a. detect leading "="
   b. formula_engine::parse("=SUM(B1:B4)")          → Ok(AST)
   c. check_supported_fns(AST)                       → all OK
   d. formula_engine::evaluate(AST, doc, "Sheet1")  → 42.0
   e. write {v:42, t:2, f:"=SUM(B1:B4)", fs:"be"}
   f. formula_engine::recalc_chain("B5", "Sheet1", doc) → [("C5",..)]
   g. for each dependent: re-evaluate + write {v, t}, emit CRDT event
4. tool result: {ok:true, cells_recalculated:1}
```

### 6.2 Formula write — needs_browser path

```
3c. check_supported_fns(AST) → unsupported: ["XLOOKUP"]
3d. SKIP evaluate
3e. write {v:"=XLOOKUP(...)", t:1, f:"=XLOOKUP(...)", fs:"needs_browser"}
3f. SKIP recalc_chain (we don't know the result, so no propagation)
4. tool result: {ok:true, warnings:[{kind:"needs_browser", addr:"B5", functions:["XLOOKUP"]}]}
```

### 6.3 Read — pandas mode (default)

```
agent → read("Sheet1")  // include_formulas defaults to false
→ projection.rs walks cells, yields cell.v as JSON scalar
→ [{"A1":5,"B5":42,"C5":52}, ...]
```

`df_records` (used by `run_python`) keeps using this path. Zero change.

### 6.4 Read — formula-aware mode

```
agent → read("Sheet1", include_formulas=true)
→ projection.rs walks cells, yields:
  - {v: scalar}                       if no f
  - {v: scalar, f: text, fs: source}  if f present
→ [{"A1":{"v":5}, "B5":{"v":42,"f":"=SUM(B1:B4)","fs":"be"}}, ...]
```

### 6.5 Pandas write-back over formula

```
1. run_python computes df["B5"] = 100, calls df_writer::apply_records_to_doc
2. df_writer:
   a. read prior cell at B5 → has f=true, prior_formula="=SUM(B1:B4)"
   b. emit CRDT event {kind:"formula_replaced_by_literal", addr:"B5", prior_formula}
   c. write {v:100, t:2}  ← f and fs removed
3. recalc_chain("B5", "Sheet1") → [("C5",..)]  → re-evaluate dependents
```

---

## 7. Error handling

| Class | Detection | Surfaced to agent | Cell state |
|---|---|---|---|
| Parse error (`=SUM(`) | formualizer parser | `{ok:false, error:"parse error: ..."}` | NOT written |
| Unknown function | check_supported_fns(AST) | `{ok:true, warnings:[{kind:"needs_browser", functions}]}` | written with `fs:"needs_browser"` |
| Eval division by zero | formualizer eval | `{ok:true, eval_errors:[{addr, err:"#DIV/0!"}]}` | `{v:"#DIV/0!", t:4, f, fs:"be"}` |
| `#REF!` (sheet not found, etc.) | formualizer eval | same shape | `{v:"#REF!", t:4, f, fs:"be"}` |
| `#NAME?` (unknown name not a function) | parser | same shape | `{v:"#NAME?", t:4, f, fs:"be"}` |
| Cycle (A1 → B1 → A1) | `recalc_chain` topo-sort | `{ok:true, warnings:[{kind:"cycle", chain:["A1","B1","A1"]}]}` | each cell in cycle: `{v:"#CYCLE!", t:4}` |
| Cross-sheet ref to missing sheet | formualizer + `CellResolver::sheet_exists` | same as `#REF!` | `{v:"#REF!", t:4, f, fs:"be"}` |

Excel error semantics keep Univer rendering them visually as red error
chips, so what the agent sees and what the user sees stay aligned.

---

## 8. Testing strategy

### 8.1 Unit tests — `formula_engine`

- Parse: ~10 tests (literal, simple ref, range, cross-sheet ref, fn call, nested, error cases).
- Evaluate: ~15 tests (arithmetic, SUM/AVG/COUNT/IF/CONCAT, type coercion, error propagation, missing cell as 0/empty).
- `dependents_of`: ~6 tests (no formulas, 1 dependent, chain, cross-sheet ref returns nothing for v1).
- `recalc_chain`: ~6 tests (linear chain, branch, diamond, cycle returns CycleError).
- `CellResolver` stub: 1 reference impl used by all eval/recalc tests.

### 8.2 Anti-divergence benchmark ⭐

`tests/formula_divergence.rs` — `#[ignore]`-gated (requires Playwright + Chromium installed):

1. ~80 fixture formulas across families: arithmetic, SUM family,
   logical, text, lookup, date, statistical.
2. For each: evaluate via formualizer; evaluate via headless Univer
   (paste into a test sheet via Playwright, read back); diff.
3. Build fails if any diff appears in the v1-supported function list.
4. Diffs for v1.1-deferred functions are logged but not fatal (just
   document them in BACKLOG).

CI runs this nightly; PRs run a fast subset (~10 representative).

### 8.3 Integration tests

`tests/graphs/agents/crdt_doc_formulas.json` — DAG graph with 5 scenarios:

1. Agent creates sheet, writes data, writes `=SUM` formula, reads result.
2. Agent reads sheet with `include_formulas=true`, verifies shape.
3. Agent changes input cell, verifies dependent formula recalculates.
4. Agent writes `=XLOOKUP(...)` (browser-only), reads it back, sees `needs_browser` warning.
5. pandas overwrites formula cell, verifies CRDT event + dependent recalc.

### 8.4 Browser smoke (manual)

- Open Univer.
- Agent runs graph #1 above.
- Confirm: cells visible with correct values, formula bar shows `=SUM(B1:B4)` on B5.

---

## 9. Performance budget

- `parse`: <1ms per formula (formualizer is fast; cached AST in v1.1 if needed).
- `evaluate`: <2ms for typical formulas (deep aggregations bounded by O(range_size)).
- `recalc_chain`: <10ms for sheets with 100 formulas, <100ms for 10K.
- `set_cell` end-to-end: <20ms target including persistence/CRDT propagation.

Anything exceeding budgets becomes a v1.1 optimization issue.

---

## 10. Migration / back-compat

| Concern | Impact |
|---|---|
| Existing cells (literals) | Unchanged — `f`/`fs` keys simply absent. `df_records` keeps reading `v` and ignoring everything else. |
| Existing graphs that pass `=...` strings expecting literal storage | **BREAKING**: those strings now get parsed and evaluated. Mitigation: pre-flight scan of `tests/graphs/` (no current graph does this). For external users: the leading `=` is the Excel convention; if someone needs a literal `=text`, they prefix with `'` (Excel convention). Document in CHANGELOG and BACKLOG. |
| ADP worker | Tool result shape changes (`set_cell` adds `cells_recalculated` + `warnings`). Verify worker doesn't break on extra keys — defensive parsing expected (and previously confirmed during F-T15). |
| Univer frontend | `fs` key is new — Univer will ignore unknown keys. To get `fs:"fe"` on user-typed formulas we add a small client hook (~30 lines in `index.html`). Optional for v1 — without it, user formulas show `fs:undefined` and backend treats them as "evaluated by someone other than backend, trust v". |

---

## 11. Out-of-scope (BACKLOG entries for v1.1)

1. **Cross-sheet eager recalc** — today refs to `Sheet2!A1` evaluate
   correctly on write but if `Sheet2!A1` later changes, dependents in
   `Sheet1` don't auto-update. Workaround: explicit `recalc` tool.
2. **`crdt_doc_recalc(sheet?, all=true)`** — explicit refresh tool for
   the cross-sheet stale case and post-import scenarios.
3. **Cross-artifact references** `='[OtherWB.xlsx]Sheet1'!A1`.
4. **Array formulas** `{=SUM(A1:A10*B1:B10)}` — validate formualizer
   array semantics, design UI for spilled results.
5. **Defined names** `=SalesTotal`.
6. **AST caching** — keep parsed AST per cell to skip re-parse on recalc.
7. **Univer-side `fs:"fe"` hook** — currently user-typed formulas have
   `fs:undefined`. The 30-line client hook is trivial but not required.

---

## 12. Implementation task preview

| ID | Task | Est. LOC |
|---|---|---:|
| D-T1 | Add `formualizer = "0.6"` to Cargo.toml + minimal smoke (parse+eval 2+2) | 50 |
| D-T2 | `formula_engine` module: parse, evaluate, `CellResolver` trait | 250 |
| D-T3 | `dependents_of` + `recalc_chain` (topo-sort with cycle detection) | 150 |
| D-T4 | Anti-divergence benchmark suite (80 fixtures, Playwright + Univer) | 200 |
| D-T5 | Extend `apply_set_cell_in_proc` to detect formulas + run recalc | 100 |
| D-T6 | Extend `crdt_doc_read` with `include_formulas` flag (projection layer) | 60 |
| D-T7 | Extend `list_sheets` with `formula_count` | 40 |
| D-T8 | `df_writer`: remove `f`/`fs` on overwrite + emit CRDT event | 50 |
| D-T9 | Skill `crdt-doc-formulas` with 3 patterns (write, read-evaluated, needs_browser) | docs |
| D-T10 | Integration test graph `crdt_doc_formulas.json` (5 scenarios) | 150 |
| D-T11 | Docs: dev guide §5.8, node_configurations.json updates, BACKLOG, CHANGELOG | docs |
| D-T12 | Final sweep: cargo test + clippy + fmt + browser smoke | — |

**Total: ~1100 LOC + tests + docs. ~3-5 days subagent-driven.**

---

## 13. Open questions for reviewer

None at design time — all major decisions explicit in §2 (scope), §4
(schema), §5 (components), §6 (flows), §7 (errors). Edge cases noted in
§11 (out-of-scope) and §10 (back-compat).

If reviewer wants to lock down any v1.1 item earlier (e.g. cross-sheet
recalc), it's a scope adjustment — move row from §11 to §12 and the plan
estimate grows by ~150-300 LOC.
