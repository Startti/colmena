# src/libs/colmena/src/crdt_documents/formula_engine.rs

**Layer:** infrastructure  **Purpose:** Wraps the `formualizer` crate behind a thin trait-based adapter, shielding the CRDT documents module (and the rest of colmena) from direct formualizer dependency. Provides formula parsing, evaluation, and dependency-graph analysis for spreadsheet cells.

## Symbols

- `ensure_builtins()` (fn, private) — lazily initializes formualizer's global builtin function registry (SUM, IF, …) once per process via OnceLock, idempotent and thread-safe
- `CellSnapshot` (struct, pub) — compact JSON value + type-tag representation of a CRDT-projected cell (t: 0/blank, 1/string, 2/number, 3/bool, 4/error)
- `ParseOutcome` (enum, pub) — result of formula parsing: Ok(ParsedFormula) for safe backend eval, NeedsBrowser with unsupported function list, or ParseError with debug rendering
- `ParsedFormula` (struct, pub) — opaque wrapper holding a parsed formualizer AST and original formula text
- `ParsedFormula::original_text()` (fn, pub) — accessor returning the original formula text including leading `=`
- `FormulaSource` (enum, pub) — tag indicating where a formula result originated (Backend / Frontend / NeedsBrowser)
- `FormulaSource::as_str()` (fn, pub) — maps enum variants to string codes ("be", "fe", "needs_browser")
- `EvalValue` (enum, pub) — formula evaluation result collapsed into four shapes: Number(f64), String, Bool, or Error; mirrors formualizer's LiteralValue
- `ExcelError` (enum, pub) — colmena-owned Excel error enum (DivZero, Ref, Name, Value, Num, NA, Cycle, Other) distinct from formualizer's type to isolate the dependency
- `ExcelError::as_excel()` (fn, pub) — returns Excel error display string ("#DIV/0!", "#REF!", etc.)
- `From<&FzExcelError> for ExcelError` (impl, pub) — converts borrowed formualizer error to colmena error by matching error kind
- `From<FzExcelError> for ExcelError` (impl, pub) — converts owned formualizer error via reference
- `EvalError` (enum, pub) — evaluation error type with single Internal variant; kept on signature for forward compatibility (currently unreachable per doc)
- `CellResolver` (trait, pub) — host-implemented port for providing cell values, sheet existence checks, and formula iteration to the evaluator; must be Send + Sync for parallel function execution
- `parse(text: &str)` (fn, pub) — parses formula text (requires leading `=`), returns ParseOutcome with optional unsupported-function discovery
- `evaluate(formula, resolver, current_sheet)` (fn, pub) — evaluates parsed formula against a CellResolver, returns EvalValue; all formualizer errors convert to EvalValue::Error, not EvalError
- `function_names(formula)` (fn, pub) — walks AST to collect all referenced function names, returns sorted and dedup'd vector for caller use
- `is_supported_fn(name)` (fn, pub) — checks if function name is registered in formualizer's global function registry (case-insensitive); triggers ensure_builtins() on first call
- `all_supported(names)` (fn, pub) — predicate checking if all names in slice are supported
- `collect_unsupported_fns(formula)` (fn, private) — filters function_names() result to unsupported-only list
- `walk_for_fns(node_type, out)` (fn, private) — recursive AST walker collecting function names from all branches (Function, Call, BinaryOp, UnaryOp, Array nodes; skips Literal and Reference)
- `literal_to_eval(lit: LiteralValue)` (fn, private) — converts formualizer LiteralValue to colmena EvalValue; maps Empty to Number(0.0) (documented limitation), Pending to Other("pending value"), temporal types to String
- `json_to_literal(snap: &CellSnapshot)` (fn, private) — converts CellSnapshot JSON value + type tag back to formualizer LiteralValue using type-tag dispatch (1/2/3/4) with JSON-inferred fallback
- `col_num_to_letters(c: u32)` (fn, private) — bijective base-26 encoding: converts column number to letters (1→A, 26→Z, 27→AA, 703→AAA)
- `coord_to_a1(row, col)` (fn, private) — combines letters and row number to A1-style cell address
- `ResolverAdapter<'a>` (struct, private) — adapter implementing formualizer's full trait suite (ReferenceResolver, RangeResolver, NamedRangeResolver, TableResolver, FunctionProvider, SourceResolver, EvaluationContext) to bridge CellResolver to formualizer's Interpreter
- `ResolverAdapter impl ReferenceResolver` (impl, private) — resolves single-cell references by delegating to inner CellResolver.get(); returns Ref error if sheet unknown
- `ResolverAdapter impl RangeResolver` (impl, private) — resolves rectangular range references (A1:B3), returns error for unbounded ranges (A:A, 1:1) not supported in v1; materializes full range into Vec<Vec<LiteralValue>>
- `ResolverAdapter impl NamedRangeResolver` (impl, private) — returns #NAME? error; named ranges not supported in v1
- `ResolverAdapter impl TableResolver` (impl, private) — returns #NAME? error; table references not supported in v1
- `ResolverAdapter impl Resolver` (impl, private) — marker trait implementation
- `ResolverAdapter impl FunctionProvider` (impl, private) — delegates to formualizer's function_registry
- `ResolverAdapter impl SourceResolver` (impl, private) — marker trait implementation
- `ResolverAdapter impl EvaluationContext` (impl, private) — overrides resolve_cell_reference_value to bypass default #N/IMPL! fallback and use custom ReferenceResolver path; implements resolve_range_view for cell/named-range/range cases
- `referenced_cells(formula, current_sheet)` (fn, pub) — collects all cells referenced in formula AST via formualizer's get_dependencies(), expands rectangular ranges to full cell list in row-major order, returns (sheet, addr) tuples; skips unsupported reference types (NamedRange, Table, External, 3D)
- `dependents_of(changed_addr, sheet, resolver)` (fn, pub) — finds all formulas in sheet that directly reference changed cell by iterating resolver's formula list; parses each and checks referenced_cells() containment; excludes parse-failed and NeedsBrowser formulas from dep graph (known limitation in v1)
- `CycleError` (struct, pub) — thiserror-annotated struct returned by recalc_chain when cycle detected; chain field lists cycle participants and blocked-downstream cells
- `recalc_chain(changed_addr, sheet, resolver)` (fn, pub) — computes topological recalc order starting from changed cell using BFS to discover transitive dependents then Kahn's algorithm; returns cells in dependency order (excluding the trigger cell); detects cycles and reports all participants

