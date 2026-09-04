//! End-to-end checks of the graph linter over whole graphs.
//!
//! These drive the public surface — [`lint_graph`], [`lint_graph_json`] and
//! [`LintContext`] — the way a caller does, so they belong here rather than in
//! a `#[cfg(test)]` module beside the implementation. Unit tests for the
//! module's private helpers stay inline in `linter.rs`.

use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::domain::lint::diagnostic::{Diagnostic, LintReport};
use colmena::dag_engine::domain::lint::{
    lint_graph, lint_graph_json, DiagnosticCode, KnownNodeTypes, LintContext, NodeCatalog, Severity,
};
use std::collections::BTreeSet;

fn graph_from(json: serde_json::Value) -> Graph {
    serde_json::from_value(json).expect("test graph must deserialize")
}
fn lint(json: serde_json::Value) -> LintReport {
    let ctx = LintContext::with_embedded_catalog();
    lint_graph(&graph_from(json), &ctx)
}
/// Lints through the raw-document entry point, which is the only one that sees
/// keys `Graph` deserialization discards — `tool_configurations` findings among
/// them.
fn lint_json(json: serde_json::Value) -> LintReport {
    let ctx = LintContext::with_embedded_catalog();
    lint_graph_json(&json, &ctx).expect("test graph must deserialize")
}
fn codes(report: &LintReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.as_str()).collect()
}
fn find(report: &LintReport, code: DiagnosticCode) -> Option<&Diagnostic> {
    report.diagnostics.iter().find(|d| d.code == code)
}
#[test]
fn an_invented_field_is_reported_with_the_field_the_author_meant() {
    let report = lint(serde_json::json!({
        "nodes": {
            "chat": {
                "type": "llm_call",
                "config": { "provider": "openai", "api_key": "k", "modle": "gpt-4o" }
            }
        },
        "edges": []
    }));

    let d = find(&report, DiagnosticCode::UnknownField).expect("invented field must be caught");
    assert_eq!(d.field.as_deref(), Some("modle"));
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.suggestion.as_deref(), Some("did you mean \"model\"?"));
}
#[test]
fn a_real_field_is_not_reported() {
    let report = lint(serde_json::json!({
        "nodes": {
            "chat": {
                "type": "llm_call",
                "config": { "provider": "openai", "api_key": "k", "model": "gpt-4o" }
            }
        },
        "edges": []
    }));
    assert!(
        !codes(&report).contains(&"UNKNOWN_FIELD"),
        "documented fields must not be flagged; got {:?}",
        report.diagnostics
    );
}
#[test]
fn engine_injected_keys_are_not_treated_as_invented() {
    let report = lint(serde_json::json!({
        "nodes": {
            "call": {
                "type": "http_request",
                "config": { "base_url": "https://x", "__colmena_session_id": "s" }
            }
        },
        "edges": []
    }));
    assert!(
        !codes(&report).contains(&"UNKNOWN_FIELD"),
        "reserved __ keys are engine-injected; got {:?}",
        report.diagnostics
    );
}
/// The catalog covers 37 types today, but the linter must degrade honestly
/// rather than call every field of an uncovered node invented.
#[test]
fn an_uncovered_node_type_reports_no_coverage_and_no_field_errors() {
    let ctx = LintContext::with_embedded_catalog();
    let graph = graph_from(serde_json::json!({
        "nodes": {
            "x": { "type": "some_future_node", "config": { "whatever": 1, "another": 2 } }
        },
        "edges": []
    }));
    let report = lint_graph(&graph, &ctx);

    assert_eq!(codes(&report), vec!["NO_CATALOG_COVERAGE"]);
    assert_eq!(report.count(Severity::Error), 0);
}
#[test]
fn an_unregistered_node_type_is_an_error_when_the_registry_is_known() {
    let registered: BTreeSet<String> = ["llm_call".to_string()].into_iter().collect();
    let ctx = LintContext {
        catalog: NodeCatalog::embedded(),
        known_node_types: KnownNodeTypes::Registry(&registered),
    };
    let graph = graph_from(serde_json::json!({
        "nodes": { "x": { "type": "llm_kall", "config": { "bogus": 1 } } },
        "edges": []
    }));
    let report = lint_graph(&graph, &ctx);

    let d = find(&report, DiagnosticCode::UnknownNodeType).expect("must flag the type");
    assert_eq!(d.suggestion.as_deref(), Some("did you mean \"llm_call\"?"));
    assert!(
        !codes(&report).contains(&"UNKNOWN_FIELD"),
        "with no valid node type there is nothing to check fields against"
    );
}
#[test]
fn an_edge_to_a_nonexistent_node_is_an_error() {
    let report = lint(serde_json::json!({
        "nodes": { "a": { "type": "log", "config": {} } },
        "edges": [{ "from": "a", "to": "typo_node" }]
    }));
    let d = find(&report, DiagnosticCode::EdgeUnknownNode).expect("dangling edge must be caught");
    assert!(d.message.contains("typo_node"), "{}", d.message);
}
#[test]
fn an_edge_naming_a_port_on_a_real_node_is_accepted() {
    let report = lint(serde_json::json!({
        "nodes": {
            "a": { "type": "log", "config": {} },
            "b": { "type": "log", "config": {} }
        },
        "edges": [{ "from": "a.output", "to": "b.message" }]
    }));
    assert!(
        !codes(&report).contains(&"EDGE_UNKNOWN_NODE"),
        "port-qualified endpoints are normal; got {:?}",
        report.diagnostics
    );
}
#[test]
fn a_value_outside_the_documented_set_is_a_warning() {
    let report = lint(serde_json::json!({
        "nodes": {
            "chat": {
                "type": "llm_call",
                "config": { "provider": "opnai", "api_key": "k", "model": "m" }
            }
        },
        "edges": []
    }));
    let d = find(&report, DiagnosticCode::InvalidFieldValue).expect("bad enum must be caught");
    assert_eq!(d.severity, Severity::Warning);
    assert!(d.suggestion.as_deref().unwrap().contains("openai"));
}
/// `${OPENAI_API_KEY}` is resolved before the node reads it, so its literal
/// text must not be measured against a documented value set or type.
#[test]
fn placeholders_are_not_measured_against_values_or_types() {
    let report = lint(serde_json::json!({
        "nodes": {
            "chat": {
                "type": "llm_call",
                "config": {
                    "provider": "${LLM_PROVIDER}",
                    "api_key": "${OPENAI_API_KEY}",
                    "model": "gpt-4o",
                    "max_tokens": "$DYNAMIC.tokens"
                }
            }
        },
        "edges": []
    }));
    assert!(
        !codes(&report).contains(&"INVALID_FIELD_VALUE"),
        "got {:?}",
        report.diagnostics
    );
    assert!(
        !codes(&report).contains(&"FIELD_TYPE_MISMATCH"),
        "got {:?}",
        report.diagnostics
    );
}
#[test]
fn a_wrong_json_type_is_a_warning() {
    let report = lint(serde_json::json!({
        "nodes": {
            "chat": {
                "type": "llm_call",
                "config": { "provider": "openai", "api_key": "k", "model": "m",
                            "max_tokens": "not a number" }
            }
        },
        "edges": []
    }));
    let d = find(&report, DiagnosticCode::FieldTypeMismatch).expect("type error must be caught");
    assert_eq!(d.field.as_deref(), Some("max_tokens"));
}
#[test]
fn a_missing_required_field_is_an_error_when_nothing_can_supply_it() {
    let report = lint(serde_json::json!({
        "nodes": { "chat": { "type": "llm_call", "config": { "model": "gpt-4o" } } },
        "edges": []
    }));
    let d = find(&report, DiagnosticCode::MissingRequiredField).expect("must flag it");
    assert_eq!(d.severity, Severity::Error);
    assert!(d.message.contains("no incoming edge"), "{}", d.message);
}
/// A node fed by an edge may legitimately omit a required field from
/// config, because the value arrives through the edge instead. Reporting
/// that as an error would fire on a large share of correct graphs.
#[test]
fn a_missing_required_field_softens_to_a_warning_behind_an_incoming_edge() {
    let report = lint(serde_json::json!({
        "nodes": {
            "src": { "type": "input", "config": {} },
            "chat": { "type": "llm_call", "config": { "model": "gpt-4o" } }
        },
        "edges": [{ "from": "src", "to": "chat" }]
    }));
    let d = find(&report, DiagnosticCode::MissingRequiredField).expect("must still mention it");
    assert_eq!(d.severity, Severity::Warning);
    assert!(d.suggestion.as_deref().unwrap().contains("incoming edge"));
}
/// `router.schema` is documented as required only in the node's mode B.
/// The linter cannot evaluate that condition, and must stay quiet.
#[test]
fn a_conditionally_required_field_is_never_reported_missing() {
    let report = lint(serde_json::json!({
        "nodes": { "r": { "type": "router", "config": { "branches": [] } } },
        "edges": []
    }));
    let missing: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::MissingRequiredField)
        .filter(|d| d.field.as_deref() == Some("schema"))
        .collect();
    assert!(
        missing.is_empty(),
        "conditional requirement must not fire: {missing:?}"
    );
}
#[test]
fn a_clean_graph_produces_no_findings() {
    let report = lint(serde_json::json!({
        "nodes": {
            "src": { "type": "input", "config": {} },
            "pow": { "type": "exponential", "config": { "exponent": 2 } },
            "out": { "type": "log", "config": {} }
        },
        "edges": [
            { "from": "src", "to": "pow" },
            { "from": "pow", "to": "out" }
        ]
    }));
    assert!(report.is_clean(), "got {:?}", report.diagnostics);
}
/// `add` and its siblings read only their `a` / `b` *inputs*; their
/// `execute` takes `_config` and ignores it. A value placed in their config
/// is inert, and the node then fails at run time for the missing input.
/// `tests/graphs/edge_resolution/default_ports_chain.json` is exactly this
/// bug, committed and passing review: it dies with
/// "Entrada no es un número: a".
#[test]
fn config_on_a_node_that_reads_only_inputs_is_reported() {
    let report = lint(serde_json::json!({
        "nodes": { "add_ten": { "type": "add", "config": { "left": 10 } } },
        "edges": []
    }));
    let d = find(&report, DiagnosticCode::UnknownField).expect("inert config must be caught");
    assert_eq!(d.field.as_deref(), Some("left"));
}
#[test]
fn the_report_is_deterministic_across_runs() {
    let g = serde_json::json!({
        "nodes": {
            "z": { "type": "llm_call", "config": { "zzz": 1 } },
            "a": { "type": "llm_call", "config": { "aaa": 1 } },
            "m": { "type": "llm_call", "config": { "mmm": 1 } }
        },
        "edges": []
    });
    let first = lint(g.clone());
    for _ in 0..20 {
        assert_eq!(
            lint(g.clone()),
            first,
            "lint output must not depend on map order"
        );
    }
}
/// The bug in `tests/graphs/edge_resolution/default_ports_chain.json`:
/// `default_input_port` looks like configuration but is discarded during
/// deserialization, so linting the `Graph` alone can never see it.
#[test]
fn an_invented_node_property_is_only_visible_from_the_raw_document() {
    let document = serde_json::json!({
        "nodes": {
            "add_ten": {
                "type": "add",
                "config": {},
                "default_input_port": "right",
                "default_output_port": "result"
            }
        },
        "edges": []
    });
    let ctx = LintContext::with_embedded_catalog();

    let from_graph = lint_graph(&graph_from(document.clone()), &ctx);
    assert!(
        !codes(&from_graph).contains(&"UNKNOWN_NODE_PROPERTY"),
        "serde has already dropped these keys; the Graph path cannot see them"
    );

    let from_json = lint_graph_json(&document, &ctx).expect("valid graph");
    let flagged: Vec<_> = from_json
        .diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::UnknownNodeProperty)
        .filter_map(|d| d.field.as_deref())
        .collect();
    assert_eq!(flagged, vec!["default_input_port", "default_output_port"]);
}
#[test]
fn declared_node_properties_are_not_flagged() {
    let document = serde_json::json!({
        "nodes": {
            "n": {
                "type": "log",
                "config": {},
                "trigger_on": "FINISHED",
                "max_total_calls": 3,
                "max_calls_from": { "other": 1 }
            }
        },
        "edges": []
    });
    let report =
        lint_graph_json(&document, &LintContext::with_embedded_catalog()).expect("valid graph");
    assert!(
        !codes(&report).contains(&"UNKNOWN_NODE_PROPERTY"),
        "got {:?}",
        report.diagnostics
    );
}
/// Annotation keys at the graph root are inert by convention and used in
/// over 260 places in this repo. Reporting them would drown the findings
/// that matter.
#[test]
fn graph_root_annotations_are_left_alone() {
    let document = serde_json::json!({
        "comment": "what this graph does",
        "metadata": { "category": "basic" },
        "_comment": "another",
        "nodes": { "n": { "type": "log", "config": {} } },
        "edges": []
    });
    let report =
        lint_graph_json(&document, &LintContext::with_embedded_catalog()).expect("valid graph");
    assert!(report.is_clean(), "got {:?}", report.diagnostics);
}
#[test]
fn a_document_that_is_not_a_graph_is_a_parse_error_not_a_finding() {
    let ctx = LintContext::with_embedded_catalog();
    assert!(lint_graph_json(&serde_json::json!({"nope": true}), &ctx).is_err());
}
/// `input` and `mock_input` emit their config as data, so an arbitrary key
/// on them is the intended usage, not an invention. Before this was
/// honoured, these two node types alone produced 93 of the 178
/// unknown-field findings across this repo's example graphs — enough noise
/// to make the whole report worthless.
#[test]
fn an_open_config_node_accepts_any_field() {
    let report = lint(serde_json::json!({
        "nodes": {
            "seed": { "type": "input", "config": { "email_body": "hi", "counter": 1 } },
            "mock": { "type": "mock_input", "config": { "x": 1, "usage": {} } }
        },
        "edges": []
    }));
    assert!(report.is_clean(), "got {:?}", report.diagnostics);
}
/// The mirror image: `log` and `output` take `_config` and ignore it, so a
/// key placed there is dead configuration the author believes is working.
#[test]
fn a_node_that_ignores_config_still_reports_dead_keys() {
    let report = lint(serde_json::json!({
        "nodes": { "l": { "type": "log", "config": { "message": "hello" } } },
        "edges": []
    }));
    let d = find(&report, DiagnosticCode::UnknownField).expect("dead config must be caught");
    assert_eq!(d.field.as_deref(), Some("message"));
}
#[test]
fn annotations_inside_config_are_left_alone() {
    let report = lint(serde_json::json!({
        "nodes": {
            "call": {
                "type": "http_request",
                "config": {
                    "base_url": "https://x",
                    "comment": "secure: true means the token is encrypted",
                    "_note": "another annotation"
                }
            }
        },
        "edges": []
    }));
    assert!(
        !codes(&report).contains(&"UNKNOWN_FIELD"),
        "got {:?}",
        report.diagnostics
    );
}
/// The annotation rule must not punch a hole through a field a node type
/// genuinely documents. No node type documents `description` today, so this
/// is checked against a purpose-built catalog rather than by asserting the
/// premise and calling it a day — a test that never invokes the linter
/// cannot fail when the rule it guards is deleted.
#[test]
fn an_annotation_name_that_is_a_real_field_is_still_checked() {
    let catalog = NodeCatalog::parse(
        r#"{
            "common_node_properties": {"type": {"valid_values": ["noted"]}, "config": {}},
            "node_types": {
                "noted": {
                    "config_fields": {
                        "description": {
                            "type": "string",
                            "required": false,
                            "valid_values": ["short", "long"]
                        }
                    }
                }
            }
        }"#,
    )
    .expect("test catalog must parse");
    let ctx = LintContext {
        catalog: &catalog,
        known_node_types: KnownNodeTypes::Unchecked,
    };
    let graph = graph_from(serde_json::json!({
        "nodes": { "n": { "type": "noted", "config": { "description": "invalid" } } },
        "edges": []
    }));

    let report = lint_graph(&graph, &ctx);
    assert!(
        find(&report, DiagnosticCode::InvalidFieldValue).is_some(),
        "a documented field named like an annotation must still be checked; got {:?}",
        report.diagnostics
    );
}
/// The other half of the same rule: on a node type that does NOT document
/// it, the same key is an inert human note and must stay quiet.
#[test]
fn an_annotation_name_on_a_node_that_does_not_document_it_is_ignored() {
    let report = lint(serde_json::json!({
        "nodes": {
            "s": {
                "type": "subgraph",
                "config": { "child_graph_path": "x.json", "description": "what this does" }
            }
        },
        "edges": []
    }));
    assert!(
        !codes(&report).contains(&"UNKNOWN_FIELD"),
        "got {:?}",
        report.diagnostics
    );
}
/// `include_extra_info` is read by the engine off any node's config
/// (`run_use_case.rs`, when stripping `extra_info` from the final output).
/// It belongs to no node's `config_fields`, so before this was handled the
/// linter called it invented on 24 of this repo's graphs — and told the
/// author "the engine ignores it silently", which is the opposite of true.
#[test]
fn an_engine_wide_config_key_is_accepted_on_any_node_type() {
    for node_type in ["log", "orchestrator", "llm_call", "http_request"] {
        let report = lint(serde_json::json!({
            "nodes": { "n": { "type": node_type, "config": { "include_extra_info": true } } },
            "edges": []
        }));
        assert!(
            !codes(&report).contains(&"UNKNOWN_FIELD"),
            "include_extra_info is valid on {node_type}; got {:?}",
            report.diagnostics
        );
    }
}
#[test]
fn an_engine_wide_config_key_is_still_type_checked() {
    let report = lint(serde_json::json!({
        "nodes": { "n": { "type": "log", "config": { "include_extra_info": "yes" } } },
        "edges": []
    }));
    let d = find(&report, DiagnosticCode::FieldTypeMismatch).expect("must be type-checked");
    assert_eq!(d.field.as_deref(), Some("include_extra_info"));
}
/// An edge that names its target port states exactly which field it fills.
/// Reporting that field as missing ignores an answer the graph already
/// gave — it accounted for 35 of 41 such warnings across this repo.
#[test]
fn a_required_field_named_by_an_incoming_edge_is_not_reported_missing() {
    let report = lint(serde_json::json!({
        "nodes": {
            "params": { "type": "input", "config": { "q": "select 1" } },
            "run_sql": { "type": "sql_query", "config": {} }
        },
        "edges": [
            { "from": "params.q", "to": "run_sql.query" },
            { "from": "params.q", "to": "run_sql.connection_url" }
        ]
    }));
    let missing: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::MissingRequiredField)
        .filter_map(|d| d.field.as_deref())
        .collect();
    assert!(
        !missing.contains(&"query"),
        "an edge writes into .query; got {missing:?}"
    );
}
/// But an edge into a *different* port says nothing about this field, and
/// the finding must survive — otherwise any single edge would silence the
/// whole check.
#[test]
fn an_edge_into_another_port_does_not_silence_a_missing_field() {
    let report = lint(serde_json::json!({
        "nodes": {
            "params": { "type": "input", "config": { "q": "x" } },
            "run_sql": { "type": "sql_query", "config": {} }
        },
        "edges": [{ "from": "params.q", "to": "run_sql.connection_url" }]
    }));
    let d = report
        .diagnostics
        .iter()
        .find(|d| d.field.as_deref() == Some("query"))
        .expect("query is still unsupplied");
    assert_eq!(d.code, DiagnosticCode::MissingRequiredField);
    assert_eq!(
        d.severity,
        Severity::Error,
        "no edge names this port, so nothing can supply it"
    );
}
/// And the same must hold end to end, through the HashMap the linter reads
/// its candidates from.
#[test]
fn a_dangling_edge_suggests_the_stable_tie_break_winner() {
    let report = lint(serde_json::json!({
        "nodes": {
            "abd": { "type": "log", "config": {} },
            "abc": { "type": "log", "config": {} },
            "abf": { "type": "log", "config": {} },
            "abe": { "type": "log", "config": {} }
        },
        "edges": [{ "from": "abc", "to": "abx" }]
    }));
    assert_eq!(
        find(&report, DiagnosticCode::EdgeUnknownNode)
            .expect("dangling edge")
            .suggestion
            .as_deref(),
        Some("did you mean \"abc\"?")
    );
}
/// `router.temperature` is documented "NOT configurable" and hardcoded to
/// 0.1 in both router modes. Setting it is dead configuration the author
/// believes is working.
#[test]
fn an_engine_populated_field_cannot_be_set_by_the_author() {
    let report = lint(serde_json::json!({
        "nodes": {
            "r": { "type": "router", "config": { "branches": [], "temperature": 0.9 } }
        },
        "edges": []
    }));
    let d = report
        .diagnostics
        .iter()
        .find(|d| d.field.as_deref() == Some("temperature"))
        .expect("an engine-populated field must be reported");
    assert_eq!(d.code, DiagnosticCode::UnknownField);
    assert!(
        d.message.contains("populated by the engine"),
        "{}",
        d.message
    );
}
#[test]
fn from_catalog_checks_node_types_without_an_engine() {
    let ctx = LintContext::from_catalog();
    let graph = graph_from(serde_json::json!({
        "nodes": { "x": { "type": "llm_kall", "config": {} } },
        "edges": []
    }));
    let report = lint_graph(&graph, &ctx);
    let d = find(&report, DiagnosticCode::UnknownNodeType).expect("must flag the type");
    assert_eq!(d.suggestion.as_deref(), Some("did you mean \"llm_call\"?"));
}
/// The defect this fixes: with only the catalog to go on, the linter used to
/// report *"is not a node type this engine can run"* for any type without an
/// entry. For a node that IS registered but not yet documented — the likeliest
/// way an unknown type appears — that sentence is simply false.
///
/// Now an unrecognised type with no near-miss is reported as a gap in our own
/// coverage, at info severity, and says nothing about the engine.
#[test]
fn an_undocumented_node_type_is_reported_as_missing_coverage_not_as_unrunnable() {
    let ctx = LintContext::from_catalog();
    let graph: Graph = serde_json::from_value(serde_json::json!({
        "nodes": { "x": { "type": "brand_new_node", "config": { "whatever": 1 } } },
        "edges": []
    }))
    .expect("valid graph");

    let report = lint_graph(&graph, &ctx);
    let d = report
        .diagnostics
        .iter()
        .find(|d| d.node_id.as_deref() == Some("x"))
        .expect("the node must be mentioned");

    assert_eq!(d.code, DiagnosticCode::NoCatalogCoverage);
    assert_eq!(d.severity, Severity::Info);
    assert!(
        !d.message.contains("engine can run"),
        "must not claim anything about the engine: {}",
        d.message
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::UnknownField),
        "an unchecked node's fields must not be called invented: {:?}",
        report.diagnostics
    );
}

