//! Integration smoke test for the `secure_suspend` node against a live Postgres.
//!
//! The test validates the round-trip lifecycle:
//!   1. Executes the graph once — `secure_suspend` pauses and emits SUSPENDED with questions.
//!   2. Resumes with answers in the canonical `question\nvalue` format.
//!   3. Asserts the `ask_creds` node output contains only opaque handles
//!      (`<sv_smoke_secret_a>`, `<sv_smoke_secret_b>`) — never the real values.
//!
//! NOTE ON DOWNSTREAM INJECTION:
//!   The engine currently calls `inject_secrets()` for all non-LLM nodes before execution,
//!   which means downstream nodes (like `log`) receive real values, not handles. This is
//!   by design for HTTP/SQL nodes that need to use the secrets. The `log_handles` node in
//!   this graph will therefore receive decrypted values. The security invariant enforced
//!   here is that the `ask_creds` NodeFinish event itself only contains handles — the
//!   handles are NOT exposed through the event stream for the producing node.
//!
//! Run with:
//!   source .env && cargo test --test secure_suspend_integration -- --ignored

use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use futures::StreamExt;
use serde_json::json;

async fn engine() -> ColmenaEngine {
    dotenvy::dotenv().ok();
    let cfg = EngineConfig::from_env().unwrap();
    ColmenaEngine::new(cfg).await.unwrap()
}

async fn cleanup(chat: &str) {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("DELETE FROM dag_runs WHERE agent_session_id = $1")
        .bind(chat)
        .execute(&pool)
        .await
        .ok();
}

fn smoke_graph() -> Graph {
    // The graph file lives at the workspace root; CARGO_MANIFEST_DIR points to the
    // crate root (src/libs/colmena), so we walk three levels up.
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/graphs/basic/secure_suspend_smoke.json"
    );
    let raw = std::fs::read_to_string(path).expect("secure_suspend_smoke.json must exist");
    serde_json::from_str(&raw).expect("valid graph JSON")
}

fn smoke_graph_inline() -> Graph {
    // Inline mirror — used for the second run so we avoid re-reading the file.
    let raw = json!({
        "nodes": {
            "ask_creds": {
                "type": "secure_suspend",
                "config": {
                    "secrets": [
                        { "question": "Q1?", "name": "smoke_secret_a" },
                        { "question": "Q2?", "name": "smoke_secret_b" }
                    ]
                }
            },
            "log_handles": {
                "type": "log"
            }
        },
        "edges": [
            { "from": "ask_creds.handles", "to": "log_handles" }
        ]
    });
    serde_json::from_value(raw).unwrap()
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn secure_suspend_smoke_round_trip() {
    let chat = format!(
        "secure_suspend_smoke_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    cleanup(&chat).await;

    let eng = engine().await;

    // --- Run 1: graph should pause at secure_suspend, emit SUSPENDED. ---
    let mut s1 = Box::pin(eng.execute_stream(
        smoke_graph(),
        None,
        None,
        false,
        None,
        Some(chat.clone()),
    ));

    let mut saw_suspended = false;
    while let Some(item) = s1.next().await {
        let ev = item.expect("stream event must not error on first run");
        if let DagExecutionEvent::GraphFinish { ref output } = ev {
            if output
                .get("__colmena_status")
                .and_then(|v| v.as_str())
                == Some("SUSPENDED")
            {
                saw_suspended = true;
            }
        }
    }
    drop(s1);

    assert!(
        saw_suspended,
        "first run: expected SUSPENDED GraphFinish event"
    );

    // --- Run 2: resume — answer covers both questions in the canonical format. ---
    // Format: <question-text>\n<value>\n<question-text>\n<value>
    let answer = "Q1?\nsmoke-val-a\nQ2?\nsmoke-val-b";

    let mut s2 = Box::pin(eng.execute_stream(
        smoke_graph_inline(),
        None,
        Some(answer.to_string()),
        false,
        None,
        Some(chat.clone()),
    ));

    // Collect all events. We inspect the `ask_creds` NodeFinish output specifically
    // because that is the boundary at which handles must never expose real values.
    let mut ask_creds_output: Option<serde_json::Value> = None;
    let mut resumed_successfully = false;

    while let Some(item) = s2.next().await {
        let ev = item.expect("stream event must not error on resume");

        if let DagExecutionEvent::NodeFinish { ref node_id, ref output } = ev {
            if node_id == "ask_creds" {
                ask_creds_output = Some(output.clone());
            }
        }

        if let DagExecutionEvent::GraphFinish { ref output } = ev {
            let status = output
                .get("__colmena_status")
                .and_then(|v| v.as_str())
                .unwrap_or("COMPLETED");
            if status != "SUSPENDED" {
                resumed_successfully = true;
            }
        }
    }
    drop(s2);

    // 1. The run must have completed (not stayed SUSPENDED).
    assert!(
        resumed_successfully,
        "second run: expected a non-SUSPENDED GraphFinish"
    );

    // 2. The `ask_creds` NodeFinish output must contain handles, not real values.
    //    This is the primary security invariant: the producing node's event output
    //    must only expose opaque handle strings.
    let creds_out = ask_creds_output.expect("ask_creds NodeFinish event must have fired");
    let creds_str = creds_out.to_string();

    assert!(
        creds_str.contains("<sv_smoke_secret_a>"),
        "ask_creds output must contain handle <sv_smoke_secret_a>, got: {creds_str}"
    );
    assert!(
        creds_str.contains("<sv_smoke_secret_b>"),
        "ask_creds output must contain handle <sv_smoke_secret_b>, got: {creds_str}"
    );
    assert!(
        !creds_str.contains("smoke-val-a"),
        "real value 'smoke-val-a' must NOT appear in ask_creds NodeFinish output: {creds_str}"
    );
    assert!(
        !creds_str.contains("smoke-val-b"),
        "real value 'smoke-val-b' must NOT appear in ask_creds NodeFinish output: {creds_str}"
    );

    cleanup(&chat).await;
    eng.shutdown().await;
}
