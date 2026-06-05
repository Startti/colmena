//! Backend formula engine. Wraps `formualizer` behind a thin trait so the
//! rest of the codebase doesn't depend on it directly. See
//! `docs/superpowers/notes/2026-06-04-formualizer-api.md` for the verified
//! upstream API this module is built on.
//!
//! Surface area exposed to the rest of `crdt_documents`:
//!
//! - `CellResolver` trait — host implements this to feed cell values to the
//!   evaluator (D-T4 wires it to a `&yrs::Doc`; tests use a `StubResolver`).
//! - `parse(text)` -> `ParseOutcome` (Ok / NeedsBrowser / ParseError).
//! - `evaluate(formula, resolver, current_sheet)` -> `EvalValue`.
//! - `function_names(formula)` + `is_supported_fn(name)` for the
//!   needs-browser-fallback path used by D-T5.
//! - `FormulaSource` enum tag for projection rows (be / fe / needs_browser).
//! - `ExcelError` (colmena-owned, distinct from `formualizer::ExcelError`)
//!   so downstream code doesn't import formualizer types directly.

use std::sync::{Arc, OnceLock};

use formualizer::eval::engine::range_view::RangeView;
use formualizer::eval::interpreter::Interpreter;
use formualizer::eval::traits::{
    EvaluationContext, FunctionProvider, InMemoryRange, NamedRangeResolver, Range, RangeResolver,
    ReferenceResolver, Resolver, SourceResolver, TableResolver,
};
use formualizer::parse::parser::{ASTNode, ASTNodeType, ReferenceType};
use formualizer::{ExcelError as FzExcelError, ExcelErrorKind, LiteralValue};
use serde::{Deserialize, Serialize};

/// Lazily register all formualizer builtins (SUM, IF, …) the first time the
/// engine is used in a process. `formualizer::eval::function_registry` is a
/// process-wide DashMap; loading is idempotent and thread-safe.
fn ensure_builtins() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        formualizer::eval::builtins::load_builtins();
    });
}

/// Compact snapshot of a single cell as projected by the CRDT layer.
///
/// `v` is the JSON-encoded value (number, string, bool, null) and `t` is the
/// cell-type tag used by the projection (`1` = string, `2` = number,
/// `3` = bool, `4` = error, `0`/other = blank). See the design spec for the
/// canonical mapping.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CellSnapshot {
    pub v: serde_json::Value,
    pub t: u8,
}

/// Outcome of parsing a formula text.
#[derive(Debug)]
pub enum ParseOutcome {
    /// Parsed cleanly; safe to evaluate backend-side.
    Ok(ParsedFormula),
    /// Parsed cleanly but references at least one function not supported by
    /// formualizer — D-T5 marks the cell as `needs_browser`.
    NeedsBrowser { unsupported_fns: Vec<String> },
    /// Could not parse — `String` is a debug rendering of the parser error.
    ParseError(String),
}

/// Opaque wrapper around a parsed formualizer AST + the original text.
#[derive(Debug)]
pub struct ParsedFormula {
    ast: ASTNode,
    original_text: String,
}

impl ParsedFormula {
    /// The original formula text including the leading `=`.
    pub fn original_text(&self) -> &str {
        &self.original_text
    }
}

/// Tag used on the projection cell to indicate where the value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormulaSource {
    Backend,
    Frontend,
    NeedsBrowser,
}

impl FormulaSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            FormulaSource::Backend => "be",
            FormulaSource::Frontend => "fe",
            FormulaSource::NeedsBrowser => "needs_browser",
        }
    }
}

/// Result of evaluating a parsed formula. Mirrors a subset of formualizer's
/// `LiteralValue` collapsed into the four shapes our projection cares about.
#[derive(Debug, Clone, PartialEq)]
pub enum EvalValue {
    Number(f64),
    String(String),
    Bool(bool),
    Error(ExcelError),
}

/// Colmena-owned Excel error enum. Distinct from `formualizer::ExcelError`
/// so the rest of the codebase doesn't import formualizer types.
///
/// `Cycle` is our extension (used by `recalc_chain` in D-T3 to mark a cell
/// that participates in a dependency cycle).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExcelError {
    DivZero, // #DIV/0!
    Ref,     // #REF!
    Name,    // #NAME?
    Value,   // #VALUE!
    Num,     // #NUM!
    NA,      // #N/A
    Cycle,   // #CYCLE! (colmena extension)
    Other(String),
}

impl ExcelError {
    pub fn as_excel(&self) -> &'static str {
        match self {
            ExcelError::DivZero => "#DIV/0!",
            ExcelError::Ref => "#REF!",
            ExcelError::Name => "#NAME?",
            ExcelError::Value => "#VALUE!",
            ExcelError::Num => "#NUM!",
            ExcelError::NA => "#N/A",
            ExcelError::Cycle => "#CYCLE!",
            ExcelError::Other(_) => "#ERROR!",
        }
    }
}

