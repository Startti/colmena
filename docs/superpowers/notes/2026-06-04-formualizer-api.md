# Formualizer 0.6 — Verified API surface (D-T1 spike)

**Status:** confirmed by `cargo test -p colmena_dag_engine --lib formualizer_parses_and_evaluates_2_plus_2` (smoke test ran green 2026-06-04 against `formualizer = "0.6.0"` resolved as `0.6.0` from crates.io).

**Audience:** the D-T2..D-T13 tasks (and anyone touching `crdt_documents::formula_engine`). Use the signatures below as ground truth — do NOT re-invent them from `claude knows X`.

## 1. Dependency declaration

In `src/libs/colmena/Cargo.toml`:

```toml
# Formula engine (D-T1 spike onwards)
formualizer = "0.6"
```

**No extra feature flags needed.** The meta-crate's `default` feature set is `["portable-wasm", "json", "csv", "system-clock"]`, where `portable-wasm` already pulls in `eval`, `workbook`, `sheetport`, `parse`, and `common`. All four code paths exercised in the smoke compile and run under the default feature set.

The dependency chain looks like:

- `formualizer 0.6.0` (meta) re-exports
  - `formualizer-parse 2.0.0` (note: parse jumped to 2.x while the meta is still 0.6.x)
  - `formualizer-eval 0.6.0`
  - `formualizer-workbook 0.6.0`
  - `formualizer-common 2.0.0`
  - `formualizer-sheetport 0.6.0`

## 2. Parse — AST-only

### Entry function (preferred — Excel dialect)

```rust
pub fn formualizer::parse::parser::parse<T: AsRef<str>>(formula: T)
    -> Result<formualizer::ASTNode, formualizer::parse::parser::ParserError>;
```

Internally calls `parse_with_dialect(formula, FormulaDialect::Excel)`. Re-exported as `formualizer::parse_with_dialect` at the meta-crate root (also as `Parser` for the streaming/batch variant).

### Returned AST type

```rust
pub struct formualizer::ASTNode { /* ... */ }                  // re-exported from formualizer_parse
pub enum   formualizer::ASTNodeType { Function, Reference, Literal, ... }
```

`ASTNode` exposes `.node_type` (an `ASTNodeType`). For walking dependents (D-T3), match on `ASTNodeType::Reference { reference: ReferenceType, .. }` and recurse over `node.children` (field on `ASTNode`).

### Parser error type

```rust
pub struct formualizer::parse::parser::ParserError {
    pub message: String,
    pub position: Option<usize>,
}
impl std::error::Error for ParserError {}
```

Plain struct, no enum variants — match on the `message` string only when you need to distinguish error kinds. The richer enum is `formualizer::parse::types::ParsingError` but it's typically wrapped inside `ParserError::message` rather than returned directly.

### Canonical formula (AST → text round-trip)

```rust
pub fn formualizer::canonical_formula(ast: &ASTNode) -> String;
```

Useful for debugging and for serializing a normalized formula back into a Y.Doc cell.

## 3. Evaluate — high-level path (Workbook)

The fastest way to get end-to-end behaviour during D-T2 prototyping. The smoke uses this; D-T4 will replace `Workbook` with a custom `EvaluationContext` backed by `&yrs::Doc`.

```rust
use formualizer::{Workbook, LiteralValue};

let mut wb = Workbook::new();
if !wb.has_sheet("Sheet1") {
    wb.add_sheet("Sheet1").expect("add Sheet1");           // ExcelError on failure
}
wb.set_formula("Sheet1", /*row*/ 1, /*col*/ 1, "=2+2").unwrap();
let value: LiteralValue = wb.evaluate_cell("Sheet1", 1, 1).unwrap();
assert_eq!(value, LiteralValue::Number(4.0));
```

Signatures (from `formualizer-workbook 0.6.0`):

```rust
impl Workbook {
    pub fn new() -> Self;
    pub fn has_sheet(&self, name: &str) -> bool;
    pub fn add_sheet(&mut self, name: &str) -> Result<(), ExcelError>;

    pub fn set_formula(
        &mut self,
        sheet: &str,
        row: u32,        // 1-based
        col: u32,        // 1-based, Excel-style (A=1)
        formula: &str,   // must start with '='
    ) -> Result<(), ExcelError>;

    pub fn evaluate_cell(
        &mut self,
        sheet: &str,
        row: u32,
        col: u32,
    ) -> Result<LiteralValue, ExcelError>;

    pub fn get_value(&self, sheet: &str, row: u32, col: u32) -> Option<LiteralValue>;
    // batch variants: set_formulas, evaluate_cells, evaluate_cells_cancellable
}
```