/// A near-miss is still an error, because that is a typo rather than a new
/// node — but the wording no longer overreaches.
#[test]
fn a_near_miss_node_type_is_an_error_worded_as_a_documentation_claim() {
    let ctx = LintContext::from_catalog();
    let graph: Graph = serde_json::from_value(serde_json::json!({
        "nodes": { "x": { "type": "llm_kall", "config": {} } },
        "edges": []
    }))
    .expect("valid graph");

    let d = lint_graph(&graph, &ctx)
        .diagnostics
        .into_iter()
        .find(|d| d.code == DiagnosticCode::UnknownNodeType)
        .expect("a typo must still be an error");
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.suggestion.as_deref(), Some("did you mean \"llm_call\"?"));
    assert!(
        !d.message.contains("engine can run"),
        "the catalog cannot support that claim: {}",
        d.message
    );
}

/// With a real registry in hand, absence IS proof, so the stronger sentence is
/// correct and must survive.
#[test]
fn with_a_registry_an_unknown_type_is_reported_as_unrunnable() {
    let registered: BTreeSet<String> = ["llm_call".to_string()].into_iter().collect();
    let catalog = NodeCatalog::embedded();
    let ctx = LintContext::from_registry(catalog, &registered);
    let graph: Graph = serde_json::from_value(serde_json::json!({
        "nodes": { "x": { "type": "log", "config": {} } },
        "edges": []
    }))
    .expect("valid graph");

    let d = lint_graph(&graph, &ctx)
        .diagnostics
        .into_iter()
        .find(|d| d.code == DiagnosticCode::UnknownNodeType)
        .expect("`log` is absent from this registry");
    assert_eq!(d.severity, Severity::Error);
    assert!(
        d.message.contains("engine can run"),
        "a registry justifies the strong claim: {}",
        d.message
    );
}

