# CRDT Formulas Implementation Plan (Subsystem D)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make formulas a first-class data type in colmena's CRDT spreadsheet backend — agents and users can write `=SUM(A1:A10)` style cells, evaluation happens server-side via the `formualizer` crate, and reads return evaluated scalars by default (preserving pandas compatibility).

**Architecture:** One new module (`crdt_documents::formula_engine`) that wraps `formualizer` behind a thin `CellResolver` trait. The existing in-proc cell mutation entry point (`apply_set_cell_in_proc`) gains formula detection + recalc-chain propagation. Reads stay back-compat by default; new `include_formulas: bool` flag opts into formula-aware shape.

**Tech Stack:** Rust 1.95, `yrs` (CRDT), `formualizer = "0.6"` (new dep), `serde_json`, existing colmena infra (tool dispatchers, projection, df_writer, skill loader, CRDT event store).

**Reference spec:** [`docs/superpowers/specs/2026-06-04-crdt-formulas-design.md`](../specs/2026-06-04-crdt-formulas-design.md)

---

## File Structure

### Create

| Path | Responsibility |
|---|---|
| `src/libs/colmena/src/crdt_documents/formula_engine.rs` | Parse, evaluate, dependents_of, recalc_chain. CellResolver trait. ~300 LOC. |
| `src/libs/colmena/src/crdt_documents/formula_engine_yrs_resolver.rs` | Production `CellResolver` impl that reads from a `&yrs::Doc`. Kept in its own file so `formula_engine.rs` stays yrs-free. ~80 LOC. |
| `docs/superpowers/notes/2026-06-04-formualizer-api.md` | Spike output: exact formualizer signatures verified to exist in v0.6 (parse fn, ast node enum, evaluator entry, etc.). Referenced by every subsequent task. |
| `tests/formula_engine_unit.rs` | Standalone unit tests for parse/eval/recalc using an in-memory `CellResolver` stub. |
| `tests/formula_divergence.rs` | Anti-divergence benchmark (`#[ignore]`-gated; requires Playwright + Chromium env). |
| `tests/graphs/agents/crdt_doc_formulas.json` | DAG smoke graph with 5 scenarios. |
| `src/libs/colmena/skills/crdt-doc-formulas/SKILL.md` | Skill body for formula patterns (write/read-evaluated/needs_browser). |
| `src/libs/colmena/skills/crdt-doc-formulas/patterns/write-formula.md` | Pattern file referenced lazily by SKILL.md. |
| `src/libs/colmena/skills/crdt-doc-formulas/patterns/read-with-formulas.md` | Pattern file referenced lazily by SKILL.md. |
| `src/libs/colmena/skills/crdt-doc-formulas/patterns/needs-browser-fallback.md` | Pattern file referenced lazily by SKILL.md. |

### Modify

| Path | Change |
|---|---|
| `src/libs/colmena/Cargo.toml` | Add `formualizer = "0.6"` to `[dependencies]`. |
| `src/libs/colmena/src/crdt_documents/mod.rs` | `pub mod formula_engine; pub mod formula_engine_yrs_resolver;`. |
| `src/libs/colmena/src/crdt_documents/tool_executor.rs` | Replace `apply_set_cell_in_proc()` unit return with `SetCellOutcome`. Add formula-detection branch + recalc-chain pass. |
| `src/libs/colmena/src/crdt_documents/projection.rs` | Extend the cell-projection helper to accept an `include_formulas: bool`. When true, emit `{v, f?, fs?}` map per cell; when false, emit scalar `v` (current behavior). |
| `src/libs/colmena/src/crdt_documents/df_writer.rs` | Before writing back from pandas: if prior cell had `f`, emit `formula_replaced_by_literal` event and remove `f`/`fs`. After batch, run `recalc_chain` for each overwritten address. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs` | (a) `crdt_doc_set_cell` / `crdt_doc_set_range` dispatchers return `cells_recalculated` + `warnings` in tool result. (b) `crdt_doc_read` accepts `include_formulas: bool`. (c) `crdt_doc_list_sheets` returns `formula_count` per sheet. |
| `docs/developer_guide/38_crdt_documents.md` | Add §5.8 "Formulas" with read/write/fallback flows + linked skill. |
| `docs/node_configurations.json` | Update tool entries for set_cell/set_range/read/list_sheets to reflect new fields. |
| `docs/BACKLOG.md` | Add 7 items from spec §11 under "Subsystem D v1.1". |
| `docs/CHANGELOG_2026-06.md` | Add D entry: bullet list of capability + breaking change note (`=text` strings now parsed). |

---

## Task 1: Spike formualizer API + add dep + smoke

**Files:**
- Modify: `src/libs/colmena/Cargo.toml`
- Create: `docs/superpowers/notes/2026-06-04-formualizer-api.md`
- Create: `src/libs/colmena/src/crdt_documents/formualizer_smoke.rs` (temporary, removed at end of task)
- Modify: `src/libs/colmena/src/crdt_documents/mod.rs` (temp `mod formualizer_smoke;`, removed at end)

- [ ] **Step 1: Add formualizer dep**

Open `src/libs/colmena/Cargo.toml`. Under `[dependencies]`, in alphabetical/grouped position next to other formula-adjacent libs (after `chrono` line is fine), add:

```toml
# Formula engine (D-T1 spike onwards)
formualizer = "0.6"
```

- [ ] **Step 2: Run cargo check to verify the dep resolves**

Run: `cargo check -p colmena_dag_engine --lib 2>&1 | tail -20`
Expected: build succeeds, no errors. If a feature flag is required, the compiler will tell you — read the error, enable the matching feature in the `formualizer = { version = "0.6", features = ["..."] }` line, repeat.

- [ ] **Step 3: Write the spike binary**

Create `src/libs/colmena/src/crdt_documents/formualizer_smoke.rs` with:

```rust
//! TEMPORARY — D-T1 spike. Removed once notes/2026-06-04-formualizer-api.md exists.
//! Confirms formualizer's parse + evaluate APIs work end-to-end on a literal "=2+2".

#[cfg(test)]
mod tests {
    #[test]
    fn formualizer_parses_and_evaluates_2_plus_2() {
        // The exact entry points may differ — try, in order:
        //   1. `formualizer::parse("=2+2")` + `formualizer::evaluate(&ast, &ctx)`
        //   2. `formualizer_parse::Parser::new(...)` + `formualizer_eval::interpreter::*`
        //   3. `formualizer::Workbook::new()` + `wb.set_formula("Sheet1", "A1", "=2+2")` + `wb.get_value("Sheet1", "A1")`
        //
        // Document whichever shape works in notes/2026-06-04-formualizer-api.md.
        // For now, leave this body empty so the test compiles even before we
        // know the API — the compiler errors when we add real calls will guide us.
        let _ = ();
    }
}
```

Add `pub mod formualizer_smoke;` to `src/libs/colmena/src/crdt_documents/mod.rs` (temporary).

- [ ] **Step 4: Iterate API discovery**

Replace the empty body with the first hypothesis, run the test:

```bash
cargo test -p colmena_dag_engine --lib formualizer_parses_and_evaluates_2_plus_2 2>&1 | tail -30
```

If it errors, read the error, adjust the call sites, repeat. Use `cargo doc --no-deps -p formualizer --open` to browse the local docs if needed:

```bash
cargo doc --no-deps -p formualizer 2>&1 | tail -5
open target/doc/formualizer/index.html  # macOS — adjust for your OS
```

Stop iterating when the test PASSES and the body looks like real code, e.g.:

```rust
let result = formualizer::evaluate_one("=2+2").expect("eval");
assert_eq!(result.as_f64(), Some(4.0));
```

(Above is one plausible shape — yours will match the real API.)

- [ ] **Step 5: Write the notes file with the verified API**

Create `docs/superpowers/notes/2026-06-04-formualizer-api.md`:

```markdown
# formualizer 0.6 — Verified API (D-T1 spike)

This file is the canonical reference for the formualizer API as wired into
colmena's `formula_engine` module. Verified against formualizer v0.6.0 on
2026-06-04 by the D-T1 smoke test.

## Parse

```rust
use formualizer::<PARSE_PATH_HERE>;

let ast = <PARSE_CALL>("=SUM(A1:A10)")?;
```

Returned type: `<TYPE_NAME_HERE>`. Errors: `<ERR_TYPE>`.

## Evaluate

Trait users implement to provide cell values:

```rust
trait <EVAL_CTX_TRAIT_HERE> {
    fn <METHOD_NAME>(&self, ...) -> ...;
}
```

Entry point:

```rust
let value: <VALUE_TYPE> = <EVAL_FN>(&ast, &my_ctx)?;
```

## Cell reference shape

References parse as: `<CELLREF_TYPE_HERE>`. Range refs: `<RANGEREF_TYPE>`.

## Function support detection

To check whether a function is supported (for `needs_browser` fallback):

```rust
<FN_SUPPORTED_CHECK_CALL>
```

## Notes / gotchas

- ...
```

Fill in every `<...>` placeholder with the actual names/types you confirmed work. Examples for each gotcha you hit during step 4.

- [ ] **Step 6: Remove the temporary smoke file**

```bash
rm src/libs/colmena/src/crdt_documents/formualizer_smoke.rs
```

Remove `pub mod formualizer_smoke;` from `src/libs/colmena/src/crdt_documents/mod.rs`.

Run: `cargo check -p colmena_dag_engine --lib`
Expected: still builds.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/Cargo.toml docs/superpowers/notes/2026-06-04-formualizer-api.md src/libs/colmena/src/crdt_documents/mod.rs
git commit -m "$(cat <<'EOF'
feat(D-T1): add formualizer dep + document verified API surface

Adds formualizer = "0.6" dependency. The actual API shape (parse fn,
evaluator entry, cell-context trait, function-support check) is captured
in docs/superpowers/notes/2026-06-04-formualizer-api.md and referenced
by all downstream D-T* tasks so they don't fabricate signatures.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `formula_engine` module — CellResolver trait + parse + evaluate

**Files:**
- Create: `src/libs/colmena/src/crdt_documents/formula_engine.rs`
- Modify: `src/libs/colmena/src/crdt_documents/mod.rs`
- Reference: `docs/superpowers/notes/2026-06-04-formualizer-api.md`

- [ ] **Step 1: Read the spike notes**

Open `docs/superpowers/notes/2026-06-04-formualizer-api.md` and keep it open while implementing. Every `formualizer::*` call in this task must match the verified API there.

- [ ] **Step 2: Write the first failing test — parse a literal**

Create `src/libs/colmena/src/crdt_documents/formula_engine.rs` with:

```rust
//! Backend formula engine. Wraps `formualizer` behind a thin trait so the
//! rest of the codebase doesn't depend on it directly.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CellSnapshot {
    pub v: serde_json::Value,
    pub t: u8,
}

/// What a parse can produce.
#[derive(Debug)]
pub enum ParseOutcome {
    Ok(ParsedFormula),
    NeedsBrowser { unsupported_fns: Vec<String> },
    ParseError(String),
}

/// Opaque wrapper around the formualizer AST so callers don't depend on it.
#[derive(Debug)]
pub struct ParsedFormula {
    // Filled in step 3.
}

