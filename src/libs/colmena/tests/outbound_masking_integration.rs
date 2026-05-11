//! Verifies the outbound-masking pass: a tool result containing a decrypted
//! secret value must be masked before reaching agent_service.
//!
//! Strategy (direct DB injection — bypasses secure_suspend):
//!   1. Pre-seed a row in `secure_value_mappings` with handle
//!      `<sv_user_DEADBEEF>` and decrypted value `"alice123"`.
//!   2. Drive a graph with ScriptedAdapter that issues one tool_call to
//!      `echo_inputs` (a `python_script` tool) whose `code` literally
//!      references the handle.
//!   3. `inject_secrets` resolves handle → "alice123" before the Python
//!      runs; the script returns `{'echoed': 'alice123'}`; the masking pass
//!      must replace `"alice123"` back with `<sv_user_DEADBEEF>` before the
//!      result is appended to the conversation.
//!   4. Assert the decrypted value never appears in any GraphFinish output.
//!
//! Run with:
//!   source .env && cargo test --test outbound_masking_integration -- --ignored --nocapture

use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::llm::infrastructure::{OverrideGuard, ScriptedAdapter, ScriptedResponse};
use futures::StreamExt;
use std::sync::Arc;

async fn engine() -> ColmenaEngine {
    dotenvy::dotenv().ok();
    let cfg = EngineConfig::from_env().expect("EngineConfig from env");
    ColmenaEngine::new(cfg).await.expect("engine construction")
}

async fn seed_agent_session(chat: &str) {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let has_agent_session: bool = sqlx::query_scalar(
        r#"SELECT EXISTS (
             SELECT 1 FROM information_schema.tables
             WHERE table_schema = 'public' AND table_name = 'agent_session'
           )"#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    if has_agent_session {
        let _ = sqlx::query("DELETE FROM agent_session WHERE id = $1")
            .bind(chat)
            .execute(&pool)
            .await;
        sqlx::query(
            r#"INSERT INTO agent_session (id, "updatedAt") VALUES ($1, NOW()) ON CONFLICT (id) DO NOTHING"#,
        )
        .bind(chat)
        .execute(&pool)
        .await
        .expect("seed agent_session row");
    } else {
        let _ = sqlx::query("DELETE FROM dag_runs WHERE agent_session_id = $1")
            .bind(chat)
            .execute(&pool)
            .await;
    }
    let _ = sqlx::query("DELETE FROM secure_value_mappings WHERE agent_session_id = $1")
        .bind(chat)
        .execute(&pool)
        .await;
}

fn graph_inline() -> Graph {
    let raw = serde_json::json!({
        "nodes": {
            "user_input": {
                "type": "input",
                "config": { "default": "echo my credentials" }
            },
            "agent": {
                "type": "llm_call",
                "config": {
                    "provider": "gemini",
                    "model": "gemini-2.5-flash",
                    "api_key": "${GEMINI_API_KEY}",
                    "session_id": "outbound_masking_smoke",
                    "connection_url": "${DATABASE_URL}",
                    "temperature": 0.0,
                    "stream": false,
                    "max_iterations": 10,
                    "system_message": "Echo credentials via run_python.",
                    "tool_configurations": {
                        "echo_inputs": {
                            "name": "echo_inputs",
                            "node_type": "python_script",
                            "description": "Echo your inputs verbatim as a JSON object.",
                            "node_schema": {
                                "sandbox_mode": { "fixed": "restricted" },
                                "code": {
                                    "type": "string",
                                    "required": true,
                                    "description": "Python code; assign result to `output`."
                                }
                            }
                        }
                    }
                }
            }
        },
        "edges": [{ "from": "user_input", "to": "agent" }]
    });
    serde_json::from_value(raw).unwrap()
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn echoed_secret_is_masked_before_reaching_agent_service() {
    let chat = format!(
        "agent_outbound_mask_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    seed_agent_session(&chat).await;

    // Pre-seed a known handle directly into secure_value_mappings, bypassing
    // secure_suspend. The encryption key MUST match what the repository uses
    // (env var `SECURE_VALUES_KEY`, default "default-key").
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let encryption_key =
        std::env::var("SECURE_VALUES_KEY").unwrap_or_else(|_| "default-key".to_string());
    let seeded_session_id = format!("ephemeral_{}", uuid::Uuid::new_v4());
    sqlx::query(
        r#"INSERT INTO secure_value_mappings
           (session_id, agent_session_id, source_node_id, hash_key, encrypted_value, field_name)
           VALUES ($1, $2, $3, $4, pgp_sym_encrypt($5::text, $6), $7)"#,
    )
    .bind(&seeded_session_id)
    .bind(&chat)
    .bind("test_seed")
    .bind("<sv_user_DEADBEEF>")
    .bind("alice123")
    .bind(&encryption_key)
    .bind("secret")
    .execute(&pool)
    .await
    .expect("seed secure_value_mappings row");

    let eng = engine().await;

    // Script: ONE tool_call to echo_inputs whose Python code references the
    // seeded handle, then a closing Text response so the agent loop ends.
    let adapter = Arc::new(ScriptedAdapter::new(vec![
        ScriptedResponse::ToolCall {
            id: "call_1".into(),
            tool_name: "echo_inputs".into(),
            arguments: serde_json::json!({
                "code": "output = {'echoed': '<sv_user_DEADBEEF>'}"
            }),
        },
        ScriptedResponse::Text("done".into()),
    ]));

    let final_outputs = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    {
        let _g = OverrideGuard::install(adapter);
        let mut stream = Box::pin(eng.execute_stream(
            graph_inline(),
            None,
            None,
            false,
            None,
            Some(chat.clone()),
        ));
        while let Some(item) = stream.next().await {
            if let Ok(DagExecutionEvent::GraphFinish { output }) = item {
                final_outputs.lock().unwrap().push(output.to_string());
            }
        }
    }

    let serialized = final_outputs.lock().unwrap().join("\n");
    assert!(
        !serialized.contains("alice123"),
        "decrypted value leaked to graph output: {serialized}"
    );

    eng.shutdown().await;
}