#[test]
fn from_catalog_accepts_every_real_node_type() {
    let ctx = LintContext::from_catalog();
    for node_type in NodeCatalog::embedded().covered_node_types() {
        let graph = graph_from(serde_json::json!({
            "nodes": { "n": { "type": node_type, "config": {} } },
            "edges": []
        }));
        let report = lint_graph(&graph, &ctx);
        assert!(
            !report
                .diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::UnknownNodeType),
            "{node_type} must be recognised"
        );
    }
}
#[test]
fn a_non_object_config_is_flagged_without_panicking() {
    let report = lint(serde_json::json!({
        "nodes": { "a": { "type": "log", "config": "oops" } },
        "edges": []
    }));
    assert!(
        codes(&report).contains(&"FIELD_TYPE_MISMATCH"),
        "got {:?}",
        report.diagnostics
    );
}

/// The defect this rule exists for, reduced to its smallest form: the executor
/// reads `node_schema` and never looks at `fixed_config`, so the plumbing in it
/// silently never reaches the node.
#[test]
fn a_fixed_config_beside_a_node_schema_is_reported_as_dead() {
    let report = lint_json(serde_json::json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "google",
                    "api_key": "k",
                    "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        "http_upload": {
                            "node_type": "http_request",
                            "fixed_config": { "base_url": "https://kb.test", "method": "POST" },
                            "node_schema": { "body": { "type": "object", "required": true } }
                        }
                    }
                }
            }
        },
        "edges": []
    }));

    let d = find(&report, DiagnosticCode::DeadFixedConfig)
        .expect("a fixed_config shadowed by node_schema must be caught");
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(
        d.field.as_deref(),
        Some("tool_configurations.http_upload.fixed_config")
    );
    assert_eq!(d.node_id.as_deref(), Some("agent"));
    // The message must name what is lost, not merely that something is wrong:
    // the author's next question is always "which of my settings vanished?".
    assert!(
        d.message.contains("\"base_url\" and \"method\""),
        "message must list the discarded keys, got: {}",
        d.message
    );
    assert!(d
        .suggestion
        .as_deref()
        .is_some_and(|s| s.contains("node_schema")));
}

