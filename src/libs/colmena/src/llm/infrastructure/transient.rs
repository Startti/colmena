//! Transient provider answers: which HTTP statuses say nothing about the
//! request or the key, and how long to wait before asking again.

use crate::llm::domain::LlmError;
use std::time::Duration;

/// How long one pre-flight key check waits for the provider. A slower answer
/// is a network error, which says nothing about the key.
pub const CREDENTIAL_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// 408, 429, 500, 502, 503, 504 and 529 (Anthropic's "overloaded"). Any other status,
/// a 5xx included, is not transient.
pub fn is_transient_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504 | 529)
}

/// Exponential backoff with full jitter: a random wait in
/// `[0, min(cap, base * 2^(attempt - 1))]`. `attempt` counts from 1.
pub fn backoff_with_jitter(attempt: u32, base: Duration, cap: Duration) -> Duration {
    backoff_with(attempt, base, cap, jitter_fraction())
}

fn backoff_with(attempt: u32, base: Duration, cap: Duration, fraction: f64) -> Duration {
    let exponent = attempt.saturating_sub(1).min(16);
    let ceiling = base.saturating_mul(1u32 << exponent).min(cap);
    ceiling.mul_f64(fraction.clamp(0.0, 1.0))
}

/// A uniform fraction in `[0, 1)` from 53 of the random low bits of a v4 UUID
/// (the crate already depends on `uuid`; no new dependency for one number).
fn jitter_fraction() -> f64 {
    let bits = (uuid::Uuid::new_v4().as_u128() & ((1u128 << 53) - 1)) as u64;
    bits as f64 / (1u64 << 53) as f64
}

/// How many times a provider adapter sends a model request again after a
/// transient answer: 2 by default, 0 turns it off, never more than 5.
pub const TRANSIENT_RETRIES_ENV: &str = "COLMENA_LLM_TRANSIENT_RETRIES";
const DEFAULT_RETRIES: u32 = 2;
const MAX_RETRIES: u32 = 5;

fn retries_from(value: Option<&str>) -> u32 {
    value
        .and_then(|v| v.trim().parse::<u32>().ok())
        .map(|n| n.min(MAX_RETRIES))
        .unwrap_or(DEFAULT_RETRIES)
}

/// How a provider adapter sends a model request again after a transient answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    /// Sends after the first one; 0 turns retrying off.
    pub retries: u32,
    pub base: Duration,
    pub cap: Duration,
    /// A `Retry-After` longer than this is not waited for: the answer is
    /// returned as it came.
    pub max_retry_after: Duration,
}

impl RetryPolicy {
    pub fn from_env() -> Self {
        Self {
            retries: retries_from(std::env::var(TRANSIENT_RETRIES_ENV).ok().as_deref()),
            base: Duration::from_secs(1),
            cap: Duration::from_secs(8),
            max_retry_after: Duration::from_secs(20),
        }
    }

    /// No waits, for tests that count requests.
    #[cfg(test)]
    pub(crate) fn immediate(retries: u32) -> Self {
        Self {
            retries,
            base: Duration::ZERO,
            cap: Duration::ZERO,
            max_retry_after: Duration::from_secs(1),
        }
    }
}

