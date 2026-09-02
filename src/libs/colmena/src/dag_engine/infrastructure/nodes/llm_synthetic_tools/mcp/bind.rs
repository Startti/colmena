//! Turning a configured MCP server into something connectable.
//!
//! The bridge the pool has been missing: a [`McpServerSpec`] names a server and
//! references its credentials; a [`McpBinding`] has resolved those references
//! and knows the pool identity they imply.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::dag_engine::application::secure_value_service::{
    is_secure_value_placeholder, SecureValueService,
};
use crate::dag_engine::domain::tool_configuration::McpServerSpec;
use crate::dag_engine::infrastructure::mcp_registry::{CredentialFingerprint, McpServerKey};
use crate::llm::domain::mcp::{McpClientPort, McpError, McpServerConfig};
use crate::llm::infrastructure::mcp_client::RmcpHttpClient;

/// One configured server, with its credentials resolved.
///
/// `Debug` is written by hand and shows header NAMES only. The whole point of
/// this type is that it holds resolved secrets, so a derived `Debug` would put
/// them in the first log line anyone adds while debugging a connection.
pub struct McpBinding {
    pub alias: String,
    pub key: McpServerKey,
    config: McpServerConfig,
    resolved_headers: BTreeMap<String, String>,
}

impl std::fmt::Debug for McpBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpBinding")
            .field("alias", &self.alias)
            .field("key", &self.key)
            .field("url", &self.config.url)
            .field(
                "header_names",
                &self.resolved_headers.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Resolve a spec's credential references and derive its pool identity.
pub async fn bind(
    alias: &str,
    spec: &McpServerSpec,
    secure_values: Option<&SecureValueService>,
    session_id: &str,
    agent_session_id: Option<&str>,
) -> Result<McpBinding, McpError> {
    let resolved_headers =
        resolve_headers(alias, spec, secure_values, session_id, agent_session_id).await?;

    let config = McpServerConfig {
        url: spec.url.clone(),
        transport: spec.transport,
        header_refs: spec.headers.clone(),
        timeout: Duration::from_secs(spec.timeout_seconds),
        cache_ttl: Duration::from_secs(spec.cache_ttl_seconds),
    };

    // Resolve FIRST, then key. That order is the whole point: keying on the
    // references would leave the identity unchanged across a rotation, and the
    // pool would go on handing back a connection built with the retired
    // secret. See `CredentialFingerprint`.
    let key = McpServerKey::from_resolved(&config, &CredentialFingerprint::of(&resolved_headers));

    Ok(McpBinding {
        alias: alias.to_string(),
        key,
        config,
        resolved_headers,
    })
}

/// Replace every secure-value reference in the spec's headers with its secret.
///
/// A header whose value is a literal is left alone: it means the same thing for
/// every caller, and that is what lets a public server stay globally pooled.
///
/// Errors deliberately name the HEADER, never the value. An injection failure
/// is reported to an operator, and the thing that failed to resolve is the
/// thing we must not print.
async fn resolve_headers(
    alias: &str,
    spec: &McpServerSpec,
    secure_values: Option<&SecureValueService>,
    session_id: &str,
    agent_session_id: Option<&str>,
) -> Result<BTreeMap<String, String>, McpError> {
    if spec.headers.is_empty() {
        return Ok(BTreeMap::new());
    }

    let Some(service) = secure_values else {
        // No service wired: only literals can be honoured. A reference would
        // otherwise reach the server AS the placeholder text, which reads to
        // the operator as a wrong credential rather than a missing service.
        if let Some(name) = spec
            .headers
            .iter()
            .find_map(|(name, value)| is_secure_value_placeholder(value).then_some(name))
        {
            return Err(McpError::InvalidConfig {
                detail: format!(
                    "MCP server '{alias}' resolves header '{name}' through secure \
                     values, but no secure-value service is available in this run"
                ),
            });
        }
        return Ok(spec.headers.clone());
    };

    let mut carrier = serde_json::to_value(&spec.headers).map_err(|e| McpError::InvalidConfig {
        detail: format!("MCP server '{alias}' has headers that are not a string map: {e}"),
    })?;

    service
        .inject_secrets(&mut carrier, session_id, agent_session_id)
        .await
        .map_err(|e| McpError::InvalidConfig {
            detail: format!("MCP server '{alias}' could not resolve its headers: {e}"),
        })?;

    let resolved: BTreeMap<String, String> =
        serde_json::from_value(carrier).map_err(|e| McpError::InvalidConfig {
            detail: format!("MCP server '{alias}' resolved to headers that are not strings: {e}"),
        })?;

    // `inject_secrets` leaves a placeholder untouched and returns `Ok` when the
    // vault has no row for it, so a successful call is NOT proof of resolution.
    // Unchecked, the literal `<sv_...>` text would travel to a third-party
    // server as an authorization header: not the secret, but a credential that
    // is silently wrong, pooled under a fingerprint of the placeholder itself.
    // The sibling no-service branch above refuses for the same reason.
    if let Some(name) = resolved
        .iter()
        .find_map(|(name, value)| is_secure_value_placeholder(value).then_some(name))
    {
        return Err(McpError::InvalidConfig {
            detail: format!(
                "MCP server '{alias}' header '{name}' references a secure value that \
                 did not resolve for this session"
            ),
        });
    }

    Ok(resolved)
}

impl McpBinding {
    /// Open a live connection using the resolved credentials.
    pub async fn connect(&self) -> Result<Arc<dyn McpClientPort>, McpError> {
        let client =
            RmcpHttpClient::connect(&self.alias, &self.config, &self.resolved_headers).await?;
        Ok(Arc::new(client))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::domain::error::DagError;
    use crate::dag_engine::domain::secure_value_repository::SecureValueRepository;
    use async_trait::async_trait;
    use serde_json::json;

    /// Keys are the FULL placeholder text, brackets included: that is what
    /// `inject_secrets` hands `decrypt`.
    ///
    /// Mirrors PRODUCTION `decrypt`: two mutually exclusive branches, an
    /// agent-only lookup when an agent id is present and a session-only lookup
    /// otherwise, with NO fallback between them. The mock inside
    /// `secure_value_service`'s own tests DOES fall back, and copying it here
    /// would let a binding pass a test that production would fail.
    struct Vault {
        rows: Vec<(Option<String>, String, String, String)>,
    }

    #[async_trait]
    impl SecureValueRepository for Vault {
        async fn persist(
            &self,
            _session_id: &str,
            _agent_session_id: Option<&str>,
            _source_node_id: &str,
            _hash_key: &str,
            _real_value: &str,
            _field_name: &str,
        ) -> Result<(), DagError> {
            Ok(())
        }

        async fn decrypt(
            &self,
            session_id: &str,
            agent_session_id: Option<&str>,
            hash_key: &str,
        ) -> Result<Option<String>, DagError> {
            Ok(self
                .rows
                .iter()
                .find(|(agent, session, key, _)| {
                    key == hash_key
                        && match agent_session_id {
                            Some(a) => agent.as_deref() == Some(a),
                            None => agent.is_none() && session == session_id,
                        }
                })
                .map(|(_, _, _, value)| value.clone()))
        }

        async fn cleanup(&self, _session_id: &str) -> Result<(), DagError> {
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

    fn spec(headers: serde_json::Value) -> McpServerSpec {
        serde_json::from_value(json!({
            "url": "https://mcp.example.com/mcp",
            "headers": headers
        }))
        .expect("spec parses")
    }

    /// A reference must become the real secret before it reaches the pool key.
    #[tokio::test]
    async fn a_reference_is_resolved_before_the_key_is_derived() {
        let vault = Vault {
            rows: vec![(
                Some("agent-a".to_string()),
                String::new(),
                "<sv_token>".to_string(),
                "Bearer real-alice".to_string(),
            )],
        };
        let svc = SecureValueService::new(Arc::new(vault));

        let b = bind(
            "srv",
            &spec(json!({ "Authorization": "<sv_token>" })),
            Some(&svc),
            "session-1",
            Some("agent-a"),
        )
        .await
        .expect("binds");

        assert_eq!(b.alias, "srv");
        assert_eq!(
            b.resolved_headers.get("Authorization").map(String::as_str),
            Some("Bearer real-alice"),
            "the reference must be resolved, not passed through"
        );
    }

    /// Two agents holding DIFFERENT secrets behind the SAME reference must not
    /// share a pool identity. This is the property the whole credential
    /// fingerprint exists for, checked end to end from the config down.
    #[tokio::test]
    async fn two_agents_behind_one_reference_get_different_keys() {
        let vault = Vault {
            rows: vec![
                (
                    Some("agent-a".to_string()),
                    String::new(),
                    "<sv_token>".to_string(),
                    "Bearer alice".to_string(),
                ),
                (
                    Some("agent-b".to_string()),
                    String::new(),
                    "<sv_token>".to_string(),
                    "Bearer bob".to_string(),
                ),
            ],
        };
        let svc = SecureValueService::new(Arc::new(vault));
        let s = spec(json!({ "Authorization": "<sv_token>" }));

        let a = bind("srv", &s, Some(&svc), "session-1", Some("agent-a"))
            .await
            .expect("binds");
        let b = bind("srv", &s, Some(&svc), "session-1", Some("agent-b"))
            .await
            .expect("binds");

        assert_ne!(
            a.key, b.key,
            "same reference, different resolved secret: different connections"
        );
    }

    /// A literal header is the same for everyone, so it must pool globally.
    #[tokio::test]
    async fn a_literal_header_pools_globally() {
        let s = spec(json!({ "X-Api-Version": "2" }));
        let a = bind("srv", &s, None, "session-1", Some("agent-a"))
            .await
            .expect("binds");
        let b = bind("srv", &s, None, "session-2", Some("agent-b"))
            .await
            .expect("binds");
        assert_eq!(a.key, b.key, "a literal is not a per-caller credential");
    }

    /// The resolved secret must never be printable.
    #[tokio::test]
    async fn a_binding_never_prints_its_resolved_secret() {
        let vault = Vault {
            rows: vec![(
                Some("agent-a".to_string()),
                String::new(),
                "<sv_token>".to_string(),
                "Bearer super-secret-value".to_string(),
            )],
        };
        let svc = SecureValueService::new(Arc::new(vault));
        let b = bind(
            "srv",
            &spec(json!({ "Authorization": "<sv_token>" })),
            Some(&svc),
            "session-1",
            Some("agent-a"),
        )
        .await
        .expect("binds");

        let printed = format!("{b:?}");
        assert!(
            !printed.contains("super-secret-value"),
            "the resolved secret reached Debug output: {printed}"
        );
    }

    /// A vault that holds no row for the reference is the dangerous case:
    /// `inject_secrets` returns `Ok` and leaves the placeholder text in place,
    /// so a bind that trusted that `Ok` would send `<sv_token>` itself to a
    /// third-party server as an authorization header.
    #[tokio::test]
    async fn a_reference_the_vault_cannot_resolve_fails_instead_of_being_sent() {
        let svc = SecureValueService::new(Arc::new(Vault { rows: vec![] }));

        let err = bind(
            "srv",
            &spec(json!({ "Authorization": "<sv_token>" })),
            Some(&svc),
            "session-1",
            Some("agent-a"),
        )
        .await
        .expect_err("an unresolved reference must not produce a binding");

        let msg = format!("{err:?}");
        assert!(
            msg.contains("Authorization"),
            "the error must name the header so an operator can act: {msg}"
        );
        assert!(
            !msg.contains("<sv_token>"),
            "the error must not echo the reference value: {msg}"
        );
    }

    /// The same refusal, one branch earlier: no service wired at all. Documented
    /// behaviour that previously had no test, so nothing stopped the branch from
    /// being deleted.
    #[tokio::test]
    async fn a_reference_without_a_secure_value_service_fails_loudly() {
        let err = bind(
            "srv",
            &spec(json!({ "Authorization": "<sv_token>" })),
            None,
            "session-1",
            Some("agent-a"),
        )
        .await
        .expect_err("a reference with no service must not produce a binding");

        assert!(
            format!("{err:?}").contains("Authorization"),
            "the error must name the header"
        );
    }

    /// `timeout` and `cache_ttl` feed neither the key nor any other assertion,
    /// so a swap between them was invisible to the whole suite. Their defaults
    /// differ (30s vs 300s), which is exactly what makes the swap detectable.
    #[tokio::test]
    async fn the_timeout_and_the_cache_ttl_are_not_swapped() {
        let s: McpServerSpec = serde_json::from_value(json!({
            "url": "https://mcp.example.com/mcp",
            "timeout_seconds": 7,
            "cache_ttl_seconds": 900
        }))
        .expect("spec parses");

        let b = bind("srv", &s, None, "session-1", None)
            .await
            .expect("binds");

        assert_eq!(b.config.timeout, Duration::from_secs(7), "timeout");
        assert_eq!(b.config.cache_ttl, Duration::from_secs(900), "cache_ttl");
    }
}
