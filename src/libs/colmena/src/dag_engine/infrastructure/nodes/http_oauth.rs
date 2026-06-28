//! Native OAuth2 (refresh_token grant) for the http_request node:
//! parsing + validation of the `auth` config block, and the 401-retry
//! send helper. The `auth` block is read ONLY from `config`, never from
//! the LLM's `inputs`.

use crate::dag_engine::domain::node::NodeInputs;
use crate::google_oauth::domain::AuthTokenProvider;
use crate::google_oauth::infrastructure::OAuthRefreshTokenProvider;
use serde_json::Value;
use std::error::Error as StdError;
use std::sync::Arc;

/// Resolved OAuth2 refresh-token auth, all `${ENV}` still unexpanded
/// (the caller resolves env vars before use).
///
/// `Debug` is hand-written to redact `client_secret` and `refresh_token`
/// so a stray `{:?}` / `tracing::debug!` can never leak them (mirrors the
/// redacted Debug convention on `RefreshTokenSecret`).
#[derive(Clone, PartialEq, Eq)]
pub struct OAuthAuthSpec {
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
}

impl std::fmt::Debug for OAuthAuthSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthAuthSpec")
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .finish()
    }
}

/// Parse and validate the `auth` block from `config`.
/// - `Ok(None)` when no `auth` block is present.
/// - `Ok(Some(spec))` when valid (raw `${ENV}`-unresolved values).
/// - `Err(msg)` on validation failure (missing fields, wrong type,
///   mutual exclusion with static auth, or LLM-supplied base_url).
pub fn parse_oauth_auth(
    config: &Value,
    inputs: &NodeInputs,
) -> Result<Option<OAuthAuthSpec>, String> {
    let auth = match config.get("auth") {
        Some(a) => a,
        None => return Ok(None),
    };

    let has_static = config.get("bearer_token").is_some()
        || config.get("authorization").is_some()
        || inputs.contains_key("bearer_token")
        || inputs.contains_key("authorization");
    if has_static {
        return Err("`auth` is mutually exclusive with `bearer_token`/`authorization`".to_string());
    }

    // Anti-exfiltration guard: with `auth` present, the destination host
    // must be operator-fixed — the LLM must not supply `base_url` via inputs.
    if inputs.contains_key("base_url") {
        return Err("`base_url` must be fixed in config when `auth` is set; \
             it cannot come from inputs (anti-exfiltration guard)"
            .to_string());
    }

    let ty = auth.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if ty != "oauth2_refresh_token" {
        return Err(format!(
            "unsupported auth.type '{ty}'; v1 supports only 'oauth2_refresh_token'"
        ));
    }

    let mut missing = Vec::new();
    let get = |k: &str| auth.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
    let token_url = get("token_url");
    let client_id = get("client_id");
    let client_secret = get("client_secret");
    let refresh_token = get("refresh_token");
    if token_url.is_none() {
        missing.push("token_url");
    }
    if client_id.is_none() {
        missing.push("client_id");
    }
    if client_secret.is_none() {
        missing.push("client_secret");
    }
    if refresh_token.is_none() {
        missing.push("refresh_token");
    }
    if !missing.is_empty() {
        return Err(format!(
            "auth block missing required fields: {}",
            missing.join(", ")
        ));
    }

    Ok(Some(OAuthAuthSpec {
        token_url: token_url.unwrap(),
        client_id: client_id.unwrap(),
        client_secret: client_secret.unwrap(),
        refresh_token: refresh_token.unwrap(),
    }))
}

