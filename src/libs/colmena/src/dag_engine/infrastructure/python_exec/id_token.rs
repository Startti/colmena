//! Identity tokens from the GCP metadata server of the host's own runtime
//! identity, audience-bound to the executor service. Cached in memory only,
//! never logged.

use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const METADATA_IDENTITY_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/identity";
/// A cached token is replaced once it has less than this left.
const REFRESH_MARGIN: Duration = Duration::from_secs(300);

pub struct IdTokenSource {
    audience: String,
    endpoint: String,
    http: reqwest::Client,
    cached: tokio::sync::Mutex<Option<(String, SystemTime)>>,
}

impl IdTokenSource {
    pub fn new(audience: String, http: reqwest::Client) -> Self {
        Self::with_endpoint(audience, METADATA_IDENTITY_URL.to_string(), http)
    }

    pub fn with_endpoint(audience: String, endpoint: String, http: reqwest::Client) -> Self {
        Self {
            audience,
            endpoint,
            http,
            cached: tokio::sync::Mutex::new(None),
        }
    }

    /// The cached token, or a new one once it is within [`REFRESH_MARGIN`] of
    /// its expiry. Concurrent callers wait for a single fetch.
    pub async fn token(&self) -> Result<String, String> {
        let mut cached = self.cached.lock().await;
        if let Some((token, expiry)) = cached.as_ref() {
            if SystemTime::now() + REFRESH_MARGIN < *expiry {
                return Ok(token.clone());
            }
        }
        let resp = self
            .http
            .get(&self.endpoint)
            .query(&[("audience", self.audience.as_str()), ("format", "standard")])
            .header("Metadata-Flavor", "Google")
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|_| "the metadata server did not answer".to_string())?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            return Err(format!("the metadata server answered HTTP {status}"));
        }
        let text = resp.text().await;
        let token = text.map_err(|_| "unreadable metadata response".to_string())?;
        let token = token.trim().to_string();
        let expiry = jwt_expiry(&token)?;
        *cached = Some((token.clone(), expiry));
        Ok(token)
    }
}

/// The `exp` claim of a JWT, read without checking its signature: the token
/// is only forwarded, and this only decides when to fetch a new one.
pub fn jwt_expiry(jwt: &str) -> Result<SystemTime, String> {
    use base64::Engine;
    let payload = jwt.split('.').nth(1).ok_or("not a JWT")?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| "JWT payload is not base64url")?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| "JWT payload is not JSON")?;
    let exp = v
        .get("exp")
        .and_then(Value::as_u64)
        .ok_or("JWT has no exp")?;
    let expiry = UNIX_EPOCH.checked_add(Duration::from_secs(exp));
    expiry.ok_or_else(|| "JWT exp is out of range".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use wiremock::matchers::{header, method, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn jwt(exp: u64) -> String {
        let p =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("{{\"exp\":{exp}}}"));
        format!("h.{p}.s")
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[tokio::test]
    async fn fetches_once_and_caches_until_near_expiry() {
        let server = MockServer::start().await;
        let exp = now() + 3600;
        Mock::given(method("GET"))
            .and(header("Metadata-Flavor", "Google"))
            .and(query_param("audience", "https://svc.example"))
            .and(query_param("format", "standard"))
            .respond_with(ResponseTemplate::new(200).set_body_string(jwt(exp)))
            .expect(1)
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        let src = IdTokenSource::with_endpoint("https://svc.example".into(), server.uri(), http);
        assert_eq!(src.token().await.unwrap(), jwt(exp));
        assert_eq!(src.token().await.unwrap(), jwt(exp));
    }

    #[tokio::test]
    async fn refreshes_a_token_about_to_expire() {
        let server = MockServer::start().await;
        let soon = now() + 60;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(jwt(soon)))
            .expect(2)
            .mount(&server)
            .await;
        let src = IdTokenSource::with_endpoint("a".into(), server.uri(), reqwest::Client::new());
        src.token().await.unwrap();
        src.token().await.unwrap();
    }

    #[tokio::test]
    async fn a_refused_request_is_an_error_without_the_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_string("detail"))
            .mount(&server)
            .await;
        let src = IdTokenSource::with_endpoint("a".into(), server.uri(), reqwest::Client::new());
        let e = src.token().await.unwrap_err();
        assert_eq!(e, "the metadata server answered HTTP 404");
    }

    #[test]
    fn reads_the_expiry_and_rejects_non_jwts() {
        assert_eq!(
            jwt_expiry(&jwt(42)),
            Ok(UNIX_EPOCH + Duration::from_secs(42))
        );
        assert!(jwt_expiry("nope").is_err());
        assert!(jwt_expiry("h.!!.s").is_err());
        assert!(jwt_expiry(&jwt(u64::MAX)).is_err());
        let no_exp = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("{}");
        assert!(jwt_expiry(&format!("h.{no_exp}.s")).is_err());
    }
}
