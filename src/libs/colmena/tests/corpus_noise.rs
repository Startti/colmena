//! The corpus noise numbers, turned from a measurement into a fence.
//!
//! Every change in this track quoted the corpus counts over the repo's example
//! graphs as evidence that a new rule added no false positives. They started at
//! `error=75 warning=5` and are coming down as the graphs get fixed — this file
//! is what makes each step of that visible.
//! Nothing held those numbers: they were re-measured by hand each time, so a
//! rule or a catalog edit could have undone the noise reduction and no test
//! would have said a word.
//!
//! This pins them. The count moving is not automatically a defect — adding a
//! graph, or fixing one, moves it legitimately — so the failure message says
//! what to do rather than pretending the corpus is frozen. What it buys is that
//! the move has to be **noticed**, in the change that caused it.
//!
//! Pinned in BOTH directions on purpose. A drop is not automatically good news:
//! a rule that stops firing is how coverage is lost silently, and that is
//! exactly the shape this file exists to catch.

use colmena::dag_engine::domain::lint::{lint_graph_json, DiagnosticCode, LintContext, Severity};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The engine's own example graphs — the corpus the linter's noise is measured
/// over. `tests/lint_examples/` is deliberately NOT here: those are broken on
/// purpose and would drown this signal.
fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/graphs")
}

fn graph_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("tests/graphs must exist") {
            let p = entry.expect("readable entry").path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "json") {
                found.push(p);
            }
        }
    }
    found.sort();
    found
}

struct Measured {
    files: usize,
    by_severity: BTreeMap<&'static str, usize>,
    by_code: BTreeMap<&'static str, usize>,
}

fn measure() -> Measured {
    let ctx = LintContext::from_catalog();
    let mut by_severity: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut by_code: BTreeMap<&'static str, usize> = BTreeMap::new();
    let files = graph_files(&corpus_root());

    for path in &files {
        let text = std::fs::read_to_string(path).expect("readable graph");
        let document: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()));
        let report = lint_graph_json(&document, &ctx)
            .unwrap_or_else(|e| panic!("{} is not a graph: {e}", path.display()));
        for d in &report.diagnostics {
            let severity = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Info => "info",
            };
            *by_severity.entry(severity).or_default() += 1;
            *by_code.entry(d.code.as_str()).or_default() += 1;
        }
    }

    Measured {
        files: files.len(),
        by_severity,
        by_code,
    }
}

/// Update these three numbers in the SAME change that moves them, and say in
/// the PR body which graphs moved and why.
const EXPECTED_FILES: usize = 303;
const EXPECTED_ERRORS: usize = 3;
const EXPECTED_WARNINGS: usize = 3;
const EXPECTED_INFOS: usize = 0;

#[test]
fn the_corpus_noise_is_what_this_track_measured() {
    let m = measure();
    let got = |k: &str| m.by_severity.get(k).copied().unwrap_or(0);

    let actual = (m.files, got("error"), got("warning"), got("info"));
    let expected = (
        EXPECTED_FILES,
        EXPECTED_ERRORS,
        EXPECTED_WARNINGS,
        EXPECTED_INFOS,
    );

    assert_eq!(
        actual, expected,
        "corpus noise moved (files, errors, warnings, info).\n\
         By code now: {:?}\n\
         This is not automatically a defect — adding or fixing a graph moves it — \
         but it has to be noticed in the change that caused it. If the move is \
         intended, update the EXPECTED_* constants in this file and say in the PR \
         which graphs moved and why. If it is not, a rule started or stopped firing.",
        m.by_code
    );
}

/// The per-code shape, so a change that keeps the total and swaps one code for
/// another cannot slip through. Two of those cancelling out is exactly the kind
/// of move a total hides.
#[test]
fn the_corpus_findings_break_down_the_way_they_did() {
    let m = measure();
    let expected: BTreeMap<&str, usize> = [
        // All that is left: two orchestrator graphs written against an older
        // contract, whose pieces are top-level nodes instead of config. Fixing
        // them is a rewrite, not a cleanup.
        ("MISSING_REQUIRED_FIELD", 6),
    ]
    .into_iter()
    .collect();

    assert_eq!(
        m.by_code, expected,
        "the mix of findings changed even if the total did not"
    );
}

/// The measurement is worthless if it silently measures nothing — an empty
/// corpus would satisfy "no findings" perfectly.
#[test]
fn the_corpus_is_actually_being_read() {
    let m = measure();
    assert!(
        m.files > 200,
        "expected the repo's example graphs, found {} files",
        m.files
    );
    assert!(
        DiagnosticCode::UnknownField.as_str() == "UNKNOWN_FIELD",
        "the codes this file pins must be the codes the linter emits"
    );
}
