//! HTTP refresh client — POSTs to Google's OAuth token endpoint to
//! exchange a refresh_token for a fresh access_token.
//!
//! Pure transport layer: no cache, no concurrency control. The
//! `OAuthRefreshTokenProvider` in `token_provider.rs` wraps this with
//! the cache + mutex. Keeping refresh logic stateless makes it
//! trivially testable with wiremock.

use crate::google_oauth::domain::OAuthError;
use crate::google_oauth::infrastructure::config::OAuthCredentials;
use serde::Deserialize;
use std::time::Duration;

/// The Google OAuth 2.0 token endpoint. Override in tests by passing
/// `RefreshClient::with_endpoint(url)` so wiremock can intercept.
const DEFAULT_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// Backoff schedule (production). On the first transient failure we
/// wait `delays[0]`; on the second we wait `delays[1]`; after that we
/// give up.
///
/// Tests bypass this with [`RefreshClient::with_fast_retries`] so
/// retry-exhaustion paths exercise quickly.
const PRODUCTION_RETRY_DELAYS: &[Duration] = &[Duration::from_secs(1), Duration::from_secs(2)];

/// Successful refresh response from Google. The `refresh_token` field
/// is `Option<String>` — Google sometimes includes a rotated value,
/// most of the time it omits the field. The caller decides what to do.
#[derive(Debug, Clone)]
pub struct RefreshResponse {
    pub access_token: String,
    pub expires_in: u64,
    /// Present iff Google rotated the refresh token in this response.
    /// The provider logs this as a WARN but does NOT persist (per
    /// design §5.4).
    pub rotated_refresh_token: Option<String>,
}

/// Stateless HTTP client for the Google OAuth token endpoint.
#[derive(Debug, Clone)]
pub struct RefreshClient {
    http: reqwest::Client,
    endpoint: String,
    retry_delays: Vec<Duration>,
}

