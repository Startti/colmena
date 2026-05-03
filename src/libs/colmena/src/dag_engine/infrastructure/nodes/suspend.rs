use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

/// A Human-in-the-Loop node that pauses graph execution to ask the user a question.
///
/// Config fields:
/// - `question` (required, string): the question to display to the user.
/// - `id` (optional, string): stable identifier for the question. Defaults to the local node_id
///   (engine-injected as `__node_id`). Used by external clients (UIs) to map the suspend
///   to a specific UI widget.
/// - `question_type` (optional, "open" | "choice"): defaults to "open" (free-text answer).
/// - `options` (optional, array of strings): only meaningful when `question_type == "choice"`.
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
        if let Some(answer) = inputs.get("__colmena_resume_answer") {
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
            .map(|s| s.to_string())
            .or_else(|| {
                inputs
                    .get("__node_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "suspend".to_string());

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

    fn inputs_with_node_id(id: &str) -> NodeInputs {
        let mut m: HashMap<String, Value> = HashMap::new();
        m.insert("__node_id".to_string(), Value::String(id.to_string()));
        m
    }

    #[tokio::test]
    async fn suspend_emits_open_when_no_config() {
        let node = SuspendNode;
        let inputs = inputs_with_node_id("ask_user");
        let cfg = json!({ "question": "Confirm?" });
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
        let inputs = inputs_with_node_id("ask_user");
        let cfg = json!({
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
    async fn suspend_uses_local_node_id_as_default_id() {
        let node = SuspendNode;
        let inputs = inputs_with_node_id("ask_user");
        let cfg = json!({ "question": "Confirm?" });
        let mut state = Value::Null;
        let out = node
            .execute(&inputs, &cfg, &mut state, empty_observer())
            .await
            .unwrap();

        assert_eq!(out["questions"][0]["id"], "ask_user");
    }

    #[tokio::test]
    async fn suspend_uses_explicit_id_when_set() {
        let node = SuspendNode;
        let inputs = inputs_with_node_id("ask_user");
        let cfg = json!({ "id": "confirm_transfer", "question": "Confirm?" });
        let mut state = Value::Null;
        let out = node
            .execute(&inputs, &cfg, &mut state, empty_observer())
            .await
            .unwrap();

        assert_eq!(out["questions"][0]["id"], "confirm_transfer");
    }

    #[tokio::test]
    async fn suspend_preserves_legacy_question_field() {
        let node = SuspendNode;
        let inputs = inputs_with_node_id("ask_user");
        let cfg =
            json!({ "question": "Confirm?", "question_type": "choice", "options": ["a","b"] });
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
    async fn resume_path_unchanged() {
        let node = SuspendNode;
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "__colmena_resume_answer".to_string(),
            Value::String("yes".to_string()),
        );
        let cfg = json!({ "question": "Confirm?" });
        let mut state = Value::Null;
        let out = node
            .execute(&inputs, &cfg, &mut state, empty_observer())
            .await
            .unwrap();

        assert_eq!(out["status"], "resumed");
        assert_eq!(out["answer_received"], "yes");
    }
}