pub fn parse(text: &str) -> ParseOutcome {
    if !text.starts_with('=') {
        return ParseOutcome::ParseError("not a formula (missing leading =)".to_string());
    }
    // Implementation in step 3.
    todo!("step 3")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_non_formula() {
        let outcome = parse("hello");
        match outcome {
            ParseOutcome::ParseError(msg) => assert!(msg.contains("missing leading")),
            other => panic!("expected ParseError, got {other:?}"),
        }
    }
}
```

Add `pub mod formula_engine;` to `src/libs/colmena/src/crdt_documents/mod.rs`.

- [ ] **Step 3: Run the test, verify it fails on the `todo!`**

Run: `cargo test -p colmena_dag_engine --lib formula_engine::tests::parse_rejects_non_formula 2>&1 | tail -10`
Expected: test runs; the `parse("hello")` branch returns ParseError successfully (no `todo!` hit). Test PASSES.

Now write the second test for the parse-happy-path that WILL hit `todo!`:

```rust
    #[test]
    fn parse_accepts_simple_formula() {
        let outcome = parse("=1+1");
        assert!(matches!(outcome, ParseOutcome::Ok(_)));
    }
```

Run: `cargo test -p colmena_dag_engine --lib formula_engine::tests::parse_accepts_simple_formula 2>&1 | tail -10`
Expected: FAIL with `not yet implemented` (the `todo!`).

- [ ] **Step 4: Implement parse using formualizer**

Following `docs/superpowers/notes/2026-06-04-formualizer-api.md`, replace the `todo!()` body with a real call. Wrap the formualizer AST inside `ParsedFormula`. Map the parser's error type to `ParseOutcome::ParseError(format!("{e:?}"))`.

The shape will look like:

```rust
pub struct ParsedFormula {
    pub(crate) ast: formualizer::<TYPE_FROM_NOTES>,
    pub(crate) original_text: String,
}

pub fn parse(text: &str) -> ParseOutcome {
    if !text.starts_with('=') {
        return ParseOutcome::ParseError("not a formula (missing leading =)".to_string());
    }
    match formualizer::<PARSE_FN_FROM_NOTES>(text) {
        Ok(ast) => ParseOutcome::Ok(ParsedFormula {
            ast,
            original_text: text.to_string(),
        }),
        Err(e) => ParseOutcome::ParseError(format!("{e:?}")),
    }
}
```

Replace `<TYPE_FROM_NOTES>` and `<PARSE_FN_FROM_NOTES>` with the verified names from the notes file.

- [ ] **Step 5: Run the two tests + verify both pass**

Run: `cargo test -p colmena_dag_engine --lib formula_engine::tests 2>&1 | tail -10`
Expected: 2 passed.

- [ ] **Step 6: Add the CellResolver trait + EvalValue + evaluate signature**

Append to `formula_engine.rs`:

```rust
/// Source of a cell's value as reported in the Y.Doc `fs` field.
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

#[derive(Debug, Clone, PartialEq)]
pub enum EvalValue {
    Number(f64),
    String(String),
    Bool(bool),
    Error(ExcelError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExcelError {
    DivZero,    // #DIV/0!
    Ref,        // #REF!
    Name,       // #NAME?
    Value,      // #VALUE!
    Num,        // #NUM!
    NA,         // #N/A
    Cycle,      // #CYCLE!  (our extension, used by recalc_chain)
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
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("internal evaluator error: {0}")]
    Internal(String),
}

/// What `formula_engine` needs from the world. Production impl reads from
/// a `&yrs::Doc`; tests use an in-memory stub.
pub trait CellResolver {
    fn get(&self, sheet: &str, addr: &str) -> Option<CellSnapshot>;
    fn sheet_exists(&self, sheet: &str) -> bool;
}

pub fn evaluate(
    formula: &ParsedFormula,
    resolver: &dyn CellResolver,
    current_sheet: &str,
) -> Result<EvalValue, EvalError> {
    let _ = (formula, resolver, current_sheet);
    todo!("step 8")
}
```

- [ ] **Step 7: Write the first eval test using a stub resolver**

Append to the `tests` module:

```rust
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
                CellSnapshot { v: serde_json::json!(v), t: 2 },
            );
        }
    }

    impl CellResolver for StubResolver {
        fn get(&self, sheet: &str, addr: &str) -> Option<CellSnapshot> {
            self.cells.get(&(sheet.to_string(), addr.to_string())).cloned()
        }
        fn sheet_exists(&self, sheet: &str) -> bool {
            self.sheets.iter().any(|s| s == sheet)
        }
    }

    #[test]
    fn evaluate_simple_arithmetic() {
        let r = StubResolver::new(&["Sheet1"]);
        let ParseOutcome::Ok(ast) = parse("=2+3*4") else { panic!() };
        let v = evaluate(&ast, &r, "Sheet1").unwrap();
        assert_eq!(v, EvalValue::Number(14.0));
    }

    #[test]
    fn evaluate_cell_ref() {
        let mut r = StubResolver::new(&["Sheet1"]);
        r.set_num("Sheet1", "A1", 7.5);
        let ParseOutcome::Ok(ast) = parse("=A1*2") else { panic!() };
        let v = evaluate(&ast, &r, "Sheet1").unwrap();
        assert_eq!(v, EvalValue::Number(15.0));
    }

    #[test]
    fn evaluate_range_sum() {
        let mut r = StubResolver::new(&["Sheet1"]);
        r.set_num("Sheet1", "A1", 1.0);
        r.set_num("Sheet1", "A2", 2.0);
        r.set_num("Sheet1", "A3", 3.0);
        let ParseOutcome::Ok(ast) = parse("=SUM(A1:A3)") else { panic!() };
        let v = evaluate(&ast, &r, "Sheet1").unwrap();
        assert_eq!(v, EvalValue::Number(6.0));
    }

    #[test]
    fn evaluate_div_by_zero_returns_error_value() {
        let r = StubResolver::new(&["Sheet1"]);
        let ParseOutcome::Ok(ast) = parse("=1/0") else { panic!() };
        let v = evaluate(&ast, &r, "Sheet1").unwrap();
        assert_eq!(v, EvalValue::Error(ExcelError::DivZero));
    }
```

- [ ] **Step 8: Implement evaluate using formualizer**

Following the notes file, implement `evaluate` by:

1. Creating an adapter from `&dyn CellResolver` to whatever ctx trait formualizer's evaluator expects (`<EVAL_CTX_TRAIT_HERE>` from the notes).
2. Calling `<EVAL_FN_FROM_NOTES>(&formula.ast, &adapter, current_sheet)`.
3. Mapping the returned formualizer value (number/string/bool/error) to `EvalValue`.
4. Mapping formualizer's error variants to `ExcelError::*`.

Run: `cargo test -p colmena_dag_engine --lib formula_engine::tests 2>&1 | tail -15`
Expected: all 6 tests pass (parse_rejects, parse_accepts, evaluate_simple_arithmetic, evaluate_cell_ref, evaluate_range_sum, evaluate_div_by_zero).

- [ ] **Step 9: Add unsupported-function detection helper**

Add to `formula_engine.rs`:

```rust
/// List the function names referenced in a parsed formula. Used to decide
/// `needs_browser` when any name isn't in formualizer's registry.
pub fn function_names(formula: &ParsedFormula) -> Vec<String> {
    // Walk formula.ast collecting Func/Call node names.
    // Exact AST node names from notes file.
    todo!("walk ast")
}

/// Returns true if every function in `names` is supported by formualizer.
pub fn all_supported(names: &[String]) -> bool {
    names.iter().all(|n| is_supported_fn(n))
}

pub fn is_supported_fn(name: &str) -> bool {
    // Either ask formualizer's registry directly (per notes file), or
    // fall back to a static set if the registry isn't queryable.
    todo!("registry lookup")
}
```

Add test:

```rust
    #[test]
    fn function_names_extracts_sum() {
        let ParseOutcome::Ok(ast) = parse("=SUM(A1:A10)") else { panic!() };
        let names = function_names(&ast);
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case("SUM")));
    }

    #[test]
    fn is_supported_returns_true_for_sum() {
        assert!(is_supported_fn("SUM"));
    }
```

Implement the two `todo!`s referencing the notes file, then run:

```
cargo test -p colmena_dag_engine --lib formula_engine::tests 2>&1 | tail -15
```

Expected: all 8 tests pass.

- [ ] **Step 10: Commit**

```bash
git add src/libs/colmena/src/crdt_documents/formula_engine.rs src/libs/colmena/src/crdt_documents/mod.rs
git commit -m "$(cat <<'EOF'
feat(D-T2): formula_engine core — CellResolver, parse, evaluate, fn names

ParsedFormula opaque wrapper, CellResolver trait, EvalValue + ExcelError
enums, parse() with leading-= guard, evaluate() routing to formualizer
with stub-resolver-backed unit tests, function_names()/is_supported_fn()
helpers for the needs_browser detection.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `dependents_of` + `recalc_chain` (topo sort + cycle detection)

**Files:**
- Modify: `src/libs/colmena/src/crdt_documents/formula_engine.rs`

- [ ] **Step 1: Write the failing tests for `dependents_of`**

Append to `tests`:

```rust
    #[test]
    fn dependents_of_finds_direct_reference() {
        let mut r = StubResolver::new(&["Sheet1"]);
        r.set_num("Sheet1", "A1", 5.0);
        // B1 has a formula referencing A1
        r.cells.insert(
            ("Sheet1".to_string(), "B1".to_string()),
            CellSnapshot {
                v: serde_json::json!(0),
                t: 2,
            },
        );
        // We also need to teach StubResolver to track formulas on cells.
        // Extend the snapshot struct or add a parallel formulas map.

        // For this test we add a method:
        let mut r2 = ResolverWithFormulas::new(&["Sheet1"]);
        r2.set_num("Sheet1", "A1", 5.0);
        r2.set_formula("Sheet1", "B1", "=A1+1");

        let deps = dependents_of("A1", "Sheet1", &r2);
        assert_eq!(deps, vec![("Sheet1".to_string(), "B1".to_string())]);
    }
```

- [ ] **Step 2: Extend `CellResolver` with formula lookup**

Update the trait in `formula_engine.rs`:

```rust
pub trait CellResolver {
    fn get(&self, sheet: &str, addr: &str) -> Option<CellSnapshot>;
    fn sheet_exists(&self, sheet: &str) -> bool;

    /// Iterate every cell that has a formula text, scoped to one sheet.
    /// Returned tuples: (addr, formula_text). Order arbitrary.
    fn iter_formulas_in_sheet<'a>(
        &'a self,
        sheet: &str,
    ) -> Box<dyn Iterator<Item = (String, String)> + 'a>;
}
```

Update `StubResolver` test fixture to track formulas:

```rust
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
                CellSnapshot { v: serde_json::json!(v), t: 2 },
            );
        }
        pub fn set_formula(&mut self, sheet: &str, addr: &str, f: &str) {
            self.formulas
                .insert((sheet.to_string(), addr.to_string()), f.to_string());
        }
    }

    impl CellResolver for ResolverWithFormulas {
        fn get(&self, sheet: &str, addr: &str) -> Option<CellSnapshot> {
            self.cells.get(&(sheet.to_string(), addr.to_string())).cloned()
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
```

Add the same `iter_formulas_in_sheet` impl to the original `StubResolver` returning an empty iterator (so the older arithmetic tests still compile).

- [ ] **Step 3: Add `referenced_cells` helper to `formula_engine`**

Add a public helper that returns the set of (sheet, addr) references inside a parsed formula:

```rust
/// Return the list of (sheet, addr) pairs referenced by `formula`. The
/// `current_sheet` is used when a reference has no explicit sheet prefix.
/// Range references expand to every cell in the range.
pub fn referenced_cells(formula: &ParsedFormula, current_sheet: &str) -> Vec<(String, String)> {
    // Walk formula.ast collecting cell+range refs; expand ranges; assign
    // current_sheet when no sheet prefix is present. AST visitor pattern.
    todo!("walk ast")
}
```

Add tests:

```rust
    #[test]
    fn referenced_cells_single_ref() {
        let ParseOutcome::Ok(ast) = parse("=A1+1") else { panic!() };
        let refs = referenced_cells(&ast, "Sheet1");
        assert_eq!(refs, vec![("Sheet1".to_string(), "A1".to_string())]);
    }

    #[test]
    fn referenced_cells_range_expanded() {
        let ParseOutcome::Ok(ast) = parse("=SUM(A1:A3)") else { panic!() };
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
        let ParseOutcome::Ok(ast) = parse("=Sheet2!A1+B2") else { panic!() };
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
```

Implement `referenced_cells` per the notes file (AST node walker). Run tests until all three pass:

```bash
cargo test -p colmena_dag_engine --lib formula_engine::tests::referenced_cells 2>&1 | tail
```

- [ ] **Step 4: Implement `dependents_of`**

Add to `formula_engine.rs`:

```rust
/// Find every cell in `sheet` whose formula directly references `(sheet, changed_addr)`.
/// Cross-sheet dependents are NOT returned in v1 (intra-sheet only).
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
```

Run: `cargo test -p colmena_dag_engine --lib formula_engine::tests::dependents_of 2>&1 | tail`
Expected: PASS.

- [ ] **Step 5: Write the failing test for recalc_chain (linear)**

```rust
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
```

- [ ] **Step 6: Implement recalc_chain**

```rust
#[derive(Debug, thiserror::Error)]
#[error("cycle detected: {chain:?}")]
pub struct CycleError {
    pub chain: Vec<(String, String)>,
}

/// Topo-sort dependents starting from `changed`. Output excludes `changed`
/// itself and is in evaluation order. Returns Err on cycle.
pub fn recalc_chain(
    changed_addr: &str,
    sheet: &str,
    resolver: &dyn CellResolver,
) -> Result<Vec<(String, String)>, CycleError> {
    use std::collections::{HashMap, HashSet, VecDeque};

    // BFS over dependents (intra-sheet only in v1).
    let mut visited: HashSet<(String, String)> = HashSet::new();
    let mut in_degree: HashMap<(String, String), usize> = HashMap::new();
    let mut adj: HashMap<(String, String), Vec<(String, String)>> = HashMap::new();

    let mut frontier: VecDeque<(String, String)> =
        VecDeque::from([(sheet.to_string(), changed_addr.to_string())]);
    while let Some(cur) = frontier.pop_front() {
        if !visited.insert(cur.clone()) {
            continue;
        }
        for dep in dependents_of(&cur.1, &cur.0, resolver) {
            adj.entry(cur.clone()).or_default().push(dep.clone());
            *in_degree.entry(dep.clone()).or_insert(0) += 1;
            frontier.push_back(dep);
        }
    }

    // Kahn's algorithm starting from nodes with in_degree=0 (= the changed cell).
    let mut order: Vec<(String, String)> = Vec::new();
    let mut queue: VecDeque<(String, String)> = visited
        .iter()
        .filter(|n| in_degree.get(*n).copied().unwrap_or(0) == 0)
        .cloned()
        .collect();

    while let Some(n) = queue.pop_front() {
        if n != (sheet.to_string(), changed_addr.to_string()) {
            order.push(n.clone());
        }
        if let Some(neighbors) = adj.get(&n) {
            for m in neighbors {
                let entry = in_degree.entry(m.clone()).or_insert(0);
                *entry -= 1;
                if *entry == 0 {
                    queue.push_back(m.clone());
                }
            }
        }
    }

    if order.len() + 1 != visited.len() {
        // Some node was never queued → cycle.
        let cycle_members: Vec<_> = visited
            .into_iter()
            .filter(|n| in_degree.get(n).copied().unwrap_or(0) > 0)
            .collect();
        return Err(CycleError { chain: cycle_members });
    }
    Ok(order)
}
```

- [ ] **Step 7: Run all formula_engine tests**

```bash
cargo test -p colmena_dag_engine --lib formula_engine 2>&1 | tail -20
```

Expected: all tests pass (parse, evaluate, function_names, is_supported_fn, referenced_cells, dependents_of, recalc_chain linear + cycle).

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/crdt_documents/formula_engine.rs
git commit -m "$(cat <<'EOF'
feat(D-T3): dependents_of + recalc_chain with cycle detection

referenced_cells walks the AST and expands ranges; dependents_of scans
the sheet's formula map; recalc_chain does Kahn-style topo sort and
returns CycleError when the dep graph has a cycle. All intra-sheet for
v1 — cross-sheet recalc deferred per spec §11.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Production `CellResolver` impl over `&yrs::Doc`

**Files:**
- Create: `src/libs/colmena/src/crdt_documents/formula_engine_yrs_resolver.rs`
- Modify: `src/libs/colmena/src/crdt_documents/mod.rs`

- [ ] **Step 1: Skeleton + first failing test**

Create `src/libs/colmena/src/crdt_documents/formula_engine_yrs_resolver.rs`:

```rust
//! Production CellResolver: reads from a `&yrs::Doc`. Kept in its own file so
//! `formula_engine.rs` stays yrs-free and trivially testable.

use crate::crdt_documents::formula_engine::{CellResolver, CellSnapshot};
use yrs::{Any, Doc, Map, ReadTxn, Transact};

pub struct YrsResolver<'a> {
    doc: &'a Doc,
}

impl<'a> YrsResolver<'a> {
    pub fn new(doc: &'a Doc) -> Self {
        Self { doc }
    }
}

impl<'a> CellResolver for YrsResolver<'a> {
    fn get(&self, sheet: &str, addr: &str) -> Option<CellSnapshot> {
        let txn = self.doc.transact();
        let workbook = txn.get_map("workbook")?;
        let sheets = match workbook.get(&txn, "sheets")? {
            yrs::Out::YArray(a) => a,
            _ => return None,
        };
        for i in 0..sheets.len(&txn) {
            let yrs::Out::YMap(s) = sheets.get(&txn, i)? else { continue };
            let yrs::Out::Any(Any::String(id)) = s.get(&txn, "id")? else { continue };
            if id.as_ref() == sheet {
                let yrs::Out::YMap(cells) = s.get(&txn, "cells")? else { return None };
                let yrs::Out::YMap(cell) = cells.get(&txn, addr)? else { return None };
                let v = match cell.get(&txn, "v")? {
                    yrs::Out::Any(a) => any_to_json(a),
                    _ => return None,
                };
                let t = match cell.get(&txn, "t")? {
                    yrs::Out::Any(Any::BigInt(n)) => n as u8,
                    yrs::Out::Any(Any::Number(n)) => n as u8,
                    _ => 1,
                };
                return Some(CellSnapshot { v, t });
            }
        }
        None
    }

    fn sheet_exists(&self, sheet: &str) -> bool {
        let txn = self.doc.transact();
        let Some(yrs::Out::YMap(workbook)) = txn.get_map("workbook").map(|m| yrs::Out::YMap(m)) else {
            return false;
        };
        let Some(yrs::Out::YArray(sheets)) = workbook.get(&txn, "sheets") else {
            return false;
        };
        for i in 0..sheets.len(&txn) {
            if let Some(yrs::Out::YMap(s)) = sheets.get(&txn, i) {
                if let Some(yrs::Out::Any(Any::String(id))) = s.get(&txn, "id") {
                    if id.as_ref() == sheet {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn iter_formulas_in_sheet<'b>(
        &'b self,
        sheet: &str,
    ) -> Box<dyn Iterator<Item = (String, String)> + 'b> {
        let txn = self.doc.transact();
        let mut out: Vec<(String, String)> = Vec::new();
        let Some(workbook) = txn.get_map("workbook") else {
            return Box::new(out.into_iter());
        };
        let Some(yrs::Out::YArray(sheets)) = workbook.get(&txn, "sheets") else {
            return Box::new(out.into_iter());
        };
        for i in 0..sheets.len(&txn) {
            let Some(yrs::Out::YMap(s)) = sheets.get(&txn, i) else { continue };
            let Some(yrs::Out::Any(Any::String(id))) = s.get(&txn, "id") else { continue };
            if id.as_ref() != sheet {
                continue;
            }
            let Some(yrs::Out::YMap(cells)) = s.get(&txn, "cells") else { continue };
            for key in cells.keys(&txn) {
                let Some(yrs::Out::YMap(cell)) = cells.get(&txn, &key) else { continue };
                if let Some(yrs::Out::Any(Any::String(f))) = cell.get(&txn, "f") {
                    out.push((key.to_string(), f.to_string()));
                }
            }
        }
        Box::new(out.into_iter())
    }
}

fn any_to_json(a: Any) -> serde_json::Value {
    match a {
        Any::Null | Any::Undefined => serde_json::Value::Null,
        Any::Bool(b) => serde_json::json!(b),
        Any::Number(n) => serde_json::json!(n),
        Any::BigInt(n) => serde_json::json!(n),
        Any::String(s) => serde_json::json!(s.as_ref()),
        Any::Buffer(_) | Any::Array(_) | Any::Map(_) => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::tool_executor::apply_set_cell_in_proc;

    #[test]
    fn yrs_resolver_reads_literal_cell() {
        let doc = Doc::new();
        apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(42));
        let r = YrsResolver::new(&doc);
        let cell = r.get("Sheet1", "A1").expect("A1");
        assert_eq!(cell.v, serde_json::json!(42));
    }

    #[test]
    fn yrs_resolver_reports_sheet_existence() {
        let doc = Doc::new();
        apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(1));
        let r = YrsResolver::new(&doc);
        assert!(r.sheet_exists("Sheet1"));
        assert!(!r.sheet_exists("Sheet99"));
    }
}
```

Add `pub mod formula_engine_yrs_resolver;` to `mod.rs`.

- [ ] **Step 2: Run the two tests**

```bash
cargo test -p colmena_dag_engine --lib formula_engine_yrs_resolver 2>&1 | tail -10
```

Expected: 2 passed. If `apply_set_cell_in_proc` signature changed in your branch already (anticipating D-T5), call sites might need adjustment — but at this point it should still be the original `()`-returning version.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/crdt_documents/formula_engine_yrs_resolver.rs src/libs/colmena/src/crdt_documents/mod.rs
git commit -m "$(cat <<'EOF'
feat(D-T4): YrsResolver — production CellResolver over &yrs::Doc

Wraps a Doc with read-only access to cells.v/cells.t/cells.f scoped to a
named sheet. Iter helper enumerates only cells that have a formula
text, which is what dependents_of needs.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Extend `apply_set_cell_in_proc` to detect + evaluate + recalc

**Files:**
- Modify: `src/libs/colmena/src/crdt_documents/tool_executor.rs`

- [ ] **Step 1: Add the SetCellOutcome type + new signature (still empty body)**

Open `src/libs/colmena/src/crdt_documents/tool_executor.rs`. Add at the top of the file:

```rust
use crate::crdt_documents::formula_engine::{
    self, all_supported, evaluate, function_names, parse, recalc_chain, EvalValue, ExcelError,
    FormulaSource, ParseOutcome,
};
use crate::crdt_documents::formula_engine_yrs_resolver::YrsResolver;

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct SetCellOutcome {
    pub cells_recalculated: usize,
    pub warnings: Vec<SetCellWarning>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind")]
pub enum SetCellWarning {
    #[serde(rename = "needs_browser")]
    NeedsBrowser { addr: String, functions: Vec<String> },
    #[serde(rename = "eval_error")]
    EvalError { addr: String, error: String },
    #[serde(rename = "cycle")]
    Cycle { chain: Vec<(String, String)> },
}
```

Change the signature of `apply_set_cell_in_proc` from `()` to `SetCellOutcome`. Leave the existing body in place — just add `SetCellOutcome::default()` at the end as the return value.

- [ ] **Step 2: Update every existing call site**

Run: `grep -rn "apply_set_cell_in_proc" src/ tests/ 2>&1 | head`

