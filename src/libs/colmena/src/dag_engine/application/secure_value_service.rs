use crate::dag_engine::domain::{error::DagError, secure_value_repository::SecureValueRepository};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// Business logic for hashing outputs and injecting secrets
pub struct SecureValueService {
    repo: Arc<dyn SecureValueRepository>,
}

/// Whether a string is a secure-value placeholder rather than a literal.
///
/// THE definition, extracted so every caller shares it. `collect_placeholders`
/// delegates here, and so does MCP connection keying, which must know whether
/// a header value is a session-scoped reference (its resolved secret differs
/// per session) or a literal (the same everywhere). A second, drifting copy of
/// this rule would eventually let one session reuse another's credential.
pub(crate) fn is_secure_value_placeholder(s: &str) -> bool {
    s.starts_with('<') && s.ends_with('>') && s.len() > 2
}

impl SecureValueService {
    pub fn new(repo: Arc<dyn SecureValueRepository>) -> Self {
        Self { repo }
    }

    /// Process output from a secure HTTP node: hash all sensitive values
    ///
    /// Returns the output with placeholders instead of real values,
    /// and stores encrypted mappings in the database.
    ///
    /// `agent_session_id` is forwarded to the repository so cross-session lookup
    /// (resume under a different ephemeral session) works. `None` preserves
    /// legacy session-only semantics.
    pub async fn hash_output(
        &self,
        output: &Value,
        config: &Value,
        session_id: &str,
        agent_session_id: Option<&str>,
        source_node_id: &str,
    ) -> Result<Value, DagError> {
        // If secure flag not set, return output unchanged
        if config.get("secure").and_then(|v| v.as_bool()) != Some(true) {
            return Ok(output.clone());
        }

        let mut hashed = output.clone();
        let mut counter = 1u32;
        let mut to_persist = Vec::new();

        // Recursively traverse and collect values to hash
        self.collect_values_to_hash(&mut hashed, &mut counter, &mut to_persist);

        // Persist all mappings to database
        for (hash_key, real_value) in to_persist {
            self.repo
                .persist(
                    session_id,
                    agent_session_id,
                    source_node_id,
                    &hash_key,
                    &real_value,
                    "value",
                )
                .await?;
        }

        Ok(hashed)
    }

    /// Collect all string/number values for hashing (synchronous traversal)
    fn collect_values_to_hash(
        &self,
        value: &mut Value,
        counter: &mut u32,
        to_persist: &mut Vec<(String, String)>,
    ) {
        match value {
            // For HTTP response: only hash the "body", not "status"
            Value::Object(map) if map.contains_key("status") => {
                if let Some(body) = map.get_mut("body") {
                    self.collect_values_to_hash(body, counter, to_persist);
                }
            }
            // Recursively process object values
            Value::Object(map) => {
                let keys: Vec<_> = map.keys().cloned().collect();
                for key in keys {
                    if let Some(v) = map.get_mut(&key) {
                        self.collect_values_to_hash(v, counter, to_persist);
                    }
                }
            }
            // Recursively process array elements
            Value::Array(arr) => {
                for i in 0..arr.len() {
                    if let Some(v) = arr.get_mut(i) {
                        self.collect_values_to_hash(v, counter, to_persist);
                    }
                }
            }
            // Hash string and number values
            Value::String(s) => {
                let real_value = s.clone();
                let hash_key = format!("<value_{}>", counter);
                *counter += 1;
                to_persist.push((hash_key.clone(), real_value));
                *value = Value::String(hash_key);
            }
            Value::Number(n) => {
                let real_value = n.to_string();
                let hash_key = format!("<value_{}>", counter);
                *counter += 1;
                to_persist.push((hash_key.clone(), real_value));
                *value = Value::String(hash_key);
            }
            // Skip null and boolean values (not sensitive)
            _ => {}
        }
    }

