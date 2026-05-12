//! `secure_suspend` — pauses the DAG to ask the user for one or more secrets,
//! persists each encrypted, and returns only opaque handles `<sv_<name>>`.
//!
//! See `docs/superpowers/specs/2026-05-07-secure-suspend-node-design.md`.

use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::domain::tool_configuration::{
    NodeSchema, NodeSchemaField, ToolConfiguration,
};
use crate::dag_engine::infrastructure::nodes::qa_response_parser::parse_qa_response;
use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

/// Canonical tool description for `secure_suspend` when exposed via
/// `tool_configurations`. Auto-injected by the LLM node when the user does not
/// provide one — lets `tool_configurations` for secure_suspend stay as terse as
/// `{ "name": "ask_secret", "node_type": "secure_suspend" }` without needing
/// any custom system_message instructions.
pub const SECURE_SUSPEND_TOOL_DESCRIPTION: &str = "Pregunta al usuario por uno o más secretos en una SOLA llamada y los almacena cifrados, devolviendo handles opacos. Llamada: pasa un argumento `secrets` que es un ARRAY de objetos, cada uno con AMBOS campos `question` (string — la pregunta que verá el usuario) y `name` (string — slug en minúsculas [a-z][a-z0-9_]{2,63} que identifica al secreto; debe ser único dentro de esta llamada). Retorna: un mapa `{<name>: \"<sv_<name>_<random>>\"}` con un handle opaco por cada secreto. NUNCA verás los valores reales — solo los handles. Pega los handles tal cual en argumentos de tools subsiguientes que requieran ese secreto; el motor los sustituirá por los valores reales antes de ejecutar la tool y los volverá a enmascarar antes de devolverte la respuesta.";

/// Auto-fill canonical `description` and `node_schema` on a tool entry whose
/// `node_type` is `secure_suspend`. Idempotent: only fills fields the user did
/// NOT provide. A no-op for any other node_type.
pub fn apply_secure_suspend_tool_defaults(tool_cfg: &mut ToolConfiguration) {
    if tool_cfg.node_type != "secure_suspend" {
        return;
    }
    if tool_cfg.description.is_empty() {
        tool_cfg.description = SECURE_SUSPEND_TOOL_DESCRIPTION.to_string();
    }
    if tool_cfg.node_schema.is_none() {
        tool_cfg.node_schema = Some(secure_suspend_tool_node_schema());
    }
}

/// Canonical `node_schema` for `secure_suspend` when used as a tool. Captures
/// the `secrets: [{question, name}]` shape so the LLM-facing tool definition
/// is self-describing without manual `node_schema` setup.
pub fn secure_suspend_tool_node_schema() -> NodeSchema {
    let mut schema: NodeSchema = std::collections::HashMap::new();
    schema.insert(
        "secrets".to_string(),
        NodeSchemaField {
            field_type: "array".to_string(),
            fixed: None,
            required: Some(true),
            description: Some(
                "Array OBLIGATORIO de objetos {question, name}. Cada item DEBE tener ambos campos: `question` (string — pregunta visible al usuario) y `name` (string — slug en minúsculas [a-z][a-z0-9_]{2,63}, único dentro del array). El `name` también es la clave bajo la cual recibirás el handle en la respuesta."
                    .to_string(),
            ),
            pattern: None,
            properties: None,
            items: Some(Box::new(NodeSchemaField {
                field_type: "object".to_string(),
                fixed: None,
                required: None,
                description: Some("Objeto con campos `question` y `name`.".to_string()),
                pattern: None,
                properties: None,
                items: None,
            })),
        },
    );
    schema
}

/// Build a minimal synthetic `ToolConfiguration` for `secure_suspend`. The
/// `description` and `node_schema` fields are intentionally left empty —
/// callers are expected to run [`apply_secure_suspend_tool_defaults`] on the
/// resulting entry (the LLM node already does this for every entry in
/// `tool_configurations`, so injecting then deferring is the cheapest path).
pub fn synthetic_secure_suspend_tool(name: &str) -> ToolConfiguration {
    #[allow(deprecated)]
    ToolConfiguration {
        name: name.to_string(),
        description: String::new(),
        node_type: "secure_suspend".to_string(),
        fixed_config: std::collections::HashMap::new(),
        exposed_inputs: None,
        parameters: None,
        mergeable_fields: None,
        field_mapping: None,
        node_schema: None,
        node_config: None,
        expose_sub_tools: None,
        summary: None,
        eager: false,
    }
}