impl From<&FzExcelError> for ExcelError {
    fn from(e: &FzExcelError) -> Self {
        match e.kind {
            ExcelErrorKind::Div => ExcelError::DivZero,
            ExcelErrorKind::Ref => ExcelError::Ref,
            ExcelErrorKind::Name => ExcelError::Name,
            ExcelErrorKind::Value => ExcelError::Value,
            ExcelErrorKind::Num => ExcelError::Num,
            ExcelErrorKind::Na => ExcelError::NA,
            ExcelErrorKind::Circ => ExcelError::Cycle,
            other => ExcelError::Other(format!(
                "{other}{}",
                match &e.message {
                    Some(m) => format!(": {m}"),
                    None => String::new(),
                }
            )),
        }
    }
}

impl From<FzExcelError> for ExcelError {
    fn from(e: FzExcelError) -> Self {
        (&e).into()
    }
}

/// Public error returned by `evaluate` for genuine internal failures (not
/// for Excel-semantics errors, which round-trip as `EvalValue::Error`).
///
/// `EvalError::Internal` is reserved for non-Excel-modeling failures (e.g.
/// interpreter panics caught by future panic guards, or yrs-level errors
/// surfaced by D-T4's `YrsResolver`). Today every formualizer-side error is
/// converted to `EvalValue::Error(ExcelError)` and this variant is
/// **currently unreachable** from this implementation — it is kept on the
/// signature deliberately for forward compatibility so adding new failure
/// paths later doesn't require a breaking API change.
#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    // NOTE: kept even though currently unreachable. See type doc comment.
    #[error("internal evaluator error: {0}")]
    Internal(String),
}

/// Host trait: provide cell values + sheet existence to the evaluator.
///
/// `addr` is an A1-style cell address ("A1", "AA12"). `sheet` is the sheet
/// name as the projection knows it.
///
/// Implementations must be `Send + Sync` because the evaluator's super-trait
/// `EvaluationContext` requires it for parallel function execution.
pub trait CellResolver: Send + Sync {
    fn get(&self, sheet: &str, addr: &str) -> Option<CellSnapshot>;
    fn sheet_exists(&self, sheet: &str) -> bool;

    /// Iterate every cell that has a formula text, scoped to one sheet.
    /// Returned tuples: (addr, formula_text). Order arbitrary.
    ///
    /// Used by `dependents_of` / `recalc_chain` to build the intra-sheet
    /// dependency graph. The default `ResolverAdapter` used during
    /// `evaluate()` never calls this — it's only needed by the dep-graph
    /// machinery — so implementations that don't drive recalc can return
    /// an empty iterator (e.g. the smoke-test `StubResolver`).
    fn iter_formulas_in_sheet<'a>(
        &'a self,
        sheet: &str,
    ) -> Box<dyn Iterator<Item = (String, String)> + 'a>;
}

/* ─────────────────────────── parse / evaluate ────────────────────────── */

/// Parse a formula text. Requires a leading `=` (formualizer is lenient about
/// it but we use that as our guard against treating literal text as a
/// formula — see D-T1 notes gotcha #3).
pub fn parse(text: &str) -> ParseOutcome {
    if !text.starts_with('=') {
        return ParseOutcome::ParseError("not a formula (missing leading =)".to_string());
    }
    ensure_builtins();
    match formualizer::parse::parser::parse(text) {
        Ok(ast) => {
            let parsed = ParsedFormula {
                ast,
                original_text: text.to_string(),
            };
            let unsupported = collect_unsupported_fns(&parsed);
            if unsupported.is_empty() {
                ParseOutcome::Ok(parsed)
            } else {
                ParseOutcome::NeedsBrowser {
                    unsupported_fns: unsupported,
                }
            }
        }
        Err(e) => ParseOutcome::ParseError(format!("{e:?}")),
    }
}

/// Evaluate a parsed formula against a `CellResolver`.
///
/// `current_sheet` is the sheet the formula lives on — used to resolve
/// unqualified references like `A1`.
///
/// Returns `EvalError::Internal` for non-Excel-modeling failures (e.g.
/// interpreter panics caught by future panic guards). Today every
/// formualizer-side error is converted to `EvalValue::Error(ExcelError)`
/// — `EvalError::Internal` is reserved for forward compatibility (D-T4's
/// `YrsResolver` may surface yrs-level errors here). The variant is
/// intentionally kept on the signature even though it is unreachable
/// from this implementation.
pub fn evaluate(
    formula: &ParsedFormula,
    resolver: &dyn CellResolver,
    current_sheet: &str,
) -> Result<EvalValue, EvalError> {
    ensure_builtins();
    let adapter = ResolverAdapter {
        inner: resolver,
        current_sheet,
    };
    let interp = Interpreter::new(&adapter, current_sheet);
    match interp.evaluate_ast(&formula.ast) {
        Ok(calc) => Ok(literal_to_eval(calc.into_literal())),
        Err(e) => Ok(EvalValue::Error((&e).into())),
    }
}