/// Either one alone is the supported way to configure a tool. Reporting them
/// would make the rule fire on almost every graph in the repo.
#[test]
fn either_block_on_its_own_is_silent() {
    for tool in [
        serde_json::json!({
            "node_type": "http_request",
            "fixed_config": { "base_url": "https://kb.test" }
        }),
        serde_json::json!({
            "node_type": "http_request",
            "node_schema": { "body": { "type": "object" } }
        }),
    ] {
        let report = lint_json(serde_json::json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": {
                        "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                        "tool_configurations": { "t": tool }
                    }
                }
            },
            "edges": []
        }));
        assert!(
            find(&report, DiagnosticCode::DeadFixedConfig).is_none(),
            "one block alone must not be reported"
        );
    }
}

/// Discarding nothing costs nothing. An empty block is noise, not a bug.
#[test]
fn an_empty_fixed_config_is_not_worth_reporting() {
    let report = lint_json(serde_json::json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        "t": {
                            "node_type": "http_request",
                            "fixed_config": {},
                            "node_schema": { "body": { "type": "object" } }
                        }
                    }
                }
            }
        },
        "edges": []
    }));
    assert!(find(&report, DiagnosticCode::DeadFixedConfig).is_none());
}

/// `lint_graph` takes a deserialized `Graph`, whose `config` is an opaque
/// `Value` — the block survives there, but the raw-document walkers do not run.
/// Pinning this keeps the two entry points from silently diverging in what they
/// promise, the way `validate_graph` once did.
#[test]
fn the_rule_belongs_to_the_raw_document_path_only() {
    let document = serde_json::json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        "t": {
                            "node_type": "http_request",
                            "fixed_config": { "base_url": "https://kb.test" },
                            "node_schema": { "body": { "type": "object" } }
                        }
                    }
                }
            }
        },
        "edges": []
    });

    assert!(find(&lint(document.clone()), DiagnosticCode::DeadFixedConfig).is_none());
    assert!(find(&lint_json(document), DiagnosticCode::DeadFixedConfig).is_some());
}