impl RefreshClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .expect("reqwest builder should not fail with default opts"),
            endpoint: DEFAULT_TOKEN_ENDPOINT.to_string(),
            retry_delays: PRODUCTION_RETRY_DELAYS.to_vec(),
        }
    }

    /// Production constructor pointing at a custom OAuth2 token endpoint.
    /// Used by the http_request node's native OAuth to support any provider
    /// (not just Google). Same timeouts/retries as `new()`.
    pub fn with_endpoint(endpoint: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .expect("reqwest builder should not fail with default opts"),
            endpoint: endpoint.to_string(),
            retry_delays: PRODUCTION_RETRY_DELAYS.to_vec(),
        }
    }

    /// Test constructor: point at a wiremock URL and use near-zero
    /// retry delays so `retries_exhausted` tests finish in milliseconds.
    #[cfg(test)]
    pub fn for_tests(endpoint: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("reqwest builder"),
            endpoint: endpoint.to_string(),
            retry_delays: vec![Duration::from_millis(20), Duration::from_millis(40)],
        }
    }

    /// Refresh once, with retries on transient failures per
    /// [`PRODUCTION_RETRY_DELAYS`]. Non-transient errors short-circuit
    /// (no retry on 400s — they're misconfig or revocation, not
    /// flakes).
    pub async fn refresh(&self, creds: &OAuthCredentials) -> Result<RefreshResponse, OAuthError> {
        let mut attempt: usize = 0;
        loop {
            match self.refresh_once(creds).await {
                Ok(resp) => return Ok(resp),
                Err(err) if !is_transient(&err) => return Err(err),
                Err(err) => {
                    if attempt >= self.retry_delays.len() {
                        // Last error becomes the surfaced one. Wrap so
                        // the caller knows retries were attempted.
                        return Err(OAuthError::Transient(format!(
                            "{} (after {} retries)",
                            transient_inner_msg(&err),
                            attempt
                        )));
                    }
                    tokio::time::sleep(self.retry_delays[attempt]).await;
                    attempt += 1;
                }
            }
        }
    }

    async fn refresh_once(&self, creds: &OAuthCredentials) -> Result<RefreshResponse, OAuthError> {
        let form: [(&str, &str); 4] = [
            ("grant_type", "refresh_token"),
            ("refresh_token", creds.refresh_token.expose()),
            ("client_id", &creds.client_id),
            ("client_secret", &creds.client_secret),
        ];

        let resp = self
            .http
            .post(&self.endpoint)
            .form(&form)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() || e.is_connect() {
                    OAuthError::Transient(format!("network: {e}"))
                } else {
                    OAuthError::Transient(format!("reqwest: {e}"))
                }
            })?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        if status.is_success() {
            let parsed: TokenSuccessBody = serde_json::from_str(&body).map_err(|e| {
                OAuthError::Transient(format!(
                    "failed to parse Google token success response: {e}: {body}"
                ))
            })?;
            return Ok(RefreshResponse {
                access_token: parsed.access_token,
                expires_in: parsed.expires_in,
                rotated_refresh_token: parsed.refresh_token,
            });
        }

        // Error path. Google returns JSON like
        //   { "error": "invalid_grant", "error_description": "..." }
        // for 4xx; for 5xx it may be HTML. Parse defensively.
        let parsed: Option<TokenErrorBody> = serde_json::from_str(&body).ok();
        let google_error = parsed.as_ref().map(|e| e.error.as_str()).unwrap_or("");

        match (status.as_u16(), google_error) {
            (400, "invalid_grant") => Err(OAuthError::RefreshTokenRevoked),
            (400, "invalid_client") => Err(OAuthError::ClientCredsInvalid(
                parsed
                    .and_then(|e| e.error_description)
                    .unwrap_or_else(|| "client credentials rejected by Google".to_string()),
            )),
            (status_code, _) if (500..600).contains(&status_code) => {
                Err(OAuthError::Transient(format!("HTTP {status_code}: {body}")))
            }
            // 401/403/other 4xx — not transient, surface as ClientCredsInvalid
            // with the body. Most operational issues we expect map to one of
            // the two specific 400 variants above; anything else here is a
            // signal that something unusual happened and is worth a hard
            // error rather than retry.
            (status_code, err_kind) => Err(OAuthError::ClientCredsInvalid(format!(
                "HTTP {status_code} {err_kind}: {body}"
            ))),
        }
    }
}

impl Default for RefreshClient {
    fn default() -> Self {
        Self::new()
    }
}

/// True iff the variant indicates a transient (retryable) failure.
fn is_transient(err: &OAuthError) -> bool {
    matches!(err, OAuthError::Transient(_))
}

fn transient_inner_msg(err: &OAuthError) -> String {
    match err {
        OAuthError::Transient(m) => m.clone(),
        other => format!("{other:?}"),
    }
}