The 1-line convenience helper for tests/docs is:

```rust
pub fn formualizer::doc_examples::eval_scalar(formula: &str)
    -> Result<LiteralValue, Box<dyn std::error::Error + Send + Sync>>;
```

## 4. Evaluate — low-level path (Interpreter + EvaluationContext)

This is the path D-T4 (YrsResolver) will use, because we cannot afford to copy every Y.Doc cell into a `Workbook` on every recalc.

### The trait the host implements

```rust
pub trait formualizer::eval::traits::EvaluationContext:
    Resolver + FunctionProvider + SourceResolver { /* default fns ... */ }

// Super-trait Resolver = ReferenceResolver + RangeResolver
//                       + NamedRangeResolver + TableResolver

pub trait ReferenceResolver: Send + Sync {
    fn resolve_cell_reference(
        &self,
        sheet: Option<&str>,
        row: u32,
        col: u32,
    ) -> Result<LiteralValue, ExcelError>;
}

pub trait RangeResolver: Send + Sync {
    fn resolve_range_reference(
        &self,
        sheet: Option<&str>,
        sr: Option<u32>, sc: Option<u32>,
        er: Option<u32>, ec: Option<u32>,
    ) -> Result<Box<dyn formualizer::eval::traits::Range>, ExcelError>;
}

pub trait NamedRangeResolver: Send + Sync {
    fn resolve_named_range_reference(&self, name: &str)
        -> Result<Vec<Vec<LiteralValue>>, ExcelError>;
}

pub trait TableResolver: Send + Sync {
    fn resolve_table_reference(
        &self,
        tref: &formualizer::parse::parser::TableReference,
    ) -> Result<Box<dyn formualizer::eval::traits::Table>, ExcelError>;
}

pub trait FunctionProvider: Send + Sync {
    fn get_function(&self, ns: &str, name: &str)
        -> Option<std::sync::Arc<dyn formualizer::eval::function::Function>>;
}
```

For D-T4's `YrsResolver`, the MVP impl needs at minimum:

- `ReferenceResolver::resolve_cell_reference` → read a single Y.Doc cell value.
- `RangeResolver::resolve_range_reference` → materialise into `Box<dyn Range>` (use `formualizer::eval::traits::InMemoryRange::new(vec_of_rows)`).
- `NamedRangeResolver::resolve_named_range_reference` → return `Err(ExcelError::new(ExcelErrorKind::Name))` until D-T11.
- `TableResolver::resolve_table_reference` → same, return a `Name` error.
- `FunctionProvider::get_function` → delegate to the built-in registry: `formualizer::eval::function_registry::get(ns, name)`.

The default `EvaluationContext` blanket trait covers the rest (cancellation, thread pool, locale, sheet indexing).

### Evaluator entry point

```rust
use formualizer::eval::interpreter::Interpreter;
use formualizer::eval::traits::CalcValue;

let interp = Interpreter::new(&resolver /* &dyn EvaluationContext */, "Sheet1");
let value: CalcValue = interp.evaluate_ast(&ast)?;          // CalcValue<'a>
// or, with the current cell so volatile/INDIRECT functions resolve correctly:
let interp = Interpreter::new_with_cell(&resolver, "Sheet1", cell_ref);
```

`CalcValue` is an enum with `Scalar(LiteralValue)`, `Range(...)`, `Callable(...)`. For our use case (single-cell formula → single value), call `value.into_literal()` or `value.as_scalar()`.

## 5. Cell reference types

From `formualizer-eval 0.6.0` (re-exported via `formualizer::eval::reference::*`):

```rust
pub type SheetId = u16;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct CellRef {
    pub sheet_id: SheetId,
    pub coord: Coord,
}
impl CellRef {
    pub const fn new(sheet_id: SheetId, coord: Coord) -> Self;
    pub fn new_absolute(sheet_id: SheetId, row: u32, col: u32) -> Self;
}

pub struct Coord { /* row, col, row_abs, col_abs (1-based) */ }
impl Coord { pub fn new(row: u32, col: u32, row_abs: bool, col_abs: bool) -> Self; }
```

The parse-time reference flavour is `formualizer::ReferenceType` (re-exported from `formualizer_parse`):