/// Several tools in one node, and several nodes in one graph, must each be
/// reported on their own rather than collapsing into a single finding.
///
/// The order asserted here is the one `LintReport::sort` imposes — by node id,
/// then by `field`, which carries the tool name. The walker itself emits in
/// document order and deliberately does not sort; a mutation that reverses any
/// ordering inside it leaves this test green, which is the correct outcome and
/// the reason no sorting lives there.
#[test]
fn every_offending_tool_is_reported_separately_under_the_report_ordering() {
    let document = serde_json::json!({
        "nodes": {
            "second": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        "zeta": {
                            "node_type": "http_request",
                            "fixed_config": { "base_url": "https://b.test" },
                            "node_schema": { "body": { "type": "object" } }
                        }
                    }
                }
            },
            "first": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        "beta": {
                            "node_type": "http_request",
                            "fixed_config": { "base_url": "https://a.test" },
                            "node_schema": { "body": { "type": "object" } }
                        },
                        "alpha": {
                            "node_type": "http_request",
                            "fixed_config": { "method": "POST" },
                            "node_schema": { "body": { "type": "object" } }
                        }
                    }
                }
            }
        },
        "edges": []
    });

    let located: Vec<(String, String)> = lint_json(document)
        .diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::DeadFixedConfig)
        .map(|d| {
            (
                d.node_id.clone().unwrap_or_default(),
                d.field.clone().unwrap_or_default(),
            )
        })
        .collect();

    assert_eq!(
        located,
        vec![
            (
                "first".to_string(),
                "tool_configurations.alpha.fixed_config".to_string()
            ),
            (
                "first".to_string(),
                "tool_configurations.beta.fixed_config".to_string()
            ),
            (
                "second".to_string(),
                "tool_configurations.zeta.fixed_config".to_string()
            ),
        ]
    );
}

