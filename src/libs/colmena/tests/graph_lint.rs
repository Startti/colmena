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

/// Builds a one-node graph whose single tool targets `node_type` with `block`
/// set to `fields`, which is the shape every test below needs.
fn tool_graph(node_type: &str, block: &str, fields: serde_json::Value) -> serde_json::Value {
    let mut entry = serde_json::json!({ "node_type": node_type });
    entry[block] = fields;
    serde_json::json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": { "t": entry }
                }
            }
        },
        "edges": []
    })
}

/// The defect this rule exists for, and the one a clean lint missed in section
/// 20: `http_request` builds its URL from `base_url` + `endpoint` and has no
/// `url` field at all.
///
/// It is a WARNING rather than an error because the engine does not ignore the
/// key — it turns any non-reserved input into a query parameter — so the
/// message has to say what actually happens instead of claiming the key is
/// inert.
#[test]
fn a_key_the_target_node_repurposes_is_reported_as_a_warning() {
    let report = lint_json(tool_graph(
        "http_request",
        "node_schema",
        serde_json::json!({ "url": { "fixed": "https://kb.test" } }),
    ));

    let d = find(&report, DiagnosticCode::RepurposedToolField)
        .expect("a repurposed key must be reported");
    assert_eq!(d.severity, Severity::Warning);
    assert_eq!(
        d.field.as_deref(),
        Some("tool_configurations.t.node_schema.url")
    );
    assert!(
        d.message.contains("query parameter"),
        "the message must say what the key actually becomes, got: {}",
        d.message
    );
}

/// The ordinary invented field, one level down from the node-level check.
#[test]
fn a_key_the_target_node_ignores_is_reported_as_an_error() {
    let report = lint_json(tool_graph(
        "sql_query",
        "node_schema",
        serde_json::json!({ "foo": { "type": "string" } }),
    ));
    let d = find(&report, DiagnosticCode::UnknownField).expect("an invented key must be caught");
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(
        d.field.as_deref(),
        Some("tool_configurations.t.node_schema.foo")
    );
}

/// A near miss should name the field the author meant, the way the node-level
/// check already does.
#[test]
fn a_misspelt_tool_field_suggests_the_real_one() {
    let report = lint_json(tool_graph(
        "sql_query",
        "node_schema",
        serde_json::json!({ "conection_url": { "fixed": "x" } }),
    ));
    let d = find(&report, DiagnosticCode::UnknownField).expect("a typo must be caught");
    assert_eq!(
        d.suggestion.as_deref(),
        Some("did you mean \"connection_url\"?")
    );
}

/// The measurement that shaped this rule: a node dispatched as a tool receives
/// its configured keys as INPUTS, so judging them against `config_fields` alone
/// reports working graphs as broken. Across this repo's corpus that mistake
/// produces 16 findings on graphs that work.
///
/// The fixtures are chosen to ISOLATE each half of the union, which an earlier
/// version of this test failed to do — it used `subgraph.task` and
/// `http_request.query_params`, and both passed for the wrong reason:
/// `subgraph` accepts any key so the rule never reached the lookup, and
/// `query_params` is also a `config_fields` entry. Mutations that deleted the
/// input-port and reserved-key lookups left it green.
///
/// `add.a` is an input port of a CLOSED-contract node and appears in no
/// `config_fields`. `http_request.query_parameters` — the backward-compatible
/// spelling — is the one key in the whole catalog that exists ONLY in
/// `reserved_input_keys`.
#[test]
fn an_input_port_or_reserved_key_is_not_an_invented_field() {
    for (node_type, key) in [("add", "a"), ("http_request", "query_parameters")] {
        let report = lint_json(tool_graph(
            node_type,
            "node_schema",
            serde_json::json!({ key: { "type": "string" } }),
        ));
        assert!(
            find(&report, DiagnosticCode::UnknownField).is_none()
                && find(&report, DiagnosticCode::RepurposedToolField).is_none(),
            "{node_type}.{key} is a real key and must not be reported"
        );
    }
}

/// A node type whose contract is "every key is data" cannot have an invented
/// field. Without this the five open-config node types alone would dominate the
/// report, the same way they did at the node level.
#[test]
fn a_node_type_that_accepts_any_key_is_never_reported() {
    let report = lint_json(tool_graph(
        "python_script",
        "node_schema",
        serde_json::json!({ "anything_at_all": { "type": "string" } }),
    ));
    assert!(find(&report, DiagnosticCode::UnknownField).is_none());
}