```rust
pub enum ReferenceType {
    Cell { sheet: Option<String>, row: u32, col: u32, row_abs: bool, col_abs: bool },
    Range { sheet: Option<String>, start_row: Option<u32>, start_col: Option<u32>,
            end_row: Option<u32>, end_col: Option<u32>, /* ... */ },
    NamedRange(String),
    Table(formualizer::parse::parser::TableReference),
    External(/* ... */),
    Cell3D { /* ... */ },
    Range3D { /* ... */ },
}
```

D-T3 (`dependents_of`) and D-T8 walk the AST and `match` on these variants to collect inbound cell coords.

## 6. Function-support lookup (needs_browser fallback)

For D-T5's "if any function referenced is not registered, mark the cell as `needs_browser`" path:

```rust
let f: Option<std::sync::Arc<dyn formualizer::eval::function::Function>>
    = formualizer::eval::function_registry::get(ns, name);
// f.is_some() ⇒ supported.
// ns is "" for builtins; named functions live under their LET/LAMBDA namespace.
```

Walk the AST, find every `ASTNodeType::Function { name, .. }`, query `get("", &name)` (uppercase), and if any returns `None` fall back to browser-side recalc.

The smoke verifies both branches:

```rust
assert!(formualizer::eval::function_registry::get("", "SUM").is_some());
assert!(formualizer::eval::function_registry::get("", "NOTAREALFUNCTION").is_none());
```

## 7. Error types

| Type | Crate | Notes |
|------|-------|-------|
| `formualizer::ExcelError` | `formualizer-common 2.0.0` | the evaluator's runtime error; carries `ExcelErrorKind` (`Ref`, `Value`, `Div0`, `Name`, `NImpl`, `Cancelled`, etc.) and an `ErrorContext`. Construct with `ExcelError::new(kind).with_message("...")`. |
| `formualizer::ExcelErrorKind` | `formualizer-common 2.0.0` | enum of all Excel error codes. Use `NImpl` for "not yet implemented" branches in `YrsResolver`. |
| `formualizer::parse::parser::ParserError` | `formualizer-parse 2.0.0` | plain struct `{ message, position }`. |
| `formualizer::parse::types::ParsingError` | `formualizer-parse 2.0.0` | rich enum used internally; usually surfaced as a `ParserError::message`. |

## 8. Gotchas

1. **`parse` jumped to 2.x while the meta-crate is still 0.6.x.** When reading
   docs.rs links, double-check you're reading `formualizer-parse 2.0.0`
   docs, not the 0.x line — the AST shape changed.
2. **1-based row/col everywhere** in `Workbook::set_formula`/`evaluate_cell`/`CellRef::new_absolute`. yrs cells are typically 0-based in our records — convert at the boundary.
3. **Formulas MUST start with `=`** when passed to `Workbook::set_formula` (the parser strips it). If a string lacks the leading `=`, the workbook stores it as a literal text cell — silently wrong.
4. **`add_sheet` errors if the sheet already exists** — always gate on `has_sheet(name)` first (as `doc_examples::eval_scalar` does).
5. **Default feature set is heavy.** `portable-wasm` brings in eval+workbook+sheetport+parse+common together. If we later need a leaner build, switch to `default-features = false, features = ["parse", "eval", "common"]` and drop the `Workbook` path.
6. **`FunctionProvider::get_function` is fallible** in the type signature (`Option`), so our `YrsResolver` impl can simply delegate to the global registry — no `Result` wrapping needed.
7. **`Interpreter::evaluate_ast` returns `CalcValue<'a>`, not `LiteralValue`** — call `.into_literal()` (or `.as_scalar()`) at the call site. The smoke uses `Workbook::evaluate_cell` which already does this unwrapping for you.
8. **`Workbook::evaluate_cell` takes `&mut self`** (it caches results). The interpreter path takes `&dyn EvaluationContext` (immutable) — important for D-T5's recalc cascade, which holds the Y.Doc read transaction.

## 9. Verified-but-not-exhaustive

The smoke covers parse + Workbook eval + doc_examples eval + function lookup. The following are documented above based on reading the source but were NOT exercised in the smoke — D-T2/D-T4 should re-confirm by writing a tiny mock resolver:

- Implementing `EvaluationContext` end-to-end and calling `Interpreter::evaluate_ast`.
- `CalcValue::into_literal` / `as_scalar` exact return type when the AST evaluates to a range.
- `InMemoryRange::new` constructor signature (called out as the canonical way to wrap `Vec<Vec<LiteralValue>>`).
- `TableReference` shape when an LLM-authored formula uses `Table1[#All]` syntax.

If any of those four turn out to differ in practice, update this file in the same commit as the divergence — it is the single source of truth referenced by every D-T* task.