/// A `tool_configurations` whose entries are not objects — or which is not one
/// itself — is skipped rather than reported. Authors do write these by hand.
///
/// This does **not** prove panic-safety, and an earlier name for it claimed it
/// did. `Value::get` and `Value::as_object` are total: on a string, an array or
/// `null` they return `None` rather than panicking, so no arrangement of these
/// fixtures can distinguish the guards from their absence. What it does pin is
/// the behavior — malformed input yields no finding instead of a bogus one.
#[test]
fn malformed_tool_configurations_yield_no_finding() {
    for tools in [
        serde_json::json!("not-an-object"),
        serde_json::json!([{ "node_type": "http_request" }]),
        serde_json::json!({ "t": "not-an-object" }),
        serde_json::json!({ "t": null }),
    ] {
        let report = lint_json(serde_json::json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": {
                        "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                        "tool_configurations": tools
                    }
                }
            },
            "edges": []
        }));
        assert!(find(&report, DiagnosticCode::DeadFixedConfig).is_none());
    }
}

/// The message is read by a human deciding whether to act, so it has to be
/// grammatical at both lengths — one discarded key and several.
#[test]
fn the_message_agrees_with_the_number_of_discarded_keys() {
    let with = |fixed: serde_json::Value| {
        let report = lint_json(serde_json::json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": {
                        "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                        "tool_configurations": {
                            "t": {
                                "node_type": "http_request",
                                "fixed_config": fixed,
                                "node_schema": { "body": { "type": "object" } }
                            }
                        }
                    }
                }
            },
            "edges": []
        }));
        find(&report, DiagnosticCode::DeadFixedConfig)
            .expect("must be reported")
            .message
            .clone()
    };

    let one = with(serde_json::json!({ "base_url": "https://kb.test" }));
    assert!(
        one.contains("\"base_url\" never reaches the node"),
        "singular form wrong: {one}"
    );

    let many = with(serde_json::json!({ "base_url": "https://kb.test", "method": "POST" }));
    assert!(
        many.contains("\"base_url\" and \"method\" never reach the node"),
        "plural form wrong: {many}"
    );
}