/// Collect every function name referenced anywhere in the parsed AST.
///
/// The returned vector is **sorted alphabetically and deduplicated**, so
/// callers (e.g. D-T5's `unsupported_fns` projection field) don't need to
/// clean up duplicates from formulas like `=SUM(A1)+SUM(B1)`.
pub fn function_names(formula: &ParsedFormula) -> Vec<String> {
    let mut names = Vec::new();
    walk_for_fns(&formula.ast.node_type, &mut names);
    names.sort();
    names.dedup();
    names
}

/// True if the named function is supported by formualizer's built-in
/// registry. Case-insensitive — formualizer canonicalises to uppercase
/// internally. Builtins are lazily loaded on first call.
pub fn is_supported_fn(name: &str) -> bool {
    ensure_builtins();
    formualizer::eval::function_registry::get("", name).is_some()
}

/// True iff every name in `names` is supported by formualizer.
pub fn all_supported(names: &[String]) -> bool {
    names.iter().all(|n| is_supported_fn(n))
}

/* ───────────────────────────── internals ─────────────────────────────── */

fn collect_unsupported_fns(formula: &ParsedFormula) -> Vec<String> {
    // `function_names` already returns a sorted, deduped vector — we only
    // need to filter to the unsupported names.
    let mut all = function_names(formula);
    all.retain(|n| !is_supported_fn(n));
    all
}

fn walk_for_fns(node_type: &ASTNodeType, out: &mut Vec<String>) {
    match node_type {
        ASTNodeType::Function { name, args } => {
            out.push(name.clone());
            for a in args {
                walk_for_fns(&a.node_type, out);
            }
        }
        ASTNodeType::Call { callee, args } => {
            walk_for_fns(&callee.node_type, out);
            for a in args {
                walk_for_fns(&a.node_type, out);
            }
        }
        ASTNodeType::BinaryOp { left, right, .. } => {
            walk_for_fns(&left.node_type, out);
            walk_for_fns(&right.node_type, out);
        }
        ASTNodeType::UnaryOp { expr, .. } => {
            walk_for_fns(&expr.node_type, out);
        }
        ASTNodeType::Array(rows) => {
            for row in rows {
                for cell in row {
                    walk_for_fns(&cell.node_type, out);
                }
            }
        }
        ASTNodeType::Literal(_) | ASTNodeType::Reference { .. } => {}
    }
}

/// Collapse a formualizer `LiteralValue` into our 4-shape `EvalValue`.
///
/// **Known limitation — `LiteralValue::Empty → Number(0.0)`**: empty cells
/// are collapsed to `Number(0.0)` matching Excel's display behaviour in
/// arithmetic context (e.g. `=A1+1` where `A1` is blank yields `1`). When
/// this propagates to a top-level evaluation, callers cannot distinguish
/// a cell that evaluates to literal `0` from a cell that was blank. This
/// is intentional for D-T2; if D-T5's projection needs to round-trip
/// blank cells faithfully, add an `EvalValue::Empty` variant and remap
/// that branch.
fn literal_to_eval(lit: LiteralValue) -> EvalValue {
    match lit {
        LiteralValue::Number(n) => EvalValue::Number(n),
        LiteralValue::Int(i) => EvalValue::Number(i as f64),
        LiteralValue::Boolean(b) => EvalValue::Bool(b),
        LiteralValue::Text(s) => EvalValue::String(s),
        LiteralValue::Error(e) => EvalValue::Error((&e).into()),
        LiteralValue::Empty => EvalValue::Number(0.0),
        LiteralValue::Pending => EvalValue::Error(ExcelError::Other("pending value".to_string())),
        LiteralValue::Date(d) => EvalValue::String(d.to_string()),
        LiteralValue::DateTime(dt) => EvalValue::String(dt.to_string()),
        LiteralValue::Time(t) => EvalValue::String(t.to_string()),
        LiteralValue::Duration(d) => EvalValue::String(format!("{d}")),
        LiteralValue::Array(_) => EvalValue::Error(ExcelError::Value),
    }
}