/// Send `builder` (which must NOT already carry an Authorization header)
/// with a fresh Bearer from `provider`. On HTTP 401, invalidate the cached
/// token, mint a new one, and retry exactly once. 403/429/etc. pass through.
pub async fn send_with_oauth_retry(
    builder: reqwest::RequestBuilder,
    provider: Arc<OAuthRefreshTokenProvider>,
) -> Result<reqwest::Response, Box<dyn StdError + Send + Sync>> {
    let token = provider.get_bearer_token().await.map_err(|e| {
        Box::new(std::io::Error::other(format!("OAuth: {e}"))) as Box<dyn StdError + Send + Sync>
    })?;

    let first = builder
        .try_clone()
        .ok_or_else(|| {
            Box::new(std::io::Error::other(
                "http_request: OAuth requires a cloneable request (no streaming body)",
            )) as Box<dyn StdError + Send + Sync>
        })?
        .header("Authorization", format!("Bearer {}", token.as_str()));
    let resp = first.send().await?;

    if resp.status().as_u16() == 401 {
        provider.invalidate_cache().await;
        let token2 = provider.get_bearer_token().await.map_err(|e| {
            Box::new(std::io::Error::other(format!("OAuth (retry): {e}")))
                as Box<dyn StdError + Send + Sync>
        })?;
        let second = builder.header("Authorization", format!("Bearer {}", token2.as_str()));
        return Ok(second.send().await?);
    }
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_valid_oauth_block() {
        let c = json!({
            "base_url": "https://api.example.com",
            "auth": { "type": "oauth2_refresh_token",
                "token_url": "https://oauth2.googleapis.com/token",
                "client_id": "cid", "client_secret": "csec", "refresh_token": "rt" }
        });
        let spec = parse_oauth_auth(&c, &Default::default())
            .expect("ok")
            .expect("some");
        assert_eq!(spec.token_url, "https://oauth2.googleapis.com/token");
        assert_eq!(spec.client_id, "cid");
    }

    #[test]
    fn none_when_no_auth_block() {
        let c = json!({ "base_url": "https://x" });
        assert!(parse_oauth_auth(&c, &Default::default())
            .expect("ok")
            .is_none());
    }

    #[test]
    fn rejects_missing_fields_listing_all() {
        let c = json!({ "auth": { "type": "oauth2_refresh_token" } });
        let err = parse_oauth_auth(&c, &Default::default()).expect_err("missing");
        assert!(
            err.contains("token_url")
                && err.contains("client_id")
                && err.contains("client_secret")
                && err.contains("refresh_token")
        );
    }

    #[test]
    fn rejects_auth_plus_bearer_token() {
        let c = json!({ "bearer_token": "abc",
            "auth": { "type": "oauth2_refresh_token", "token_url": "u",
                "client_id": "c", "client_secret": "s", "refresh_token": "r" } });
        let err = parse_oauth_auth(&c, &Default::default()).expect_err("mutually exclusive");
        assert!(err.to_lowercase().contains("mutually exclusive") || err.contains("bearer_token"));
    }

    #[test]
    fn rejects_base_url_from_inputs_when_auth_present() {
        let c = json!({ "auth": { "type": "oauth2_refresh_token", "token_url": "u",
            "client_id": "c", "client_secret": "s", "refresh_token": "r" } });
        let mut inputs = std::collections::HashMap::new();
        inputs.insert("base_url".to_string(), json!("https://evil.com"));
        let err = parse_oauth_auth(&c, &inputs).expect_err("base_url from inputs blocked");
        assert!(err.contains("base_url"));
    }

    #[test]
    fn rejects_unknown_type() {
        let c = json!({ "auth": { "type": "client_credentials" } });
        let err = parse_oauth_auth(&c, &Default::default()).expect_err("unknown type v1");
        assert!(err.contains("oauth2_refresh_token"));
    }

    #[test]
    fn debug_redacts_secrets() {
        let spec = OAuthAuthSpec {
            token_url: "https://oauth2.googleapis.com/token".to_string(),
            client_id: "cid".to_string(),
            client_secret: "SUPERSECRET".to_string(),
            refresh_token: "1//RTSECRET".to_string(),
        };
        let dbg = format!("{spec:?}");
        assert!(!dbg.contains("SUPERSECRET"));
        assert!(!dbg.contains("1//RTSECRET"));
        assert!(dbg.contains("<redacted>"));
        // Non-secret fields stay visible for debuggability.
        assert!(dbg.contains("cid"));
        assert!(dbg.contains("oauth2.googleapis.com"));
    }

    #[tokio::test]
    async fn retries_once_on_401_with_fresh_token() {
        use crate::google_oauth::infrastructure::OAuthProviderCache;
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let token_srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"access_token":"TOKEN_A","expires_in":3600,"token_type":"Bearer"}"#,
            ))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&token_srv)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"access_token":"TOKEN_B","expires_in":3600,"token_type":"Bearer"}"#,
            ))
            .with_priority(2)
            .mount(&token_srv)
            .await;

        let api = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .and(header("Authorization", "Bearer TOKEN_A"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&api)
            .await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .and(header("Authorization", "Bearer TOKEN_B"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&api)
            .await;

        let cache = OAuthProviderCache::new();
        let provider = cache.get_or_create(&token_srv.uri(), "cid", "csec", "rt");
        let client = reqwest::Client::builder().http1_only().build().unwrap();
        let builder = client.get(format!("{}/x", api.uri()));
        let resp = send_with_oauth_retry(builder, provider).await.expect("ok");
        assert_eq!(resp.status().as_u16(), 200);
    }
}