/// The case two lenses caught and the author's own mutations could not: a
/// mutation only attacks code that exists, and this test did not.
///
/// `NodeSchema` is a `HashMap`, so `"node_schema": {}` deserializes to
/// `Some(empty)`. The executor branches on `Option::is_some` and never looks
/// inside, so PATH 0 is taken and the `fixed_config` is discarded exactly as if
/// the schema had fields. A rule that required a non-empty schema was silent on
/// the very defect it exists to catch.
#[test]
fn an_empty_node_schema_still_shadows_the_fixed_config() {
    let report = lint_json(serde_json::json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        "t": {
                            "node_type": "http_request",
                            "fixed_config": { "base_url": "https://kb.test" },
                            "node_schema": {}
                        }
                    }
                }
            }
        },
        "edges": []
    }));
    assert!(
        find(&report, DiagnosticCode::DeadFixedConfig).is_some(),
        "an empty node_schema still wins PATH 0 and must be reported"
    );
}

/// A `null` schema is not a schema, so the precedence rule stays silent.
///
/// The name matters: this pins the LINTER's silence, not a live `fixed_config`.
/// `Graph::validate` deserializes the value into `NodeSchema` — a bare `HashMap`,
/// not an `Option` — so an explicit `null` is rejected at load and the graph
/// never runs at all. Only an ABSENT key reaches the executor with the
/// `fixed_config` intact. Silence here is free either way, which is why the rule
/// does not need to distinguish them.
#[test]
fn a_null_node_schema_does_not_trigger_the_precedence_rule() {
    let report = lint_json(serde_json::json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        "t": {
                            "node_type": "http_request",
                            "fixed_config": { "base_url": "https://kb.test" },
                            "node_schema": null
                        }
                    }
                }
            }
        },
        "edges": []
    }));
    assert!(find(&report, DiagnosticCode::DeadFixedConfig).is_none());
}