fn json_to_literal(snap: &CellSnapshot) -> LiteralValue {
    match snap.t {
        // Per design spec: 0=blank, 1=string, 2=number, 3=bool, 4=error.
        1 => snap
            .v
            .as_str()
            .map(|s| LiteralValue::Text(s.to_string()))
            .unwrap_or(LiteralValue::Empty),
        2 => {
            if let Some(n) = snap.v.as_f64() {
                LiteralValue::Number(n)
            } else if let Some(i) = snap.v.as_i64() {
                LiteralValue::Int(i)
            } else {
                LiteralValue::Empty
            }
        }
        3 => snap
            .v
            .as_bool()
            .map(LiteralValue::Boolean)
            .unwrap_or(LiteralValue::Empty),
        4 => LiteralValue::Error(FzExcelError::new(ExcelErrorKind::Error)),
        _ => {
            // Fallback: infer from JSON.
            match &snap.v {
                serde_json::Value::Null => LiteralValue::Empty,
                serde_json::Value::Bool(b) => LiteralValue::Boolean(*b),
                serde_json::Value::Number(n) => n
                    .as_f64()
                    .map(LiteralValue::Number)
                    .unwrap_or(LiteralValue::Empty),
                serde_json::Value::String(s) => LiteralValue::Text(s.clone()),
                _ => LiteralValue::Empty,
            }
        }
    }
}

fn col_num_to_letters(mut c: u32) -> String {
    let mut letters = String::new();
    while c > 0 {
        let rem = ((c - 1) % 26) as u8;
        letters.insert(0, (b'A' + rem) as char);
        c = (c - 1) / 26;
    }
    letters
}

fn coord_to_a1(row: u32, col: u32) -> String {
    format!("{}{}", col_num_to_letters(col), row)
}

/* ───────────────── ResolverAdapter: CellResolver -> formualizer ──────── */

struct ResolverAdapter<'a> {
    inner: &'a dyn CellResolver,
    current_sheet: &'a str,
}

impl<'a> ReferenceResolver for ResolverAdapter<'a> {
    fn resolve_cell_reference(
        &self,
        sheet: Option<&str>,
        row: u32,
        col: u32,
    ) -> Result<LiteralValue, FzExcelError> {
        let sheet_name = sheet.unwrap_or(self.current_sheet);
        if !self.inner.sheet_exists(sheet_name) {
            return Err(FzExcelError::new(ExcelErrorKind::Ref)
                .with_message(format!("unknown sheet '{sheet_name}'")));
        }
        let addr = coord_to_a1(row, col);
        match self.inner.get(sheet_name, &addr) {
            Some(snap) => Ok(json_to_literal(&snap)),
            None => Ok(LiteralValue::Empty),
        }
    }
}

impl<'a> RangeResolver for ResolverAdapter<'a> {
    fn resolve_range_reference(
        &self,
        sheet: Option<&str>,
        sr: Option<u32>,
        sc: Option<u32>,
        er: Option<u32>,
        ec: Option<u32>,
    ) -> Result<Box<dyn Range>, FzExcelError> {
        let sheet_name = sheet.unwrap_or(self.current_sheet);
        if !self.inner.sheet_exists(sheet_name) {
            return Err(FzExcelError::new(ExcelErrorKind::Ref)
                .with_message(format!("unknown sheet '{sheet_name}'")));
        }
        // We only support fully-bounded rectangular ranges in v1.
        let (sr, sc, er, ec) = match (sr, sc, er, ec) {
            (Some(sr), Some(sc), Some(er), Some(ec)) => (sr, sc, er, ec),
            _ => {
                return Err(FzExcelError::new(ExcelErrorKind::NImpl).with_message(
                    "unbounded ranges (e.g. A:A, 1:1) are not yet supported".to_string(),
                ));
            }
        };
        let (sr, er) = if sr <= er { (sr, er) } else { (er, sr) };
        let (sc, ec) = if sc <= ec { (sc, ec) } else { (ec, sc) };
        let mut rows = Vec::with_capacity((er - sr + 1) as usize);
        for r in sr..=er {
            let mut row_vec = Vec::with_capacity((ec - sc + 1) as usize);
            for c in sc..=ec {
                let addr = coord_to_a1(r, c);
                let val = match self.inner.get(sheet_name, &addr) {
                    Some(snap) => json_to_literal(&snap),
                    None => LiteralValue::Empty,
                };
                row_vec.push(val);
            }
            rows.push(row_vec);
        }
        Ok(Box::new(InMemoryRange::new(rows)))
    }
}

impl<'a> NamedRangeResolver for ResolverAdapter<'a> {
    fn resolve_named_range_reference(
        &self,
        name: &str,
    ) -> Result<Vec<Vec<LiteralValue>>, FzExcelError> {
        Err(FzExcelError::new(ExcelErrorKind::Name)
            .with_message(format!("named ranges not supported: {name}")))
    }
}

impl<'a> TableResolver for ResolverAdapter<'a> {
    fn resolve_table_reference(
        &self,
        _tref: &formualizer::parse::parser::TableReference,
    ) -> Result<Box<dyn formualizer::eval::traits::Table>, FzExcelError> {
        Err(FzExcelError::new(ExcelErrorKind::Name)
            .with_message("table references not supported".to_string()))
    }
}

impl<'a> Resolver for ResolverAdapter<'a> {}