/// Engine-injected and annotation keys are exempt inside a tool entry for the
/// same reason they are exempt on a node.
#[test]
fn engine_and_annotation_keys_are_exempt_inside_a_tool_entry() {
    for key in ["__colmena_session_id", "_nota", "$comment", "comment"] {
        let report = lint_json(tool_graph(
            "sql_query",
            "node_schema",
            serde_json::json!({ key: { "fixed": "x" } }),
        ));
        assert!(
            find(&report, DiagnosticCode::UnknownField).is_none(),
            "{key} must be exempt"
        );
    }
}

/// All three blocks carry node field names, so all three are checked.
/// `node_config` matters because it is the only one a toolkit entry uses —
/// every `expose_sub_tools` entry in this repo's corpus configures its node
/// through it and never through `fixed_config`.
#[test]
fn every_block_that_names_node_fields_is_checked() {
    for block in ["node_schema", "fixed_config", "node_config"] {
        let report = lint_json(tool_graph(
            "sql_query",
            block,
            serde_json::json!({ "invented_here": { "type": "string" } }),
        ));
        let d = find(&report, DiagnosticCode::UnknownField)
            .unwrap_or_else(|| panic!("block {block} must be checked"));
        assert_eq!(
            d.field.as_deref(),
            Some(format!("tool_configurations.t.{block}.invented_here").as_str())
        );
    }
}

/// A tool targeting a node type the catalog does not document is reported once
/// for the ENTRY, not once per key — otherwise a single misconfigured tool
/// would bury everything else under one finding per field.
///
/// The fixture was `data_run_python` until the catalog gained its
/// `tool_only_node_types` section; that name is now known, so this test would
/// have quietly stopped exercising the branch it was written for. It uses a
/// name that is neither a node type nor a tool-only type.
#[test]
fn an_uncatalogued_target_is_reported_once_per_entry() {
    let report = lint_json(tool_graph(
        "definitely_not_a_tool_type",
        "fixed_config",
        serde_json::json!({ "sql": "select 1", "enable_gsheets": true, "code": "x" }),
    ));

    let coverage: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code == DiagnosticCode::NoCatalogCoverage
                && d.field.as_deref() == Some("tool_configurations.t")
        })
        .collect();
    assert_eq!(coverage.len(), 1, "one info for the entry, not one per key");
    assert_eq!(coverage[0].severity, Severity::Info);
    assert!(find(&report, DiagnosticCode::UnknownField).is_none());
}

/// Without a `node_type` there is nothing to check the keys against. Guessing
/// would be worse than silence; the engine rejects the entry at load anyway.
#[test]
fn a_tool_entry_without_a_node_type_is_left_alone() {
    let report = lint_json(serde_json::json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        "t": { "node_schema": { "whatever": { "type": "string" } } }
                    }
                }
            }
        },
        "edges": []
    }));
    assert!(find(&report, DiagnosticCode::UnknownField).is_none());
    assert!(find(&report, DiagnosticCode::NoCatalogCoverage).is_none());
}

/// Builds a graph whose single tool entry is keyed `key` and targets `node_type`.
fn keyed_tool_graph(key: &str, node_type: &str) -> serde_json::Value {
    serde_json::json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        key: {
                            "node_type": node_type,
                            "fixed_config": { "sql": { "connection_url": "postgres://x/y" } }
                        }
                    }
                }
            }
        },
        "edges": []
    })
}

/// The working form, which must stay silent — it is how eleven graphs in this
/// repo are written.
#[test]
fn a_synthetic_tool_keyed_by_its_own_name_is_left_alone() {
    for name in [
        "data_run_python",
        "attachment_run_python",
        "sql_inspect_attachment",
        "sql_bulk_insert_from_attachment",
    ] {
        let report = lint_json(keyed_tool_graph(name, name));
        assert!(
            report.is_clean(),
            "{name} keyed by its own name must produce nothing, got: {:?}",
            codes(&report)
        );
    }
}

