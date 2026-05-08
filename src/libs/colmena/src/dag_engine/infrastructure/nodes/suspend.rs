use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::infrastructure::nodes::qa_response_parser::{
    is_valid_qa_id, parse_qa_response,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

/// A Human-in-the-Loop node that pauses graph execution to ask the user a question.
///
/// Config fields:
/// - `question` (required, string): the question to display to the user.
/// - `id` (required, string): stable identifier for the question. Must match
///   `[A-Za-z0-9_-]{1,64}`. Used by external clients (UIs) to map the suspend
///   to a specific UI widget, and used on resume to parse the ID-keyed Q/A
///   answer format produced by the user.
/// - `question_type` (optional, "open" | "choice"): defaults to "open" (free-text answer).
/// - `options` (optional, array of strings): only meaningful when `question_type == "choice"`.
///   Note: `options` is a UX suggestion list — answers on resume are NOT validated against it.
pub struct SuspendNode;

#[async_trait]
impl ExecutableNode for SuspendNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _global_state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        // Resume path: user provided an answer.
        if let Some(answer_val) = inputs.get("__colmena_resume_answer") {
            let raw = answer_val.as_str().ok_or_else(|| {
                Box::<dyn Error + Send + Sync>::from(
                    "suspend: __colmena_resume_answer must be a string",
                )
            })?;
            let id = config
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    Box::<dyn Error + Send + Sync>::from(
                        "suspend: config.id is required on resume",
                    )
                })?;
            let mut parsed = parse_qa_response(raw, &[id])
                .map_err(|e| Box::<dyn Error + Send + Sync>::from(format!("suspend: {e}")))?;
            let answer = parsed.remove(id).expect("parser guarantees the id is present");

            return Ok(json!({
                "status": "resumed",
                "answer_received": answer
            }));
        }

        // Suspend path: build canonical question and emit both the legacy `question`
        // string and the canonical `questions` array.
        let question = config
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("What is your input?")
            .to_string();

        let id = config
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                Box::<dyn Error + Send + Sync>::from("suspend: config.id is required")
            })?
            .to_string();

        if !is_valid_qa_id(&id) {
            return Err(Box::<dyn Error + Send + Sync>::from(format!(
                "suspend: invalid config.id '{id}' (must match [A-Za-z0-9_-]{{1,64}})"
            )));
        }

        let question_type = config
            .get("question_type")
            .and_then(|v| v.as_str())
            .unwrap_or("open")
            .to_string();

        let options: Option<Vec<String>> = config
            .get("options")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        let mut question_obj = serde_json::Map::new();
        question_obj.insert("id".to_string(), Value::String(id));
        question_obj.insert("question".to_string(), Value::String(question.clone()));
        question_obj.insert("type".to_string(), Value::String(question_type));
        question_obj.insert(
            "options".to_string(),
            options
                .map(|opts| Value::Array(opts.into_iter().map(Value::String).collect()))
                .unwrap_or(Value::Null),
        );

        Ok(json!({
            "__colmena_status": "SUSPENDED",
            "question": question,
            "questions": [Value::Object(question_obj)]
        }))
    }

    fn default_input(&self) -> Option<&str> {
        Some("question")
    }

    fn default_output(&self) -> Option<&str> {
        Some("answer_received")
    }

    fn schema(&self) -> Value {
        json!({})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn empty_observer() -> Option<Arc<dyn ExecutionObserver>> {
        None
    }

    fn empty_inputs() -> NodeInputs {
        HashMap::new()
    }

    #[tokio::test]
    async fn suspend_emits_open_by_default() {
        let node = SuspendNode;
        let inputs = empty_inputs();
        let cfg = json!({ "id": "ask_user", "question": "Confirm?" });
        let mut state = Value::Null;
        let out = node
            .execute(&inputs, &cfg, &mut state, empty_observer())
            .await
            .unwrap();

        assert_eq!(out["__colmena_status"], "SUSPENDED");
        assert_eq!(out["question"], "Confirm?");
        assert_eq!(out["questions"][0]["type"], "open");
        assert_eq!(out["questions"][0]["options"], Value::Null);
    }

    #[tokio::test]
    async fn suspend_emits_choice_with_options() {
        let node = SuspendNode;
        let inputs = empty_inputs();
        let cfg = json!({
            "id": "ask_user",
            "question": "Pick one",
            "question_type": "choice",
            "options": ["a", "b", "c"]
        });
        let mut state = Value::Null;
        let out = node
            .execute(&inputs, &cfg, &mut state, empty_observer())
            .await
            .unwrap();

        assert_eq!(out["questions"][0]["type"], "choice");
        assert_eq!(out["questions"][0]["options"], json!(["a", "b", "c"]));
    }

    #[tokio::test]
    async fn suspend_uses_explicit_id() {
        let node = SuspendNode;
        let inputs = empty_inputs();
        let cfg = json!({ "id": "confirm_transfer", "question": "Confirm?" });
        let mut state = Value::Null;
        let out = node
            .execute(&inputs, &cfg, &mut state, empty_observer())
            .await
            .unwrap();

        // Explicit config.id is used (NOT the local __node_id fallback).
        assert_eq!(out["questions"][0]["id"], "confirm_transfer");
    }

    #[tokio::test]
    async fn suspend_preserves_legacy_question_field() {
        let node = SuspendNode;
        let inputs = empty_inputs();
        let cfg = json!({
            "id": "ask_user",
            "question": "Confirm?",
            "question_type": "choice",
            "options": ["a","b"]
        });
        let mut state = Value::Null;
        let out = node
            .execute(&inputs, &cfg, &mut state, empty_observer())
            .await
            .unwrap();

        // Legacy field still present alongside the canonical questions array.
        assert_eq!(out["question"], "Confirm?");
        assert!(out["questions"].is_array());
    }

    #[tokio::test]
    async fn config_without_id_is_rejected_at_execute() {
        let node = SuspendNode;
        let inputs: NodeInputs = HashMap::new();
        let cfg = json!({ "question": "Confirm?" }); // no id
        let mut state = Value::Null;
        let err = node
            .execute(&inputs, &cfg, &mut state, empty_observer())
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("config.id is required"));
    }

    #[tokio::test]
    async fn resume_with_qa_open_yields_answer() {
        let node = SuspendNode;
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "__colmena_resume_answer".to_string(),
            Value::String("Q[confirm]: Confirm?\nA[confirm]: yes".to_string()),
        );
        let cfg = json!({ "id": "confirm", "question": "Confirm?" });
        let mut state = Value::Null;
        let out = node
            .execute(&inputs, &cfg, &mut state, empty_observer())
            .await
            .unwrap();
        assert_eq!(out["status"], "resumed");
        assert_eq!(out["answer_received"], "yes");
    }

    #[tokio::test]
    async fn resume_with_qa_choice_accepts_option_value() {
        let node = SuspendNode;
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "__colmena_resume_answer".to_string(),
            Value::String("Q[env]: Pick env\nA[env]: production".to_string()),
        );
        let cfg = json!({
            "id": "env",
            "question": "Pick env",
            "question_type": "choice",
            "options": ["production", "staging"]
        });
        let mut state = Value::Null;
        let out = node
            .execute(&inputs, &cfg, &mut state, empty_observer())
            .await
            .unwrap();
        assert_eq!(out["answer_received"], "production");
    }

    #[tokio::test]
    async fn resume_with_qa_choice_accepts_free_text() {
        // Options are suggestions, not a whitelist — free-text is accepted.
        let node = SuspendNode;
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "__colmena_resume_answer".to_string(),
            Value::String("Q[env]: Pick env\nA[env]: review-app-123".to_string()),
        );
        let cfg = json!({
            "id": "env",
            "question": "Pick env",
            "question_type": "choice",
            "options": ["production", "staging"]
        });
        let mut state = Value::Null;
        let out = node
            .execute(&inputs, &cfg, &mut state, empty_observer())
            .await
            .unwrap();
        assert_eq!(out["answer_received"], "review-app-123");
    }

    #[tokio::test]
    async fn resume_path_propagates_parser_errors() {
        let node = SuspendNode;
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "__colmena_resume_answer".to_string(),
            Value::String("just a raw string".to_string()),
        );
        let cfg = json!({ "id": "x", "question": "Confirm?" });
        let mut state = Value::Null;
        let err = node
            .execute(&inputs, &cfg, &mut state, empty_observer())
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("missing answer"));
    }
}