impl<'a> FunctionProvider for ResolverAdapter<'a> {
    fn get_function(
        &self,
        ns: &str,
        name: &str,
    ) -> Option<Arc<dyn formualizer::eval::function::Function>> {
        formualizer::eval::function_registry::get(ns, name)
    }
}

impl<'a> SourceResolver for ResolverAdapter<'a> {}

impl<'a> EvaluationContext for ResolverAdapter<'a> {
    fn resolve_cell_reference_value(
        &self,
        sheet: Option<&str>,
        row: u32,
        col: u32,
        _current_sheet: &str,
    ) -> Result<LiteralValue, FzExcelError> {
        // Bypass the default impl (which routes through `resolve_range_view`
        // and returns `#N/IMPL!` by default). Delegate directly to our
        // legacy ReferenceResolver path.
        self.resolve_cell_reference(sheet, row, col)
    }

    fn resolve_range_view<'c>(
        &'c self,
        reference: &ReferenceType,
        current_sheet: &str,
    ) -> Result<RangeView<'c>, FzExcelError> {
        match reference {
            ReferenceType::Cell {
                sheet, row, col, ..
            } => {
                let sheet_name = sheet.as_deref().unwrap_or(current_sheet);
                let v = match self.resolve_cell_reference(Some(sheet_name), *row, *col) {
                    Ok(val) => val,
                    Err(e) => LiteralValue::Error(e),
                };
                Ok(RangeView::from_owned_rows(
                    vec![vec![v]],
                    self.date_system(),
                ))
            }
            ReferenceType::NamedRange(name) => {
                let rows = self.resolve_named_range_reference(name)?;
                Ok(RangeView::from_owned_rows(rows, self.date_system()))
            }
            _ => {
                // Tables, rectangular ranges, 3D refs etc — go through the
                // generic Resolver helper which materialises into a Box<dyn Range>.
                let r = self.resolve_range_like(reference)?;
                let owned: Vec<Vec<LiteralValue>> = r.materialise().into_owned();
                Ok(RangeView::from_owned_rows(owned, self.date_system()))
            }
        }
    }
}

/* ────────────────────── dependency graph (D-T3) ──────────────────────── */

/// Collect every cell that `formula` references, resolved against
/// `current_sheet` for unqualified refs (e.g. `A1` on `Sheet1` → `("Sheet1","A1")`).
/// Range references are expanded to the full set of constituent cells in
/// row-major order (left-to-right within each row, top-to-bottom across rows).
///
/// Returned tuples are `(sheet, addr)` where `addr` is A1-style.
///
/// **Implementation choice:** delegates to formualizer's built-in
/// `ASTNode::get_dependencies()` (verified at
/// `formualizer-parse-2.0.0/src/parser.rs:1823`) which already walks every
/// AST variant and returns `Vec<&ReferenceType>` in source order. We only
/// need to (a) drop non-cell flavours (NamedRange/Table/External/3D — they
/// can't appear in v1's intra-sheet dep graph anyway) and (b) expand range
/// references to their full cell list. Hand-rolling the walker (à la
/// `walk_for_fns`) would duplicate `collect_dependencies` for no benefit.
///
/// Non-cell reference flavours (`NamedRange`, `Table`, `External`, `Cell3D`,
/// `Range3D`) are silently skipped — D-T3's recalc graph is intra-sheet
/// rectangular only; spec §11 defers those flavours to a later iteration.
pub fn referenced_cells(formula: &ParsedFormula, current_sheet: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for r in formula.ast.get_dependencies() {
        match r {
            ReferenceType::Cell {
                sheet, row, col, ..
            } => {
                let sheet_name = sheet.as_deref().unwrap_or(current_sheet).to_string();
                out.push((sheet_name, coord_to_a1(*row, *col)));
            }
            ReferenceType::Range {
                sheet,
                start_row,
                start_col,
                end_row,
                end_col,
                ..
            } => {
                let (Some(sr), Some(sc), Some(er), Some(ec)) =
                    (*start_row, *start_col, *end_row, *end_col)
                else {
                    // Unbounded ranges (A:A, 1:1) are not supported in v1 —
                    // skip rather than fabricate a phantom cell list.
                    continue;
                };
                let (sr, er) = if sr <= er { (sr, er) } else { (er, sr) };
                let (sc, ec) = if sc <= ec { (sc, ec) } else { (ec, sc) };
                let sheet_name = sheet.as_deref().unwrap_or(current_sheet).to_string();
                for row in sr..=er {
                    for col in sc..=ec {
                        out.push((sheet_name.clone(), coord_to_a1(row, col)));
                    }
                }
            }
            // NamedRange / Table / External / Cell3D / Range3D — out of scope
            // for the v1 dep graph.
            _ => {}
        }
    }
    out
}