/// `mcp` is a tool-only type like the other four, so it must not be reported as
/// missing catalog coverage either — even though its shape is different: there
/// the `node_type` field IS what selects the entry, and the map key is the
/// server alias that prefixes every tool the server publishes.
///
/// The fixture carries a `fixed_config` it would not have in real life. That is
/// deliberate, and a review caught the earlier version for lacking it: the
/// coverage report only fires for an entry that has one of the field blocks, so
/// without one this test passed whether the tool-only skip was present or
/// deleted — it proved nothing about `mcp`. Verified by mutation: removing the
/// skip entirely now fails this test, where before it stayed green and only its
/// sibling noticed.
#[test]
fn an_mcp_entry_is_not_judged_by_its_key() {
    let report = lint_json(serde_json::json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        "deepwiki": {
                            "node_type": "mcp",
                            "mcp": { "url": "https://mcp.example.test/sse" },
                            "fixed_config": { "unused_by_mcp": true }
                        }
                    }
                }
            }
        },
        "edges": []
    }));
    assert!(find(&report, DiagnosticCode::NoCatalogCoverage).is_none());
}

/// Before the catalog named the synthetic tools, this info fired identically
/// for `data_run_python` (correct, used by eleven graphs) and for a typo of it,
/// so it could not distinguish them and its advice — add a catalog entry —
/// pointed a typo the wrong way.
#[test]
fn a_mistyped_synthetic_tool_is_still_reported_and_names_the_real_one() {
    let report = lint_json(keyed_tool_graph("data_run_pythonn", "data_run_pythonn"));

    let d = find(&report, DiagnosticCode::NoCatalogCoverage)
        .expect("an unknown target must still be reported");
    assert_eq!(
        d.suggestion.as_deref(),
        Some("did you mean \"data_run_python\"?")
    );
}

/// The case the whole slice exists for, and the one a review caught as
/// newly-silenced: a synthetic tool named by `node_type` but keyed under
/// something else is never handed to the model.
///
/// Reproduced live before the rule was written. Two graphs identical but for
/// the map key: the one keyed `data_run_python` emitted `tool-input-*` and
/// `tool-output-available`, and the model called the tool; the one keyed
/// `mi_python` emitted no tool frame at all and the agent answered that it had
/// no tool. Both exited zero.
#[test]
fn a_synthetic_tool_keyed_under_another_name_is_reported_as_never_exposed() {
    let report = lint_json(keyed_tool_graph("mi_python", "data_run_python"));

    let d = find(&report, DiagnosticCode::ToolNeverExposed)
        .expect("a synthetic tool that will never be exposed must be caught");
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.field.as_deref(), Some("tool_configurations.mi_python"));
    assert!(
        d.message.contains("KEY"),
        "the message must name what actually turns the tool on, got: {}",
        d.message
    );
    assert!(d
        .suggestion
        .as_deref()
        .is_some_and(|s| s.contains("data_run_python")));
}

/// The regression guard for the reason this rule could not be deferred.
///
/// Teaching the linter the five tool-only names means skipping them, and an
/// unconditional skip would take the mis-keyed shape above from a weak
/// `NO_CATALOG_COVERAGE` note to complete silence — strictly worse than before
/// the catalog knew the names at all. This pins that the mis-keyed entry is
/// never silent: it must produce a finding, and specifically not the coverage
/// note, which would be the wrong diagnosis.
#[test]
fn teaching_the_linter_these_names_never_makes_a_broken_entry_quieter() {
    let report = lint_json(keyed_tool_graph("mi_python", "data_run_python"));

    assert!(
        !report.is_clean(),
        "a mis-keyed synthetic tool must never lint clean"
    );
    assert!(
        find(&report, DiagnosticCode::NoCatalogCoverage).is_none(),
        "the coverage note is the wrong diagnosis here — the name IS known"
    );
}

/// A tool-only type whose activation is by `node_type` rather than by the key
/// must not be judged on its key. `mcp` is the only such type today, and its map
/// key is deliberately the server alias, so comparing it would be a false
/// positive on the documented, correct shape.
#[test]
fn a_node_type_activated_entry_is_not_judged_by_its_key() {
    let report = lint_json(serde_json::json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        "deepwiki": {
                            "node_type": "mcp",
                            "mcp": { "url": "https://mcp.example.test/sse" },
                            "fixed_config": { "unused_by_mcp": true }
                        }
                    }
                }
            }
        },
        "edges": []
    }));
    assert!(find(&report, DiagnosticCode::ToolNeverExposed).is_none());
}

/// Builds a graph whose single tool entry carries `schema` as its `node_schema`.
fn schema_tool_graph(schema: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
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
    })
}

