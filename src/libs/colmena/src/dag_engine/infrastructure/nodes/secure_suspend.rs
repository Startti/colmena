//! `secure_suspend` — pauses the DAG to ask the user for one or more secrets,
//! persists each encrypted, and returns only opaque handles `<sv_<name>>`.
//!
//! See `docs/superpowers/specs/2026-05-07-secure-suspend-node-design.md`.

use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

static NAME_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-z][a-z0-9_]{2,63}$").expect("name regex"));

#[derive(Debug, Clone)]
struct Secret {
    question: String,
    name: String,
}

fn parse_and_validate_secrets(config: &Value) -> Result<Vec<Secret>, String> {
    let arr = config
        .get("secrets")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| "secure_suspend: secrets list missing or empty".to_string())?;

    let mut out: Vec<Secret> = Vec::with_capacity(arr.len());
    for item in arr {
        let question = item
            .get("question")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "secure_suspend: each secret must have a 'question' string".to_string()
            })?;
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "secure_suspend: each secret must have a 'name' string".to_string()
            })?;
        if !NAME_RE.is_match(name) {
            return Err(format!(
                "secure_suspend: name '{name}' invalid (expected lowercase slug, 3-64 chars)"
            ));
        }
        out.push(Secret {
            question: question.to_string(),
            name: name.to_string(),
        });
    }

    // Uniqueness checks (O(n^2) is fine — list is bounded).
    for i in 0..out.len() {
        for j in (i + 1)..out.len() {
            if out[i].name == out[j].name {
                return Err(format!(
                    "secure_suspend: duplicate name '{}' in secrets list",
                    out[i].name
                ));
            }
            if out[i].question == out[j].question {
                return Err(
                    "secure_suspend: duplicate question text — make each question unique"
                        .to_string(),
                );
            }
        }
    }

    Ok(out)
}

/// Anchored parser: each `secret.question` must appear in `answer` in the same
/// order as in `secrets`. The value associated with `secrets[i]` is everything
/// between the `\n` after question i and the start of question i+1 (or end of
/// string for the last one). Trailing `\n` is stripped from the value but
/// internal newlines are preserved. Empty values are rejected.
///
/// Not yet called from `execute` — Task 8 wires this into the resume path.
#[allow(dead_code)]
fn parse_answers(answer: &str, secrets: &[Secret]) -> Result<Vec<String>, String> {
    let mut positions: Vec<usize> = Vec::with_capacity(secrets.len());
    let mut search_from = 0usize;
    for s in secrets {
        match answer[search_from..].find(s.question.as_str()) {
            Some(rel) => {
                let abs = search_from + rel;
                positions.push(abs);
                search_from = abs + s.question.len();
            }
            None => {
                return Err(format!(
                    "secure_suspend: missing answer for secret '{}' (question not found in response)",
                    s.name
                ));
            }
        }
    }

    let mut values: Vec<String> = Vec::with_capacity(secrets.len());
    for i in 0..secrets.len() {
        let q_end = positions[i] + secrets[i].question.len();
        // Skip the single newline immediately after the question, if present.
        let value_start = if answer[q_end..].starts_with('\n') {
            q_end + 1
        } else {
            q_end
        };
        let value_end = if i + 1 < secrets.len() {
            positions[i + 1]
        } else {
            answer.len()
        };
        let raw = &answer[value_start..value_end];
        // Strip trailing newlines but keep internal ones.
        let trimmed = raw.trim_end_matches('\n');
        if trimmed.is_empty() {
            return Err(format!(
                "secure_suspend: empty value for secret '{}'",
                secrets[i].name
            ));
        }
        values.push(trimmed.to_string());
    }
    Ok(values)
}

/// Node that batches user-secret collection through the suspend mechanism
/// and stores the answers in the encrypted secure-values table.
pub struct SecureSuspendNode {
    secure_value_service: Arc<SecureValueService>,
}

impl SecureSuspendNode {
    pub fn new(secure_value_service: Arc<SecureValueService>) -> Self {
        Self {
            secure_value_service,
        }
    }
}