/// Find every cell in `sheet` whose formula directly references
/// `(sheet, changed_addr)`. Intra-sheet only — cross-sheet dependents are
/// not returned in v1 (spec §11).
pub fn dependents_of(
    changed_addr: &str,
    sheet: &str,
    resolver: &dyn CellResolver,
) -> Vec<(String, String)> {
    let target = (sheet.to_string(), changed_addr.to_string());
    let mut out = Vec::new();
    for (other_addr, text) in resolver.iter_formulas_in_sheet(sheet) {
        if let ParseOutcome::Ok(ast) = parse(&text) {
            if referenced_cells(&ast, sheet).contains(&target) {
                out.push((sheet.to_string(), other_addr));
            }
        }
    }
    out
}

/// Error returned by `recalc_chain` when the dependency graph contains a
/// cycle. `chain` lists the cells that participate in the cycle (i.e. the
/// nodes whose in-degree never reached zero in the Kahn pass).
#[derive(Debug, thiserror::Error)]
#[error("cycle detected: {chain:?}")]
pub struct CycleError {
    pub chain: Vec<(String, String)>,
}

/// Compute the topological recalc order starting from a cell that just
/// changed. Walks the transitive intra-sheet dependents reachable from
/// `(sheet, changed_addr)` and returns them in dependency order — every cell
/// appears after all the cells it depends on.
///
/// The starting cell is **not** included in the returned chain (it is the
/// cause of the recalc, not a target of it).
///
/// Returns `CycleError` if the dep graph contains a cycle; the error's
/// `chain` lists the cycle participants.
///
/// Algorithm: BFS to discover the transitive set of dependents, then build
/// adjacency / in-degree maps and run Kahn's topo sort starting from the
/// changed cell (in-degree 0). Any node still showing in-degree > 0 after
/// the queue empties is on a cycle.
pub fn recalc_chain(
    changed_addr: &str,
    sheet: &str,
    resolver: &dyn CellResolver,
) -> Result<Vec<(String, String)>, CycleError> {
    use std::collections::{HashMap, HashSet, VecDeque};

    let start = (sheet.to_string(), changed_addr.to_string());

    // BFS to find every cell transitively dependent on `start`. Each edge
    // points from a cell to a cell that references it (predecessor →
    // successor in the recalc order).
    let mut nodes: HashSet<(String, String)> = HashSet::new();
    nodes.insert(start.clone());
    let mut adjacency: HashMap<(String, String), Vec<(String, String)>> = HashMap::new();
    let mut frontier: VecDeque<(String, String)> = VecDeque::new();
    frontier.push_back(start.clone());

    while let Some(cell) = frontier.pop_front() {
        let direct = dependents_of(&cell.1, &cell.0, resolver);
        for dep in direct {
            adjacency.entry(cell.clone()).or_default().push(dep.clone());
            if nodes.insert(dep.clone()) {
                frontier.push_back(dep);
            }
        }
    }

    // Compute in-degree for each discovered node.
    let mut in_degree: HashMap<(String, String), usize> =
        nodes.iter().map(|n| (n.clone(), 0_usize)).collect();
    for succs in adjacency.values() {
        for s in succs {
            *in_degree.entry(s.clone()).or_insert(0) += 1;
        }
    }

    // Kahn: start the queue from nodes with in-degree 0. In a healthy graph
    // that's just `start` (everything else is reached via at least one
    // edge); on a cycle there may be no zero-degree node and the queue
    // stays empty.
    let mut queue: VecDeque<(String, String)> = in_degree
        .iter()
        .filter_map(|(n, &d)| if d == 0 { Some(n.clone()) } else { None })
        .collect();

    let mut order: Vec<(String, String)> = Vec::with_capacity(nodes.len());
    while let Some(cell) = queue.pop_front() {
        order.push(cell.clone());
        if let Some(succs) = adjacency.get(&cell) {
            for s in succs {
                if let Some(d) = in_degree.get_mut(s) {
                    *d -= 1;
                    if *d == 0 {
                        queue.push_back(s.clone());
                    }
                }
            }
        }
    }

    // Any node with in_degree > 0 at this point is on a cycle.
    if order.len() < nodes.len() {
        let mut chain: Vec<(String, String)> = in_degree
            .into_iter()
            .filter_map(|(n, d)| if d > 0 { Some(n) } else { None })
            .collect();
        chain.sort();
        return Err(CycleError { chain });
    }

    // Drop the changed cell itself — callers only want the cells they need
    // to recalc, not the trigger.
    order.retain(|n| n != &start);
    Ok(order)
}