/// The row of the guide's table that had documentation but no test until a
/// fourth review round asked which shapes were pinned and which were merely
/// asserted.
///
/// A scalar, an array or a boolean is not a schema, so the precedence rule stays
/// silent — and `Graph::validate` rejects the graph at load anyway, so the
/// silence costs nothing. Note this is a well-formed tool ENTRY whose
/// `node_schema` VALUE is a scalar, which is a different fixture from
/// `malformed_tool_configurations_yield_no_finding`, where the entry itself is
/// malformed.
#[test]
fn a_scalar_node_schema_does_not_trigger_the_precedence_rule() {
    for schema in [
        serde_json::json!("not-a-schema"),
        serde_json::json!(7),
        serde_json::json!([]),
        serde_json::json!(true),
    ] {
        let report = lint_json(serde_json::json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": {
                        "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                        "tool_configurations": {
                            "t": {
                                "node_type": "http_request",
                                "fixed_config": { "base_url": "https://kb.test" },
                                "node_schema": schema
                            }
                        }
                    }
                }
            },
            "edges": []
        }));
        assert!(
            find(&report, DiagnosticCode::DeadFixedConfig).is_none(),
            "a non-object node_schema is not a schema and must not be reported"
        );
    }
}

/// The remaining row: an object-shaped `node_schema` whose nested field is
/// invalid. The rule DOES report it, because `is_object` looks at the top level
/// only — and that is deliberate. `Graph::validate` rejects such a graph at load,
/// so the finding lands on something that never runs; narrowing the guard to
/// inspect nested validity would trade this harmless imprecision for a real
/// blind spot, silencing the precedence rule on a malformed entry. The honest
/// fix is a separate `MALFORMED_TOOL_ENTRY` code, recorded in BACKLOG.md.
///
/// Pinned so the behavior is deliberate rather than accidental.
#[test]
fn an_object_node_schema_with_an_invalid_nested_field_is_still_reported() {
    let report = lint_json(serde_json::json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        "t": {
                            "node_type": "http_request",
                            "fixed_config": { "base_url": "https://kb.test" },
                            "node_schema": { "body": "not-a-field-definition" }
                        }
                    }
                }
            }
        },
        "edges": []
    }));
    assert!(find(&report, DiagnosticCode::DeadFixedConfig).is_some());
}
