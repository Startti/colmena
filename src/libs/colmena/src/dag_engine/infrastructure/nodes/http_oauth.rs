//! Native OAuth2 (refresh_token grant) for the http_request node:
//! parsing + validation of the `auth` config block, and the 401-retry
//! send helper. The `auth` block is read ONLY from `config`, never from
//! the LLM's `inputs`.

use crate::dag_engine::domain::node::NodeInputs;
use crate::google_oauth::domain::{AuthTokenProvider, OAuthError};
use crate::google_oauth::infrastructure::parse_oauth_refresh_block;
use serde_json::Value;
use std::error::Error as StdError;

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

    // Field extraction + validation is shared with
    // `llm_call.google_workspace_auth` (same messages as before).
    let block = parse_oauth_refresh_block(auth, true)?;

    Ok(Some(OAuthAuthSpec {
        token_url: block
            .token_url
            .expect("token_url validated by parse_oauth_refresh_block(require_token_url = true)"),
        client_id: block.client_id,
        client_secret: block.client_secret,
        refresh_token: block.refresh_token,
    }))
}

/// `bearer_refresh` (CX7): `{handle, expires_at}` lets the host mint a fresh
/// access token for the author's `bearer_token` through `HostTokenPort`. Read
/// from `config` or a tool's `fixed` value, never from what the model wrote.
/// `Ok(None)` when absent. The errors never repeat a value.
pub fn parse_bearer_refresh(
    config: &Value,
    inputs: &NodeInputs,
) -> Result<Option<(String, i64)>, String> {
    let authored = |k| super::http::HttpNode::author_value(inputs, config, k);
    let Some(block) = authored("bearer_refresh") else {
        return Ok(None);
    };
    if config.get("auth").is_some() {
        return Err("bearer_refresh cannot be combined with auth".into());
    }
    if authored("bearer_token").is_none() {
        return Err("bearer_refresh needs bearer_token".into());
    }
    let handle = block.get("handle").and_then(Value::as_str);
    let handle = handle
        .filter(|s| !s.trim().is_empty())
        .ok_or("bearer_refresh.handle must be a non-empty string")?;
    let expires_at = block.get("expires_at").and_then(Value::as_i64);
    let expires_at = expires_at.ok_or("bearer_refresh.expires_at must be an integer")?;
    Ok(Some((handle.to_string(), expires_at)))
}