#[async_trait]
impl ExecutableNode for SecureSuspendNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _global_state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        // Keep the _svc workaround until Task 8 introduces a real read of the field.
        let _svc = Arc::clone(&self.secure_value_service);

        let secrets = parse_and_validate_secrets(config)
            .map_err(Box::<dyn Error + Send + Sync>::from)?;

        // Resume-path: handled in Task 8. For now, fall through to suspend.
        if inputs.get("__colmena_resume_answer").is_some() {
            return Err("secure_suspend: resume not implemented".into());
        }

        // Suspend-path emission.
        let base_id = config
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                inputs
                    .get("__node_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "secure_suspend".to_string());

        let questions: Vec<Value> = secrets
            .iter()
            .enumerate()
            .map(|(i, s)| {
                json!({
                    "id": format!("{base_id}__{}", i + 1),
                    "question": s.question,
                    "type": "secret",
                    "options": Value::Null
                })
            })
            .collect();

        Ok(json!({
            "__colmena_status": "SUSPENDED",
            "questions": questions
        }))
    }

    fn default_input(&self) -> Option<&str> {
        Some("secrets")
    }

    fn default_output(&self) -> Option<&str> {
        Some("handles")
    }

    fn schema(&self) -> Value {
        json!({})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::domain::error::DagError;
    use crate::dag_engine::domain::secure_value_repository::SecureValueRepository;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Test double that records persist calls and supports exists.
    pub(super) struct StubRepo {
        pub(super) storage: Mutex<HashMap<String, String>>,
    }

    impl StubRepo {
        pub(super) fn new() -> Arc<Self> {
            Arc::new(Self {
                storage: Mutex::new(HashMap::new()),
            })
        }
    }

    #[async_trait]
    impl SecureValueRepository for StubRepo {
        async fn persist(
            &self,
            _session_id: &str,
            _source_node_id: &str,
            hash_key: &str,
            real_value: &str,
            _field_name: &str,
        ) -> Result<(), DagError> {
            self.storage
                .lock()
                .unwrap()
                .insert(hash_key.to_string(), real_value.to_string());
            Ok(())
        }
        async fn decrypt(
            &self,
            _session_id: &str,
            hash_key: &str,
        ) -> Result<Option<String>, DagError> {
            Ok(self.storage.lock().unwrap().get(hash_key).cloned())
        }
        async fn exists(
            &self,
            _session_id: &str,
            hash_key: &str,
        ) -> Result<bool, DagError> {
            Ok(self.storage.lock().unwrap().contains_key(hash_key))
        }
        async fn cleanup(&self, _session_id: &str) -> Result<(), DagError> {
            self.storage.lock().unwrap().clear();
            Ok(())
        }
        async fn cleanup_expired(&self) -> Result<u64, DagError> {
            Ok(0)
        }
    }

    pub(super) fn build_node() -> (SecureSuspendNode, Arc<StubRepo>) {
        let repo = StubRepo::new();
        let svc = Arc::new(SecureValueService::new(repo.clone()));
        (SecureSuspendNode::new(svc), repo)
    }

    pub(super) fn inputs_with(node_id: &str, session_id: &str) -> NodeInputs {
        let mut m: HashMap<String, Value> = HashMap::new();
        m.insert("__node_id".into(), Value::String(node_id.into()));
        m.insert(
            "__colmena_session_id".into(),
            Value::String(session_id.into()),
        );
        m
    }

    #[tokio::test]
    async fn execute_with_valid_config_emits_suspended() {
        let (node, _) = build_node();
        let mut state = Value::Null;
        let cfg = json!({
            "secrets": [{"question": "Q?", "name": "valid_name"}]
        });
        let out = node
            .execute(&inputs_with("ask", "s1"), &cfg, &mut state, None)
            .await
            .unwrap();
        assert_eq!(out["__colmena_status"], "SUSPENDED");
    }

    #[tokio::test]
    async fn validation_rejects_missing_secrets() {
        let (node, _) = build_node();
        let mut state = Value::Null;
        let result = node
            .execute(
                &inputs_with("ask", "s1"),
                &json!({}),
                &mut state,
                None,
            )
            .await;
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("secrets list missing or empty"),
            "got: {msg}"
        );
    }

    #[tokio::test]
    async fn validation_rejects_empty_secrets_array() {
        let (node, _) = build_node();
        let mut state = Value::Null;
        let result = node
            .execute(
                &inputs_with("ask", "s1"),
                &json!({"secrets": []}),
                &mut state,
                None,
            )
            .await;
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("secrets list missing or empty"));
    }

    #[tokio::test]
    async fn validation_rejects_invalid_name() {
        let (node, _) = build_node();
        let mut state = Value::Null;
        let cfg = json!({
            "secrets": [{ "question": "Q", "name": "BadName" }]
        });
        let result = node
            .execute(&inputs_with("ask", "s1"), &cfg, &mut state, None)
            .await;
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("name 'BadName' invalid"), "got: {msg}");
    }

    #[tokio::test]
    async fn validation_rejects_duplicate_names() {
        let (node, _) = build_node();
        let mut state = Value::Null;
        let cfg = json!({
            "secrets": [
                { "question": "Q1", "name": "dup_name" },
                { "question": "Q2", "name": "dup_name" }
            ]
        });
        let result = node
            .execute(&inputs_with("ask", "s1"), &cfg, &mut state, None)
            .await;
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("duplicate name 'dup_name'"), "got: {msg}");
    }

    #[tokio::test]
    async fn suspend_path_emits_n_questions_with_secret_type() {
        let (node, _) = build_node();
        let mut state = Value::Null;
        let cfg = json!({
            "secrets": [
                { "question": "Cliente ID?",     "name": "amadeus_client_id" },
                { "question": "Cliente secret?", "name": "amadeus_client_secret" }
            ]
        });
        let out = node
            .execute(&inputs_with("ask_creds", "sx"), &cfg, &mut state, None)
            .await
            .unwrap();

        assert_eq!(out["__colmena_status"], "SUSPENDED");
        let qs = out["questions"].as_array().expect("questions array");
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0]["question"], "Cliente ID?");
        assert_eq!(qs[0]["type"], "secret");
        assert_eq!(qs[0]["id"], "ask_creds__1");
        assert_eq!(qs[0]["options"], Value::Null);
        assert_eq!(qs[1]["question"], "Cliente secret?");
        assert_eq!(qs[1]["id"], "ask_creds__2");
    }

    #[tokio::test]
    async fn suspend_path_uses_explicit_id_from_config() {
        let (node, _) = build_node();
        let mut state = Value::Null;
        let cfg = json!({
            "id": "custom_block",
            "secrets": [{"question": "Q?", "name": "n_a"}]
        });
        let out = node
            .execute(&inputs_with("ignored_node_id", "sx"), &cfg, &mut state, None)
            .await
            .unwrap();
        assert_eq!(out["questions"][0]["id"], "custom_block__1");
    }

    #[tokio::test]
    async fn validation_rejects_duplicate_questions() {
        let (node, _) = build_node();
        let mut state = Value::Null;
        let cfg = json!({
            "secrets": [
                { "question": "Same?", "name": "n_one" },
                { "question": "Same?", "name": "n_two" }
            ]
        });
        let result = node
            .execute(&inputs_with("ask", "s1"), &cfg, &mut state, None)
            .await;
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("duplicate question text"),
            "got: {msg}"
        );
    }

    #[test]
    fn parser_extracts_two_values_in_order() {
        let secrets = vec![
            Secret { question: "Q1?".into(), name: "n1".into() },
            Secret { question: "Q2?".into(), name: "n2".into() },
        ];
        let answer = "Q1?\nval-one\nQ2?\nval-two";
        let values = parse_answers(answer, &secrets).unwrap();
        assert_eq!(values, vec!["val-one".to_string(), "val-two".to_string()]);
    }

    #[test]
    fn parser_preserves_internal_newlines_in_value() {
        let secrets = vec![
            Secret { question: "Paste private key:".into(), name: "pk".into() },
            Secret { question: "Paste public key:".into(), name: "pubk".into() },
        ];
        let multiline = "-----BEGIN-----\nline1\nline2\n-----END-----";
        let answer = format!(
            "Paste private key:\n{multiline}\nPaste public key:\nABCDEF"
        );
        let values = parse_answers(&answer, &secrets).unwrap();
        assert_eq!(values[0], multiline);
        assert_eq!(values[1], "ABCDEF");
    }

    #[test]
    fn parser_errors_on_missing_question() {
        let secrets = vec![
            Secret { question: "Q1?".into(), name: "n1".into() },
            Secret { question: "Q2?".into(), name: "n2".into() },
        ];
        let answer = "Q1?\nval-one";
        let err = parse_answers(answer, &secrets).unwrap_err();
        assert!(err.contains("missing answer for secret 'n2'"), "got: {err}");
    }

    #[test]
    fn parser_errors_on_empty_value() {
        let secrets = vec![
            Secret { question: "Q1?".into(), name: "n1".into() },
            Secret { question: "Q2?".into(), name: "n2".into() },
        ];
        // No newline-value between Q1? and Q2? → empty value for n1.
        let answer = "Q1?\nQ2?\nv2";
        let err = parse_answers(answer, &secrets).unwrap_err();
        assert!(err.contains("empty value for secret 'n1'"), "got: {err}");
    }

    #[test]
    fn parser_errors_when_questions_out_of_order() {
        let secrets = vec![
            Secret { question: "FIRST?".into(), name: "n1".into() },
            Secret { question: "SECOND?".into(), name: "n2".into() },
        ];
        // FIRST? appears at offset 12 in the string, after SECOND?.
        // Parser anchors: finds FIRST? at pos 12, then searches for SECOND?
        // starting from pos 18 → not found → errors on n2.
        let answer = "SECOND?\nv2\nFIRST?\nv1";
        let err = parse_answers(answer, &secrets).unwrap_err();
        assert!(err.contains("missing answer for secret 'n2'"), "got: {err}");
    }
}