#[derive(Debug, Deserialize)]
struct TokenSuccessBody {
    access_token: String,
    expires_in: u64,
    /// Present only when Google rotates the refresh token in the
    /// response (rare in practice). Optional so JSON parsing succeeds
    /// in the much more common no-rotation case.
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenErrorBody {
    error: String,
    error_description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn creds() -> OAuthCredentials {
        OAuthCredentials::for_tests("CLIENT_ID_FOO", "CLIENT_SECRET_BAR", "RT_VALUE")
    }

    #[tokio::test]
    async fn with_endpoint_targets_custom_url() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"access_token":"ya29.custom","expires_in":3600,"token_type":"Bearer"}"#,
            ))
            .mount(&server)
            .await;
        let client = RefreshClient::with_endpoint(&server.uri());
        let creds = OAuthCredentials::for_tests("cid", "csec", "rt");
        let resp = client.refresh(&creds).await.expect("refresh ok");
        assert_eq!(resp.access_token.as_str(), "ya29.custom");
    }

    #[tokio::test]
    async fn refresh_happy_path_returns_access_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("refresh_token=RT_VALUE"))
            .and(body_string_contains("client_id=CLIENT_ID_FOO"))
            .and(body_string_contains("client_secret=CLIENT_SECRET_BAR"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"access_token":"ya29.NEW","expires_in":3599,"token_type":"Bearer"}"#,
            ))
            .mount(&server)
            .await;

        let client = RefreshClient::for_tests(&server.uri());
        let resp = client.refresh(&creds()).await.unwrap();
        assert_eq!(resp.access_token, "ya29.NEW");
        assert_eq!(resp.expires_in, 3599);
        assert_eq!(resp.rotated_refresh_token, None);
    }

    #[tokio::test]
    async fn refresh_returns_rotated_refresh_token_when_present() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"access_token":"ya29.NEW","expires_in":3599,
                    "refresh_token":"1//ROTATED","token_type":"Bearer"}"#,
            ))
            .mount(&server)
            .await;

        let client = RefreshClient::for_tests(&server.uri());
        let resp = client.refresh(&creds()).await.unwrap();
        assert_eq!(resp.access_token, "ya29.NEW");
        assert_eq!(resp.rotated_refresh_token.as_deref(), Some("1//ROTATED"));
    }

    #[tokio::test]
    async fn refresh_maps_invalid_grant_to_revoked() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string(
                r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
            ))
            .mount(&server)
            .await;

        let client = RefreshClient::for_tests(&server.uri());
        let err = client.refresh(&creds()).await.unwrap_err();
        assert_eq!(err, OAuthError::RefreshTokenRevoked);
    }

    #[tokio::test]
    async fn refresh_maps_invalid_client_to_client_creds_invalid() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string(
                r#"{"error":"invalid_client","error_description":"The OAuth client was not found."}"#,
            ))
            .mount(&server)
            .await;

        let client = RefreshClient::for_tests(&server.uri());
        let err = client.refresh(&creds()).await.unwrap_err();
        match err {
            OAuthError::ClientCredsInvalid(msg) => {
                assert!(msg.contains("OAuth client was not found"));
            }
            other => panic!("expected ClientCredsInvalid, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn refresh_retries_on_5xx_and_succeeds() {
        let server = MockServer::start().await;
        // First call → 503; second → 200. The retry budget covers
        // exactly one failure → second attempt succeeds.
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string("Service Unavailable"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"access_token":"ya29.RETRY_OK","expires_in":3599,"token_type":"Bearer"}"#,
            ))
            .mount(&server)
            .await;

        let client = RefreshClient::for_tests(&server.uri());
        let resp = client.refresh(&creds()).await.unwrap();
        assert_eq!(resp.access_token, "ya29.RETRY_OK");
    }

    #[tokio::test]
    async fn refresh_returns_transient_after_retry_budget_exhausted() {
        let server = MockServer::start().await;
        // Every call returns 503. After the for_tests retry schedule
        // (2 retries), the client gives up.
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string("Service Unavailable"))
            .mount(&server)
            .await;

        let client = RefreshClient::for_tests(&server.uri());
        let err = client.refresh(&creds()).await.unwrap_err();
        match err {
            OAuthError::Transient(msg) => {
                assert!(
                    msg.contains("after 2 retries"),
                    "expected retry-count annotation, got: {msg}"
                );
            }
            other => panic!("expected Transient, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn refresh_does_not_retry_on_4xx_invalid_grant() {
        // Two 400s would be observed if we retried. Mock only allows
        // ONE response — a second call would hit no matcher and fail
        // the test loudly.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_string(r#"{"error":"invalid_grant","error_description":"revoked"}"#),
            )
            .expect(1) // STRICT — exactly one call.
            .mount(&server)
            .await;

        let client = RefreshClient::for_tests(&server.uri());
        let err = client.refresh(&creds()).await.unwrap_err();
        assert_eq!(err, OAuthError::RefreshTokenRevoked);
    }
}