/// The linter now says, before anything runs, what the engine would only say at
/// load — which is the whole point of having a linter.
///
/// `Graph::validate` refuses the graph for each of these shapes, and since
/// section 18 that validation runs on every engine entry rather than just the
/// CLI. Every shape below was checked against the built binary: the linter
/// reports it and `dag_engine run` refuses it, with no case where one speaks and
/// the other does not.
#[test]
fn a_node_schema_the_engine_will_refuse_is_reported_before_running() {
    for (name, schema) in [
        ("null", serde_json::json!(null)),
        ("string", serde_json::json!("x")),
        ("number", serde_json::json!(7)),
        ("array", serde_json::json!([])),
        ("bool", serde_json::json!(true)),
        (
            "nested field is not an object",
            serde_json::json!({ "body": "nope" }),
        ),
        (
            "llm-visible field without a type",
            serde_json::json!({ "body": { "required": true } }),
        ),
        (
            "array field without items",
            serde_json::json!({ "rows": { "type": "array" } }),
        ),
        (
            "array field whose items have no type",
            serde_json::json!({ "rows": { "type": "array", "items": {} } }),
        ),
    ] {
        let report = lint_json(schema_tool_graph(schema));
        let d = find(&report, DiagnosticCode::MalformedToolEntry)
            .unwrap_or_else(|| panic!("{name} must be reported"));
        assert_eq!(d.severity, Severity::Error, "{name}");
        assert_eq!(
            d.field.as_deref(),
            Some("tool_configurations.t.node_schema"),
            "{name}"
        );
    }
}

/// With two fields the engine would refuse, the message names the same one
/// every run.
///
/// `parse_node_schema` walks a `HashMap` and returns on the FIRST field it
/// dislikes, so asking it about the whole schema hands the choice to the
/// process's hash seed. A load-time crash can live with that; a report meant to
/// be diffed in CI cannot. The linter asks one field at a time in sorted order,
/// so `alpha` — first alphabetically, not first by hash — is always the one
/// named.
#[test]
fn a_schema_with_two_bad_fields_always_names_the_same_one() {
    // `alpha` sorts first and is VALID, so the probe loop has to advance past
    // it — that is what makes this test bite a `take(1)` on the loop. The four
    // bad fields after it are what make a whole-schema call unlikely to name
    // `bravo` by luck: it picks one of them per process, so that mutation dies
    // four runs in five.
    // Repeated on purpose. One pass is not a guard: an unsorted implementation
    // picks one of the four offenders per `HashMap`, so it lands on `bravo` —
    // and passes — about one run in four. Every `lint_json` here builds a fresh
    // map with a fresh `RandomState`, so twenty passes leave a broken
    // implementation ~4^-20 of a chance, while the sorted one is invariant.
    for pass in 0..20 {
        let report = lint_json(schema_tool_graph(serde_json::json!({
            "zulu": { "required": true },
            "delta": { "required": true },
            "bravo": { "required": true },
            "echo": { "required": true },
            "alpha": { "type": "string" },
        })));
        let d = find(&report, DiagnosticCode::MalformedToolEntry).expect("must be reported");
        assert!(
            d.message.contains("'bravo'"),
            "pass {pass}: the sorted-first OFFENDER must be the one named, got: {}",
            d.message
        );
        for later in ["'delta'", "'echo'", "'zulu'"] {
            assert!(
                !d.message.contains(later),
                "pass {pass}: only the first offender is reported, but {later} appeared in: {}",
                d.message
            );
        }
    }
}