/// `Retry-After` in seconds; the HTTP-date form is not read.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Sends `request`, and sends it again while the provider answers a transient
/// status (see [`is_transient_status`]), up to `policy.retries` more times:
/// after the wait `Retry-After` asks for when it is within
/// `policy.max_retry_after`, otherwise after an exponential backoff with full
/// jitter. The decision reads only the status and the headers, so the body of
/// the answer that is returned has not been touched: a stream is never sent
/// again once its output has started. A transport error is returned at once,
/// because the request may have reached the provider, and without the URL in
/// its text; so is the first answer to a request whose body cannot be copied
/// (a stream). The last answer is returned as it came, so the caller reads its
/// status and body as before.
pub async fn send_with_transient_retry(
    provider: &'static str,
    policy: &RetryPolicy,
    request: reqwest::RequestBuilder,
) -> Result<reqwest::Response, LlmError> {
    let network = |e: reqwest::Error| LlmError::network_error(e.without_url().to_string());
    let mut retried = 0u32;
    loop {
        let Some(attempt) = request.try_clone() else {
            return request.send().await.map_err(network);
        };
        let response = attempt.send().await.map_err(network)?;
        let status = response.status().as_u16();
        if !is_transient_status(status) || retried >= policy.retries {
            return Ok(response);
        }
        let wait = match retry_after(response.headers()) {
            Some(asked) if asked > policy.max_retry_after => return Ok(response),
            Some(asked) => asked,
            None => backoff_with_jitter(retried + 1, policy.base, policy.cap),
        };
        tracing::warn!(
            target: "colmena::llm",
            provider,
            status,
            retry = retried + 1,
            wait_ms = wait.as_millis() as u64,
            "provider answered a transient status; sending the request again"
        );
        drop(response);
        tokio::time::sleep(wait).await;
        retried += 1;
    }
}

