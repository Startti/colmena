//! Integration tests for the lazy tool loading feature. Exercises the public
//! catalog/reconstruction surface directly rather than driving a full agent
//! loop, since the closure that consumes these is deeply internal to LlmNode.

use serde_json::json;

#[tokio::test]
async fn discovered_set_grows_across_turns_with_describe_then_direct_call() {
    use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
        reconstruct_discovered_set, CatalogEntry,
    };
    use colmena::llm::domain::{FunctionCall, LlmMessage, ToolCall};

    // Turn 1: assistant calls describe_tool("X").
    let m1 = LlmMessage::assistant_with_tool_calls(
        String::new(),
        vec![ToolCall::new(
            "c1".to_string(),
            FunctionCall::new(
                "describe_tool".to_string(),
                json!({ "name": "X" }).to_string(),
            ),
        )],
    )
    .unwrap();

    // Turn 2: assistant calls X directly (after seeing the schema).
    let m2 = LlmMessage::assistant_with_tool_calls(
        String::new(),
        vec![ToolCall::new(
            "c2".to_string(),
            FunctionCall::new("X".to_string(), json!({ "a": 1 }).to_string()),
        )],
    )
    .unwrap();

    let catalog = vec![CatalogEntry {
        name: "X".to_string(),
        summary: "tool X".to_string(),
    }];

    let after_t1 = reconstruct_discovered_set(&[m1.clone()], &catalog);
    assert!(after_t1.contains("X"), "rule (1) must catch describe_tool");

    let after_t2_only = reconstruct_discovered_set(&[m2.clone()], &catalog);
    assert!(
        after_t2_only.contains("X"),
        "rule (2) must catch direct calls — needed for truncation/seeded-history cases"
    );

    let after_both = reconstruct_discovered_set(&[m1, m2], &catalog);
    assert_eq!(after_both.len(), 1);
    assert!(after_both.contains("X"));
}

#[tokio::test]
async fn uncataloged_tool_calls_do_not_enter_set() {
    use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
        reconstruct_discovered_set, CatalogEntry,
    };
    use colmena::llm::domain::{FunctionCall, LlmMessage, ToolCall};

    let renamed_call = LlmMessage::assistant_with_tool_calls(
        String::new(),
        vec![ToolCall::new(
            "c1".to_string(),
            FunctionCall::new("legacy_renamed".to_string(), json!({}).to_string()),
        )],
    )
    .unwrap();

    let catalog = vec![CatalogEntry {
        name: "current_name".to_string(),
        summary: "renamed tool".to_string(),
    }];

    let set = reconstruct_discovered_set(&[renamed_call], &catalog);
    assert!(set.is_empty(), "stale tool names must not enter the set");
}

#[tokio::test]
async fn pending_set_shrinks_as_tools_are_discovered() {
    use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
        build_describe_tool_definition, reconstruct_discovered_set, CatalogEntry,
    };
    use colmena::llm::domain::{FunctionCall, LlmMessage, ToolCall};

    let catalog = vec![
        CatalogEntry {
            name: "a".to_string(),
            summary: "A".to_string(),
        },
        CatalogEntry {
            name: "b".to_string(),
            summary: "B".to_string(),
        },
        CatalogEntry {
            name: "c".to_string(),
            summary: "C".to_string(),
        },
    ];

    // Initially: nothing discovered → all 3 in pending.
    let pending: Vec<&CatalogEntry> = catalog.iter().collect();
    let td_initial = build_describe_tool_definition(&pending);
    let enum_initial = td_initial
        .parameters
        .properties
        .get("name")
        .unwrap()
        .enum_values
        .as_ref()
        .unwrap();
    assert_eq!(enum_initial.len(), 3);

    // After describe_tool("a"): pending = [b, c].
    let m_describe_a = LlmMessage::assistant_with_tool_calls(
        String::new(),
        vec![ToolCall::new(
            "c1".to_string(),
            FunctionCall::new(
                "describe_tool".to_string(),
                json!({ "name": "a" }).to_string(),
            ),
        )],
    )
    .unwrap();
    let discovered = reconstruct_discovered_set(&[m_describe_a], &catalog);
    let pending_after: Vec<&CatalogEntry> = catalog
        .iter()
        .filter(|e| !discovered.contains(&e.name))
        .collect();
    let td_after = build_describe_tool_definition(&pending_after);
    let enum_after = td_after
        .parameters
        .properties
        .get("name")
        .unwrap()
        .enum_values
        .as_ref()
        .unwrap();
    assert_eq!(enum_after, &vec!["b".to_string(), "c".to_string()]);
}
