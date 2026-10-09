//! The tool list a real `llm_call` node offers, for the synthetic tools added after the
//! catalog filter: what makes each available is unchanged, and `!name` removes it.

use super::node_harness::{
    registry_with_storage, run_turn_with_config, CountingStorage, RecordingModel,
};
use crate::llm::infrastructure::ScriptedResponse;
use serde_json::{json, Value};
use std::sync::Arc;

fn declared(names: &[&str]) -> Value {
    let mut map = serde_json::Map::new();
    for n in names {
        map.insert((*n).to_string(), json!({"node_type": n}));
    }
    Value::Object(map)
}

/// The names of the tools the node offered for this configuration.
async fn offered(declared_tools: &[&str], enabled_tools: Option<Value>) -> Vec<String> {
    let db = tempfile::NamedTempFile::new().unwrap();
    let url = format!("sqlite://{}", db.path().display());
    let reg = registry_with_storage(Some(Arc::new(CountingStorage::default())));
    let model = RecordingModel::scripted(vec![ScriptedResponse::Text("ok".into())]);
    let extra = match enabled_tools {
        Some(v) => json!({"enabled_tools": v}),
        None => json!({}),
    };
    let tool_configurations = if declared_tools.is_empty() {
        Value::Null
    } else {
        declared(declared_tools)
    };
    run_turn_with_config(&reg, &url, vec![], tool_configurations, extra, &model)
        .await
        .unwrap();
    model
        .tools_offered()
        .iter()
        .map(|t| t.split(':').next().unwrap_or("").trim().to_string())
        .collect()
}

const ARP: &str = "attachment_run_python";
const DRP: &str = "data_run_python";
const SQL: &str = "sql_inspect_attachment";

#[tokio::test]
#[serial_test::serial]
async fn a_declared_tool_is_offered_and_a_named_one_is_not_unless_it_was_always_so() {
    let tools = offered(&[ARP, SQL, DRP], None).await;
    for t in [ARP, SQL, DRP] {
        assert!(tools.iter().any(|n| n == t), "{t}: {tools:?}");
    }
    // Naming alone never offered these three; it did and still does offer data_run_python.
    let tools = offered(&[], Some(json!([ARP, SQL, DRP]))).await;
    assert!(!tools.iter().any(|n| n == ARP || n == SQL), "{tools:?}");
    assert!(tools.iter().any(|n| n == DRP), "{tools:?}");
    let tools = offered(&[], Some(json!(["*"]))).await;
    assert!(!tools.iter().any(|n| n == ARP || n == SQL), "{tools:?}");
    assert!(tools.iter().any(|n| n == DRP), "{tools:?}");
}

#[tokio::test]
#[serial_test::serial]
async fn an_exclusion_removes_each_tool_even_with_a_wildcard() {
    for (tool, config) in [(ARP, vec![ARP]), (SQL, vec![SQL]), (DRP, vec![DRP])] {
        let excluded = json!([format!("!{tool}")]);
        let tools = offered(&config, Some(excluded)).await;
        assert!(!tools.iter().any(|n| n == tool), "{tool}: {tools:?}");
        let star = json!(["*", format!("!{tool}")]);
        let tools = offered(&config, Some(star)).await;
        assert!(!tools.iter().any(|n| n == tool), "*, !{tool}: {tools:?}");
    }
}

#[tokio::test]
#[serial_test::serial]
async fn an_allow_list_that_omits_a_declared_tool_keeps_it() {
    let tools = offered(&[ARP, SQL, DRP], Some(json!(["tavily_search"]))).await;
    for t in [ARP, SQL, DRP] {
        assert!(tools.iter().any(|n| n == t), "{t}: {tools:?}");
    }
}

/// `!*` is a literal that matches nothing for the catalog filter; it never removed the
/// three declaration-only tools and still removes `data_run_python`.
#[tokio::test]
#[serial_test::serial]
async fn bang_star_keeps_the_declaration_only_tools_and_removes_data_run_python() {
    for config in [json!(["!*"]), json!(["*", "!*"])] {
        let tools = offered(&[ARP, SQL, DRP], Some(config.clone())).await;
        for t in [ARP, SQL] {
            assert!(tools.iter().any(|n| n == t), "{config}: {t}: {tools:?}");
        }
        assert!(!tools.iter().any(|n| n == DRP), "{config}: {tools:?}");
    }
}

/// With lazy tool loading a tool the owner excluded by name is neither listed in the
/// catalog the model is shown nor describable: it could never become callable.
#[tokio::test]
#[serial_test::serial]
async fn an_excluded_tool_is_not_in_the_lazy_catalog() {
    use super::node_harness::run_turn_with_config;
    for (excluded, expect_listed) in [(None, true), (Some("!attachment_run_python"), false)] {
        let db = tempfile::NamedTempFile::new().unwrap();
        let url = format!("sqlite://{}", db.path().display());
        let reg = registry_with_storage(Some(Arc::new(CountingStorage::default())));
        let model = RecordingModel::scripted(vec![ScriptedResponse::Text("ok".into())]);
        let mut extra = json!({"lazy_tool_loading": true});
        if let Some(e) = excluded {
            extra["enabled_tools"] = json!([e]);
        }
        run_turn_with_config(&reg, &url, vec![], declared(&[ARP]), extra, &model)
            .await
            .unwrap();
        let shown = model.tools_offered().join("\n") + &model.seen();
        assert_eq!(
            shown.contains("attachment_run_python"),
            expect_listed,
            "excluded={excluded:?}: {shown}"
        );
    }
}