/// The message names shapes and keys, never a value.
///
/// The first version of this rule forwarded serde's own error, and serde
/// renders the offending string literally. So the single most likely way to
/// write this mistake — putting a value straight under a key instead of inside
/// a field definition — printed that value to stdout and into the `--format
/// json` report, which is read in CI logs where the graph body is not. A
/// review caught it. Every other diagnostic in this linter echoes keys, node
/// ids and type names only, and this test is what keeps this one in line.
#[test]
fn a_refused_node_schema_never_prints_the_value_it_found() {
    let secret = "sk-live-must-never-be-printed";
    // A second secret one level DOWN, so the object branch is covered too. It is
    // the branch a credential is most likely to reach: `{"creds": {...}}` looks
    // like a field definition and only fails because an inner key has the wrong
    // type. Without a row like this the object branch could print what it found
    // and every other test here would still pass.
    let nested_secret = "sk-live-nested-must-never-be-printed";
    // Written OUT of alphabetical order on purpose. With the keys already
    // sorted in the document, dropping `offenders.sort()` changes nothing and
    // the ordering assertion below passes over a broken implementation —
    // `serde_json::Map` preserves document order, so the fixture IS the guard.
    let report = lint_json(schema_tool_graph(serde_json::json!({
        "rows": ["a"],
        "api_key": secret,
        "count": 7,
        "creds": { "required": nested_secret, "type": 5 },
    })));
    let d = find(&report, DiagnosticCode::MalformedToolEntry).expect("must be reported");
    let printed = format!("{} {}", d.message, d.suggestion.clone().unwrap_or_default());

    assert!(
        !printed.contains(secret),
        "the secret leaked into the diagnostic: {printed}"
    );
    assert!(
        !printed.contains(nested_secret),
        "the nested secret leaked out of the object branch: {printed}"
    );
    assert!(!printed.contains('7'), "the number leaked: {printed}");

    // The keys DO belong there — they are what the author has to go fix — and
    // they are listed sorted, so two runs over the same file read the same.
    assert!(
        printed.contains("`api_key` is a string")
            && printed.contains("`count` is a number")
            && printed.contains("`rows` is an array")
            && printed.contains("`creds` is an object but not a valid field definition"),
        "each offending key must be named by shape: {printed}"
    );
    let api_at = printed.find("`api_key`").expect("api_key named");
    let count_at = printed.find("`count`").expect("count named");
    let creds_at = printed.find("`creds`").expect("creds named");
    let rows_at = printed.find("`rows`").expect("rows named");
    assert!(
        api_at < count_at && count_at < creds_at && creds_at < rows_at,
        "offenders must be listed in a canonical order, not the author's: {printed}"
    );
}

/// The rule must not fire on the shapes the engine accepts, or it would report
/// working graphs. An absent `node_schema` is valid, and so is a well-formed
/// one.
#[test]
fn a_node_schema_the_engine_accepts_is_left_alone() {
    let valid = lint_json(schema_tool_graph(
        serde_json::json!({ "body": { "type": "object", "required": true } }),
    ));
    assert!(find(&valid, DiagnosticCode::MalformedToolEntry).is_none());

    let absent = lint_json(serde_json::json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        "t": { "node_type": "http_request", "fixed_config": { "base_url": "https://kb.test" } }
                    }
                }
            }
        },
        "edges": []
    }));
    assert!(find(&absent, DiagnosticCode::MalformedToolEntry).is_none());
}

/// An entry the engine refuses has no meaningful precedence or field problem,
/// and the other rules' advice would fix nothing. Before this, a `node_schema`
/// whose nested field was invalid drew a `DEAD_FIXED_CONFIG` telling the author
/// to move keys into that very schema — advice that leaves the graph just as
/// unloadable.
///
/// This supersedes `an_object_node_schema_with_an_invalid_nested_field_is_still_reported`,
/// which pinned that imprecision deliberately while its own docstring named the
/// remedy: a separate `MALFORMED_TOOL_ENTRY` code. That is what this is, so the
/// old test asserted behavior the fix removes and was deleted rather than left
/// contradicting this one.
#[test]
fn a_refused_entry_reports_only_the_reason_it_is_refused() {
    let report = lint_json(schema_tool_graph(serde_json::json!({ "body": "nope" })));

    assert!(find(&report, DiagnosticCode::MalformedToolEntry).is_some());
    assert!(
        find(&report, DiagnosticCode::DeadFixedConfig).is_none(),
        "the precedence rule must not pile onto an entry that will not load"
    );
}

/// A tool-only name used one level too high is an exact name in the wrong
/// place, not a coverage gap. Telling its author to add a catalog entry sends
/// them somewhere they cannot go: `node_types` is closed in both directions
/// against the engine registry, so that entry would fail the suite.
#[test]
fn a_tool_only_type_used_as_a_graph_node_says_where_it_belongs() {
    let ctx = LintContext::from_catalog();
    let document = serde_json::json!({
        "nodes": { "raro": { "type": "data_run_python", "config": {} } },
        "edges": []
    });
    let report = lint_graph_json(&document, &ctx).expect("valid graph");

    let d = find(&report, DiagnosticCode::UnknownNodeType)
        .expect("a tool-only type used as a node type must be reported");
    assert_eq!(d.severity, Severity::Error);
    assert!(
        d.message.contains("tool_configurations"),
        "the message must say where the name belongs, got: {}",
        d.message
    );
}