/* ────────────────────────────── tests ────────────────────────────────── */

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    pub(super) struct StubResolver {
        pub cells: HashMap<(String, String), CellSnapshot>,
        pub sheets: Vec<String>,
    }

    impl StubResolver {
        pub fn new(sheets: &[&str]) -> Self {
            Self {
                cells: HashMap::new(),
                sheets: sheets.iter().map(|s| s.to_string()).collect(),
            }
        }
        pub fn set_num(&mut self, sheet: &str, addr: &str, v: f64) {
            self.cells.insert(
                (sheet.to_string(), addr.to_string()),
                CellSnapshot {
                    v: serde_json::json!(v),
                    t: 2,
                },
            );
        }
    }

    impl CellResolver for StubResolver {
        fn get(&self, sheet: &str, addr: &str) -> Option<CellSnapshot> {
            self.cells
                .get(&(sheet.to_string(), addr.to_string()))
                .cloned()
        }
        fn sheet_exists(&self, sheet: &str) -> bool {
            self.sheets.iter().any(|s| s == sheet)
        }
        fn iter_formulas_in_sheet<'a>(
            &'a self,
            _sheet: &str,
        ) -> Box<dyn Iterator<Item = (String, String)> + 'a> {
            Box::new(std::iter::empty())
        }
    }

    #[test]
    fn parse_rejects_non_formula() {
        let outcome = parse("hello");
        match outcome {
            ParseOutcome::ParseError(msg) => assert!(msg.contains("missing leading")),
            other => panic!("expected ParseError, got {other:?}"),
        }
    }

    #[test]
    fn parse_accepts_simple_formula() {
        let outcome = parse("=1+1");
        assert!(matches!(outcome, ParseOutcome::Ok(_)));
    }

    #[test]
    fn evaluate_simple_arithmetic() {
        let r = StubResolver::new(&["Sheet1"]);
        let ParseOutcome::Ok(ast) = parse("=2+3*4") else {
            panic!()
        };
        let v = evaluate(&ast, &r, "Sheet1").unwrap();
        assert_eq!(v, EvalValue::Number(14.0));
    }

    #[test]
    fn evaluate_cell_ref() {
        let mut r = StubResolver::new(&["Sheet1"]);
        r.set_num("Sheet1", "A1", 7.5);
        let ParseOutcome::Ok(ast) = parse("=A1*2") else {
            panic!()
        };
        let v = evaluate(&ast, &r, "Sheet1").unwrap();
        assert_eq!(v, EvalValue::Number(15.0));
    }

    #[test]
    fn evaluate_range_sum() {
        let mut r = StubResolver::new(&["Sheet1"]);
        r.set_num("Sheet1", "A1", 1.0);
        r.set_num("Sheet1", "A2", 2.0);
        r.set_num("Sheet1", "A3", 3.0);
        let ParseOutcome::Ok(ast) = parse("=SUM(A1:A3)") else {
            panic!()
        };
        let v = evaluate(&ast, &r, "Sheet1").unwrap();
        assert_eq!(v, EvalValue::Number(6.0));
    }

    #[test]
    fn evaluate_div_by_zero_returns_error_value() {
        let r = StubResolver::new(&["Sheet1"]);
        let ParseOutcome::Ok(ast) = parse("=1/0") else {
            panic!()
        };
        let v = evaluate(&ast, &r, "Sheet1").unwrap();
        assert_eq!(v, EvalValue::Error(ExcelError::DivZero));
    }

    #[test]
    fn function_names_extracts_sum() {
        let ParseOutcome::Ok(ast) = parse("=SUM(A1:A10)") else {
            panic!()
        };
        let names = function_names(&ast);
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case("SUM")));
    }

    #[test]
    fn is_supported_returns_true_for_sum() {
        assert!(is_supported_fn("SUM"));
    }

    #[test]
    fn coord_to_a1_handles_multi_letter_columns() {
        // Bijective base-26: col=1 → A, col=26 → Z, col=27 → AA, col=703 → AAA.
        assert_eq!(coord_to_a1(1, 1), "A1");
        assert_eq!(coord_to_a1(1, 26), "Z1");
        assert_eq!(coord_to_a1(1, 27), "AA1");
        assert_eq!(coord_to_a1(12, 703), "AAA12");
    }

    #[test]
    fn function_names_dedupes_repeats() {
        let ParseOutcome::Ok(ast) = parse("=SUM(A1)+SUM(B1)+AVERAGE(A1:A3)") else {
            panic!()
        };
        let names = function_names(&ast);
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted, names,
            "function_names output must already be dedup'd"
        );
        assert_eq!(names.len(), 2, "expected 2 distinct fns, got {names:?}");
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case("SUM")));
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case("AVERAGE")));
    }

    /* ───── dep-graph fixture + tests (D-T3) ───── */

    pub(super) struct ResolverWithFormulas {
        pub cells: HashMap<(String, String), CellSnapshot>,
        pub formulas: HashMap<(String, String), String>,
        pub sheets: Vec<String>,
    }

    impl ResolverWithFormulas {
        pub fn new(sheets: &[&str]) -> Self {
            Self {
                cells: HashMap::new(),
                formulas: HashMap::new(),
                sheets: sheets.iter().map(|s| s.to_string()).collect(),
            }
        }
        pub fn set_num(&mut self, sheet: &str, addr: &str, v: f64) {
            self.cells.insert(
                (sheet.to_string(), addr.to_string()),
                CellSnapshot {
                    v: serde_json::json!(v),
                    t: 2,
                },
            );
        }
        pub fn set_formula(&mut self, sheet: &str, addr: &str, f: &str) {
            self.formulas
                .insert((sheet.to_string(), addr.to_string()), f.to_string());
        }
    }

    impl CellResolver for ResolverWithFormulas {
        fn get(&self, sheet: &str, addr: &str) -> Option<CellSnapshot> {
            self.cells
                .get(&(sheet.to_string(), addr.to_string()))
                .cloned()
        }
        fn sheet_exists(&self, sheet: &str) -> bool {
            self.sheets.iter().any(|s| s == sheet)
        }
        fn iter_formulas_in_sheet<'a>(
            &'a self,
            sheet: &str,
        ) -> Box<dyn Iterator<Item = (String, String)> + 'a> {
            let s = sheet.to_string();
            Box::new(
                self.formulas
                    .iter()
                    .filter(move |((sh, _), _)| sh == &s)
                    .map(|((_, addr), f)| (addr.clone(), f.clone())),
            )
        }
    }

    #[test]
    fn referenced_cells_single_ref() {
        let ParseOutcome::Ok(ast) = parse("=A1+1") else {
            panic!()
        };
        let refs = referenced_cells(&ast, "Sheet1");
        assert_eq!(refs, vec![("Sheet1".to_string(), "A1".to_string())]);
    }

    #[test]
    fn referenced_cells_range_expanded() {
        let ParseOutcome::Ok(ast) = parse("=SUM(A1:A3)") else {
            panic!()
        };
        let refs = referenced_cells(&ast, "Sheet1");
        assert_eq!(
            refs,
            vec![
                ("Sheet1".to_string(), "A1".to_string()),
                ("Sheet1".to_string(), "A2".to_string()),
                ("Sheet1".to_string(), "A3".to_string()),
            ]
        );
    }

    #[test]
    fn referenced_cells_cross_sheet_keeps_other_sheet() {
        let ParseOutcome::Ok(ast) = parse("=Sheet2!A1+B2") else {
            panic!()
        };
        let mut refs = referenced_cells(&ast, "Sheet1");
        refs.sort();
        assert_eq!(
            refs,
            vec![
                ("Sheet1".to_string(), "B2".to_string()),
                ("Sheet2".to_string(), "A1".to_string()),
            ]
        );
    }

    #[test]
    fn dependents_of_finds_direct_reference() {
        let mut r = ResolverWithFormulas::new(&["Sheet1"]);
        r.set_num("Sheet1", "A1", 5.0);
        r.set_formula("Sheet1", "B1", "=A1+1");

        let deps = dependents_of("A1", "Sheet1", &r);
        assert_eq!(deps, vec![("Sheet1".to_string(), "B1".to_string())]);
    }

    #[test]
    fn recalc_chain_linear_order() {
        // A1 changes; B1 = A1+1; C1 = B1*2. Expected order: B1, then C1.
        let mut r = ResolverWithFormulas::new(&["Sheet1"]);
        r.set_num("Sheet1", "A1", 1.0);
        r.set_formula("Sheet1", "B1", "=A1+1");
        r.set_formula("Sheet1", "C1", "=B1*2");
        let chain = recalc_chain("A1", "Sheet1", &r).unwrap();
        assert_eq!(
            chain,
            vec![
                ("Sheet1".to_string(), "B1".to_string()),
                ("Sheet1".to_string(), "C1".to_string()),
            ]
        );
    }

    #[test]
    fn recalc_chain_detects_cycle() {
        let mut r = ResolverWithFormulas::new(&["Sheet1"]);
        r.set_formula("Sheet1", "A1", "=B1+1");
        r.set_formula("Sheet1", "B1", "=A1+1");
        let res = recalc_chain("A1", "Sheet1", &r);
        assert!(matches!(res, Err(CycleError { .. })));
    }

    #[test]
    fn parse_marks_unknown_function_as_needs_browser() {
        // "BOGUSXYZ" is not a real Excel function and should not be registered.
        // If formualizer ever adds it, swap for another guaranteed-missing name.
        assert!(
            !is_supported_fn("BOGUSXYZ"),
            "precondition: BOGUSXYZ must be unsupported"
        );
        let outcome = parse("=BOGUSXYZ(A1)");
        match outcome {
            ParseOutcome::NeedsBrowser { unsupported_fns } => {
                assert!(
                    unsupported_fns
                        .iter()
                        .any(|n| n.eq_ignore_ascii_case("BOGUSXYZ")),
                    "expected BOGUSXYZ in unsupported_fns, got {unsupported_fns:?}"
                );
            }
            other => panic!("expected NeedsBrowser, got {other:?}"),
        }
    }
}
