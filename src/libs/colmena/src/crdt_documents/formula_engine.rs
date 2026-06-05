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
    pub(crate) ast: ASTNode,
    pub(crate) original_text: String,
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
#[derive(Debug, thiserror::Error)]
pub enum EvalError {
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
/// Excel-semantic errors (`#DIV/0!`, `#REF!`, etc.) are returned inside
/// `EvalValue::Error(_)`. `EvalError::Internal` is only returned for genuine
/// infrastructure failures (today there are none — kept for forward
/// compatibility with D-T4's YrsResolver).
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
pub fn function_names(formula: &ParsedFormula) -> Vec<String> {
    let mut names = Vec::new();
    walk_for_fns(&formula.ast.node_type, &mut names);
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
    let mut all = function_names(formula);
    all.retain(|n| !is_supported_fn(n));
    all.sort();
    all.dedup();
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
}