/// A type that is genuinely unknown keeps the original advice — the fix above
/// must not swallow the ordinary case.
#[test]
fn a_genuinely_unknown_node_type_still_points_at_the_catalog() {
    let ctx = LintContext::from_catalog();
    let document = serde_json::json!({
        "nodes": { "x": { "type": "no_such_thing_anywhere", "config": {} } },
        "edges": []
    });
    let report = lint_graph_json(&document, &ctx).expect("valid graph");

    let d = find(&report, DiagnosticCode::NoCatalogCoverage).expect("must be reported");
    assert!(d
        .suggestion
        .as_deref()
        .is_some_and(|s| s.contains("node_configurations.json")));
}

/// The suppression must be narrow, which an earlier version of it was not.
///
/// A malformed `node_schema` and an invented key in `fixed_config` are two
/// independent defects. Suppressing the precedence rule is right — its advice
/// is to move keys into the very schema that is broken. Suppressing the field
/// rules is not: "this key is not a field of that node type" stays true and
/// actionable whatever shape the schema is in, and hiding it costs the author a
/// second round trip for a defect unrelated to the first.
///
/// Reproduced against the binary before the fix: the invented key vanished from
/// the report entirely.
#[test]
fn a_refused_entry_still_reports_defects_that_stand_on_their_own() {
    let report = lint_json(serde_json::json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        "t": {
                            "node_type": "http_request",
                            "node_schema": null,
                            "fixed_config": { "bogus_key": "x" }
                        }
                    }
                }
            }
        },
        "edges": []
    }));

    assert!(
        find(&report, DiagnosticCode::MalformedToolEntry).is_some(),
        "the refusal must still be reported"
    );
    let field = find(&report, DiagnosticCode::RepurposedToolField)
        .expect("an invented key is a defect of its own and must survive the suppression");
    assert_eq!(
        field.field.as_deref(),
        Some("tool_configurations.t.fixed_config.bogus_key")
    );
}

/// The same correction on the other entry point.
///
/// `LintContext::with_embedded_catalog` draws no conclusions about node types,
/// so it reaches a different arm of `lint_node` than the CLI's `from_catalog`
/// does. Both were changed to name where a tool-only type belongs; a review
/// pointed out only one of them had a test.
#[test]
fn the_unchecked_context_also_says_where_a_tool_only_type_belongs() {
    let ctx = LintContext::with_embedded_catalog();
    let document = serde_json::json!({
        "nodes": { "raro": { "type": "data_run_python", "config": {} } },
        "edges": []
    });
    let report = lint_graph_json(&document, &ctx).expect("valid graph");

    let d = find(&report, DiagnosticCode::NoCatalogCoverage)
        .expect("an uncovered type must still be reported here");
    assert!(
        d.suggestion
            .as_deref()
            .is_some_and(|s| s.contains("tool_configurations")),
        "the advice must point at the right place, got: {:?}",
        d.suggestion
    );
}

// ---------------------------------------------------------------------------
// L2b — the `target` of a `for_each`
// ---------------------------------------------------------------------------

/// A `for_each` node whose embedded target configures `http_request`.
fn for_each_graph(target: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "nodes": {
            "loop": {
                "type": "for_each",
                "config": { "items": [{ "a": 1 }], "target": target }
            }
        },
        "edges": []
    })
}

/// The section-20 defect, one container down: `http_request` reads `base_url`
/// and `endpoint`, never `url`. Inside a tool entry this is already reported;
/// inside a `for_each` target — the same `{node_type, node_schema}` shape —
/// nothing looked.
#[test]
fn a_for_each_target_field_the_node_does_not_read_is_reported() {
    let report = lint_json(for_each_graph(serde_json::json!({
        "node_type": "http_request",
        "node_schema": { "url": { "fixed": "https://example.com" } }
    })));

    let d = find(&report, DiagnosticCode::RepurposedToolField)
        .expect("an invented target field must be caught");
    assert_eq!(
        d.field.as_deref(),
        Some("target.node_schema.url"),
        "the finding must point at the target, not at the node's own config"
    );
}

#[test]
fn a_for_each_target_field_the_node_does_read_is_left_alone() {
    let report = lint_json(for_each_graph(serde_json::json!({
        "node_type": "http_request",
        "node_schema": { "base_url": { "fixed": "https://example.com" } }
    })));

    assert!(
        find(&report, DiagnosticCode::RepurposedToolField).is_none()
            && find(&report, DiagnosticCode::UnknownField).is_none(),
        "a real field must not be reported: {:?}",
        codes(&report)
    );
}

