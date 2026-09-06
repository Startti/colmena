//! The documented example catalogue, checked against the linter that produced it.
//!
//! [`docs/developer_guide/51_graph_linter.md`] shows real output for every graph
//! in `tests/lint_examples/`. Output pasted into a document rots the moment a
//! rule changes, and a stale example is worse than none: it teaches a diagnosis
//! the tool no longer gives. This test pins each example to the codes it is
//! there to demonstrate, so a rule change that alters one fails here. A second
//! test requires every `DiagnosticCode` to have an example at all, so a new one
//! cannot ship with the guide's table growing a row and the catalogue quietly
//! falling behind it.
//!
//! Deliberately outside `tests/graphs/`. That tree is the corpus the linter's
//! noise is measured over — 303 realistic graphs, `error=75 warning=5` — and
//! dropping eighteen deliberately-broken files into it would poison the one
//! number that says whether the tool is worth listening to.

use colmena::dag_engine::domain::lint::{lint_graph_json, LintContext};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/lint_examples")
}

/// The codes each example exists to demonstrate, as a set.
///
/// A set, not a count: an example may legitimately produce two findings of the
/// same code (two invented keys in one block), and pinning the count would make
/// the table brittle for no gain.
fn expected(name: &str) -> &'static [&'static str] {
    match name {
        "01_invented_config_field.json" => &["UNKNOWN_FIELD"],
        "02_unknown_node_type.json" => &["UNKNOWN_NODE_TYPE"],
        "03_unknown_node_property.json" => &["UNKNOWN_NODE_PROPERTY"],
        "04_missing_required_field.json" => &["MISSING_REQUIRED_FIELD"],
        "05_invalid_field_value.json" => &["INVALID_FIELD_VALUE"],
        "06_field_type_mismatch.json" => &["FIELD_TYPE_MISMATCH"],
        "07_edge_to_nowhere.json" => &["EDGE_UNKNOWN_NODE"],
        "08_no_catalog_coverage.json" => &["NO_CATALOG_COVERAGE"],
        "09_read_only_field.json" => &["UNKNOWN_FIELD"],
        "10_dead_fixed_config.json" => &["DEAD_FIXED_CONFIG"],
        "11_repurposed_tool_field.json" => &["REPURPOSED_TOOL_FIELD"],
        "12_tool_never_exposed.json" => &["TOOL_NEVER_EXPOSED"],
        "13_malformed_tool_entry.json" => &["MALFORMED_TOOL_ENTRY"],
        "14_for_each_target_invented_field.json" => &["REPURPOSED_TOOL_FIELD"],
        "15_for_each_target_malformed_schema.json" => &["MALFORMED_TOOL_ENTRY"],
        // The blind spot. Every defect in this file lives inside
        // `child_graph_inline`, and no rule enters there — so the catalogue
        // documents silence, and this line is what fails the day L2 closes and
        // someone forgets the example says otherwise. Verified by hoisting one
        // of its inner defects to the top level: the test goes red.
        //
        // What it CANNOT catch is this fixture quietly ceasing to be broken:
        // repairing a defect inside the inline child changes nothing the linter
        // can see, so the expectation still holds. That is inherent — the whole
        // point of the example is that nothing looks in there — and it is why
        // the fixture carries three separate defects rather than one.
        "16_blind_spot_inline_subgraph.json" => &[],
        "17_several_defects_at_once.json" => &[
            "EDGE_UNKNOWN_NODE",
            "UNKNOWN_FIELD",
            "UNKNOWN_NODE_PROPERTY",
            "MISSING_REQUIRED_FIELD",
            "INVALID_FIELD_VALUE",
        ],
        "18_clean_graph.json" => &[],
        other => panic!(
            "example {other} has no entry in this table. Every file in \
             tests/lint_examples/ is shown in guide 51 with its real output; add \
             the codes it demonstrates here so the document cannot go stale."
        ),
    }
}

fn example_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(examples_dir())
        .expect("tests/lint_examples must exist")
        .map(|e| e.expect("readable dir entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    files
}

#[test]
fn every_example_still_produces_the_diagnosis_the_guide_shows() {
    let ctx = LintContext::from_catalog();
    for path in example_files() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let document: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("readable"))
                .unwrap_or_else(|e| panic!("{name} must be valid JSON: {e}"));
        let report = lint_graph_json(&document, &ctx)
            .unwrap_or_else(|e| panic!("{name} must deserialize as a graph: {e}"));

        let got: BTreeSet<&str> = report.diagnostics.iter().map(|d| d.code.as_str()).collect();
        let want: BTreeSet<&str> = expected(&name).iter().copied().collect();
        assert_eq!(
            got, want,
            "{name} no longer demonstrates what guide 51 says it does"
        );
    }
}

/// The catalogue is only worth reading if it covers what the tool can say.
///
/// Without this, a new `DiagnosticCode` ships with no example and nobody
/// notices — the guide's table grows a row and the catalogue quietly falls
/// behind it.
///
/// The list is written out rather than derived: `DiagnosticCode` has no
/// iteration, and adding one here by hand is the point at which someone has to
/// think about which example demonstrates it.
#[test]
fn every_diagnostic_code_the_linter_can_emit_has_an_example() {
    let all_codes: BTreeSet<&str> = [
        "UNKNOWN_NODE_TYPE",
        "UNKNOWN_FIELD",
        "UNKNOWN_NODE_PROPERTY",
        "MISSING_REQUIRED_FIELD",
        "INVALID_FIELD_VALUE",
        "FIELD_TYPE_MISMATCH",
        "EDGE_UNKNOWN_NODE",
        "NO_CATALOG_COVERAGE",
        "DEAD_FIXED_CONFIG",
        "REPURPOSED_TOOL_FIELD",
        "TOOL_NEVER_EXPOSED",
        "MALFORMED_TOOL_ENTRY",
    ]
    .into_iter()
    .collect();

    let demonstrated: BTreeSet<&str> = example_files()
        .iter()
        .flat_map(|p| {
            expected(&p.file_name().unwrap().to_string_lossy())
                .iter()
                .copied()
        })
        .collect();

    let missing: Vec<&&str> = all_codes.difference(&demonstrated).collect();
    assert!(
        missing.is_empty(),
        "these diagnostic codes have no example in tests/lint_examples/: {missing:?}"
    );
}
