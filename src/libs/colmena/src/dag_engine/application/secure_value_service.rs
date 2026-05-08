use crate::dag_engine::domain::{error::DagError, secure_value_repository::SecureValueRepository};
use serde_json::Value;
use std::sync::Arc;

/// Business logic for hashing outputs and injecting secrets
pub struct SecureValueService {
    repo: Arc<dyn SecureValueRepository>,
}

impl SecureValueService {
    pub fn new(repo: Arc<dyn SecureValueRepository>) -> Self {
        Self { repo }
    }

    /// Process output from a secure HTTP node: hash all sensitive values
    ///
    /// Returns the output with placeholders instead of real values,
    /// and stores encrypted mappings in the database.
    pub async fn hash_output(
        &self,
        output: &Value,
        config: &Value,
        session_id: &str,
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
                .persist(session_id, None, source_node_id, &hash_key, &real_value, "value")
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

    /// Inject real values back into inputs before non-LLM node execution
    /// Automatically detects placeholders (<value_N>) and replaces them
    pub async fn inject_secrets(
        &self,
        inputs: &mut Value,
        session_id: &str,
    ) -> Result<(), DagError> {
        // First pass: collect all placeholders
        let mut placeholders = Vec::new();
        self.collect_placeholders(inputs, &mut placeholders);

        // Second pass: decrypt and replace
        for placeholder in placeholders {
            if let Some(real) = self.repo.decrypt(session_id, None, &placeholder).await? {
                self.replace_placeholder(inputs, &placeholder, real);
            }
        }

        Ok(())
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
            Value::String(s) if s.starts_with('<') && s.ends_with('>') && s.len() > 2 => {
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

    /// Store a single named secret as `<sv_<name>>` and persist its real value.
    /// The handle returned to callers is `format!("<sv_{}>", name)`.
    /// Used by the `secure_suspend` node to record user-supplied secrets.
    pub async fn persist_secret(
        &self,
        session_id: &str,
        source_node_id: &str,
        name: &str,
        real_value: &str,
    ) -> Result<String, DagError> {
        let handle = format!("<sv_{}>", name);
        self.repo
            .persist(session_id, None, source_node_id, &handle, real_value, "secret")
            .await?;
        Ok(handle)
    }

    /// Check whether a handle is already registered in this session.
    /// Used by `secure_suspend` to detect collisions before persisting.
    pub async fn handle_exists(
        &self,
        session_id: &str,
        handle: &str,
    ) -> Result<bool, DagError> {
        self.repo.exists(session_id, None, handle).await
    }

    /// Cleanup all secure values for a session
    pub async fn cleanup(&self, session_id: &str) -> Result<(), DagError> {
        self.repo.cleanup(session_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::HashMap;

    /// Mock repository for testing
    struct MockSecureValueRepository {
        storage: std::sync::Mutex<HashMap<String, String>>,
    }

    #[async_trait]
    impl SecureValueRepository for MockSecureValueRepository {
        async fn persist(
            &self,
            _session_id: &str,
            _agent_session_id: Option<&str>,
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
            _agent_session_id: Option<&str>,
            hash_key: &str,
        ) -> Result<Option<String>, DagError> {
            Ok(self.storage.lock().unwrap().get(hash_key).cloned())
        }

        async fn exists(
            &self,
            _session_id: &str,
            _agent_session_id: Option<&str>,
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

    #[tokio::test]
    async fn test_hash_output_with_secure_flag() {
        let repo = Arc::new(MockSecureValueRepository {
            storage: std::sync::Mutex::new(HashMap::new()),
        });
        let service = SecureValueService::new(repo);

        let output = json!({
            "status": 200,
            "body": {
                "token": "sk_live_abc123",
                "user_id": "456"
            }
        });

        let config = json!({ "secure": true });

        let hashed = service
            .hash_output(&output, &config, "session_1", "http_node_1")
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
        let repo = Arc::new(MockSecureValueRepository {
            storage: std::sync::Mutex::new(HashMap::new()),
        });
        let service = SecureValueService::new(repo);

        let output = json!({
            "status": 200,
            "body": {
                "token": "sk_live_abc123"
            }
        });

        let config = json!({ "secure": false });

        let result = service
            .hash_output(&output, &config, "session_1", "http_node_1")
            .await
            .unwrap();

        // Should be unchanged
        assert_eq!(result["body"]["token"].as_str(), Some("sk_live_abc123"));
    }

    #[tokio::test]
    async fn test_repo_exists_true_after_persist() {
        let repo = Arc::new(MockSecureValueRepository {
            storage: std::sync::Mutex::new(HashMap::new()),
        });
        repo.persist("s1", None, "n1", "<sv_token>", "real", "secret")
            .await
            .unwrap();
        assert!(repo.exists("s1", None, "<sv_token>").await.unwrap());
        assert!(!repo.exists("s1", None, "<sv_other>").await.unwrap());
    }

    #[tokio::test]
    async fn test_handle_exists_after_persist_secret() {
        let repo = Arc::new(MockSecureValueRepository {
            storage: std::sync::Mutex::new(HashMap::new()),
        });
        let service = SecureValueService::new(repo);
        service
            .persist_secret("session_a", "node1", "amadeus_client_id", "real_id_value")
            .await
            .unwrap();
        assert!(
            service
                .handle_exists("session_a", "<sv_amadeus_client_id>")
                .await
                .unwrap()
        );
        assert!(
            !service
                .handle_exists("session_a", "<sv_unknown>")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn test_persist_secret_can_be_decrypted_via_inject() {
        let repo = Arc::new(MockSecureValueRepository {
            storage: std::sync::Mutex::new(HashMap::new()),
        });
        let service = SecureValueService::new(repo);
        service
            .persist_secret("s2", "node1", "tok", "real_token_xyz")
            .await
            .unwrap();
        let mut inputs = json!({"bearer": "<sv_tok>"});
        service.inject_secrets(&mut inputs, "s2").await.unwrap();
        assert_eq!(inputs["bearer"].as_str(), Some("real_token_xyz"));
    }

    #[tokio::test]
    async fn test_inject_secrets_restores_values() {
        let repo = Arc::new(MockSecureValueRepository {
            storage: std::sync::Mutex::new(HashMap::new()),
        });
        let service = SecureValueService::new(repo);

        // First: hash
        let output = json!({
            "status": 200,
            "body": { "token": "sk_live_abc123" }
        });

        let config = json!({ "secure": true });
        let hashed = service
            .hash_output(&output, &config, "session_1", "http_node_1")
            .await
            .unwrap();

        // Then: inject
        let mut inputs = json!({
            "bearer_token": hashed["body"]["token"].clone()
        });

        service
            .inject_secrets(&mut inputs, "session_1")
            .await
            .unwrap();

        // Should be restored
        assert_eq!(inputs["bearer_token"].as_str(), Some("sk_live_abc123"));
    }
}