/// A `for_each` dispatched as an LLM tool receives its `target` through the
/// entry's own schema, so the same defect hides one level deeper.
#[test]
fn a_for_each_exposed_as_a_tool_has_its_target_checked_too() {
    let report = lint_json(serde_json::json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        "loop": {
                            "node_type": "for_each",
                            "node_schema": {
                                "target": { "fixed": {
                                    "node_type": "http_request",
                                    "node_schema": { "url": { "fixed": "https://example.com" } }
                                }},
                                "items": { "type": "array", "items": { "type": "object" } }
                            }
                        }
                    }
                }
            }
        },
        "edges": []
    }));

    assert!(
        find(&report, DiagnosticCode::RepurposedToolField).is_some(),
        "the target of a for_each exposed as a tool must be checked: {:?}",
        codes(&report)
    );
}

/// The third door into a target: a `for_each` tool with no `node_schema` takes
/// its whole configuration from `fixed_config`, and `cfg_or_input` finds the
/// `target` there just the same. No graph in this repo uses it, so this test is
/// the only thing holding the door open.
#[test]
fn a_for_each_target_reached_through_fixed_config_is_checked_too() {
    let report = lint_json(serde_json::json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "google", "api_key": "k", "model": "gemini-2.5-flash",
                    "tool_configurations": {
                        "loop": {
                            "node_type": "for_each",
                            "fixed_config": {
                                "target": {
                                    "node_type": "http_request",
                                    "node_schema": { "url": { "fixed": "https://example.com" } }
                                }
                            }
                        }
                    }
                }
            }
        },
        "edges": []
    }));

    let d = find(&report, DiagnosticCode::RepurposedToolField)
        .expect("a target reached through fixed_config must be checked");
    assert_eq!(
        d.field.as_deref(),
        Some("tool_configurations.loop.fixed_config.target.node_schema.url")
    );
}

/// A malformed target schema is NOT refused at load the way a tool entry's is:
/// `Graph::validate` only inspects `config.tool_configurations`, so the graph
/// starts. The message must not claim otherwise.
///
/// It must not claim the opposite either. An earlier version of this work said
/// the rows dispatched UNVALIDATED, reasoned from the `if let Ok(...)` pair in
/// `for_each.rs` that has no else arm. An E2E run falsified it: every row fails
/// at dispatch with `Invalid node_schema`, because `merge_args_into_schema`
/// runs those same two checks first. Both halves are pinned here so neither
/// wrong sentence can come back.
#[test]
fn a_malformed_for_each_target_schema_says_what_the_run_actually_does() {
    let report = lint_json(for_each_graph(serde_json::json!({
        "node_type": "http_request",
        "node_schema": { "body": "not-an-object" }
    })));

    let d = find(&report, DiagnosticCode::MalformedToolEntry)
        .expect("a malformed target schema must be reported");
    assert_eq!(d.field.as_deref(), Some("target.node_schema"));
    assert!(
        !d.message.contains("refuses at load"),
        "nothing refuses this graph AT LOAD: {}",
        d.message
    );
    assert!(
        !d.message.contains("unvalidated"),
        "the rows do not dispatch unvalidated — verified by running it — and saying \
         so would send an operator hunting for corrupted rows that do not exist: {}",
        d.message
    );
    assert!(
        d.message.contains("fails at dispatch"),
        "the message must say what the run actually does: {}",
        d.message
    );
    assert!(
        !d.message.contains("not-an-object"),
        "the value must not be echoed: {}",
        d.message
    );
}

/// The second family of rejection: the block deserializes into `NodeSchema`
/// cleanly and `parse_node_schema` still refuses it. `{"body": {"required":
/// true}}` is a well-formed field, so the first check waves it through — and
/// the run still dies, one row at a time.
#[test]
fn a_target_schema_that_only_parse_rejects_is_reported_too() {
    let report = lint_json(for_each_graph(serde_json::json!({
        "node_type": "http_request",
        "node_schema": { "body": { "required": true } }
    })));

    assert!(
        find(&report, DiagnosticCode::MalformedToolEntry).is_some(),
        "the parse-only family must be caught: {:?}",
        codes(&report)
    );
}

/// A target whose schema the engine accepts must not be reported.
#[test]
fn a_target_schema_the_engine_accepts_is_left_alone() {
    let report = lint_json(for_each_graph(serde_json::json!({
        "node_type": "http_request",
        "node_schema": { "base_url": { "fixed": "https://example.com" } }
    })));

    assert!(
        find(&report, DiagnosticCode::MalformedToolEntry).is_none(),
        "a well-formed schema must be left alone: {:?}",
        codes(&report)
    );
}