/// Send `builder` (which must NOT already carry an Authorization header)
/// with a Bearer from `provider`. On HTTP 401, invalidate the token, get a new
/// one, and retry exactly once. 403/429/etc. pass through.
///
/// With `fallback` (the seed of a host-refreshed `bearer_token`), a provider
/// that cannot give a token leaves the request as it would be without one: the
/// seed goes out, and a 401 comes back as the response.
pub async fn send_with_oauth_retry(
    builder: reqwest::RequestBuilder,
    provider: &dyn AuthTokenProvider,
    fallback: Option<&str>,
) -> Result<reqwest::Response, Box<dyn StdError + Send + Sync>> {
    let oauth_err = |what: &str, e: OAuthError| -> Box<dyn StdError + Send + Sync> {
        Box::new(std::io::Error::other(format!("{what}: {e}")))
    };
    // The host's texts carry neither the handle nor a token.
    let fell_back = |e: &OAuthError| {
        tracing::warn!(target: "colmena::http_request", error = %e,
        "host token refresh failed; the request goes on as without bearer_refresh")
    };
    let (token, seeded) = match (provider.get_bearer_token().await, fallback) {
        (Ok(t), _) => (t.0, false),
        (Err(e), Some(seed)) => {
            fell_back(&e);
            (seed.to_string(), true)
        }
        (Err(e), None) => return Err(oauth_err("OAuth", e)),
    };

    let first = builder
        .try_clone()
        .ok_or_else(|| {
            Box::new(std::io::Error::other(
                "http_request: OAuth requires a cloneable request (no streaming body)",
            )) as Box<dyn StdError + Send + Sync>
        })?
        .header("Authorization", format!("Bearer {token}"));
    let resp = first.send().await?;

    if resp.status().as_u16() == 401 && !seeded {
        provider.invalidate().await;
        let token2 = match provider.get_bearer_token().await {
            Ok(t) => t,
            Err(e) if fallback.is_some() => {
                fell_back(&e);
                return Ok(resp);
            }
            Err(e) => return Err(oauth_err("OAuth (retry)", e)),
        };
        let second = builder.header("Authorization", format!("Bearer {}", token2.as_str()));
        return Ok(second.send().await?);
    }
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::infrastructure::env_provenance::AUTHORED_INPUTS_KEY;
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
    fn rejects_empty_refresh_token_with_http_request_wording() {
        let c = json!({ "auth": { "type": "oauth2_refresh_token", "token_url": "https://t/token",
            "client_id": "cid", "client_secret": "cs", "refresh_token": "" } });
        let err = parse_oauth_auth(&c, &Default::default()).expect_err("empty refresh_token");
        assert_eq!(err, "auth block missing required fields: refresh_token");
    }

    #[test]
    fn unknown_type_keeps_http_request_wording() {
        let c = json!({ "auth": { "type": "client_credentials" } });
        let err = parse_oauth_auth(&c, &Default::default()).expect_err("unknown type v1");
        assert!(
            err.starts_with("unsupported auth.type 'client_credentials'"),
            "{err}"
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

    fn refresh(c: Value, inputs: &NodeInputs) -> Result<Option<(String, i64)>, String> {
        parse_bearer_refresh(&c, inputs)
    }

    #[test]
    fn bearer_refresh_absent_is_none_and_well_formed_is_parsed() {
        let none = Default::default();
        assert_eq!(refresh(json!({ "bearer_token": "t" }), &none), Ok(None));
        let c = json!({ "bearer_token": "t",
            "bearer_refresh": { "handle": "cth1-cx7-h", "expires_at": 1790000000 } });
        let want = Some(("cth1-cx7-h".to_string(), 1790000000));
        assert_eq!(refresh(c, &none), Ok(want.clone()));
        // A tool's `fixed` value counts; one the model wrote does not.
        let block = json!({ "handle": "cth1-cx7-h", "expires_at": 1790000000 });
        let mut inputs: NodeInputs = [
            ("bearer_token".to_string(), json!("t")),
            ("bearer_refresh".to_string(), block),
        ]
        .into();
        assert_eq!(refresh(json!({}), &inputs), Ok(None));
        let authored = json!(["bearer_token", "bearer_refresh"]);
        inputs.insert(AUTHORED_INPUTS_KEY.into(), authored);
        assert_eq!(refresh(json!({}), &inputs), Ok(want));
    }

    #[test]
    fn bearer_refresh_errors_never_repeat_the_value() {
        let none = Default::default();
        let (h, int) = ("cth1-cx7-h", "must be an integer");
        let bad = [
            (
                json!({ "handle": " ", "expires_at": 1 }),
                "must be a non-empty",
            ),
            (json!({ "handle": h, "expires_at": "1" }), int),
            (json!({ "handle": h }), int),
        ];
        for (block, want) in bad {
            let c = json!({ "bearer_token": "tok-cx7-seed", "bearer_refresh": block });
            let err = refresh(c, &none).expect_err("malformed");
            assert!(err.contains(want) && !err.contains("-cx7-"), "{err}");
        }
        let block = json!({ "handle": "cth1-cx7-h", "expires_at": 1 });
        let err = refresh(json!({ "bearer_refresh": block }), &none).expect_err("no token");
        assert_eq!(err, "bearer_refresh needs bearer_token");
        let c = json!({ "bearer_refresh": block, "auth": { "type": "oauth2_refresh_token" } });
        let err = refresh(c, &none).expect_err("with auth");
        assert_eq!(err, "bearer_refresh cannot be combined with auth");
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
        let resp = send_with_oauth_retry(builder, provider.as_ref(), None)
            .await
            .expect("ok");
        assert_eq!(resp.status().as_u16(), 200);
    }
}
