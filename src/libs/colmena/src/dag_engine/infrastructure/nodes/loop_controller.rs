use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;

pub struct LoopControllerNode;

/// The statuses the engine's loop routing actually understands.
///
/// `api.rs` stops a serve-mode loop only on `FINISHED` (or a suspend / output
/// node). Any other value falls through every comparison and the loop takes
/// another turn, so an unrecognized status must not be passed through verbatim.
/// `FINISHED_PHASE` is emitted by `orchestrator` and is deliberately valid.
const KNOWN_LOOP_STATUSES: [&str; 4] = ["NEXT_TURN", "FINISHED", "SUSPENDED", "FINISHED_PHASE"];

impl Default for LoopControllerNode {
    fn default() -> Self {
        Self::new()
    }
}

impl LoopControllerNode {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl ExecutableNode for LoopControllerNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // Evaluate loop status
        let mut loop_status = inputs
            .get("loop_status")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("loop_status").and_then(|v| v.as_str()))
            .unwrap_or("FINISHED")
            .to_string();

        // A typo such as "FINISHEDD" used to be emitted verbatim; nothing
        // downstream recognises it, so the serve-mode loop would spin on. Coerce
        // to FINISHED -- stopping early is a visible, debuggable failure, an
        // unbounded loop is not.
        if !KNOWN_LOOP_STATUSES.contains(&loop_status.as_str()) {
            tracing::warn!(
                received = %loop_status,
                valid = ?KNOWN_LOOP_STATUSES,
                "loop_controller: unrecognized loop_status, coercing to FINISHED"
            );
            loop_status = "FINISHED".to_string();
        }

        // Check for suspend flag
        let suspend_flag = inputs
            .get("suspend_flag")
            .and_then(|v| v.as_bool())
            .or_else(|| config.get("suspend_flag").and_then(|v| v.as_bool()))
            .unwrap_or(false);

        if suspend_flag {
            loop_status = "SUSPENDED".to_string();
        }

        let mut output_payload = json!({
            "__colmena_loop_status": loop_status
        });

        // Add additional context based on status
        if loop_status == "SUSPENDED" {
            if let Some(question) = inputs.get("question").or_else(|| config.get("question")) {
                output_payload
                    .as_object_mut()
                    .unwrap()
                    .insert("question".to_string(), question.clone());
            }
        } else if loop_status == "FINISHED" {
            if let Some(final_result) = inputs.get("all_tasks").or_else(|| config.get("all_tasks"))
            {
                output_payload
                    .as_object_mut()
                    .unwrap()
                    .insert("final_result".to_string(), final_result.clone());
            }
        }

        Ok(json!({
            "output": output_payload
        }))
    }

    fn description(&self) -> Option<&str> {
        Some("Aggregates state and determines whether to continue the loop (NEXT_TURN), suspend (SUSPENDED), or break it (FINISHED).")
    }

    fn default_input(&self) -> Option<&str> {
        Some("loop_status")
    }

    fn default_output(&self) -> Option<&str> {
        Some("output")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "loop_controller",
            "inputs": {
                "loop_status": "string (NEXT_TURN, FINISHED, SUSPENDED, FINISHED_PHASE; defaults to FINISHED. An unrecognized value is coerced to FINISHED with a warning)",
                "suspend_flag": "boolean (If true, overrides loop_status to SUSPENDED)",
                "question": "string (Question to prompt the user if suspended)",
                "all_tasks": "array/object (The final payload to output if FINISHED)"
            },
            "outputs": {
                "__colmena_loop_status": "string (NEXT_TURN, FINISHED, or SUSPENDED)",
                "question": "string (optional)",
                "final_result": "any (optional)"
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::domain::observer::ExecutionObserver;
    use std::collections::HashMap;

    async fn run(loop_status: Option<&str>, suspend_flag: Option<bool>) -> Value {
        let mut cfg = serde_json::Map::new();
        if let Some(s) = loop_status {
            cfg.insert("loop_status".into(), json!(s));
        }
        if let Some(b) = suspend_flag {
            cfg.insert("suspend_flag".into(), json!(b));
        }
        let inputs: NodeInputs = HashMap::new();
        let observer: Option<Arc<dyn ExecutionObserver>> = None;
        let mut state = Value::Null;
        LoopControllerNode::new()
            .execute(&inputs, &Value::Object(cfg), &mut state, observer)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn known_statuses_pass_through_untouched() {
        for s in KNOWN_LOOP_STATUSES {
            let out = run(Some(s), None).await;
            assert_eq!(
                out["output"]["__colmena_loop_status"], s,
                "{s} must survive verbatim"
            );
        }
    }

    #[tokio::test]
    async fn finished_phase_is_not_collapsed_into_finished() {
        // orchestrator emits FINISHED_PHASE to mean "phase done, keep going".
        // Coercing it to FINISHED would stop the loop a phase early.
        let out = run(Some("FINISHED_PHASE"), None).await;
        assert_eq!(out["output"]["__colmena_loop_status"], "FINISHED_PHASE");
    }

    #[tokio::test]
    async fn unrecognized_status_is_coerced_to_finished() {
        // A typo used to be emitted verbatim; nothing downstream matches it, so
        // the serve-mode loop kept taking turns.
        for typo in ["FINISHEDD", "finished", "NEXT_TURNN", "", "MY_CUSTOM_LABEL"] {
            let out = run(Some(typo), None).await;
            assert_eq!(
                out["output"]["__colmena_loop_status"], "FINISHED",
                "{typo:?} must be coerced to a status that stops the loop"
            );
        }
    }

    #[tokio::test]
    async fn absent_status_still_defaults_to_finished() {
        let out = run(None, None).await;
        assert_eq!(out["output"]["__colmena_loop_status"], "FINISHED");
    }

    #[tokio::test]
    async fn suspend_flag_still_overrides_a_valid_status() {
        let out = run(Some("NEXT_TURN"), Some(true)).await;
        assert_eq!(out["output"]["__colmena_loop_status"], "SUSPENDED");
    }

    #[tokio::test]
    async fn suspend_flag_overrides_even_an_invalid_status() {
        // Coercion runs first, the override still wins.
        let out = run(Some("GARBAGE"), Some(true)).await;
        assert_eq!(out["output"]["__colmena_loop_status"], "SUSPENDED");
    }
}
