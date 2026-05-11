//! `secure_suspend` — pauses the DAG to ask the user for one or more secrets,
//! persists each encrypted, and returns only opaque handles `<sv_<name>>`.
//!
//! See `docs/superpowers/specs/2026-05-07-secure-suspend-node-design.md`.

use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::infrastructure::nodes::qa_response_parser::parse_qa_response;
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
        inputs: &NodeInputs,
        config: &Value,
        _global_state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        // When invoked as an LLM tool, the dispatcher places the LLM-supplied
        // arguments in `inputs` (with config = {}). When invoked as a
        // top-level DAG node, they live in `config`. Accept both.
        let effective_config = if inputs.get("secrets").is_some() || inputs.get("id").is_some() {
            let mut merged = serde_json::Map::new();
            if let Some(v) = inputs.get("secrets") {
                merged.insert("secrets".to_string(), v.clone());
            }
            if let Some(v) = inputs.get("id") {
                merged.insert("id".to_string(), v.clone());
            }
            // Fall back to config for any field not in inputs.
            if let Some(obj) = config.as_object() {
                for (k, v) in obj {
                    merged.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            Value::Object(merged)
        } else {
            config.clone()
        };
        let secrets = parse_and_validate_secrets(&effective_config)
            .map_err(Box::<dyn Error + Send + Sync>::from)?;

        if let Some(answer_val) = inputs.get("__colmena_resume_answer") {
            let answer = answer_val
                .as_str()
                .ok_or_else(|| {
                    Box::<dyn Error + Send + Sync>::from(
                        "secure_suspend: __colmena_resume_answer must be a string",
                    )
                })?;
            let session_id = inputs
                .get("__colmena_session_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    Box::<dyn Error + Send + Sync>::from(
                        "secure_suspend: missing __colmena_session_id in inputs",
                    )
                })?;
            // Optional chat handle: when present, secrets are persisted *and*
            // looked up agent-first so a fresh ephemeral session can still
            // resolve them on resume.
            let agent_session_id = inputs
                .get("__colmena_agent_session_id")
                .and_then(|v| v.as_str());
            let node_id = inputs
                .get("__node_id")
                .and_then(|v| v.as_str())
                .unwrap_or("secure_suspend");

            let id_refs: Vec<&str> = secrets.iter().map(|s| s.name.as_str()).collect();
            let answer_map = parse_qa_response(answer, &id_refs).map_err(|e| {
                Box::<dyn Error + Send + Sync>::from(format!("secure_suspend: {e}"))
            })?;
            // Parser contract: Ok(map) ⇒ every id in `id_refs` is a key in `map`.
            // Walking `secrets` here (not `answer_map`) preserves declaration order
            // for the indexed collision/persist loops below.
            let values: Vec<String> = secrets
                .iter()
                .map(|s| {
                    answer_map
                        .get(&s.name)
                        .cloned()
                        .expect("parser guarantees all expected ids are present")
                })
                .collect();

            // Collision pre-check (all secrets) before any write.
            for s in &secrets {
                let handle = format!("<sv_{}>", s.name);
                if self
                    .secure_value_service
                    .handle_exists(session_id, agent_session_id, &handle)
                    .await
                    .map_err(|e| Box::<dyn Error + Send + Sync>::from(format!("{e}")))?
                {
                    return Err(Box::<dyn Error + Send + Sync>::from(format!(
                        "secure_suspend: handle {handle} already exists in session — use a different name"
                    )));
                }
            }

            // Persist + collect handles.
            let mut handles = serde_json::Map::new();
            for (s, v) in secrets.iter().zip(values.iter()) {
                let handle = self
                    .secure_value_service
                    .persist_secret(session_id, agent_session_id, node_id, &s.name, v)
                    .await
                    .map_err(|e| Box::<dyn Error + Send + Sync>::from(format!("{e}")))?;
                handles.insert(s.name.clone(), Value::String(handle));
            }

            return Ok(json!({
                "status": "resumed",
                "handles": Value::Object(handles)
            }));
        }

        // Suspend-path emission.
        let base_id = effective_config
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
    /// Also captures the `agent_session_id` of the most recent persist so
    /// propagation tests can assert it.
    pub(super) struct StubRepo {
        pub(super) storage: Mutex<HashMap<String, String>>,
        pub(super) last_persist_agent: Mutex<Option<Option<String>>>,
        pub(super) last_exists_agent: Mutex<Option<Option<String>>>,
    }

    impl StubRepo {
        pub(super) fn new() -> Arc<Self> {
            Arc::new(Self {
                storage: Mutex::new(HashMap::new()),
                last_persist_agent: Mutex::new(None),
                last_exists_agent: Mutex::new(None),
            })
        }
    }

    #[async_trait]
    impl SecureValueRepository for StubRepo {
        async fn persist(
            &self,
            _session_id: &str,
            agent_session_id: Option<&str>,
            _source_node_id: &str,
            hash_key: &str,
            real_value: &str,
            _field_name: &str,
        ) -> Result<(), DagError> {
            *self.last_persist_agent.lock().unwrap() =
                Some(agent_session_id.map(|s| s.to_string()));
            self.storage
                .lock()
                .unwrap()
                .insert(hash_key.to_string(), real_value.to_string());
            Ok(())
        }
        async fn decrypt(
            &self,
            _session_id: &str,
            _agent_session_id: Option<&str>,
            hash_key: &str,
        ) -> Result<Option<String>, DagError> {
            Ok(self.storage.lock().unwrap().get(hash_key).cloned())
        }
        async fn exists(
            &self,
            _session_id: &str,
            agent_session_id: Option<&str>,
            hash_key: &str,
        ) -> Result<bool, DagError> {
            *self.last_exists_agent.lock().unwrap() =
                Some(agent_session_id.map(|s| s.to_string()));
            Ok(self.storage.lock().unwrap().contains_key(hash_key))
        }
        async fn cleanup(&self, _session_id: &str) -> Result<(), DagError> {
            self.storage.lock().unwrap().clear();
            Ok(())
        }
        async fn cleanup_expired(&self) -> Result<u64, DagError> {
            Ok(0)
        }
        async fn cleanup_expired_for_run(
            &self,
            _session_id: &str,
            _agent_session_id: Option<&str>,
        ) -> Result<u64, DagError> {
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

    pub(super) fn inputs_with_agent(
        node_id: &str,
        session_id: &str,
        agent_session_id: &str,
    ) -> NodeInputs {
        let mut m = inputs_with(node_id, session_id);
        m.insert(
            "__colmena_agent_session_id".into(),
            Value::String(agent_session_id.into()),
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
    fn parser_extracts_two_values_by_id() {
        let secrets = vec![
            Secret { question: "Q1?".into(), name: "n1".into() },
            Secret { question: "Q2?".into(), name: "n2".into() },
        ];
        let answer = "Q[n1]: Q1?\nA[n1]: val-one\nQ[n2]: Q2?\nA[n2]: val-two";
        let values = call_parser(answer, &secrets).unwrap();
        assert_eq!(values.get("n1").unwrap(), "val-one");
        assert_eq!(values.get("n2").unwrap(), "val-two");
    }

    #[test]
    fn parser_order_independent() {
        let secrets = vec![
            Secret { question: "Q1?".into(), name: "n1".into() },
            Secret { question: "Q2?".into(), name: "n2".into() },
        ];
        let answer = "Q[n2]: Q2?\nA[n2]: val-two\nQ[n1]: Q1?\nA[n1]: val-one";
        let values = call_parser(answer, &secrets).unwrap();
        assert_eq!(values.get("n1").unwrap(), "val-one");
        assert_eq!(values.get("n2").unwrap(), "val-two");
    }

    #[test]
    fn parser_preserves_internal_newlines_in_value() {
        let secrets = vec![
            Secret { question: "PEM?".into(), name: "key".into() },
            Secret { question: "FP?".into(), name: "fp".into() },
        ];
        let answer = "Q[key]: PEM?\nA[key]: line-1\nline-2\nline-3\nQ[fp]: FP?\nA[fp]: ab:cd";
        let values = call_parser(answer, &secrets).unwrap();
        assert_eq!(values.get("key").unwrap(), "line-1\nline-2\nline-3");
        assert_eq!(values.get("fp").unwrap(), "ab:cd");
    }

    #[test]
    fn parser_errors_on_missing_id() {
        let secrets = vec![
            Secret { question: "Q1?".into(), name: "n1".into() },
            Secret { question: "Q2?".into(), name: "n2".into() },
        ];
        let answer = "Q[n1]: Q1?\nA[n1]: only";
        let err = call_parser(answer, &secrets).unwrap_err();
        assert!(err.contains("missing answer for id 'n2'"), "got: {err}");
    }

    #[test]
    fn parser_errors_on_empty_value() {
        let secrets = vec![
            Secret { question: "Q1?".into(), name: "n1".into() },
            Secret { question: "Q2?".into(), name: "n2".into() },
        ];
        let answer = "Q[n1]: Q1?\nA[n1]: \nQ[n2]: Q2?\nA[n2]: v2";
        let err = call_parser(answer, &secrets).unwrap_err();
        assert!(err.contains("empty answer for A[n1]"), "got: {err}");
    }

    // Adapter that calls the shared parser with the secrets' names as the
    // expected id set.
    fn call_parser(
        answer: &str,
        secrets: &[Secret],
    ) -> Result<std::collections::HashMap<String, String>, String> {
        use crate::dag_engine::infrastructure::nodes::qa_response_parser::parse_qa_response;
        let ids: Vec<&str> = secrets.iter().map(|s| s.name.as_str()).collect();
        parse_qa_response(answer, &ids).map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn resume_persists_two_secrets_and_returns_handles_map() {
        let (node, repo) = build_node();
        let mut state = Value::Null;
        let cfg = json!({
            "secrets": [
                { "question": "Cliente ID?",     "name": "amadeus_client_id" },
                { "question": "Cliente secret?", "name": "amadeus_client_secret" }
            ]
        });
        let mut inputs = inputs_with("ask_creds", "sx");
        inputs.insert(
            "__colmena_resume_answer".into(),
            Value::String(
                "Q[amadeus_client_id]: Cliente ID?\nA[amadeus_client_id]: ABC-CLI-ID\nQ[amadeus_client_secret]: Cliente secret?\nA[amadeus_client_secret]: XYZ-CLI-SEC".into(),
            ),
        );

        let out = node
            .execute(&inputs, &cfg, &mut state, None)
            .await
            .unwrap();

        assert_eq!(out["status"], "resumed");
        assert_eq!(
            out["handles"]["amadeus_client_id"],
            "<sv_amadeus_client_id>"
        );
        assert_eq!(
            out["handles"]["amadeus_client_secret"],
            "<sv_amadeus_client_secret>"
        );

        // Real values must NEVER appear in the output.
        let serialized = out.to_string();
        assert!(!serialized.contains("ABC-CLI-ID"), "real value leaked: {serialized}");
        assert!(!serialized.contains("XYZ-CLI-SEC"), "real value leaked: {serialized}");

        // But they ARE persisted in the repo.
        let stored = repo.storage.lock().unwrap();
        assert_eq!(
            stored.get("<sv_amadeus_client_id>").map(String::as_str),
            Some("ABC-CLI-ID")
        );
        assert_eq!(
            stored.get("<sv_amadeus_client_secret>").map(String::as_str),
            Some("XYZ-CLI-SEC")
        );
    }

    #[tokio::test]
    async fn resume_errors_on_handle_collision() {
        let (node, repo) = build_node();
        // Pre-populate a colliding handle.
        repo.storage
            .lock()
            .unwrap()
            .insert("<sv_dup_token>".into(), "preexisting".into());

        let mut state = Value::Null;
        let cfg = json!({
            "secrets": [{ "question": "Token?", "name": "dup_token" }]
        });
        let mut inputs = inputs_with("ask", "sx");
        inputs.insert(
            "__colmena_resume_answer".into(),
            Value::String("Q[dup_token]: Token?\nA[dup_token]: new-value".into()),
        );

        let err = node
            .execute(&inputs, &cfg, &mut state, None)
            .await
            .unwrap_err();
        assert!(
            format!("{err}").contains("handle <sv_dup_token> already exists"),
            "got: {err}"
        );

        // Repo must NOT have been mutated.
        assert_eq!(
            repo.storage
                .lock()
                .unwrap()
                .get("<sv_dup_token>")
                .map(String::as_str),
            Some("preexisting")
        );
    }

    /// Resume must forward `__colmena_agent_session_id` (when present in inputs)
    /// to both `handle_exists` and `persist_secret` so cross-session lookup works.
    #[tokio::test]
    async fn resume_forwards_agent_session_id_to_repo() {
        let (node, repo) = build_node();
        let mut state = Value::Null;
        let cfg = json!({
            "secrets": [{ "question": "Token?", "name": "tok" }]
        });
        let mut inputs = inputs_with_agent("ask", "ephemeral_session_a", "agent_demo_001");
        inputs.insert(
            "__colmena_resume_answer".into(),
            Value::String("Q[tok]: Token?\nA[tok]: real-tok".into()),
        );

        let _out = node
            .execute(&inputs, &cfg, &mut state, None)
            .await
            .unwrap();

        // exists() must have been called with the agent_session_id.
        assert_eq!(
            repo.last_exists_agent.lock().unwrap().as_ref().unwrap(),
            &Some("agent_demo_001".to_string()),
            "handle_exists must receive agent_session_id from inputs"
        );
        // persist() must have been called with the same agent_session_id.
        assert_eq!(
            repo.last_persist_agent.lock().unwrap().as_ref().unwrap(),
            &Some("agent_demo_001".to_string()),
            "persist_secret must receive agent_session_id from inputs"
        );
    }

    /// When `__colmena_agent_session_id` is absent, resume must pass `None`
    /// to the repo (legacy session-only behavior).
    #[tokio::test]
    async fn resume_without_agent_session_id_passes_none() {
        let (node, repo) = build_node();
        let mut state = Value::Null;
        let cfg = json!({
            "secrets": [{ "question": "Token?", "name": "tok2" }]
        });
        let mut inputs = inputs_with("ask", "session_only");
        inputs.insert(
            "__colmena_resume_answer".into(),
            Value::String("Q[tok2]: Token?\nA[tok2]: real-tok".into()),
        );

        let _out = node
            .execute(&inputs, &cfg, &mut state, None)
            .await
            .unwrap();

        assert_eq!(
            repo.last_exists_agent.lock().unwrap().as_ref().unwrap(),
            &None,
            "handle_exists must receive None when agent_session_id is absent"
        );
        assert_eq!(
            repo.last_persist_agent.lock().unwrap().as_ref().unwrap(),
            &None,
            "persist_secret must receive None when agent_session_id is absent"
        );
    }

    #[tokio::test]
    async fn resume_propagates_parser_errors() {
        let (node, _) = build_node();
        let mut state = Value::Null;
        let cfg = json!({
            "secrets": [{ "question": "Q?", "name": "nxt" }]
        });
        let mut inputs = inputs_with("ask", "sx");
        inputs.insert(
            "__colmena_resume_answer".into(),
            Value::String("UnrelatedText\nval".into()),
        );
        let err = node
            .execute(&inputs, &cfg, &mut state, None)
            .await
            .unwrap_err();
        assert!(
            format!("{err}").contains("missing answer for id 'nxt'"),
            "got: {err}"
        );
    }

    /// Regression guard: the resume path must never emit the real secret value
    /// through `tracing`, regardless of log level.  If a future maintainer adds
    /// a `tracing::info!("answer = {answer}")` style line this test will catch it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn resume_does_not_log_real_values() {
        use std::io::Write;
        use std::sync::Mutex as StdMutex;
        use tracing_subscriber::fmt;

        /// Writer that buffers everything emitted by tracing.
        #[derive(Clone, Default)]
        struct BufWriter(std::sync::Arc<StdMutex<Vec<u8>>>);
        impl<'a> fmt::MakeWriter<'a> for BufWriter {
            type Writer = BufWriterHandle;
            fn make_writer(&'a self) -> Self::Writer {
                BufWriterHandle(self.0.clone())
            }
        }
        struct BufWriterHandle(std::sync::Arc<StdMutex<Vec<u8>>>);
        impl Write for BufWriterHandle {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let buf = BufWriter::default();
        let subscriber = fmt::Subscriber::builder()
            .with_writer(buf.clone())
            .with_max_level(tracing::Level::TRACE)
            .finish();

        let secret_marker = "SUPER_SECRET_MARKER_qwerty12345";
        let cfg = json!({
            "secrets": [{ "question": "Marker?", "name": "marker_secret" }]
        });
        let (node, _repo) = build_node();
        let mut inputs = inputs_with("ask", "sx");
        inputs.insert(
            "__colmena_resume_answer".into(),
            Value::String(format!("Q[marker_secret]: Marker?\nA[marker_secret]: {secret_marker}")),
        );
        let mut state = Value::Null;

        let _guard = tracing::subscriber::set_default(subscriber);
        let _out = node
            .execute(&inputs, &cfg, &mut state, None)
            .await
            .unwrap();
        drop(_guard);

        let captured = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(
            !captured.contains(secret_marker),
            "secret leaked into tracing: {captured}"
        );
    }
}