/// Inject a synthetic `ask_secret` tool into the given `tool_configurations`
/// map iff `flag` is true AND no existing entry already wires `secure_suspend`
/// AND the target key is free. Idempotent and conflict-safe — explicit user
/// declarations always win.
pub fn maybe_inject_secure_suspend_tool(
    flag: bool,
    tool_configurations: &mut std::collections::HashMap<String, ToolConfiguration>,
) {
    if !flag {
        return;
    }
    let already_declared = tool_configurations
        .values()
        .any(|tc| tc.node_type == "secure_suspend");
    if already_declared {
        return;
    }
    const INJECTED_KEY: &str = "ask_secret";
    if tool_configurations.contains_key(INJECTED_KEY) {
        return;
    }
    tool_configurations.insert(
        INJECTED_KEY.to_string(),
        synthetic_secure_suspend_tool(INJECTED_KEY),
    );
}

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
            .ok_or_else(|| "secure_suspend: each secret must have a 'name' string".to_string())?;
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

        tracing::info!(
            target: "colmena::secure_suspend",
            secret_count = secrets.len(),
            has_resume_answer = inputs.get("__colmena_resume_answer").is_some(),
            has_session_id = inputs.get("__colmena_session_id").is_some(),
            has_agent_session_id = inputs.get("__colmena_agent_session_id").is_some(),
            "secure_suspend: execute entry"
        );

        if let Some(answer_val) = inputs.get("__colmena_resume_answer") {
            let answer = answer_val.as_str().ok_or_else(|| {
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

            // Minimum-length check: outbound masking does substring matching on
            // decrypted values, so very short values cause pathological over-masking.
            // Reject values < 4 chars with a clear error before any write.
            const MIN_SECRET_VALUE_LEN: usize = 4;
            for (s, v) in secrets.iter().zip(values.iter()) {
                if v.chars().count() < MIN_SECRET_VALUE_LEN {
                    return Err(Box::<dyn Error + Send + Sync>::from(format!(
                        "secure_suspend: value for secret '{}' is too short \
                         (min {MIN_SECRET_VALUE_LEN} chars). Short values cause \
                         unsafe outbound masking — please supply ≥{MIN_SECRET_VALUE_LEN} chars.",
                        s.name
                    )));
                }
            }

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
                tracing::info!(
                    target: "colmena::secure_suspend",
                    name = %s.name,
                    session_id = %session_id,
                    agent_session_id = ?agent_session_id,
                    "secure_suspend: persisting secret"
                );
                let handle = self
                    .secure_value_service
                    .persist_secret(session_id, agent_session_id, node_id, &s.name, v)
                    .await
                    .map_err(|e| Box::<dyn Error + Send + Sync>::from(format!("{e}")))?;
                tracing::info!(
                    target: "colmena::secure_suspend",
                    handle = %handle,
                    "secure_suspend: persist OK"
                );
                handles.insert(s.name.clone(), Value::String(handle));
            }

            return Ok(json!({
                "status": "resumed",
                "handles": Value::Object(handles)
            }));
        }

        // Suspend-path emission. The id of each question is the secret's `name`
        // — this is the contract the resume path expects (see `id_refs` above).
        // `config.id` and `__node_id` are intentionally ignored here: in
        // `secure_suspend`, names ARE the ids.
        let questions: Vec<Value> = secrets
            .iter()
            .map(|s| {
                json!({
                    "id": s.name,
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
mod tool_defaults_tests {
    use super::*;
    use crate::dag_engine::domain::tool_configuration::ToolConfiguration;

    fn empty_tool_cfg(node_type: &str) -> ToolConfiguration {
        serde_json::from_value(serde_json::json!({
            "name": "alias",
            "node_type": node_type
        }))
        .expect(
            "ToolConfiguration with only name+node_type must deserialize \
                 — description should be #[serde(default)]",
        )
    }

    #[test]
    fn defaults_injected_when_description_and_schema_missing() {
        let mut cfg = empty_tool_cfg("secure_suspend");
        assert!(cfg.description.is_empty());
        assert!(cfg.node_schema.is_none());

        apply_secure_suspend_tool_defaults(&mut cfg);

        assert_eq!(cfg.description, SECURE_SUSPEND_TOOL_DESCRIPTION);
        let schema = cfg.node_schema.expect("schema injected");
        let secrets = schema.get("secrets").expect("secrets field");
        assert_eq!(secrets.field_type, "array");
        assert_eq!(secrets.required, Some(true));
        let items = secrets.items.as_ref().expect("items declared");
        assert_eq!(items.field_type, "object");
    }

    #[test]
    fn defaults_preserve_user_provided_description() {
        let mut cfg = serde_json::from_value::<ToolConfiguration>(serde_json::json!({
            "name": "alias",
            "node_type": "secure_suspend",
            "description": "MI descripción custom"
        }))
        .unwrap();
        apply_secure_suspend_tool_defaults(&mut cfg);
        assert_eq!(cfg.description, "MI descripción custom");
        // schema still auto-injected since user didn't provide one
        assert!(cfg.node_schema.is_some());
    }

    #[test]
    fn defaults_preserve_user_provided_schema() {
        let mut cfg = serde_json::from_value::<ToolConfiguration>(serde_json::json!({
            "name": "alias",
            "node_type": "secure_suspend",
            "node_schema": {
                "secrets": { "type": "array", "required": true, "items": { "type": "string" } }
            }
        }))
        .unwrap();
        apply_secure_suspend_tool_defaults(&mut cfg);
        // description auto-injected since user didn't provide one
        assert!(!cfg.description.is_empty());
        // schema preserved as-is — items must still be string, not object
        let schema = cfg.node_schema.expect("schema preserved");
        let secrets = schema.get("secrets").unwrap();
        assert_eq!(secrets.items.as_ref().unwrap().field_type, "string");
    }

    #[test]
    fn no_op_for_other_node_types() {
        let mut cfg = empty_tool_cfg("http_request");
        apply_secure_suspend_tool_defaults(&mut cfg);
        assert!(
            cfg.description.is_empty(),
            "http_request must NOT inherit secure_suspend defaults"
        );
        assert!(
            cfg.node_schema.is_none(),
            "http_request must NOT receive a schema"
        );
    }

    #[test]
    fn idempotent_when_called_twice() {
        let mut cfg = empty_tool_cfg("secure_suspend");
        apply_secure_suspend_tool_defaults(&mut cfg);
        let desc_first = cfg.description.clone();
        apply_secure_suspend_tool_defaults(&mut cfg);
        assert_eq!(
            cfg.description, desc_first,
            "second call must not duplicate or alter the description"
        );
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
            *self.last_exists_agent.lock().unwrap() = Some(agent_session_id.map(|s| s.to_string()));
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
            .execute(&inputs_with("ask", "s1"), &json!({}), &mut state, None)
            .await;
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("secrets list missing or empty"), "got: {msg}");
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
        assert_eq!(qs[0]["id"], "amadeus_client_id");
        assert_eq!(qs[0]["options"], Value::Null);
        assert_eq!(qs[1]["question"], "Cliente secret?");
        assert_eq!(qs[1]["id"], "amadeus_client_secret");
    }

    /// Regression: in secure_suspend the question `id` is ALWAYS the secret's
    /// `name`. `config.id` and `__node_id` must NOT influence it — the resume
    /// branch keys answers by `name`, so any other id would break the contract.
    #[tokio::test]
    async fn suspend_ids_ignore_config_id_and_node_id() {
        let (node, _) = build_node();
        let mut state = Value::Null;
        let cfg = json!({
            "id": "custom_block",
            "secrets": [{"question": "Q?", "name": "the_name"}]
        });
        let out = node
            .execute(&inputs_with("some_node_id", "sx"), &cfg, &mut state, None)
            .await
            .unwrap();
        assert_eq!(out["questions"][0]["id"], "the_name");
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
        assert!(msg.contains("duplicate question text"), "got: {msg}");
    }

    #[test]
    fn parser_extracts_two_values_by_id() {
        let secrets = vec![
            Secret {
                question: "Q1?".into(),
                name: "n1".into(),
            },
            Secret {
                question: "Q2?".into(),
                name: "n2".into(),
            },
        ];
        let answer = "Q[n1]: Q1?\nA[n1]: val-one\nQ[n2]: Q2?\nA[n2]: val-two";
        let values = call_parser(answer, &secrets).unwrap();
        assert_eq!(values.get("n1").unwrap(), "val-one");
        assert_eq!(values.get("n2").unwrap(), "val-two");
    }

    #[test]
    fn parser_order_independent() {
        let secrets = vec![
            Secret {
                question: "Q1?".into(),
                name: "n1".into(),
            },
            Secret {
                question: "Q2?".into(),
                name: "n2".into(),
            },
        ];
        let answer = "Q[n2]: Q2?\nA[n2]: val-two\nQ[n1]: Q1?\nA[n1]: val-one";
        let values = call_parser(answer, &secrets).unwrap();
        assert_eq!(values.get("n1").unwrap(), "val-one");
        assert_eq!(values.get("n2").unwrap(), "val-two");
    }

    #[test]
    fn parser_preserves_internal_newlines_in_value() {
        let secrets = vec![
            Secret {
                question: "PEM?".into(),
                name: "key".into(),
            },
            Secret {
                question: "FP?".into(),
                name: "fp".into(),
            },
        ];
        let answer = "Q[key]: PEM?\nA[key]: line-1\nline-2\nline-3\nQ[fp]: FP?\nA[fp]: ab:cd";
        let values = call_parser(answer, &secrets).unwrap();
        assert_eq!(values.get("key").unwrap(), "line-1\nline-2\nline-3");
        assert_eq!(values.get("fp").unwrap(), "ab:cd");
    }

    #[test]
    fn parser_errors_on_missing_id() {
        let secrets = vec![
            Secret {
                question: "Q1?".into(),
                name: "n1".into(),
            },
            Secret {
                question: "Q2?".into(),
                name: "n2".into(),
            },
        ];
        let answer = "Q[n1]: Q1?\nA[n1]: only";
        let err = call_parser(answer, &secrets).unwrap_err();
        assert!(err.contains("missing answer for id 'n2'"), "got: {err}");
    }

    #[test]
    fn parser_errors_on_empty_value() {
        let secrets = vec![
            Secret {
                question: "Q1?".into(),
                name: "n1".into(),
            },
            Secret {
                question: "Q2?".into(),
                name: "n2".into(),
            },
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

    /// End-to-end contract test: the ids emitted by the SUSPEND branch must be
    /// EXACTLY the ids the RESUME branch accepts. Builds a Q/A response using
    /// only the ids that came back from the SUSPENDED output and asserts the
    /// resume succeeds.
    #[tokio::test]
    async fn suspend_ids_match_resume_expected_ids() {
        let (node, _) = build_node();
        let cfg = json!({
            "secrets": [
                { "question": "User?", "name": "uname" },
                { "question": "Pass?", "name": "pword" }
            ]
        });

        // (1) Suspend branch — capture emitted ids.
        let mut state = Value::Null;
        let out = node
            .execute(&inputs_with("any_node", "sx"), &cfg, &mut state, None)
            .await
            .unwrap();
        let qs = out["questions"].as_array().expect("questions");
        let emitted_ids: Vec<String> = qs
            .iter()
            .map(|q| q["id"].as_str().unwrap().to_string())
            .collect();

        // (2) Build a Q/A response using ONLY the emitted ids.
        let answer = format!(
            "Q[{0}]: User?\nA[{0}]: alice123\nQ[{1}]: Pass?\nA[{1}]: hunter22",
            emitted_ids[0], emitted_ids[1]
        );

        // (3) Resume branch — must accept the answer without error.
        let mut resume_inputs = inputs_with("any_node", "sx");
        resume_inputs.insert("__colmena_resume_answer".into(), Value::String(answer));
        let resumed = node
            .execute(&resume_inputs, &cfg, &mut state, None)
            .await
            .expect("resume must accept ids the suspend branch emitted");
        assert_eq!(resumed["status"], "resumed");
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

        let out = node.execute(&inputs, &cfg, &mut state, None).await.unwrap();

        assert_eq!(out["status"], "resumed");
        let handle_id = out["handles"]["amadeus_client_id"]
            .as_str()
            .expect("handle for amadeus_client_id must be a string")
            .to_string();
        let handle_secret = out["handles"]["amadeus_client_secret"]
            .as_str()
            .expect("handle for amadeus_client_secret must be a string")
            .to_string();
        assert!(
            handle_id.starts_with("<sv_amadeus_client_id_") && handle_id.ends_with('>'),
            "got: {handle_id}"
        );
        assert!(
            handle_secret.starts_with("<sv_amadeus_client_secret_") && handle_secret.ends_with('>'),
            "got: {handle_secret}"
        );

        // Real values must NEVER appear in the output.
        let serialized = out.to_string();
        assert!(
            !serialized.contains("ABC-CLI-ID"),
            "real value leaked: {serialized}"
        );
        assert!(
            !serialized.contains("XYZ-CLI-SEC"),
            "real value leaked: {serialized}"
        );

        // But they ARE persisted in the repo under the returned handles.
        let stored = repo.storage.lock().unwrap();
        assert_eq!(
            stored.get(handle_id.as_str()).map(String::as_str),
            Some("ABC-CLI-ID")
        );
        assert_eq!(
            stored.get(handle_secret.as_str()).map(String::as_str),
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

        let _out = node.execute(&inputs, &cfg, &mut state, None).await.unwrap();

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

        let _out = node.execute(&inputs, &cfg, &mut state, None).await.unwrap();

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
    async fn execute_rejects_value_shorter_than_4_chars() {
        let service = Arc::new(SecureValueService::new(StubRepo::new()));
        let node = SecureSuspendNode::new(service);

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "__colmena_session_id".to_string(),
            Value::String("s".into()),
        );
        inputs.insert(
            "__colmena_resume_answer".to_string(),
            Value::String("Q[shortie]: ?\nA[shortie]: abc".into()), // 3 chars
        );
        inputs.insert(
            "secrets".to_string(),
            json!([{"question": "?", "name": "shortie"}]),
        );

        let cfg = json!({});
        let mut state = Value::Null;
        let err = node
            .execute(&inputs, &cfg, &mut state, None)
            .await
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("too short") && msg.contains("shortie"),
            "got: {msg}"
        );
    }

    #[tokio::test]
    async fn execute_accepts_value_of_exactly_4_chars() {
        let service = Arc::new(SecureValueService::new(StubRepo::new()));
        let node = SecureSuspendNode::new(service);

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "__colmena_session_id".to_string(),
            Value::String("s".into()),
        );
        inputs.insert(
            "__colmena_resume_answer".to_string(),
            Value::String("Q[four]: ?\nA[four]: abcd".into()), // 4 chars
        );
        inputs.insert(
            "secrets".to_string(),
            json!([{"question": "?", "name": "four"}]),
        );

        let cfg = json!({});
        let mut state = Value::Null;
        let out = node.execute(&inputs, &cfg, &mut state, None).await.unwrap();
        assert_eq!(out["status"], "resumed");
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

    // ── injection-helper tests ────────────────────────────────────────────────

    #[test]
    fn maybe_inject_secure_suspend_tool_noop_when_flag_false() {
        let mut map: std::collections::HashMap<String, ToolConfiguration> =
            std::collections::HashMap::new();
        maybe_inject_secure_suspend_tool(false, &mut map);
        assert!(map.is_empty(), "flag=false must not inject anything");
    }

    #[test]
    fn maybe_inject_secure_suspend_tool_inserts_when_flag_true_and_no_conflict() {
        let mut map: std::collections::HashMap<String, ToolConfiguration> =
            std::collections::HashMap::new();
        maybe_inject_secure_suspend_tool(true, &mut map);
        let entry = map
            .get("ask_secret")
            .expect("ask_secret entry must be inserted");
        assert_eq!(entry.name, "ask_secret");
        assert_eq!(entry.node_type, "secure_suspend");
        assert!(entry.description.is_empty());
        assert!(entry.node_schema.is_none());
    }

    #[test]
    fn maybe_inject_secure_suspend_tool_noop_when_user_already_declared_secure_suspend() {
        let mut map: std::collections::HashMap<String, ToolConfiguration> =
            std::collections::HashMap::new();
        map.insert(
            "ask_credentials".to_string(),
            synthetic_secure_suspend_tool("ask_credentials"),
        );
        maybe_inject_secure_suspend_tool(true, &mut map);
        assert_eq!(
            map.len(),
            1,
            "must not inject when user already declared secure_suspend"
        );
        assert!(map.contains_key("ask_credentials"));
        assert!(!map.contains_key("ask_secret"));
    }

    #[test]
    fn maybe_inject_secure_suspend_tool_noop_when_ask_secret_key_taken_by_other_tool() {
        let mut map: std::collections::HashMap<String, ToolConfiguration> =
            std::collections::HashMap::new();
        let mut other = synthetic_secure_suspend_tool("ask_secret");
        other.node_type = "log".to_string();
        map.insert("ask_secret".to_string(), other);
        maybe_inject_secure_suspend_tool(true, &mut map);
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get("ask_secret").unwrap().node_type,
            "log",
            "must not clobber existing key"
        );
    }

    /// Regression guard: the resume path must never emit the real secret value
    /// through `tracing`, regardless of log level.  If a future maintainer adds
    /// a `tracing::info!("answer = {answer}")` style line this test will catch it.
    // ── logging-leak regression ───────────────────────────────────────────────

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
            Value::String(format!(
                "Q[marker_secret]: Marker?\nA[marker_secret]: {secret_marker}"
            )),
        );
        let mut state = Value::Null;

        let _guard = tracing::subscriber::set_default(subscriber);
        let _out = node.execute(&inputs, &cfg, &mut state, None).await.unwrap();
        drop(_guard);

        let captured = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(
            !captured.contains(secret_marker),
            "secret leaked into tracing: {captured}"
        );
    }
}
