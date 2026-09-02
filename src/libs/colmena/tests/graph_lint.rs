//! End-to-end checks of the graph linter over whole graphs.
//!
//! These drive the public surface — [`lint_graph`], [`lint_graph_json`] and
//! [`LintContext`] — the way a caller does, so they belong here rather than in
//! a `#[cfg(test)]` module beside the implementation. Unit tests for the
//! module's private helpers stay inline in `linter.rs`.

use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::domain::lint::diagnostic::{Diagnostic, LintReport};
use colmena::dag_engine::domain::lint::{
    lint_graph, lint_graph_json, DiagnosticCode, LintContext, NodeCatalog, Severity,
};
use std::collections::BTreeSet;

fn graph_from(json: serde_json::Value) -> Graph {
    serde_json::from_value(json).expect("test graph must deserialize")
}
fn lint(json: serde_json::Value) -> LintReport {
    let ctx = LintContext::with_embedded_catalog();
    lint_graph(&graph_from(json), &ctx)
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
        registered_node_types: Some(&registered),
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
        registered_node_types: None,
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