Expected to find call sites in `df_writer.rs`, `df_records.rs` (tests), `narration.rs` (tests), `xlsx_export.rs` (tests), `tool_executor.rs` tests, and the WS variant `apply_set_cell_via_ws`.

In every test-only call site, the call's return value was previously unused — `let _ = apply_set_cell_in_proc(...);` is fine. In non-test call sites (`df_writer.rs`, `apply_set_cell_via_ws`), preserve the new outcome locally so we can use it in later tasks: `let _outcome = apply_set_cell_in_proc(...);` (we'll inspect it in T8).

Run: `cargo check -p colmena_dag_engine --lib 2>&1 | tail -10`
Expected: builds clean.

- [ ] **Step 3: Write the failing test — formula detection writes f + fs**

Append to the `tests` module in `tool_executor.rs`:

```rust
    #[test]
    fn set_cell_persists_formula_and_evaluated_value() {
        let doc = Doc::new();
        // Seed dependencies.
        apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(2));
        apply_set_cell_in_proc(&doc, "Sheet1", "A2", &serde_json::json!(3));
        apply_set_cell_in_proc(&doc, "Sheet1", "A3", &serde_json::json!(5));

        // Write the formula.
        let outcome = apply_set_cell_in_proc(
            &doc,
            "Sheet1",
            "B1",
            &serde_json::json!("=SUM(A1:A3)"),
        );

        // Read B1 back via YrsResolver.
        let r = YrsResolver::new(&doc);
        let cell = r.get("Sheet1", "B1").expect("B1");
        assert_eq!(cell.v, serde_json::json!(10.0));
        // Verify f + fs are stored.
        // (Read raw via yrs since YrsResolver only exposes v/t.)
        let txn = doc.transact();
        let workbook = txn.get_map("workbook").unwrap();
        let sheets = match workbook.get(&txn, "sheets").unwrap() {
            yrs::Out::YArray(a) => a,
            _ => panic!(),
        };
        let yrs::Out::YMap(sheet) = sheets.get(&txn, 0).unwrap() else { panic!() };
        let yrs::Out::YMap(cells) = sheet.get(&txn, "cells").unwrap() else { panic!() };
        let yrs::Out::YMap(b1) = cells.get(&txn, "B1").unwrap() else { panic!() };
        let yrs::Out::Any(yrs::Any::String(f)) = b1.get(&txn, "f").unwrap() else { panic!() };
        assert_eq!(f.as_ref(), "=SUM(A1:A3)");
        let yrs::Out::Any(yrs::Any::String(fs)) = b1.get(&txn, "fs").unwrap() else { panic!() };
        assert_eq!(fs.as_ref(), "be");

        assert_eq!(outcome.cells_recalculated, 0); // No dependents on B1 yet.
        assert!(outcome.warnings.is_empty());
    }
```

