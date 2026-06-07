//! Anti-divergence benchmark for the formula engine (D-T11).
//!
//! Compares formualizer's evaluation against expected Excel/Univer behavior
//! on a fixture corpus. Today only the formualizer side is wired; the
//! Univer-side comparison is a Playwright/headless-Chromium follow-up
//! tracked in BACKLOG (D-T12 v1.1).
//!
//! Gated by `#[ignore]` because:
//! - It exercises the full formula engine surface (slow vs. unit tests).
//! - The future Univer-side will require Playwright + Chromium installed.
//!
//! Run locally:
//!   cargo test --test formula_divergence -- --ignored --nocapture

use colmena::crdt_documents::formula_engine::{
    evaluate, parse, CellResolver, CellSnapshot, EvalValue, ParseOutcome,
};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct Fixture {
    name: String,
    formula: String,
    #[serde(default)]
    seed_cells: HashMap<String, serde_json::Value>,
    #[serde(default)]
    expected_value: Option<serde_json::Value>,
    #[serde(default)]
    expected_error: Option<String>,
}

struct FixtureResolver {
    cells: HashMap<(String, String), CellSnapshot>,
}

impl FixtureResolver {
    fn from_seed(sheet: &str, seed: &HashMap<String, serde_json::Value>) -> Self {
        let mut cells = HashMap::new();
        for (addr, v) in seed {
            // Type tag per design spec: 1=string, 2=number, 3=bool.
            let t = match v {
                serde_json::Value::Number(_) => 2,
                serde_json::Value::Bool(_) => 3,
                _ => 1,
            };
            cells.insert(
                (sheet.to_string(), addr.clone()),
                CellSnapshot { v: v.clone(), t },
            );
        }
        Self { cells }
    }
}

impl CellResolver for FixtureResolver {
    fn get(&self, sheet: &str, addr: &str) -> Option<CellSnapshot> {
        self.cells
            .get(&(sheet.to_string(), addr.to_string()))
            .cloned()
    }
    fn sheet_exists(&self, _: &str) -> bool {
        true
    }
    fn iter_formulas_in_sheet<'a>(
        &'a self,
        _: &str,
    ) -> Box<dyn Iterator<Item = (String, String)> + 'a> {
        Box::new(std::iter::empty())
    }
}

fn load_fixtures() -> Vec<Fixture> {
    let json = include_str!("formula_divergence_fixtures.json");
    serde_json::from_str(json).expect("fixtures parse")
}

fn evaluate_via_formualizer(
    formula: &str,
    seed: &HashMap<String, serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let resolver = FixtureResolver::from_seed("Sheet1", seed);
    let ast = match parse(formula) {
        ParseOutcome::Ok(a) => a,
        ParseOutcome::ParseError(e) => return Err(format!("parse: {e}")),
        ParseOutcome::NeedsBrowser { unsupported_fns } => {
            return Err(format!("needs_browser: {unsupported_fns:?}"));
        }
    };
    match evaluate(&ast, &resolver, "Sheet1") {
        Ok(EvalValue::Number(n)) => Ok(serde_json::json!(n)),
        Ok(EvalValue::String(s)) => Ok(serde_json::json!(s)),
        Ok(EvalValue::Bool(b)) => Ok(serde_json::json!(b)),
        Ok(EvalValue::Error(e)) => Err(e.as_excel().to_string()),
        Err(e) => Err(format!("internal: {e}")),
    }
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

#[test]
#[ignore = "anti-divergence harness — run with `cargo test --test formula_divergence -- --ignored`"]
fn formualizer_matches_expected_values() {
    let fixtures = load_fixtures();
    println!("loaded {} fixtures", fixtures.len());

    let mut failed: Vec<(String, String)> = Vec::new();

    for f in &fixtures {
        let actual = evaluate_via_formualizer(&f.formula, &f.seed_cells);

        match (&f.expected_value, &f.expected_error, &actual) {
            (Some(exp_v), None, Ok(act_v)) => {
                if !values_equivalent(exp_v, act_v) {
                    failed.push((f.name.clone(), format!("expected {exp_v:?}, got {act_v:?}")));
                }
            }
            (None, Some(exp_e), Err(act_e)) => {
                if !act_e.contains(exp_e) {
                    failed.push((
                        f.name.clone(),
                        format!("expected error containing {exp_e:?}, got {act_e:?}"),
                    ));
                }
            }
            (Some(exp_v), None, Err(act_e)) => {
                failed.push((
                    f.name.clone(),
                    format!("expected value {exp_v:?}, got error {act_e:?}"),
                ));
            }
            (None, Some(exp_e), Ok(act_v)) => {
                failed.push((
                    f.name.clone(),
                    format!("expected error {exp_e:?}, got value {act_v:?}"),
                ));
            }
            _ => {
                failed.push((
                    f.name.clone(),
                    "fixture has neither expected_value nor expected_error".to_string(),
                ));
            }
        }
    }

    if !failed.is_empty() {
        for (name, msg) in &failed {
            eprintln!("DIVERGE [{name}]: {msg}");
        }
        panic!("{} of {} fixtures diverged", failed.len(), fixtures.len());
    }
}

#[test]
#[ignore = "Playwright bridge to Univer — deferred to BACKLOG v1.1 (see D-T12 BACKLOG entry)"]
fn univer_matches_formualizer() {
    // TODO(v1.1): spawn headless Chromium via Playwright, load Univer,
    // evaluate each fixture's formula in the browser, compare to
    // evaluate_via_formualizer. Diff fails the build for v1-supported
    // function families.
    //
    // This is documented in docs/superpowers/specs/2026-06-04-crdt-formulas-design.md §8.2
    // and tracked in BACKLOG ("Subsystem D v1.1 — Anti-divergence Playwright bridge").
    unimplemented!("Playwright bridge — see BACKLOG");
}