// ---------------------------------------------------------------------------
// L2 — inside a `subgraph`'s inline child
// ---------------------------------------------------------------------------

/// A `subgraph` whose child is written inline, with `child` as its body.
fn inline_subgraph(child: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "nodes": { "nested": { "type": "subgraph", "config": { "child_graph_inline": child } } },
        "edges": []
    })
}

/// The engine hands that inline object straight to the child executor, which
/// deserializes it as a `Graph` and validates it. So the defects in it are real
/// defects — they were simply invisible to every rule, which walked only the
/// parent's own nodes.
#[test]
fn an_invented_field_inside_an_inline_child_is_reported() {
    let report = lint_json(inline_subgraph(serde_json::json!({
        "nodes": { "chat": { "type": "llm_call",
                             "config": { "provider": "openai", "api_key": "k", "modle": "gpt-4o" } } },
        "edges": []
    })));

    let d = find(&report, DiagnosticCode::UnknownField).expect("the child must be checked");
    assert_eq!(
        d.node_id.as_deref(),
        Some("nested/chat"),
        "the finding must name the path to the child node, not the bare id"
    );
    assert_eq!(d.field.as_deref(), Some("modle"));
}

/// A dangling edge inside the child carries no node id of its own, so the
/// parent path is what tells the reader which subgraph it is in.
#[test]
fn a_dangling_edge_inside_an_inline_child_names_the_subgraph_it_is_in() {
    let report = lint_json(inline_subgraph(serde_json::json!({
        "nodes": { "a": { "type": "log", "config": {} } },
        "edges": [{ "from": "a", "to": "nowhere" }]
    })));

    let d = find(&report, DiagnosticCode::EdgeUnknownNode).expect("child edges must be checked");
    assert_eq!(d.node_id.as_deref(), Some("nested"));
}

/// Nesting is finite because the document is literal JSON, so the walk simply
/// follows it down however deep it goes.
#[test]
fn a_child_of_a_child_is_reached_too() {
    let report = lint_json(inline_subgraph(inline_subgraph(serde_json::json!({
        "nodes": { "chat": { "type": "llm_call",
                             "config": { "provider": "openai", "api_key": "k", "modle": "gpt-4o" } } },
        "edges": []
    }))));

    let d = find(&report, DiagnosticCode::UnknownField).expect("two levels down must be checked");
    assert_eq!(d.node_id.as_deref(), Some("nested/nested/chat"));
}

/// A `subgraph` dispatched as a tool carries its child in the entry, fixed.
#[test]
fn an_inline_child_reached_through_a_tool_entry_is_checked() {
    let report = lint_json(serde_json::json!({
        "nodes": { "agent": { "type": "llm_call", "config": {
            "provider": "openai", "api_key": "k", "model": "gpt-4o",
            "tool_configurations": { "helper": {
                "node_type": "subgraph",
                "node_schema": { "child_graph_inline": { "fixed": {
                    "nodes": { "chat": { "type": "llm_call",
                        "config": { "provider": "openai", "api_key": "k", "modle": "gpt-4o" } } },
                    "edges": []
                }}, "task": { "type": "string", "required": true, "description": "what to do" } }
            }}
        }}},
        "edges": []
    }));

    let d =
        find(&report, DiagnosticCode::UnknownField).expect("a tool's inline child must be checked");
    assert_eq!(d.node_id.as_deref(), Some("agent/helper/chat"));
}

/// An inline child that is not a graph at all fails the run when the executor
/// deserializes it. Saying so before the run is the same trade the rest of the
/// linter makes.
#[test]
fn an_inline_child_that_is_not_a_graph_is_reported_rather_than_crashing_the_lint() {
    let report = lint_json(inline_subgraph(serde_json::json!({ "nodes": "not-a-map" })));

    assert!(
        !report.is_clean(),
        "a child that cannot deserialize must be reported: {:?}",
        codes(&report)
    );
}

/// A correct inline child must stay silent, or the rule is worse than the gap.
#[test]
fn a_correct_inline_child_is_left_alone() {
    let report = lint_json(inline_subgraph(serde_json::json!({
        "nodes": { "chat": { "type": "llm_call",
                             "config": { "provider": "openai", "api_key": "k", "model": "gpt-4o" } } },
        "edges": []
    })));

    assert!(
        report.is_clean(),
        "a correct child must be silent: {:?}",
        codes(&report)
    );
}