Run: `cargo test -p colmena_dag_engine --lib set_cell_persists_formula 2>&1 | tail -10`
Expected: FAIL (we haven't implemented formula path yet — cell `v` is the formula text, not 10.0; no `f`/`fs` written).

- [ ] **Step 4: Implement formula detection branch**

Inside `apply_set_cell_in_proc`, before the existing `let (any, type_tag) = json_to_any(value); cell.insert(...);` lines, branch on whether `value` is a string starting with `=`:

```rust
    // ── Formula path ────────────────────────────────────────────────
    let formula_text = value.as_str().filter(|s| s.starts_with('='));
    if let Some(text) = formula_text {
        // Inside the same transaction we already started, parse + eval +
        // write {v, t, f, fs}. We need to drop the txn first to call
        // YrsResolver (which transact()s read-only); easier path: do the
        // parse OUTSIDE the txn block above, then mutate.
        //
        // Refactor: pull the txn open lower in the function, or split the
        // function so the formula path computes the value BEFORE mutation.
    }
```

The simplest refactor:

1. Move all `let mut txn = doc.transact_mut();` and subsequent mutation into a helper `write_cell_raw(doc, sheet_id, addr, &v_any, t_tag, formula_text_opt, fs_opt)`.
2. The main `apply_set_cell_in_proc` body becomes:

```rust
pub fn apply_set_cell_in_proc(
    doc: &Doc,
    sheet_id: &str,
    addr: &str,
    value: &Value,
) -> SetCellOutcome {
    let mut outcome = SetCellOutcome::default();

    // Branch on formula vs literal.
    if let Some(text) = value.as_str().filter(|s| s.starts_with('=')) {
        match parse(text) {
            ParseOutcome::Ok(ast) => {
                let fns = function_names(&ast);
                if !all_supported(&fns) {
                    let unsupported: Vec<String> = fns
                        .iter()
                        .filter(|n| !formula_engine::is_supported_fn(n))
                        .cloned()
                        .collect();
                    // Persist as needs_browser placeholder.
                    write_cell_raw(
                        doc,
                        sheet_id,
                        addr,
                        &Value::String(text.to_string()),
                        1, // string
                        Some(text),
                        Some(FormulaSource::NeedsBrowser),
                    );
                    outcome.warnings.push(SetCellWarning::NeedsBrowser {
                        addr: addr.to_string(),
                        functions: unsupported,
                    });
                    return outcome;
                }
                // Evaluate
                let resolver = YrsResolver::new(doc);
                let (eval_v, eval_t) = match evaluate(&ast, &resolver, sheet_id) {
                    Ok(EvalValue::Number(n)) => (serde_json::json!(n), 2u8),
                    Ok(EvalValue::String(s)) => (serde_json::json!(s), 1u8),
                    Ok(EvalValue::Bool(b)) => (serde_json::json!(b), 3u8),
                    Ok(EvalValue::Error(err)) => {
                        outcome.warnings.push(SetCellWarning::EvalError {
                            addr: addr.to_string(),
                            error: err.as_excel().to_string(),
                        });
                        (serde_json::json!(err.as_excel()), 4u8)
                    }
                    Err(e) => {
                        outcome.warnings.push(SetCellWarning::EvalError {
                            addr: addr.to_string(),
                            error: format!("internal: {e}"),
                        });
                        (
                            serde_json::json!(ExcelError::Value.as_excel()),
                            4u8,
                        )
                    }
                };
                write_cell_raw(
                    doc,
                    sheet_id,
                    addr,
                    &eval_v,
                    eval_t,
                    Some(text),
                    Some(FormulaSource::Backend),
                );
                // Recalc dependents.
                let resolver = YrsResolver::new(doc);
                match recalc_chain(addr, sheet_id, &resolver) {
                    Ok(chain) => {
                        for (sh, ad) in &chain {
                            let formula_text = resolver
                                .iter_formulas_in_sheet(sh)
                                .find(|(a, _)| a == ad)
                                .map(|(_, t)| t);
                            if let Some(ft) = formula_text {
                                if let ParseOutcome::Ok(dep_ast) = parse(&ft) {
                                    let resolver_inner = YrsResolver::new(doc);
                                    let (dv, dt) = match evaluate(&dep_ast, &resolver_inner, sh) {
                                        Ok(EvalValue::Number(n)) => (serde_json::json!(n), 2u8),
                                        Ok(EvalValue::String(s)) => (serde_json::json!(s), 1u8),
                                        Ok(EvalValue::Bool(b)) => (serde_json::json!(b), 3u8),
                                        Ok(EvalValue::Error(e)) => {
                                            (serde_json::json!(e.as_excel()), 4u8)
                                        }
                                        Err(_) => (
                                            serde_json::json!(ExcelError::Value.as_excel()),
                                            4u8,
                                        ),
                                    };
                                    // Preserve original f / fs on dependent.
                                    write_cell_raw(doc, sh, ad, &dv, dt, Some(&ft), Some(FormulaSource::Backend));
                                }
                            }
                        }
                        outcome.cells_recalculated = chain.len();
                    }
                    Err(cycle) => {
                        outcome.warnings.push(SetCellWarning::Cycle {
                            chain: cycle.chain,
                        });
                    }
                }
                return outcome;
            }
            ParseOutcome::ParseError(_) | ParseOutcome::NeedsBrowser { .. } => {
                // Fall through to literal write below — agent's tool dispatcher
                // will surface the parse error separately.
            }
        }
    }

    // ── Literal path (original logic) ───────────────────────────────
    let (any, type_tag) = json_to_any(value);
    write_cell_raw(doc, sheet_id, addr, value, type_tag, None, None);
    outcome
}
```

Implement `write_cell_raw` by moving the existing `let mut txn = doc.transact_mut(); ...` block into a function taking `(doc, sheet_id, addr, value_json, type_tag, formula_text_opt, fs_opt)` and writing `v`, `t`, optionally `f` and `fs`. When `formula_text_opt` is `None`, also explicitly remove any pre-existing `f` and `fs` keys on the cell (this is the "literal overwrites formula" case at the y-doc level — D-T8 emits the event around it).

Run: `cargo test -p colmena_dag_engine --lib set_cell_persists_formula 2>&1 | tail`
Expected: PASS.

- [ ] **Step 5: Add the recalc-cascade test**

Append:

```rust
    #[test]
    fn set_cell_recalculates_dependents_in_topo_order() {
        let doc = Doc::new();
        apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(1));
        apply_set_cell_in_proc(&doc, "Sheet1", "B1", &serde_json::json!("=A1+10")); // 11
        apply_set_cell_in_proc(&doc, "Sheet1", "C1", &serde_json::json!("=B1*2"));  // 22

        // Change A1; B1 and C1 must update.
        let outcome = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(5));

        assert_eq!(outcome.cells_recalculated, 2);

        let r = YrsResolver::new(&doc);
        assert_eq!(r.get("Sheet1", "B1").unwrap().v, serde_json::json!(15.0));
        assert_eq!(r.get("Sheet1", "C1").unwrap().v, serde_json::json!(30.0));
    }
```

Run: `cargo test -p colmena_dag_engine --lib set_cell_recalculates_dependents 2>&1 | tail`
Expected: PASS.

- [ ] **Step 6: Add needs_browser test**

```rust
    #[test]
    fn set_cell_with_unsupported_function_marks_needs_browser() {
        // Pick a function that formualizer does NOT support. If formualizer
        // actually supports XLOOKUP, swap for something genuinely missing in
        // the registry — confirm via formula_engine::is_supported_fn before
        // committing.
        let doc = Doc::new();
        let outcome = apply_set_cell_in_proc(
            &doc,
            "Sheet1",
            "A1",
            &serde_json::json!("=XLOOKUP(\"x\", A:A, B:B)"),
        );
        match outcome.warnings.as_slice() {
            [SetCellWarning::NeedsBrowser { addr, functions }] => {
                assert_eq!(addr, "A1");
                assert!(functions.iter().any(|f| f.eq_ignore_ascii_case("XLOOKUP")));
            }
            _ => panic!("expected one NeedsBrowser warning, got {:?}", outcome.warnings),
        }
        let r = YrsResolver::new(&doc);
        let cell = r.get("Sheet1", "A1").unwrap();
        // v is the formula text placeholder.
        assert_eq!(cell.v, serde_json::json!("=XLOOKUP(\"x\", A:A, B:B)"));
    }
```

If formualizer DOES support `XLOOKUP`, replace with whatever the notes file confirms is genuinely missing (e.g. `=GEMINI()` won't exist). Document the choice in a code comment so the test is robust.

Run: `cargo test -p colmena_dag_engine --lib set_cell_with_unsupported_function 2>&1 | tail`
Expected: PASS.

- [ ] **Step 7: Run the full tool_executor test suite + clippy**

```bash
cargo test -p colmena_dag_engine --lib tool_executor 2>&1 | tail -20
cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings 2>&1 | tail -20
```

Expected: all passing, no clippy warnings.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/crdt_documents/tool_executor.rs
git commit -m "$(cat <<'EOF'
feat(D-T5): apply_set_cell_in_proc evaluates formulas + cascades recalc

When value starts with '=', parse + check function support; supported →
evaluate via YrsResolver and persist {v,t,f,fs:'be'}, then recalc all
intra-sheet dependents in topo order; unsupported function set → persist
{v,t:1,f,fs:'needs_browser'} placeholder and emit a NeedsBrowser warning.
Returns SetCellOutcome carrying cells_recalculated + warnings so dispatchers
can surface them to the agent.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `crdt_doc_read` — `include_formulas` flag

**Files:**
- Modify: `src/libs/colmena/src/crdt_documents/projection.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs`

- [ ] **Step 1: Inspect current projection API**

Run: `grep -n "pub fn\|pub struct" src/libs/colmena/src/crdt_documents/projection.rs | head -20`

Identify the existing function that turns a sheet's cells into the JSON shape pandas reads. Likely named `project_sheet`, `cells_to_records`, or similar.

- [ ] **Step 2: Add include_formulas variant**

Add a new public function alongside (don't break the existing one):

```rust
/// Like the default project but each cell becomes `{v}` or `{v,f,fs}` when
/// it has formula metadata. Used by `crdt_doc_read(include_formulas=true)`.
pub fn project_sheet_with_formulas(
    doc: &yrs::Doc,
    sheet_id: &str,
) -> Vec<serde_json::Map<String, serde_json::Value>> {
    use yrs::{Any, Map, Out, ReadTxn, Transact};
    let txn = doc.transact();
    let mut out: Vec<serde_json::Map<String, serde_json::Value>> = Vec::new();
    let Some(workbook) = txn.get_map("workbook") else { return out };
    let Some(Out::YArray(sheets)) = workbook.get(&txn, "sheets") else { return out };
    for i in 0..sheets.len(&txn) {
        let Some(Out::YMap(sheet)) = sheets.get(&txn, i) else { continue };
        let Some(Out::Any(Any::String(id))) = sheet.get(&txn, "id") else { continue };
        if id.as_ref() != sheet_id { continue; }
        let Some(Out::YMap(cells)) = sheet.get(&txn, "cells") else { continue };

        // Determine row count by parsing addresses (A1 → row 1, etc.).
        let mut by_row: std::collections::BTreeMap<u32, serde_json::Map<String, serde_json::Value>> =
            std::collections::BTreeMap::new();
        for key in cells.keys(&txn) {
            let Some(Out::YMap(cell)) = cells.get(&txn, &key) else { continue };
            let (_col_letters, row_num) = match split_a1(&key) {
                Some(p) => p,
                None => continue,
            };
            let mut entry = serde_json::Map::new();
            if let Some(Out::Any(a)) = cell.get(&txn, "v") {
                entry.insert("v".to_string(), any_to_json(a));
            }
            if let Some(Out::Any(Any::String(f))) = cell.get(&txn, "f") {
                entry.insert("f".to_string(), serde_json::json!(f.to_string()));
            }
            if let Some(Out::Any(Any::String(fs))) = cell.get(&txn, "fs") {
                entry.insert("fs".to_string(), serde_json::json!(fs.to_string()));
            }
            by_row
                .entry(row_num)
                .or_insert_with(serde_json::Map::new)
                .insert(key.to_string(), serde_json::Value::Object(entry));
        }
        for (_row, row_map) in by_row {
            out.push(row_map);
        }
        break;
    }
    out
}

fn split_a1(addr: &str) -> Option<(String, u32)> {
    let split = addr.find(|c: char| c.is_ascii_digit())?;
    let (col, row) = addr.split_at(split);
    let row_num: u32 = row.parse().ok()?;
    Some((col.to_string(), row_num))
}

fn any_to_json(a: yrs::Any) -> serde_json::Value {
    use yrs::Any;
    match a {
        Any::Null | Any::Undefined => serde_json::Value::Null,
        Any::Bool(b) => serde_json::json!(b),
        Any::Number(n) => serde_json::json!(n),
        Any::BigInt(n) => serde_json::json!(n),
        Any::String(s) => serde_json::json!(s.as_ref()),
        Any::Buffer(_) | Any::Array(_) | Any::Map(_) => serde_json::Value::Null,
    }
}
```

If `split_a1` or `any_to_json` already exist in this file, reuse them — don't duplicate.

- [ ] **Step 3: Write the failing test**

Append to `projection.rs` tests module:

```rust
#[cfg(test)]
mod formula_projection_tests {
    use super::*;
    use crate::crdt_documents::tool_executor::apply_set_cell_in_proc;
    use yrs::Doc;

    #[test]
    fn project_with_formulas_emits_v_f_fs() {
        let doc = Doc::new();
        apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(5));
        apply_set_cell_in_proc(&doc, "Sheet1", "B1", &serde_json::json!("=A1*2"));
        let out = project_sheet_with_formulas(&doc, "Sheet1");
        assert_eq!(out.len(), 1);
        let row = &out[0];
        // A1 has no formula → just {v:5}.
        let a1 = row["A1"].as_object().unwrap();
        assert_eq!(a1["v"], serde_json::json!(5));
        assert!(a1.get("f").is_none());
        // B1 has formula → {v:10, f:"=A1*2", fs:"be"}.
        let b1 = row["B1"].as_object().unwrap();
        assert_eq!(b1["v"], serde_json::json!(10.0));
        assert_eq!(b1["f"], serde_json::json!("=A1*2"));
        assert_eq!(b1["fs"], serde_json::json!("be"));
    }
}
```

Run: `cargo test -p colmena_dag_engine --lib formula_projection_tests 2>&1 | tail`
Expected: PASS.

- [ ] **Step 4: Extend the `crdt_doc_read` tool dispatcher**

Open `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs`. Locate the `ReadArgs` struct and the `dispatch_crdt_doc_read` function (or whatever the existing names are).

Add a new optional argument:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadArgs {
    pub sheet: String,
    #[serde(default)]
    pub include_formulas: bool,
}
```

In the dispatcher body, branch on `args.include_formulas`:

```rust
let records: Vec<_> = if args.include_formulas {
    crate::crdt_documents::projection::project_sheet_with_formulas(&doc, &args.sheet)
        .into_iter()
        .map(serde_json::Value::Object)
        .collect()
} else {
    // existing call to the default projection
};
```

Update the tool description text so the agent knows about the flag (one extra sentence is enough — keep it brief; full details are in the skill).

- [ ] **Step 5: Add integration smoke for dispatcher**

If the file has dispatcher-level unit tests (it does for the other tools — search for `#[tokio::test]` or `#[test]` in the file), add:

```rust
    #[tokio::test]
    async fn dispatch_read_with_include_formulas_returns_f_keys() {
        // Use the same scaffolding pattern as the existing read tests.
        // Seed: A1=5, B1="=A1*2". Call dispatcher with include_formulas=true.
        // Assert one of the returned rows has B1 with keys v, f, fs.
        // ... see existing test for the exact fixture builder used here.
    }
```

If the file has no such tests, skip this step — D-T10's integration graph covers it.

- [ ] **Step 6: Build + test**

```bash
cargo test -p colmena_dag_engine --lib crdt_doc_tools 2>&1 | tail -15
cargo test -p colmena_dag_engine --lib projection 2>&1 | tail -10
cargo clippy -p colmena_dag_engine --lib -- -D warnings 2>&1 | tail
```

Expected: pass + no warnings.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/crdt_documents/projection.rs src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs
git commit -m "$(cat <<'EOF'
feat(D-T6): crdt_doc_read include_formulas — formula-aware projection

Default read shape unchanged (scalars, pandas-compat). Opt-in to {v,f,fs}
per cell via include_formulas:true. project_sheet_with_formulas() walks
the y-doc emitting one map per row keyed by A1 address; cells without a
formula stay as {v} only.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `list_sheets` returns `formula_count`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs`
- Or modify the projection module if `list_sheets` lives there.

- [ ] **Step 1: Locate list_sheets dispatcher**

Run: `grep -rn "list_sheets\|ListSheets" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/ 2>&1 | head`

- [ ] **Step 2: Extend the response shape**

Identify the response struct (e.g. `SheetInfo { name, rows, cols }`). Add `formula_count: u32`. The count is the number of cells in the sheet that have a non-empty `f` key.

In the dispatcher, after assembling the sheet list, iterate each sheet's cells once and count:

```rust
let formula_count = {
    use yrs::{Any, Map, Out, ReadTxn, Transact};
    let txn = doc.transact();
    let mut n: u32 = 0;
    if let Some(Out::YMap(workbook)) = txn.get_map("workbook").map(Out::YMap) {
        if let Some(Out::YArray(sheets)) = workbook.get(&txn, "sheets") {
            for i in 0..sheets.len(&txn) {
                if let Some(Out::YMap(s)) = sheets.get(&txn, i) {
                    if let Some(Out::Any(Any::String(id))) = s.get(&txn, "id") {
                        if id.as_ref() == sheet_name {
                            if let Some(Out::YMap(cells)) = s.get(&txn, "cells") {
                                for k in cells.keys(&txn) {
                                    if let Some(Out::YMap(cell)) = cells.get(&txn, &k) {
                                        if cell.get(&txn, "f").is_some() {
                                            n += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    n
};
```

- [ ] **Step 3: Test it**

```rust
    #[tokio::test]
    async fn list_sheets_reports_formula_count() {
        let doc = Doc::new();
        apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(1));
        apply_set_cell_in_proc(&doc, "Sheet1", "B1", &serde_json::json!("=A1+1"));
        apply_set_cell_in_proc(&doc, "Sheet1", "C1", &serde_json::json!("=A1*2"));
        // Use the same dispatcher harness as other list_sheets tests.
        // Assert Sheet1 has formula_count == 2.
    }
```

Run: `cargo test -p colmena_dag_engine --lib list_sheets_reports_formula_count 2>&1 | tail`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs
git commit -m "$(cat <<'EOF'
feat(D-T7): crdt_doc_list_sheets reports formula_count per sheet

Lets the agent decide whether to pay the cost of include_formulas=true.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: `df_writer` — strip f/fs + emit replaced event + cascade recalc

**Files:**
- Modify: `src/libs/colmena/src/crdt_documents/df_writer.rs`

- [ ] **Step 1: Locate the write-back path**

Run: `grep -n "pub fn\|set_cell\|insert" src/libs/colmena/src/crdt_documents/df_writer.rs | head -20`

Find where each cell is written back to the doc.

- [ ] **Step 2: Failing test**

Append to `df_writer.rs` tests:

```rust
    #[test]
    fn df_writer_overwriting_formula_clears_f_and_fs() {
        let doc = Doc::new();
        apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(10));
        apply_set_cell_in_proc(&doc, "Sheet1", "B1", &serde_json::json!("=A1*2")); // 20

        // Build a 1-row dataframe-equivalent that overwrites B1 with 999.
        let records = vec![serde_json::json!({"A1": 10, "B1": 999})];
        // Call the actual write path used by run_python.
        apply_records_to_doc(&doc, "Sheet1", &records).unwrap();

        // Read B1 raw.
        let txn = doc.transact();
        use yrs::{Out, ReadTxn};
        let workbook = txn.get_map("workbook").unwrap();
        let Out::YArray(sheets) = workbook.get(&txn, "sheets").unwrap() else { panic!() };
        let Out::YMap(sheet) = sheets.get(&txn, 0).unwrap() else { panic!() };
        let Out::YMap(cells) = sheet.get(&txn, "cells").unwrap() else { panic!() };
        let Out::YMap(b1) = cells.get(&txn, "B1").unwrap() else { panic!() };
        assert!(b1.get(&txn, "f").is_none(), "f should be removed");
        assert!(b1.get(&txn, "fs").is_none(), "fs should be removed");
        // v is the new literal.
        let Out::Any(yrs::Any::Number(v)) = b1.get(&txn, "v").unwrap() else { panic!() };
        assert!((v - 999.0).abs() < 1e-9);
    }
```

(Adjust the call to `apply_records_to_doc` to match the actual public function name in `df_writer.rs`.)

Run the test → FAIL (f/fs remain).

- [ ] **Step 3: Implement the strip + event emit**

Around the line that writes the new value into a cell that may previously have had a formula, add:

```rust
use yrs::{Any, Out, ReadTxn};
// Before writing the new value:
let prior_formula: Option<String> = {
    let txn = doc.transact();
    // Walk to the cell; pull f if present.
    walk_cell_for_formula(&txn, sheet_id, addr)
};
if let Some(prior) = prior_formula {
    // Emit CRDT event. df_writer already has access to the change_tracker
    // via runtime — look for existing event-emit helper used by C-T4.
    record_formula_replaced_by_literal(/* args */ sheet_id, addr, &prior).await;
}

// Now do the write. Inside the txn_mut, remove f and fs keys if present.
let mut txn = doc.transact_mut();
// ... find cell map ...
cell.remove(&mut txn, "f");
cell.remove(&mut txn, "fs");
// ... write new v and t ...
```

`walk_cell_for_formula` is a small read helper (~10 lines). `record_formula_replaced_by_literal` reuses whatever event-emission path `df_writer` already uses for "cell updated" events from C — check the existing code for the helper name and call shape.

- [ ] **Step 4: Add the recalc-cascade after batch**

After all records in the batch are written, collect the set of (sheet, addr) that changed and run `recalc_chain` for each, evaluating + writing dependents the same way D-T5 does for `set_cell`. Refactor the relevant block into a helper if it's getting big — but keep the change scoped to `df_writer.rs`.

```rust
// Collect overwritten addresses.
let mut changed: Vec<(String, String)> = Vec::new();
for record in records {
    for (col_letter, _) in record.as_object().unwrap() {
        // col_letter is the A1 column part; combine with row index.
        changed.push((sheet_id.to_string(), format!("{col_letter}{row_idx}")));
    }
    // (adjust the iteration to match the actual schema)
}

let resolver = YrsResolver::new(doc);
let mut chain_set: std::collections::BTreeSet<(String, String)> = Default::default();
for (sh, ad) in &changed {
    if let Ok(chain) = recalc_chain(ad, sh, &resolver) {
        for tuple in chain {
            chain_set.insert(tuple);
        }
    }
}
for (sh, ad) in chain_set {
    // Re-evaluate that cell's formula and write back (same shape as D-T5 recalc loop).
}
```

Pull the per-cell re-evaluation block into a small helper in `tool_executor.rs` (pub fn `recompute_dependent(doc, sheet, addr)`) so both `set_cell` and `df_writer` call it instead of duplicating the eval logic.

- [ ] **Step 5: Test both behaviours**

Add a second test:

```rust
    #[test]
    fn df_writer_overwriting_input_recalculates_dependent_formula() {
        let doc = Doc::new();
        apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(10));
        apply_set_cell_in_proc(&doc, "Sheet1", "B1", &serde_json::json!("=A1*2")); // 20

        // Overwrite A1 (no formula on A1).
        let records = vec![serde_json::json!({"A1": 50, "B1": 20})];
        apply_records_to_doc(&doc, "Sheet1", &records).unwrap();

        // B1 still has its formula, recalculated to 100.
        let r = YrsResolver::new(&doc);
        let b1 = r.get("Sheet1", "B1").unwrap();
        assert_eq!(b1.v, serde_json::json!(100.0));
    }
```

Run: `cargo test -p colmena_dag_engine --lib df_writer 2>&1 | tail`
Expected: both PASS.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/crdt_documents/df_writer.rs src/libs/colmena/src/crdt_documents/tool_executor.rs
git commit -m "$(cat <<'EOF'
feat(D-T8): df_writer strips f/fs + emits replaced event + cascades recalc

When pandas overwrites a cell that had a formula, the formula text and
source marker are removed and a formula_replaced_by_literal CRDT event
is emitted so the UI/peer log surfaces the action. After the batch of
records is applied, any dependent formulas (intra-sheet) recompute via
the same code path set_cell uses, ensuring derived columns stay coherent.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Skill `crdt-doc-formulas`

**Files:**
- Create: `src/libs/colmena/skills/crdt-doc-formulas/SKILL.md`
- Create: `src/libs/colmena/skills/crdt-doc-formulas/patterns/write-formula.md`
- Create: `src/libs/colmena/skills/crdt-doc-formulas/patterns/read-with-formulas.md`
- Create: `src/libs/colmena/skills/crdt-doc-formulas/patterns/needs-browser-fallback.md`

- [ ] **Step 1: Inspect existing skill format**

Read `src/libs/colmena/skills/crdt-doc-run-python/SKILL.md` to confirm frontmatter conventions (the F-T12 work standardized this).

- [ ] **Step 2: Write SKILL.md**

Create `src/libs/colmena/skills/crdt-doc-formulas/SKILL.md`:

```markdown
---
name: crdt-doc-formulas
description: Use when writing or reading Excel-style formulas in a CRDT spreadsheet. Includes the {v,f,fs} cell schema, when to use include_formulas=true, and how to react to needs_browser warnings.
---

# crdt-doc-formulas

Backend understands `=...` formulas. Three things to know:

1. **Writing formulas.** Just call `crdt_doc_set_cell(sheet, addr, "=SUM(A1:A10)")` — the leading `=` triggers parse+evaluate server-side. Dependent cells auto-recalculate. See `patterns/write-formula.md` for examples and the `cells_recalculated` field in the tool response.

2. **Reading formulas vs values.** Default reads return scalar values (pandas-friendly). Pass `include_formulas=true` when you need to see the formula text — useful for auditing, modifying an existing formula, or differentiating a literal `42` from `=2*21`. See `patterns/read-with-formulas.md`.

3. **Browser-only functions.** Some Excel functions (e.g. XLOOKUP if formualizer lacks it, custom add-ins) can't be evaluated server-side. Cells use `fs="needs_browser"` and the value is the formula text itself as placeholder. When you see a `needs_browser` warning, decide: ignore the cell, ask the user to open the workbook in Univer to refresh, or rewrite the formula using supported functions. See `patterns/needs-browser-fallback.md`.

## Quick reference

- Output of `crdt_doc_set_cell` with a formula: `{ok, cells_recalculated, warnings: []}`.
- Output of `crdt_doc_set_cell` with unsupported function: `warnings: [{kind:"needs_browser", addr, functions}]`. Cell IS written, value is the formula text.
- Output of `crdt_doc_read(include_formulas=true)`: each cell is `{v}` (literal) or `{v,f,fs}` (formula).
- `crdt_doc_list_sheets` returns `formula_count` per sheet — check before calling include_formulas=true to avoid noise.

## When NOT to use formulas

For derived values produced by `run_python`, prefer writing the computed numbers directly — pandas evaluation runs once at write-time, while server-side formulas re-evaluate every time an input changes. Use formulas when you want the formula to be visible to the user / live-updating, literals when you just want a snapshot.
```

- [ ] **Step 3: Pattern file — write-formula**

`patterns/write-formula.md`:

```markdown
# Pattern: Writing formulas to cells

## Basic — single formula

```jsonc
// tool call
{
  "name": "crdt_doc_set_cell",
  "arguments": {
    "sheet": "Sheet1",
    "addr": "C2",
    "value": "=B2*0.21"
  }
}

// expected result
{
  "ok": true,
  "cells_recalculated": 0,
  "warnings": []
}
```

`cells_recalculated` is non-zero if other cells already had formulas referencing C2.

## Multi-cell batch — derived column

```jsonc
{
  "name": "crdt_doc_set_range",
  "arguments": {
    "sheet": "Sheet1",
    "start_addr": "C2",
    "values": [["=B2*0.21"], ["=B3*0.21"], ["=B4*0.21"]]
  }
}
```

The batch evaluates each cell, then runs one recalc pass over the union of dependents. Output includes `total_cells_recalculated`.

## Evaluation errors

If a formula evaluates to `#DIV/0!` etc., the cell IS written (Excel-compatible: a cell with an error value is a valid state). The tool result includes:

```jsonc
{
  "ok": true,
  "cells_recalculated": 0,
  "warnings": [{"kind": "eval_error", "addr": "D2", "error": "#DIV/0!"}]
}
```

You can choose to: ignore (the user will see the error chip), rewrite the formula, or `set_cell` over it with a sentinel.
```

- [ ] **Step 4: Pattern file — read-with-formulas**

`patterns/read-with-formulas.md`:

```markdown
# Pattern: Reading cells with formula text

## Default — scalar values (pandas-friendly)

```jsonc
{
  "name": "crdt_doc_read",
  "arguments": { "sheet": "Sheet1" }
}
// → [{"A1": 5, "B1": 10}, {"A1": 7, "B1": 14}]
```

Use this shape any time you want to feed cells to `run_python`. pandas sees scalars and reads naturally.

## Formula-aware — include_formulas=true

```jsonc
{
  "name": "crdt_doc_read",
  "arguments": { "sheet": "Sheet1", "include_formulas": true }
}
// → [
//     {"A1": {"v": 5}, "B1": {"v": 10, "f": "=A1*2", "fs": "be"}},
//     {"A1": {"v": 7}, "B1": {"v": 14, "f": "=A1*2", "fs": "be"}}
//   ]
```

Cells without a formula stay as `{v}` only. Use this shape when:

- You want to know whether a value was computed or typed.
- You're about to rewrite a formula (read the existing one first).
- The user asked "why is this cell showing X" — the formula is the answer.

## Workflow tip

Before calling `include_formulas=true`, call `crdt_doc_list_sheets`: if `formula_count: 0`, skip the formula-aware read.
```

- [ ] **Step 5: Pattern file — needs-browser-fallback**

`patterns/needs-browser-fallback.md`:

```markdown
# Pattern: Handling needs_browser warnings

When `crdt_doc_set_cell` returns:

```jsonc
{
  "ok": true,
  "cells_recalculated": 0,
  "warnings": [{
    "kind": "needs_browser",
    "addr": "E2",
    "functions": ["XLOOKUP"]
  }]
}
```

The cell IS written, but the backend can't evaluate it. `fs:"needs_browser"` and `v` is the formula text as placeholder. Three responses:

## 1. Accept — the user will see it computed when they open Univer

If the workflow continues only when a human reviews it anyway, do nothing. Move on. Univer evaluates `XLOOKUP` correctly client-side.

## 2. Rewrite — find a supported equivalent

For lookups, `INDEX/MATCH` is widely supported:

```text
Before: =XLOOKUP(key, A:A, B:B, "")
After:  =IFERROR(INDEX(B:B, MATCH(key, A:A, 0)), "")
```

Then `set_cell` again with the rewrite. Surface to user: "I used INDEX/MATCH instead of XLOOKUP because the backend evaluator doesn't support XLOOKUP — visually identical."

## 3. Ask the user

If the function can't be replaced (custom add-in, sparkline, dynamic array), tell the user the cell needs them to open Univer once to materialise the value, then continue.
```

- [ ] **Step 6: Verify the skill loads via the auto-discovery infra**

Run the skills test suite (whichever F-T12 added):

```bash
cargo test -p colmena_dag_engine --lib skill 2>&1 | tail
```

Expected: no failures. If F-T12's infra walks `src/libs/colmena/skills/` and validates frontmatter, this skill will be picked up.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/skills/crdt-doc-formulas/
git commit -m "$(cat <<'EOF'
docs(D-T9): skill crdt-doc-formulas — 3 patterns

SKILL.md indexes write/read/needs_browser patterns. Patterns live in
separate files so they load lazily (see F-T12 skill-out-of-history /
layered tool context).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Integration test graph + smoke

**Files:**
- Create: `tests/graphs/agents/crdt_doc_formulas.json`

- [ ] **Step 1: Pattern an existing CRDT smoke graph**

```bash
ls tests/graphs/agents/ | grep -i crdt
cat tests/graphs/agents/<latest crdt graph>.json | head -80
```

Copy the structure as your starting point.

- [ ] **Step 2: Write the formulas smoke graph**

Create `tests/graphs/agents/crdt_doc_formulas.json` with one trigger node, one llm_call agent (using `google/gemini-2.5-flash` per project defaults), and `lazy_tool_loading: true`. System message instructs the agent to:

```
1. Create a sheet named "Sheet1".
2. Set A1..A5 to 10, 20, 30, 40, 50.
3. Set B1 to formula =SUM(A1:A5). Report cells_recalculated.
4. Change A3 to 100. Verify B1 auto-recalculates.
5. Read sheet with include_formulas=true. Confirm B1 has f="=SUM(A1:A5)" and fs="be".
6. Set C1 to "=GEMINI()" (an unsupported function — adjust per
   docs/superpowers/notes/2026-06-04-formualizer-api.md). Confirm
   warnings includes needs_browser and the cell's v is the formula text.
7. Use run_python to overwrite B1 with literal 999. Verify the formula was removed.
8. Final report: list everything that happened in <200 words.
```

Wire `enabled_tools` for all `crdt_doc_*` tools plus `crdt_doc_run_python`. Add `node_skills: ["crdt-doc-formulas", "crdt-doc-run-python"]` so the agent gets the patterns in-context.

Save the graph and add a brief description at the top in a comment field if your schema allows (or document in dev guide).

- [ ] **Step 3: Run the graph end-to-end**

Source the env, then:

```bash
set -a; source .env; set +a
mkdir -p /tmp/colmena_e2e
cargo run --bin dag_engine -- run tests/graphs/agents/crdt_doc_formulas.json \
  --agent-session-id agent_d_formulas_smoke_001 \
  --include-extra-info 2>&1 | tee /tmp/colmena_e2e/d_formulas_smoke.sse
```

Verify the agent:

- Reported `cells_recalculated >= 1` after step 4.
- Got the formula-aware read shape in step 5.
- Got a `needs_browser` warning in step 6 (cell visible, value = formula text).
- Showed B1 returned to a literal 999 after step 7 (no `f`/`fs`).

If any step fails, debug per the rust_dev / test_graph skill protocols; iterate the graph or implementation, do not move on until 5/5 steps work.

- [ ] **Step 4: Friendly report**

Per project convention, summarise the smoke in a short report (NOT a paste of the full SSE):
- Input prompt
- Final answer (truncated to ~300 chars)
- Tool calls made (count per tool)
- Total tokens (prompt + completion)
- Pass/fail per step

- [ ] **Step 5: Commit**

```bash
git add tests/graphs/agents/crdt_doc_formulas.json
git commit -m "$(cat <<'EOF'
test(D-T10): integration graph + smoke for formulas

Single multi-step graph covers: write formula + recalc, read with
include_formulas, needs_browser fallback, pandas overwrite removes f/fs.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Anti-divergence benchmark vs Univer (#[ignore]-gated)

**Files:**
- Create: `tests/formula_divergence.rs`
- Create: `tests/formula_divergence_fixtures.json`

- [ ] **Step 1: Define the fixture set**

Create `tests/formula_divergence_fixtures.json` listing 80 formulas in 8 families: arithmetic, SUM/AVG/COUNT, IF/AND/OR, CONCAT/LEFT/RIGHT, ROUND/INT, MIN/MAX, lookup (INDEX/MATCH), date (TODAY/DATE). Each entry is `{name, formula, seed_cells: {...}, expected_value}`:

```json
[
  {"name":"add",                "formula":"=2+2",             "seed_cells":{},          "expected_value":4},
  {"name":"sum_range",          "formula":"=SUM(A1:A3)",      "seed_cells":{"A1":1,"A2":2,"A3":3}, "expected_value":6},
  {"name":"if_true",            "formula":"=IF(A1>0,\"y\",\"n\")", "seed_cells":{"A1":5}, "expected_value":"y"},
  ...
]
```

Don't fabricate the `expected_value` — leave it `null` for fixtures where you don't have ground-truth; the test will then only check formualizer-vs-Univer parity, not absolute value.

- [ ] **Step 2: Write the test**

`tests/formula_divergence.rs`:

```rust
//! Anti-divergence benchmark: every fixture formula evaluated in both
//! formualizer (backend) and Univer (browser, via Playwright/chromiumoxide).
//! Diffs fail the build for fixtures in the v1-supported family list;
//! diffs for v1.1-deferred functions are logged but not fatal.
//!
//! Gated by #[ignore] because it requires Playwright + Chromium installed.
//! Run with: cargo test --test formula_divergence -- --ignored --nocapture

#[test]
#[ignore = "requires Playwright + Chromium env — run with `cargo test --test formula_divergence -- --ignored`"]
fn formualizer_does_not_diverge_from_univer() {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("formula_divergence_fixtures.json")).unwrap();
    let cases = fixtures.as_array().expect("fixtures is array");

    let mut diffs: Vec<String> = Vec::new();

    for case in cases {
        let name = case["name"].as_str().unwrap();
        let formula = case["formula"].as_str().unwrap();
        let seed = case["seed_cells"].as_object().cloned().unwrap_or_default();

        let be_value = eval_via_formualizer(formula, &seed);
        let fe_value = eval_via_univer(formula, &seed);

        match (be_value, fe_value) {
            (Ok(b), Ok(f)) if !values_equivalent(&b, &f) => {
                diffs.push(format!("{name}: formualizer={b:?} univer={f:?}"));
            }
            (Err(eb), Err(ef)) if !errors_equivalent(&eb, &ef) => {
                diffs.push(format!("{name}: formualizer_err={eb} univer_err={ef}"));
            }
            (Err(eb), Ok(_)) | (Ok(_), Err(eb)) => {
                diffs.push(format!("{name}: one side errored: {eb}"));
            }
            _ => {}
        }
    }

    if !diffs.is_empty() {
        for d in &diffs { eprintln!("DIVERGE: {d}"); }
        panic!("{} formulas diverged between formualizer and Univer", diffs.len());
    }
}

fn eval_via_formualizer(formula: &str, seed: &serde_json::Map<String, serde_json::Value>) -> Result<serde_json::Value, String> {
    // Use the production formula_engine module path.
    use colmena::crdt_documents::formula_engine::{evaluate, parse, CellResolver, CellSnapshot, ParseOutcome, EvalValue};
    struct M<'a> { cells: &'a serde_json::Map<String, serde_json::Value> }
    impl<'a> CellResolver for M<'a> {
        fn get(&self, _: &str, addr: &str) -> Option<CellSnapshot> {
            self.cells.get(addr).map(|v| CellSnapshot { v: v.clone(), t: 2 })
        }
        fn sheet_exists(&self, _: &str) -> bool { true }
        fn iter_formulas_in_sheet<'b>(&'b self, _: &str) -> Box<dyn Iterator<Item=(String,String)> + 'b> { Box::new(std::iter::empty()) }
    }
    let ast = match parse(formula) {
        ParseOutcome::Ok(a) => a,
        ParseOutcome::ParseError(e) => return Err(e),
        ParseOutcome::NeedsBrowser{..} => return Err("needs_browser".into()),
    };
    match evaluate(&ast, &M { cells: seed }, "Sheet1") {
        Ok(EvalValue::Number(n)) => Ok(serde_json::json!(n)),
        Ok(EvalValue::String(s)) => Ok(serde_json::json!(s)),
        Ok(EvalValue::Bool(b)) => Ok(serde_json::json!(b)),
        Ok(EvalValue::Error(e)) => Err(e.as_excel().to_string()),
        Err(e) => Err(format!("{e}")),
    }
}

fn eval_via_univer(formula: &str, seed: &serde_json::Map<String, serde_json::Value>) -> Result<serde_json::Value, String> {
    // Spawn the dag_engine `serve` binary against a tiny graph (one llm_call
    // node with crdt_documents), open a headless Chromium via chromiumoxide
    // or Playwright (subprocess), seed the cells, paste the formula into a
    // target cell, read back via the Univer JS API exposed in window.
    //
    // Implementation note: this is the heavy bit of the benchmark. The
    // straightforward path is a small shell script invoked here that runs
    // a node.js Playwright snippet — keep that snippet inside the repo at
    // tests/formula_divergence_univer_eval.mjs and shell out to it.
    todo!("Playwright shell-out implementation")
}

fn values_equivalent(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a, b) {
        (serde_json::Value::Number(x), serde_json::Value::Number(y)) => {
            let xv = x.as_f64().unwrap_or(f64::NAN);
            let yv = y.as_f64().unwrap_or(f64::NAN);
            (xv - yv).abs() < 1e-9
        }
        _ => a == b,
    }
}

fn errors_equivalent(a: &str, b: &str) -> bool {
    a.contains("#") && b.contains("#") && a == b  // exact match on excel error strings
}
```

The `todo!("Playwright shell-out implementation")` is real and intentional — this task accepts that the Playwright bridge is a follow-up effort. The test stays `#[ignore]`-gated; CI doesn't run it. We DOC it as "anti-divergence harness wired, Univer bridge pending" in BACKLOG.

- [ ] **Step 3: Verify the test compiles and is skipped by default**

```bash
cargo test --test formula_divergence 2>&1 | tail
```

Expected: 1 test, 1 ignored. No build errors.

- [ ] **Step 4: Add BACKLOG line for the Playwright bridge**

(This step lives in D-T11 if you're combining doc updates; tracked here for clarity. If T11 is being done in this same session, defer the BACKLOG edit to T11 step 1.)

- [ ] **Step 5: Commit**

```bash
git add tests/formula_divergence.rs tests/formula_divergence_fixtures.json
git commit -m "$(cat <<'EOF'
test(D-T11): anti-divergence benchmark harness — 80 fixtures + Playwright stub

Test is #[ignore]-gated. formualizer path is wired and runs against the
production CellResolver; the Univer path is a todo!() that BACKLOG tracks
as a follow-up — the harness itself is the deliverable for v1 so future
work can fill in the Playwright bridge.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Docs (dev guide §5.8, node_configurations.json, BACKLOG, CHANGELOG)

**Files:**
- Modify: `docs/developer_guide/38_crdt_documents.md`
- Modify: `docs/node_configurations.json`
- Modify: `docs/BACKLOG.md`
- Modify: `docs/CHANGELOG_2026-06.md`

- [ ] **Step 1: dev guide §5.8**

Open `docs/developer_guide/38_crdt_documents.md`. Append a new section after §5.7 (the F section):

```markdown
### 5.8 Formulas (Subsystem D)

Cells with a value starting with `=` are treated as Excel-style formulas:

- **Parsed and evaluated server-side** by [`formualizer`](https://crates.io/crates/formualizer)
  before persistence (`apply_set_cell_in_proc`).
- Cell state grows two optional keys: `f` (formula text) and
  `fs` (source: `"be"` backend, `"fe"` browser, `"needs_browser"`).
- Dependent formulas in the same sheet recalculate immediately
  (intra-sheet eager — cross-sheet recalc is v1.1).
- Reads return scalars by default. Pass `include_formulas: true`
  to `crdt_doc_read` to see the `{v, f, fs}` shape.
- `crdt_doc_list_sheets` returns `formula_count` per sheet so agents can
  decide whether to bother with the formula-aware read.
- Unsupported functions (e.g. XLOOKUP if formualizer lacks it): cell is
  written with `fs:"needs_browser"`, value = formula text as placeholder.
  Tool result includes `warnings: [{kind:"needs_browser", addr, functions}]`.
- pandas (`run_python`) writing back over a formula cell removes `f`/`fs`
  and emits a `formula_replaced_by_literal` CRDT event.

The full design is at
[`docs/superpowers/specs/2026-06-04-crdt-formulas-design.md`](../superpowers/specs/2026-06-04-crdt-formulas-design.md).
Agent-facing patterns live in the skill at
`src/libs/colmena/skills/crdt-doc-formulas/`.
```

- [ ] **Step 2: node_configurations.json**

Open `docs/node_configurations.json`. Locate the entries for `crdt_doc_set_cell`, `crdt_doc_set_range`, `crdt_doc_read`, `crdt_doc_list_sheets`. Update each:

- `crdt_doc_set_cell`: add to the "returns" / "tool_result_schema" section a `cells_recalculated: number` and `warnings: array<{kind, ...}>` field. Update the description.
- `crdt_doc_set_range`: same plus `total_cells_recalculated`.
- `crdt_doc_read`: add `include_formulas: bool, default false` to the arguments schema, plus a note that output shape changes when true.
- `crdt_doc_list_sheets`: add `formula_count: number` to each sheet entry in the output.

Follow the existing JSON-schema-style conventions in the file. Validate the file parses by:

```bash
python3 -c "import json; json.load(open('docs/node_configurations.json'))" && echo OK
```

- [ ] **Step 3: BACKLOG**

Open `docs/BACKLOG.md`. Find or create a section "Subsystem D v1.1" and add:

```markdown
## Subsystem D v1.1 (formulas)

- [ ] **Cross-sheet eager recalc** — when `Sheet2!A1` changes, dependents in
  `Sheet1` that reference it should auto-update. Today they're stale until
  someone toggles them. Spec §11.
- [ ] **`crdt_doc_recalc(sheet?, all=true)` tool** — explicit refresh,
  needed for the cross-sheet stale case and post-import scenarios.
- [ ] **Cross-artifact references** `='[OtherWB.xlsx]Sheet1'!A1`.
- [ ] **Array formulas** `{=SUM(A1:A10*B1:B10)}` — validate formualizer
  semantics, design spill UI.
- [ ] **Defined names** `=SalesTotal`.
- [ ] **AST caching** per cell to skip re-parse on recalc.
- [ ] **Univer-side `fs:"fe"` hook** — small client patch (~30 lines) so
  user-typed formulas carry `fs:"fe"` instead of `fs:undefined`.
- [ ] **Anti-divergence Playwright bridge** — finish the Univer-side
  evaluator in `tests/formula_divergence.rs`; today only the formualizer
  side is wired.
```

- [ ] **Step 4: CHANGELOG**

Open `docs/CHANGELOG_2026-06.md`. Add a D entry under the latest dated section (or create a new dated subsection):

```markdown
### D — CRDT formulas (subsystem D, v1)

- **Backend formula evaluator** via `formualizer = "0.6"`. Cells with a
  leading `=` are parsed, evaluated, and persisted with `{v, t, f, fs}`.
- **Intra-sheet eager recalc** — dependent formulas refresh in topo
  order on every `set_cell`/`set_range`.
- **`crdt_doc_read(include_formulas: bool = false)`** — back-compat
  default for pandas; opt-in formula-aware shape `{v, f?, fs?}` per cell.
- **`crdt_doc_list_sheets`** now returns `formula_count` per sheet.
- **`needs_browser` fallback** — functions outside formualizer's set are
  persisted as placeholders with a warning so the agent can decide.
- **pandas write-back** strips formula metadata and emits a
  `formula_replaced_by_literal` CRDT event.
- **Skill `crdt-doc-formulas`** — 3 patterns (write, read-evaluated,
  needs_browser fallback).
- **⚠️ BREAKING**: strings starting with `=` passed to `crdt_doc_set_cell`
  are now parsed as formulas. To store a literal `=text`, prefix with `'`
  (Excel convention). No existing test graphs do this.

Refs: spec `docs/superpowers/specs/2026-06-04-crdt-formulas-design.md`,
plan `docs/superpowers/plans/2026-06-04-crdt-formulas.md`.
```

- [ ] **Step 5: Commit**

```bash
git add docs/developer_guide/38_crdt_documents.md docs/node_configurations.json docs/BACKLOG.md docs/CHANGELOG_2026-06.md
git commit -m "$(cat <<'EOF'
docs(D-T12): dev guide §5.8 + node configs + backlog + changelog

Documents the formula data model, read/write/fallback flows, breaking
change (=text strings now parse), and 8 v1.1 backlog items.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Final sweep

**Files:** none modified — verification only.

- [ ] **Step 1: Full cargo test**

```bash
cargo test --verbose 2>&1 | tail -30
```

Expected: all pass (unit + integration + doctests). If any test fails, fix it before continuing — don't proceed with broken tests.

- [ ] **Step 2: Clippy with deny-warnings**

```bash
cargo clippy --all-targets -- -D warnings 2>&1 | tail -15
```

Expected: zero warnings.

- [ ] **Step 3: fmt**

```bash
cargo fmt --all -- --check 2>&1 | tail
```

Expected: no diffs. If diffs appear, run `cargo fmt --all` and commit the fmt-only changes.

- [ ] **Step 4: Re-run the smoke graph from D-T10**

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/crdt_doc_formulas.json \
  --agent-session-id agent_d_final_sweep_001 \
  --include-extra-info 2>&1 | tee /tmp/colmena_e2e/d_final_sweep.sse
```

Expected: all 7 steps pass (same criteria as D-T10).

- [ ] **Step 5: Friendly final report**

Summarise to the user:

- Tasks completed: 13 (D-T1 through D-T13).
- LOC added vs. estimated.
- Tests added: unit (count), integration graphs (count), divergence harness (count fixtures).
- Smoke pass rate: 7/7 (assuming).
- Token consumption of the smoke vs. baseline.
- BACKLOG items deferred: 8.
- Known limitations called out.

- [ ] **Step 6: Final commit (if any fmt-only changes from step 3)**

```bash
git status --short
# if non-empty:
git add -p
git commit -m "chore(D-T13): cargo fmt sweep"
```

---

## Self-review pass (against spec)

| Spec section | Plan coverage |
|---|---|
| §2 Goals (v1) — detect formulas | D-T5 step 4 |
| §2 Goals — evaluate via formualizer | D-T2 step 8 |
| §2 Goals — persist `{v, t, f, fs}` | D-T5 step 4 (`write_cell_raw`) |
| §2 Goals — intra-sheet recalc | D-T3 + D-T5 step 4 (recalc loop) |
| §2 Goals — back-compat reads default | D-T6 (no change to existing projection) |
| §2 Goals — `include_formulas` opt-in | D-T6 |
| §2 Goals — `needs_browser` fallback | D-T5 step 6 |
| §2 Goals — Excel errors | D-T2 step 6 (ExcelError enum) + D-T5 step 4 (mapping) |
| §2 Goals — anti-divergence benchmark | D-T11 |
| §4 Data model | D-T5 (`write_cell_raw` writes the four keys correctly) |
| §5 Component design — formula_engine API | D-T2, D-T3 |
| §5 — CellResolver | D-T2 step 6 + D-T3 step 2 (extension) + D-T4 (yrs impl) |
| §5 — `SetCellOutcome` | D-T5 step 1 |
| §5 — `crdt_doc_set_range` updates | Implicit in D-T6/T7 via dispatcher edits — **GAP: spec calls out set_range explicitly; the plan covers it only as "same as set_cell" but lacks an explicit set_range test.** Adding mitigation below. |
| §5 — `crdt_doc_list_sheets formula_count` | D-T7 |
| §6 Data flow — all 5 flows | D-T5, D-T6, D-T8 |
| §7 Error handling table | D-T5 step 4 + D-T2 ExcelError variants |
| §8 Testing strategy | D-T2, D-T3 (unit), D-T10 (integration), D-T11 (divergence) |
| §10 Migration / back-compat | D-T12 CHANGELOG breaking-change note |
| §11 Out-of-scope BACKLOG | D-T12 step 3 |

**Gap identified:** the spec calls out `crdt_doc_set_range` as a distinct
dispatcher with `total_cells_recalculated`, but no task adds an explicit
set_range test. Inline mitigation: extend D-T5 with one additional test
covering a 2x2 range write whose results sum to a recalc count > 0.

**Type consistency check:**
- `FormulaSource::Backend.as_str()` returns `"be"` in D-T2 step 6.
- D-T5 step 4 writes `Some(FormulaSource::Backend)` to `write_cell_raw`; test asserts the persisted string is `"be"`. ✓
- D-T6 projection emits the same `"be"`/`"fe"`/`"needs_browser"` strings. ✓
- `SetCellOutcome::cells_recalculated` (D-T5 step 1) is a `usize`; tool dispatchers serialise via serde-default. ✓
- `recalc_chain` returns `Vec<(String, String)>` (D-T3 step 6); `dependents_of` returns same shape. ✓
- `CellResolver::iter_formulas_in_sheet` returns `Box<dyn Iterator<Item = (String, String)> + 'a>` consistently across stub (D-T3 step 2) and yrs impl (D-T4 step 1). ✓

**Mitigation for set_range gap — add this step to D-T5 between step 6 and step 7:**

- [ ] **Step 6b: set_range with mixed literal + formula recalculates dependent**

```rust
    #[test]
    fn set_range_with_mixed_cells_recalculates() {
        let doc = Doc::new();
        // Pre-seed: D1 has a formula referencing A1 + B1.
        apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(0));
        apply_set_cell_in_proc(&doc, "Sheet1", "B1", &serde_json::json!(0));
        apply_set_cell_in_proc(&doc, "Sheet1", "D1", &serde_json::json!("=A1+B1"));

        // Now write a 1x3 range A1, B1, C1 (literals); cells_recalculated should be 1 (D1).
        // The plan currently lacks an apply_set_range_in_proc helper — if your
        // codebase has one, use it; if not, the cleanest path is two set_cell
        // calls in a single dispatcher block. For the unit test, simulate by:
        let mut total_recalc = 0usize;
        let o1 = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(5));
        total_recalc += o1.cells_recalculated;
        let o2 = apply_set_cell_in_proc(&doc, "Sheet1", "B1", &serde_json::json!(10));
        total_recalc += o2.cells_recalculated;
        let o3 = apply_set_cell_in_proc(&doc, "Sheet1", "C1", &serde_json::json!(20));
        total_recalc += o3.cells_recalculated;

        // D1 recalculated to 15 by the time the third call landed.
        // Each individual call may have recalc'd D1 once → total_recalc >= 2.
        assert!(total_recalc >= 2);
        let r = YrsResolver::new(&doc);
        assert_eq!(r.get("Sheet1", "D1").unwrap().v, serde_json::json!(15.0));
    }
```

If the codebase already exposes a batched `set_range` helper (check
`grep -rn "set_range\|apply_set_range" src/libs/colmena/src/crdt_documents/`),
use it directly and assert the aggregate `cells_recalculated` from one call.

---

**Placeholder scan:** no `TBD`, no `???`, no "implement later". The
`todo!("Playwright shell-out implementation")` in D-T11 step 2 is
intentional and **explicitly documented as a deferred Univer bridge in
BACKLOG (D-T12 step 3 line "Anti-divergence Playwright bridge")**. The
test is `#[ignore]`-gated so CI doesn't trip on it.

Everywhere else, `todo!("step N")` is a TDD marker that points to the
exact next step in the same task — those land filled in by the time the
task is finished.