    /// Inject real values back into inputs before non-LLM node execution.
    /// Automatically detects placeholders (`<value_N>`, `<sv_*>`) and replaces them.
    ///
    /// `agent_session_id` is forwarded to the repository, and it takes
    /// PRECEDENCE: when present the lookup is agent-only, and when absent it is
    /// session-only. The two branches are mutually exclusive — there is no
    /// fallback from one to the other (`PostgresSecureValueRepository::decrypt`
    /// is a plain if/else over two `WHERE` clauses).
    ///
    /// The effect an earlier version of this comment called a "fallback" is
    /// real but arrives differently: because the agent branch ignores
    /// `session_id` entirely, a resume under a fresh ephemeral session still
    /// finds secrets persisted under the same agent. Purpose achieved,
    /// mechanism different — and the difference matters to anything that
    /// partitions on these ids, such as MCP connection keying.
    ///
    /// Returns the map of `(decrypted_value → handle)` for every placeholder that
    /// was successfully resolved and substituted. Outbound-masking callers (e.g.
    /// the DAG tool executor) use this to rewrite the real value back to its
    /// handle in any response that echoes it. Callers that don't need masking
    /// can discard the returned map.
    pub async fn inject_secrets(
        &self,
        inputs: &mut Value,
        session_id: &str,
        agent_session_id: Option<&str>,
    ) -> Result<HashMap<String, String>, DagError> {
        // First pass: collect all placeholders
        let mut placeholders = Vec::new();
        self.collect_placeholders(inputs, &mut placeholders);

        // Second pass: decrypt and replace; record (decrypted → handle) pairs
        // so callers can mask outbound responses.
        let mut applied: HashMap<String, String> = HashMap::new();
        for placeholder in placeholders {
            if let Some(real) = self
                .repo
                .decrypt(session_id, agent_session_id, &placeholder)
                .await?
            {
                applied.insert(real.clone(), placeholder.clone());
                self.replace_placeholder(inputs, &placeholder, real);
            }
        }

        Ok(applied)
    }

