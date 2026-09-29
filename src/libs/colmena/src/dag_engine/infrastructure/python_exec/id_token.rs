//! Identity tokens from the GCP metadata server of the host's own runtime
//! identity, audience-bound to the executor service. Cached in memory only,
//! never logged.

use serde_json::Value;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const METADATA_IDENTITY_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/identity";
/// A cached token is replaced once it has less than this left.
const REFRESH_MARGIN: Duration = Duration::from_secs(300);
/// If replacing it fails, a cached token with more than this left is used.
const MIN_LEFT: Duration = Duration::from_secs(30);

/// `http` must not use a proxy (build it with `.no_proxy()`): the metadata
/// request is plain http, and the token would cross the proxy in clear text.
pub struct IdTokenSource {
    audience: String,
    endpoint: String,
    http: reqwest::Client,
    cache: tokio::sync::Mutex<Cache>,
}

#[derive(Default)]
struct Cache {
    token: Option<(String, SystemTime)>,
    /// When the last fetch failed, and its error.
    failed: Option<(Instant, String)>,
}

impl Cache {
    /// The token, if it is still valid `margin` from now.
    fn valid_for(&self, margin: Duration) -> Option<String> {
        let (token, expiry) = self.token.as_ref()?;
        (SystemTime::now() + margin < *expiry).then(|| token.clone())
    }
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
            cache: Default::default(),
        }
    }

    /// The cached token, or a new one once it is within [`REFRESH_MARGIN`] of
    /// its expiry. Concurrent callers wait for one fetch and share its token,
    /// or its error. If a refresh fails, a cached token with more than
    /// [`MIN_LEFT`] left is returned, and the next call tries again.
    pub async fn token(&self) -> Result<String, String> {
        let asked = Instant::now();
        let mut cache = self.cache.lock().await;
        if let Some(token) = cache.valid_for(REFRESH_MARGIN) {
            return Ok(token);
        }
        let error = match &cache.failed {
            // A fetch failed while this caller waited: no retry in a row.
            Some((at, e)) if *at > asked => e.clone(),
            _ => match self.fetch().await {
                Ok((token, expiry)) => {
                    cache.token = Some((token.clone(), expiry));
                    cache.failed = None;
                    return Ok(token);
                }
                Err(e) => {
                    cache.failed = Some((Instant::now(), e.clone()));
                    e
                }
            },
        };
        cache.valid_for(MIN_LEFT).ok_or(error)
    }

    async fn fetch(&self) -> Result<(String, SystemTime), String> {
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
        Ok((token, expiry))
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
    let exp = v["exp"].as_u64().ok_or("JWT has no exp")?;
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
        UNIX_EPOCH.elapsed().unwrap().as_secs()
    }

    fn source(server: &MockServer) -> IdTokenSource {
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        IdTokenSource::with_endpoint("https://svc.example".into(), server.uri(), http)
    }

    /// Answers the next `n` requests with `status` and `body`, after `ms`.
    async fn answer(server: &MockServer, n: u64, status: u16, body: &str, ms: u64) {
        let r = ResponseTemplate::new(status).set_body_string(body);
        let r = r.set_delay(Duration::from_millis(ms));
        let mock = Mock::given(method("GET")).respond_with(r).up_to_n_times(n);
        mock.mount(server).await;
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
        let src = source(&server);
        assert_eq!(src.token().await.unwrap(), jwt(exp));
        assert_eq!(src.token().await.unwrap(), jwt(exp));
    }

    #[tokio::test]
    async fn refreshes_a_token_about_to_expire() {
        let server = MockServer::start().await;
        answer(&server, 2, 200, &jwt(now() + 60), 0).await;
        let src = source(&server);
        src.token().await.unwrap();
        src.token().await.unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn concurrent_callers_share_one_fetch_and_its_failure() {
        let server = MockServer::start().await;
        let tok = jwt(now() + 3600);
        answer(&server, 1, 503, "", 200).await;
        answer(&server, 1, 200, &tok, 200).await;
        let src = source(&server);
        let wave = || futures::future::join_all((0..8).map(|_| src.token()));
        let err = Err("the metadata server answered HTTP 503".to_string());
        assert_eq!(wave().await, vec![err; 8]);
        assert_eq!(wave().await, vec![Ok(tok); 8]);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_failed_refresh_keeps_a_token_that_is_still_valid() {
        for (left, kept) in [(120, true), (10, false)] {
            let server = MockServer::start().await;
            let tok = jwt(now() + left);
            answer(&server, 1, 200, &tok, 0).await;
            answer(&server, 1, 503, "", 0).await;
            let src = source(&server);
            assert_eq!(src.token().await, Ok(tok.clone()));
            let err = "the metadata server answered HTTP 503".to_string();
            assert_eq!(src.token().await, if kept { Ok(tok) } else { Err(err) });
            assert_eq!(server.received_requests().await.unwrap().len(), 2);
        }
    }

    #[tokio::test]
    async fn a_refused_request_is_an_error_without_the_body() {
        let server = MockServer::start().await;
        answer(&server, 1, 404, "detail", 0).await;
        let e = source(&server).token().await.unwrap_err();
        assert_eq!(e, "the metadata server answered HTTP 404");
    }

    #[test]
    fn reads_the_expiry_and_rejects_non_jwts() {
        let at_42 = UNIX_EPOCH + Duration::from_secs(42);
        assert_eq!(jwt_expiry(&jwt(42)), Ok(at_42));
        assert!(jwt_expiry("nope").is_err());
        assert!(jwt_expiry("h.!!.s").is_err());
        assert!(jwt_expiry(&jwt(u64::MAX)).is_err());
        let no_exp = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("{}");
        assert!(jwt_expiry(&format!("h.{no_exp}.s")).is_err());
    }
}