/// A stub provider for tests: `first` answers the first POST, `then` (or
/// `first` again) every later one.
#[cfg(test)]
pub(crate) async fn stub_answering(
    first: wiremock::ResponseTemplate,
    then: Option<wiremock::ResponseTemplate>,
) -> wiremock::MockServer {
    use wiremock::{matchers::method, Mock, MockServer};
    let server = MockServer::start().await;
    let later = then.unwrap_or_else(|| first.clone());
    Mock::given(method("POST"))
        .respond_with(first)
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(later)
        .with_priority(2)
        .mount(&server)
        .await;
    server
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::ResponseTemplate;

    #[test]
    fn transient_statuses() {
        for s in [408, 429, 500, 502, 503, 504, 529] {
            assert!(is_transient_status(s), "{s}");
        }
        for s in [200, 400, 401, 403, 404, 422, 501] {
            assert!(!is_transient_status(s), "{s}");
        }
    }

    #[test]
    fn the_ceiling_doubles_per_attempt_and_stops_at_the_cap() {
        let base = Duration::from_millis(100);
        let cap = Duration::from_millis(350);
        assert_eq!(backoff_with(1, base, cap, 1.0), Duration::from_millis(100));
        assert_eq!(backoff_with(2, base, cap, 1.0), Duration::from_millis(200));
        assert_eq!(backoff_with(3, base, cap, 1.0), Duration::from_millis(350));
        assert_eq!(backoff_with(40, base, cap, 1.0), Duration::from_millis(350));
    }

    #[test]
    fn the_wait_is_a_fraction_of_the_ceiling() {
        let base = Duration::from_millis(100);
        let cap = Duration::from_secs(1);
        assert_eq!(backoff_with(1, base, cap, 0.0), Duration::ZERO);
        assert_eq!(backoff_with(1, base, cap, 0.5), Duration::from_millis(50));
        assert_eq!(backoff_with(1, base, cap, 7.0), Duration::from_millis(100));
    }

    #[test]
    fn jitter_is_spread_over_the_unit_interval() {
        let samples: Vec<f64> = (0..200).map(|_| jitter_fraction()).collect();
        assert!(samples.iter().all(|f| (0.0..1.0).contains(f)));
        assert!(samples.iter().any(|f| *f < 0.5) && samples.iter().any(|f| *f >= 0.5));
        assert!(
            backoff_with_jitter(1, Duration::from_millis(100), Duration::from_secs(1))
                <= Duration::from_millis(100)
        );
    }

    /// (status of the last answer, requests the server received)
    async fn post(
        first: ResponseTemplate,
        then: Option<ResponseTemplate>,
        retries: u32,
    ) -> (u16, usize) {
        let server = stub_answering(first, then).await;
        let request = reqwest::Client::new().post(server.uri());
        let policy = RetryPolicy::immediate(retries);
        let response = send_with_transient_retry("test", &policy, request)
            .await
            .unwrap();
        (
            response.status().as_u16(),
            server.received_requests().await.unwrap().len(),
        )
    }

    #[tokio::test]
    async fn a_transient_status_is_sent_again_until_the_retries_run_out() {
        assert_eq!(post(ResponseTemplate::new(503), None, 2).await, (503, 3));
    }

    #[tokio::test]
    async fn the_answer_after_a_transient_one_is_returned() {
        let then = Some(ResponseTemplate::new(200));
        assert_eq!(post(ResponseTemplate::new(429), then, 2).await, (200, 2));
    }

    #[tokio::test]
    async fn a_status_that_is_not_transient_is_not_sent_again() {
        assert_eq!(post(ResponseTemplate::new(400), None, 2).await, (400, 1));
    }

    #[tokio::test]
    async fn zero_retries_turns_it_off() {
        assert_eq!(post(ResponseTemplate::new(503), None, 0).await, (503, 1));
    }

    #[tokio::test]
    async fn a_retry_after_over_the_limit_is_not_waited_for() {
        let first = ResponseTemplate::new(503).insert_header("Retry-After", "30");
        assert_eq!(post(first, None, 2).await, (503, 1));
    }

    #[tokio::test]
    async fn a_retry_after_within_the_limit_is_waited_for() {
        let first = ResponseTemplate::new(503).insert_header("Retry-After", "1");
        let started = std::time::Instant::now();
        let then = Some(ResponseTemplate::new(200));
        assert_eq!(post(first, then, 2).await, (200, 2));
        assert!(started.elapsed() >= Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn the_default_backoff_is_a_real_wait() {
        let mut policy = RetryPolicy::from_env();
        policy.retries = 1;
        let s = Duration::from_secs;
        let got = (policy.base, policy.cap, policy.max_retry_after);
        assert_eq!(got, (s(1), s(8), s(20)));
        let server =
            stub_answering(ResponseTemplate::new(503), Some(ResponseTemplate::new(200))).await;
        // No idle-pool timer: the paused clock would jump to it during the I/O.
        let client = reqwest::Client::builder()
            .pool_idle_timeout(None)
            .build()
            .unwrap();
        let started = tokio::time::Instant::now();
        let sent = send_with_transient_retry("test", &policy, client.post(server.uri())).await;
        assert!(sent.is_ok() && started.elapsed() > Duration::ZERO);
    }

    #[tokio::test]
    async fn a_body_that_cannot_be_copied_is_sent_once() {
        let server = stub_answering(ResponseTemplate::new(503), None).await;
        let body = futures::stream::iter([Ok::<_, std::io::Error>("{}")]);
        let request = reqwest::Client::new()
            .post(server.uri())
            .body(reqwest::Body::wrap_stream(body));
        let policy = RetryPolicy::immediate(2);
        let response = send_with_transient_retry("test", &policy, request).await;
        assert_eq!(response.unwrap().status().as_u16(), 503);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_transport_error_is_not_sent_again() {
        // Accepts each connection and closes it without an answer.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1?key=FAKE-KEY", listener.local_addr().unwrap());
        let accepted = tokio::spawn(async move {
            let mut count = 0;
            let quiet = Duration::from_millis(500);
            while let Ok(Ok((socket, _))) = tokio::time::timeout(quiet, listener.accept()).await {
                drop(socket);
                count += 1;
            }
            count
        });
        let request = reqwest::Client::new().post(url);
        let result = send_with_transient_retry("test", &RetryPolicy::immediate(2), request).await;
        let Err(LlmError::NetworkError { message }) = result else {
            panic!("{result:?}");
        };
        assert!(!message.contains("FAKE-KEY") && !message.contains("127.0.0.1"));
        assert_eq!(accepted.await.unwrap(), 1);
    }

    #[test]
    fn the_number_of_retries_comes_from_the_environment_and_is_bounded() {
        assert_eq!(retries_from(None), 2);
        assert_eq!(retries_from(Some("0")), 0);
        assert_eq!(retries_from(Some(" 3 ")), 3);
        assert_eq!(retries_from(Some("99")), 5);
        assert_eq!(retries_from(Some("x")), 2);
    }
}
