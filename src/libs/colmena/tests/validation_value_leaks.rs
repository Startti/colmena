//! No load-time rejection may echo the value it rejected.
//!
//! The linter stopped doing this in section 26: serde renders `Unexpected::Str`
//! with the literal string it found, so a `node_schema` holding a credential
//! published it. The two paths the ENGINE takes were left open, and a third —
//! the `mcp` block — was not even on the list. All three are measured here
//! against a value that would be unmistakable in a log.
//!
//! These are not hypothetical shapes. Putting a bare value under a key instead
//! of inside a field definition, or writing `headers: "Bearer …"` instead of a
//! map, are the ordinary ways to get these wrong.

use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::domain::tool_configuration::validate_mcp_config;

/// A string no reader could mistake for anything but a leak.
const SECRET: &str = "sk-live-NEVER-PRINT-THIS";

fn graph_with_tool(tool: serde_json::Value) -> Graph {
    serde_json::from_value(serde_json::json!({
        "nodes": { "agent": { "type": "llm_call", "config": {
            "provider": "openai", "api_key": "k", "model": "gpt-4o",
            "tool_configurations": { "t": tool }
        }}},
        "edges": []
    }))
    .expect("the graph itself must deserialize; the rejection happens in validate")
}

/// L4 — `Graph::validate` forwards serde's error for a `node_schema`.
#[test]
fn rejecting_a_node_schema_does_not_print_the_value() {
    let graph = graph_with_tool(serde_json::json!({
        "node_type": "http_request",
        "node_schema": { "api_key": SECRET }
    }));

    let err = graph
        .validate()
        .expect_err("a bare value is not a field definition");
    let message = err.to_string();
    assert!(
        !message.contains(SECRET),
        "the rejection published the credential: {message}"
    );
    assert!(
        message.contains("api_key"),
        "naming the offending key is what makes it fixable: {message}"
    );
}

/// L5 — the non-HTTPS message quotes the URL, and an MCP URL can carry a token
/// in its query.
#[test]
fn refusing_a_plaintext_mcp_url_does_not_print_the_url() {
    let cfg = serde_json::json!({
        "mcp": { "url": format!("http://host/mcp?token={SECRET}") }
    });

    let reason = validate_mcp_config("mcp", &cfg).expect_err("http must be refused");
    assert!(
        !reason.contains(SECRET),
        "the refusal published the token: {reason}"
    );
    assert!(
        reason.contains("http"),
        "the author still has to learn which scheme was refused: {reason}"
    );
}

/// The one the backlog did not list, and the worst of the three: `mcp.headers`
/// is where a bearer token lives, and a malformed block forwarded serde's error
/// verbatim.
#[test]
fn rejecting_a_malformed_mcp_block_does_not_print_the_value() {
    let cfg = serde_json::json!({
        "mcp": { "url": "https://host/mcp", "headers": format!("Bearer {SECRET}") }
    });

    let reason = validate_mcp_config("mcp", &cfg).expect_err("headers must be a map");
    assert!(
        !reason.contains(SECRET),
        "the rejection published the bearer token: {reason}"
    );
    assert!(
        reason.contains("headers"),
        "naming the offending key is what makes it fixable: {reason}"
    );
}

/// A well-formed config must still pass — a guard that rejects everything
/// proves nothing about the guard.
#[test]
fn a_valid_mcp_block_is_still_accepted() {
    let cfg = serde_json::json!({
        "mcp": { "url": "https://host/mcp", "headers": { "Authorization": "Bearer x" } }
    });
    assert!(validate_mcp_config("mcp", &cfg).is_ok());
}

/// Prints the three messages so a reviewer can see they stayed actionable —
/// removing a leak by removing the information is not a fix.
#[test]
fn the_messages_still_say_what_is_wrong() {
    let schema = graph_with_tool(serde_json::json!({
        "node_type": "http_request",
        "node_schema": { "api_key": SECRET }
    }));
    println!("L4  -> {}", schema.validate().unwrap_err());
    println!(
        "L5  -> {}",
        validate_mcp_config(
            "mcp",
            &serde_json::json!({
                "mcp": { "url": format!("http://host/mcp?token={SECRET}") }
            })
        )
        .unwrap_err()
    );
    println!(
        "L5b -> {}",
        validate_mcp_config(
            "mcp",
            &serde_json::json!({
                "mcp": { "url": "https://host/mcp", "headers": format!("Bearer {SECRET}") }
            })
        )
        .unwrap_err()
    );
}