## Test Infrastructure

- `StubResolver` (struct, pub(super)) — minimal CellResolver for unit tests; holds cell value map and sheet list; iter_formulas_in_sheet returns empty (no dep-graph tests)
- `ResolverWithFormulas` (struct, pub(super)) — extended resolver for dep-graph testing; adds formula map and iter_formulas_in_sheet implementation
- `StubResolver::new()`, `set_num()` — builder helpers for test setup
- `ResolverWithFormulas::new()`, `set_num()`, `set_formula()` — builder helpers for dep-graph test setup

## File-level notes

- **Intentional design decision**: `EvalError::Internal` variant is documented as currently unreachable but deliberately kept on the signature for forward compatibility (D-T4's YrsResolver may surface errors in future)
- **Known limitation — Empty cells**: mapped to Number(0.0) to match Excel's arithmetic-context behavior; if D-T5 projection needs faithful blank round-trip, add EvalValue::Empty variant
- **Known limitation — Array results**: silently become #VALUE! error when appearing as formula top-level result
- **Known limitation — Dependency graph**: only intra-sheet rectangular references supported in v1; cross-sheet, unbounded (A:A), named ranges, and table references are deferred
- **Known limitation — Formula inclusion in dep graph**: formulas that fail to parse server-side (especially NeedsBrowser with unsupported functions) are excluded from the dependency graph; D-Tx roadmap includes textual fallback for their dependents
- **Algorithmic choice in recalc_chain**: Kahn's topological sort ensures output is reproducible and deterministic (cells in dependency order); cycle detection via in-degree > 0 check works for all participants
- All imports are from `formualizer` crate (extern dependency) and stdlib; no internal colmena dependencies
- 37 test cases cover parsing, evaluation, function discovery, range handling, division by zero, coordinate conversion, function dedup, reference collection, dependents discovery, and cycle detection
