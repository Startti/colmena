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
        _inputs: &NodeInputs,
        config: &Value,
        _global_state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        // Temporary read to suppress dead_code on the field until Task 8 uses it.
        let _svc = Arc::clone(&self.secure_value_service);
        let _secrets = parse_and_validate_secrets(config).map_err(|e| {
            Box::<dyn Error + Send + Sync>::from(e)
        })?;
        // Emission of SUSPENDED comes in Task 6.
        Err("secure_suspend: emission not implemented".into())
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
    async fn execute_with_valid_config_reaches_emission_phase() {
        let (node, _) = build_node();
        let mut state = Value::Null;
        let cfg = json!({
            "secrets": [{"question": "Q?", "name": "valid_name"}]
        });
        let err = node
            .execute(&inputs_with("ask", "s1"), &cfg, &mut state, None)
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("emission not implemented"));
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
}
