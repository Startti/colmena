//! The corpus noise numbers, turned from a measurement into a fence.
//!
//! Every change in this track quoted the corpus counts over the repo's example
//! graphs as evidence that a new rule added no false positives. They started at
//! `error=75 warning=5` and reached zero on both — this file is what makes each
//! step of that visible.
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
// Bumped 305 -> 306: adds tests/graphs/basic/input_template_resolution.json
// (the input node template-resolution E2E graph; lints clean).
// Bumped 306 -> 307: adds tests/graphs/security/tool_template_source_e2e.json
// (the template-source hijack-fix E2E graph; lints clean).
// Bumped 308 -> 309: adds tests/graphs/security/tool_env_provenance_e2e.json
// (the http_request env-provenance gate E2E graph; lints clean).
// Bumped 309 -> 310: adds tests/graphs/security/secure_value_stream_leak_e2e.json
// (the secure-value stream-masking E2E graph; lints clean).
// Bumped 310 -> 311: adds tests/graphs/agents/llm_tool_error_boundary.json
// (the node-end status/errorText E2E graph; lints clean).
// Bumped 311 -> 312: adds tests/graphs/security/secure_value_run_loop_masking_e2e.json
// (the run-loop secure-value masking E2E graph; lints clean).
// Bumped 312 -> 313: adds tests/graphs/agents/subgraph_tool_error_boundary.json
// (the subgraph-as-tool boundary-close-on-failure E2E graph; lints clean).
// Bumped 313 -> 314: adds tests/graphs/advanced/edge_wired_subgraph_failure.json
// (edge-wired nested subgraph double-close-guard E2E graph; lints clean).
// Bumped 314 -> 315: adds tests/graphs/agents/child_graph_ref_unavailable.json
// (the child_graph_ref refusal-path E2E graph; lints clean).
// Bumped 315 -> 318: adds three E2E graphs for PR 3/5 of child_graph_ref (router,
// orchestrator and preflight now recognise the third source key):
// tests/graphs/control_flow/router_subgraph_ref_unavailable.json,
// tests/graphs/advanced/orchestrator_agent_by_reference_unavailable.json,
// tests/graphs/basic/subgraph_ref_only_preflight.json. All three lint clean.
// Bumped 318 -> 321: adds three E2E graphs for Task 4/5 of child_graph_ref (a
// `dynamic` subgraph tool whose `thread_id` is FIXED via node_schema, one memory
// thread per `agentId` with nothing exposed to the model):
// tests/graphs/agents/subgraph_fixed_thread_id/turn1_tell_a1.json,
// tests/graphs/agents/subgraph_fixed_thread_id/turn2_ask_a2.json,
// tests/graphs/agents/subgraph_fixed_thread_id/turn3_recall_a1.json. All three
// lint clean.
// Bumped 321 -> 322: adds tests/graphs/agents/provider_key_id_usage_e2e.json,
// the Task 5/5 E2E for `llm_call.config.provider_key_id` (one node configures
// it, a sibling does not; both run under the same trigger). Lints clean.
// Bumped 322 -> 323: adds tests/graphs/agents/mcp_deepwiki_named_e2e.json (an
// MCP entry keyed by node id and named in `name`, the shape ADP compiles; the
// model must see `deepwiki__<tool>`, not `<node-id>__<tool>`). Lints clean.
// Bumped 323 -> 324: adds tests/graphs/agents/mcp_deepwiki_tools_e2e.json (the
// same entry with `mcp.tools: ["ask_wiki_question", "no_such_tool"]`: one tool of
// DeepWiki's three exposed, the unpublished one reported). Lints clean.
// Bumped 324 -> 325: adds tests/graphs/advanced/subgraph_resume_fresh_graph/turn1_suspend.json
// (a resumed inline child runs the parent's current graph; turn 2 — config or
// structure changed — is derived with jq at E2E time, not committed, per its
// folder README). Lints clean.
// Bumped 325 -> 326: adds tests/graphs/security/tool_child_graph_source_e2e.json
// (a model-supplied child-graph source the tool does not offer is dropped).
// Bumped 326 -> 327: adds tests/graphs/security/tool_unoffered_dispatch_e2e.json (a
// model call to a registered node the request did not offer is refused).
// Lints clean.
// Bumped 327 -> 328: adds tests/graphs/agents/child_graph_ref_resume.json (the
// `child_graph_ref` resume E2E: `dag_engine run` has no resolver configured, so
// this graph only exercises the `unavailable` refusal path from the CLI; the
// positive re-resolve path needs a stub resolver, covered by
// `src/libs/colmena/tests/child_graph_ref_resume.rs`). Lints clean.
// Bumped 328 -> 329: adds tests/graphs/basic/stopped_turn_fresh_queue.json (a turn
// the idle watchdog stopped is not resumed by the next one under the same
// session id; turn 2 is derived with jq, per the graph's `comment`). Lints clean.
// Bumped 329 -> 330: adds tests/graphs/security/graph_edge_engine_keys_e2e.json
// (engine-reserved keys an upstream output carries do not reach a node through a
// field-less edge in graph mode). Lints clean.
const EXPECTED_FILES: usize = 330;
const EXPECTED_ERRORS: usize = 0;
const EXPECTED_WARNINGS: usize = 0;
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
        // Empty, and that is the whole point of the track: 80 findings over the
        // example graphs, then zero. The last three were
        // `advanced/test_orchestrator.json`, an orchestrator with an empty
        // config written against a contract the engine dropped; it was rewritten
        // against the current one and now runs end to end.
        //
        // An empty map is a real assertion here, not an absent one: the test
        // below proves the corpus is actually being read, so "no findings"
        // cannot be satisfied by measuring nothing.
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