    /// Collect all placeholder strings in the value tree
    fn collect_placeholders(&self, value: &Value, placeholders: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for v in map.values() {
                    self.collect_placeholders(v, placeholders);
                }
            }
            Value::Array(arr) => {
                for v in arr {
                    self.collect_placeholders(v, placeholders);
                }
            }
            Value::String(s) if is_secure_value_placeholder(s) => {
                placeholders.push(s.clone());
            }
            _ => {}
        }
    }

    /// Replace all occurrences of a placeholder with the real value
    fn replace_placeholder(&self, value: &mut Value, placeholder: &str, real: String) {
        match value {
            Value::Object(map) => {
                for v in map.values_mut() {
                    self.replace_placeholder(v, placeholder, real.clone());
                }
            }
            Value::Array(arr) => {
                for v in arr {
                    self.replace_placeholder(v, placeholder, real.clone());
                }
            }
            Value::String(s) if s == placeholder => {
                *s = real.clone();
            }
            _ => {}
        }
    }

    /// Store a single named secret as `<sv_<name>_<8hex>>` and persist its real
    /// value. The handle returned to callers is unique per persist (a random
    /// 8-hex suffix taken from a fresh UUID v4). Used by the `secure_suspend`
    /// node to record user-supplied secrets.
    pub async fn persist_secret(
        &self,
        session_id: &str,
        agent_session_id: Option<&str>,
        source_node_id: &str,
        name: &str,
        real_value: &str,
    ) -> Result<String, DagError> {
        let handle = Self::new_handle(name);
        self.repo
            .persist(
                session_id,
                agent_session_id,
                source_node_id,
                &handle,
                real_value,
                "secret",
            )
            .await?;
        Ok(handle)
    }

    /// Build a handle `<sv_<name>_<8-hex>>` using random bytes from a v4
    /// UUID. The suffix prevents an LLM from guessing/forging handle names
    /// (e.g. `<sv_admin>`) — every persist yields a unique label.
    fn new_handle(name: &str) -> String {
        let id = Uuid::new_v4().simple().to_string(); // 32 hex chars
        let suffix: String = id.chars().take(8).collect();
        format!("<sv_{name}_{suffix}>")
    }

    /// Check whether a handle is already registered (agent-first when provided,
    /// else session-scoped). Used by `secure_suspend` to detect collisions
    /// before persisting.
    pub async fn handle_exists(
        &self,
        session_id: &str,
        agent_session_id: Option<&str>,
        handle: &str,
    ) -> Result<bool, DagError> {
        self.repo.exists(session_id, agent_session_id, handle).await
    }

    /// Recursively walk `value`. For each JSON string, replace every
    /// substring equal to any key in `mapping` with the corresponding value.
    /// Replacements are applied longest-key-first so two secrets sharing a
    /// prefix do not leak partial content. Keys shorter than 4 chars are
    /// skipped as a defense-in-depth check (secure_suspend already rejects
    /// short values at persist time).
    pub fn mask_outbound(&self, value: &mut Value, mapping: &HashMap<String, String>) {
        if mapping.is_empty() {
            return;
        }
        // Sort keys longest-first.
        let mut ordered: Vec<(&String, &String)> = mapping
            .iter()
            .filter(|(k, _)| k.chars().count() >= 4)
            .collect();
        ordered.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));

        Self::mask_walk(value, &ordered);
    }

    fn mask_walk(value: &mut Value, ordered: &[(&String, &String)]) {
        match value {
            Value::String(s) => {
                for (k, handle) in ordered {
                    if s.contains(k.as_str()) {
                        *s = s.replace(k.as_str(), handle.as_str());
                    }
                }
            }
            Value::Array(arr) => {
                for item in arr {
                    Self::mask_walk(item, ordered);
                }
            }
            Value::Object(map) => {
                for (_, v) in map.iter_mut() {
                    Self::mask_walk(v, ordered);
                }
            }
            _ => {}
        }
    }

    /// Cleanup all secure values for a session
    pub async fn cleanup(&self, session_id: &str) -> Result<(), DagError> {
        self.repo.cleanup(session_id).await
    }

    /// Per-run sweep: delete only rows whose `expires_at < NOW()` and whose
    /// `session_id` matches this run, OR whose `agent_session_id` matches
    /// this conversation. Live rows survive — they are reused on the next
    /// turn. Returns the count of deleted rows.
    pub async fn cleanup_expired_for_run(
        &self,
        session_id: &str,
        agent_session_id: Option<&str>,
    ) -> Result<u64, DagError> {
        self.repo
            .cleanup_expired_for_run(session_id, agent_session_id)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    /// Per-write entry the mock repo records. Lets tests assert agent-first
    /// vs session-only lookup semantics meaningfully.
    #[derive(Clone, Debug)]
    struct MockEntry {
        session: String,
        agent: Option<String>,
        hash: String,
        value: String,
    }

    /// Mock repository for testing. Stores every persist as a row.
    ///
    /// CAVEAT: `decrypt`/`exists` here look up agent-first and then FALL BACK
    /// to session, which production does NOT do — the real repository takes
    /// exactly one of two mutually exclusive branches. The mock is therefore
    /// more permissive than production and must not be used to reason about
    /// how secrets partition. Anything that keys on that partition (MCP
    /// connection identity, for one) has to read
    /// `PostgresSecureValueRepository::decrypt` instead.
    struct MockSecureValueRepository {
        rows: Mutex<Vec<MockEntry>>,
    }

    impl MockSecureValueRepository {
        fn new() -> Self {
            Self {
                rows: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl SecureValueRepository for MockSecureValueRepository {
        async fn persist(
            &self,
            session_id: &str,
            agent_session_id: Option<&str>,
            _source_node_id: &str,
            hash_key: &str,
            real_value: &str,
            _field_name: &str,
        ) -> Result<(), DagError> {
            self.rows.lock().unwrap().push(MockEntry {
                session: session_id.to_string(),
                agent: agent_session_id.map(|s| s.to_string()),
                hash: hash_key.to_string(),
                value: real_value.to_string(),
            });
            Ok(())
        }

        async fn decrypt(
            &self,
            session_id: &str,
            agent_session_id: Option<&str>,
            hash_key: &str,
        ) -> Result<Option<String>, DagError> {
            let rows = self.rows.lock().unwrap();
            // Agent-first: if an agent is provided, look up by agent + hash.
            if let Some(agent) = agent_session_id {
                if let Some(row) = rows
                    .iter()
                    .find(|r| r.agent.as_deref() == Some(agent) && r.hash == hash_key)
                {
                    return Ok(Some(row.value.clone()));
                }
            }
            // Fallback: session-scoped lookup.
            Ok(rows
                .iter()
                .find(|r| r.session == session_id && r.hash == hash_key)
                .map(|r| r.value.clone()))
        }

        async fn exists(
            &self,
            session_id: &str,
            agent_session_id: Option<&str>,
            hash_key: &str,
        ) -> Result<bool, DagError> {
            let rows = self.rows.lock().unwrap();
            if let Some(agent) = agent_session_id {
                if rows
                    .iter()
                    .any(|r| r.agent.as_deref() == Some(agent) && r.hash == hash_key)
                {
                    return Ok(true);
                }
            }
            Ok(rows
                .iter()
                .any(|r| r.session == session_id && r.hash == hash_key))
        }

        async fn cleanup(&self, session_id: &str) -> Result<(), DagError> {
            self.rows
                .lock()
                .unwrap()
                .retain(|r| r.session != session_id);
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

    fn build_service() -> (Arc<MockSecureValueRepository>, SecureValueService) {
        let repo = Arc::new(MockSecureValueRepository::new());
        let svc = SecureValueService::new(repo.clone());
        (repo, svc)
    }

    #[tokio::test]
    async fn test_hash_output_with_secure_flag() {
        let (_repo, service) = build_service();

        let output = json!({
            "status": 200,
            "body": {
                "token": "sk_live_abc123",
                "user_id": "456"
            }
        });

        let config = json!({ "secure": true });

        let hashed = service
            .hash_output(&output, &config, "session_1", None, "http_node_1")
            .await
            .unwrap();

        // Status should be unchanged
        assert_eq!(hashed["status"], 200);

        // Body values should be hashed
        let token_val = hashed["body"]["token"].as_str().unwrap();
        assert!(token_val.starts_with('<') && token_val.ends_with('>'));

        let user_id_val = hashed["body"]["user_id"].as_str().unwrap();
        assert!(user_id_val.starts_with('<') && user_id_val.ends_with('>'));
    }

    #[tokio::test]
    async fn test_hash_output_without_secure_flag() {
        let (_repo, service) = build_service();

        let output = json!({
            "status": 200,
            "body": {
                "token": "sk_live_abc123"
            }
        });

        let config = json!({ "secure": false });

        let result = service
            .hash_output(&output, &config, "session_1", None, "http_node_1")
            .await
            .unwrap();

        // Should be unchanged
        assert_eq!(result["body"]["token"].as_str(), Some("sk_live_abc123"));
    }

    #[tokio::test]
    async fn test_repo_exists_true_after_persist() {
        let repo = Arc::new(MockSecureValueRepository::new());
        repo.persist("s1", None, "n1", "<sv_token>", "real", "secret")
            .await
            .unwrap();
        assert!(repo.exists("s1", None, "<sv_token>").await.unwrap());
        assert!(!repo.exists("s1", None, "<sv_other>").await.unwrap());
    }

    #[tokio::test]
    async fn test_handle_exists_after_persist_secret() {
        let (_repo, service) = build_service();
        let handle = service
            .persist_secret(
                "session_a",
                None,
                "node1",
                "amadeus_client_id",
                "real_id_value",
            )
            .await
            .unwrap();
        assert!(
            handle.starts_with("<sv_amadeus_client_id_"),
            "got: {handle}"
        );
        assert!(service
            .handle_exists("session_a", None, &handle)
            .await
            .unwrap());
        assert!(!service
            .handle_exists("session_a", None, "<sv_unknown>")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_persist_secret_can_be_decrypted_via_inject() {
        let (_repo, service) = build_service();
        let handle = service
            .persist_secret("s2", None, "node1", "tok", "real_token_xyz")
            .await
            .unwrap();
        assert!(handle.starts_with("<sv_tok_"), "got: {handle}");
        let mut inputs = json!({ "bearer": handle.clone() });
        service
            .inject_secrets(&mut inputs, "s2", None)
            .await
            .unwrap();
        assert_eq!(inputs["bearer"].as_str(), Some("real_token_xyz"));
    }

    #[tokio::test]
    async fn persist_secret_handle_includes_random_suffix() {
        let (_repo, svc) = build_service();
        let h1 = svc
            .persist_secret("s1", None, "n", "user", "alice123")
            .await
            .unwrap();
        let h2 = svc
            .persist_secret("s2", None, "n", "user", "alice123")
            .await
            .unwrap();

        assert!(h1.starts_with("<sv_user_"), "got: {h1}");
        assert!(h2.starts_with("<sv_user_"), "got: {h2}");
        assert!(h1.ends_with('>'));
        assert!(h2.ends_with('>'));
        assert_ne!(h1, h2, "two persists must yield distinct handles");

        let prefix = "<sv_user_";
        let suffix1 = &h1[prefix.len()..h1.len() - 1];
        assert_eq!(suffix1.len(), 8);
        assert!(suffix1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn test_inject_secrets_restores_values() {
        let (_repo, service) = build_service();

        // First: hash
        let output = json!({
            "status": 200,
            "body": { "token": "sk_live_abc123" }
        });

        let config = json!({ "secure": true });
        let hashed = service
            .hash_output(&output, &config, "session_1", None, "http_node_1")
            .await
            .unwrap();

        // Then: inject
        let mut inputs = json!({
            "bearer_token": hashed["body"]["token"].clone()
        });

        service
            .inject_secrets(&mut inputs, "session_1", None)
            .await
            .unwrap();

        // Should be restored
        assert_eq!(inputs["bearer_token"].as_str(), Some("sk_live_abc123"));
    }

    /// Persist under (session_A, agent_X), then resolve from a *different*
    /// ephemeral session (session_B) but the same agent_X. Agent-first lookup
    /// must succeed.
    #[tokio::test]
    async fn test_inject_secrets_agent_first_cross_session() {
        let (_repo, service) = build_service();

        let handle = service
            .persist_secret(
                "ephemeral_A",
                Some("agent_demo_001"),
                "ask",
                "tok",
                "real_token_xyz",
            )
            .await
            .unwrap();

        let mut inputs = json!({ "bearer": handle });
        service
            .inject_secrets(&mut inputs, "ephemeral_B", Some("agent_demo_001"))
            .await
            .unwrap();
        assert_eq!(
            inputs["bearer"].as_str(),
            Some("real_token_xyz"),
            "agent_first lookup must find the secret under a different ephemeral session"
        );
    }

    /// Without an agent, lookup is session-scoped: a different session_id
    /// must NOT see another session's secret.
    #[tokio::test]
    async fn test_inject_secrets_no_agent_session_isolated() {
        let (_repo, service) = build_service();

        let handle = service
            .persist_secret("ephemeral_A", None, "ask", "tok", "real_token_xyz")
            .await
            .unwrap();

        let mut inputs = json!({ "bearer": handle.clone() });
        service
            .inject_secrets(&mut inputs, "ephemeral_B", None)
            .await
            .unwrap();
        // Placeholder must remain — different session, no agent → isolated.
        assert_eq!(inputs["bearer"].as_str(), Some(handle.as_str()));
    }

    #[test]
    fn mask_outbound_replaces_string_literal() {
        let svc = SecureValueService::new(Arc::new(MockSecureValueRepository::new()));
        let mut value = json!("token=alice123");
        let map: HashMap<String, String> = [("alice123".to_string(), "<sv_user_X>".to_string())]
            .into_iter()
            .collect();
        svc.mask_outbound(&mut value, &map);
        assert_eq!(value, json!("token=<sv_user_X>"));
    }

    #[test]
    fn mask_outbound_walks_nested_objects() {
        let svc = SecureValueService::new(Arc::new(MockSecureValueRepository::new()));
        let mut value = json!({"user": {"name": "alice123"}});
        let map: HashMap<String, String> = [("alice123".to_string(), "<sv_user_X>".to_string())]
            .into_iter()
            .collect();
        svc.mask_outbound(&mut value, &map);
        assert_eq!(value, json!({"user": {"name": "<sv_user_X>"}}));
    }

    #[test]
    fn mask_outbound_walks_arrays() {
        let svc = SecureValueService::new(Arc::new(MockSecureValueRepository::new()));
        let mut value = json!(["alice123", "bob"]);
        let map: HashMap<String, String> = [("alice123".to_string(), "<sv_user_X>".to_string())]
            .into_iter()
            .collect();
        svc.mask_outbound(&mut value, &map);
        assert_eq!(value, json!(["<sv_user_X>", "bob"]));
    }

    #[test]
    fn mask_outbound_orders_longest_first() {
        let svc = SecureValueService::new(Arc::new(MockSecureValueRepository::new()));
        let mut value = json!("alicezhang123");
        let map: HashMap<String, String> = [
            ("alice123".to_string(), "<sv_short_X>".to_string()),
            ("alicezhang123".to_string(), "<sv_long_X>".to_string()),
        ]
        .into_iter()
        .collect();
        svc.mask_outbound(&mut value, &map);
        assert_eq!(value, json!("<sv_long_X>"));
    }

    #[test]
    fn mask_outbound_skips_short_values() {
        let svc = SecureValueService::new(Arc::new(MockSecureValueRepository::new()));
        let mut value = json!("okok");
        let map: HashMap<String, String> = [("ok".to_string(), "<sv_flag_X>".to_string())]
            .into_iter()
            .collect();
        svc.mask_outbound(&mut value, &map);
        assert_eq!(value, json!("okok"), "should not replace keys < 4 chars");
    }

    #[test]
    fn mask_outbound_is_noop_on_empty_map() {
        let svc = SecureValueService::new(Arc::new(MockSecureValueRepository::new()));
        let mut value = json!({"a": "alice123"});
        svc.mask_outbound(&mut value, &HashMap::new());
        assert_eq!(value, json!({"a": "alice123"}));
    }

    #[test]
    fn mask_outbound_does_not_modify_numbers_or_booleans() {
        let svc = SecureValueService::new(Arc::new(MockSecureValueRepository::new()));
        let mut value = json!({"count": 42, "active": true, "name": "alice123"});
        let map: HashMap<String, String> = [("alice123".to_string(), "<sv_user_X>".to_string())]
            .into_iter()
            .collect();
        svc.mask_outbound(&mut value, &map);
        assert_eq!(
            value,
            json!({"count": 42, "active": true, "name": "<sv_user_X>"})
        );
    }

    /// `handle_exists` must follow the same agent-first rule.
    #[tokio::test]
    async fn test_handle_exists_agent_first_cross_session() {
        let (_repo, service) = build_service();

        let handle = service
            .persist_secret(
                "ephemeral_A",
                Some("agent_demo_001"),
                "ask",
                "amadeus_client_id",
                "real",
            )
            .await
            .unwrap();

        // Same agent, different ephemeral session — must report exists=true.
        assert!(service
            .handle_exists("ephemeral_B", Some("agent_demo_001"), &handle)
            .await
            .unwrap());
    }
}
