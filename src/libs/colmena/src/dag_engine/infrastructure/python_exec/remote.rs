//! Sends each Python call to a `python_executor serve` endpoint, with zstd
//! bodies, within one deadline: the call's timeout plus [`TRANSFER_GRACE`].
//! Only a call that did not run is sent again: after a connection error or a
//! bare 503, once; after 429, or 503 with `Retry-After`, as it says, while the
//! code would still get its whole timeout. It never falls back to running the
//! code in process. Requests go only to the configured URL: proxy variables
//! are ignored and redirects are not followed.

use super::config::{ExecutorConfigError, RemoteAuthConfig, RemoteConfig};
use super::id_token::IdTokenSource;
use super::protocol::{input_too_large_message, result_too_large_message as too_large};
use super::protocol::{WireRequest, WireResponse};
use crate::dag_engine::domain::python_executor::PythonRunResult;
use crate::dag_engine::domain::python_executor::{ExecutorKind, PythonExecutor};
use crate::dag_engine::domain::python_executor::{PythonRunError, PythonRunRequest};
use crate::dag_engine::log_policy::T_PYTHON_EXEC;
use bytes::Bytes;
use reqwest::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE, RETRY_AFTER};
use reqwest::{RequestBuilder, Url};
use std::io::Read;
use std::time::Duration;
use tokio::time::Instant;

/// Time beyond a call's timeout for waits and transfers; short in unit tests.
const TRANSFER_GRACE: Duration = Duration::from_secs(if cfg!(test) { 4 } else { 30 });
const RETRY_DELAY: Duration = Duration::from_millis(250);
/// Short in unit tests, which wait for it to expire.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(if cfg!(test) { 200 } else { 10_000 });
/// How long [`PythonExecutor::warm`] waits for the service, and how often it asks.
const READY_WITHIN: Duration = Duration::from_secs(120);
const READY_POLL: Duration = Duration::from_secs(1);
const REJECTED: &str =
    "PythonExecutorError: the isolated Python executor rejected this caller's credentials";
const MALFORMED: &str = "PythonExecutorError: malformed response from the isolated Python executor";

pub struct RemoteExecutor {
    cfg: RemoteConfig,
    max_timeout: Duration,
    ready_within: Duration,
    http: reqwest::Client,
    id_tokens: Option<IdTokenSource>,
}

enum Attempt {
    Done(Result<PythonRunResult, PythonRunError>),
    /// Did not run: why, and the wait asked for (none after a connection error or a bare 503).
    Retry(String, Option<Duration>),
}

fn internal(m: impl Into<String>) -> PythonRunError {
    PythonRunError::Internal(m.into())
}

fn unavailable(why: impl std::fmt::Display) -> PythonRunError {
    let m = format!("PythonExecutorError: the isolated Python executor is unavailable ({why})");
    internal(m)
}

/// The `Retry-After` of `r` in seconds, from 250 ms to 2 s, plus up to 255 ms
/// so that callers told the same do not all come back at once.
fn retry_after(r: &reqwest::Response) -> Duration {
    let secs = r.headers().get(RETRY_AFTER);
    let secs = secs.and_then(|v| v.to_str().ok()?.parse().ok());
    let wait = secs.map_or(RETRY_DELAY, Duration::from_secs);
    let jitter = Duration::from_millis(uuid::Uuid::new_v4().as_bytes()[0].into());
    wait.clamp(RETRY_DELAY, Duration::from_secs(2)) + jitter
}

/// `path` under `base`, keeping the path of `base` whether or not it ends in `/`.
pub(crate) fn endpoint(base: &Url, path: &str) -> Url {
    let mut url = base.clone();
    url.set_path(&format!("{}/{path}", base.path().trim_end_matches('/')));
    url
}

/// A response body, decompressed and parsed, within `max` bytes.
fn decode(zstd_body: bool, body: Vec<u8>, max: usize) -> Result<PythonRunResult, PythonRunError> {
    let mut raw = Vec::new();
    match zstd_body {
        false => raw = body,
        true => zstd::stream::read::Decoder::new(&body[..])
            .and_then(|d| d.take(max as u64 + 1).read_to_end(&mut raw))
            .map(drop)
            .map_err(|_| internal(MALFORMED))?,
    }
    if raw.len() > max {
        return Err(PythonRunError::Python(too_large(max)));
    }
    let wire = serde_json::from_slice::<WireResponse>(&raw).map_err(|_| internal(MALFORMED))?;
    wire.into_result()
}

impl RemoteExecutor {
    /// Takes `cfg` as [`RemoteConfig::from_lookup`] checked it: the URL rules live there.
    pub fn new(cfg: RemoteConfig, max_timeout: Duration) -> Result<Self, ExecutorConfigError> {
        let http = reqwest::Client::builder()
            .use_rustls_tls()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            // No pool: a kept connection is served by the runtime that opened it.
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|e| ExecutorConfigError(format!("cannot build the HTTP client: {e}")))?;
        let id_tokens = match &cfg.auth {
            RemoteAuthConfig::GcpIdToken { audience } => Some(audience.clone()),
            _ => None,
        };
        let id_tokens = id_tokens.map(|a| IdTokenSource::new(a, http.clone()));
        Ok(Self {
            cfg,
            max_timeout,
            ready_within: READY_WITHIN,
            http,
            id_tokens,
        })
    }

    /// `rb` with this caller's credentials. The token file is read on each
    /// call, so a rotated token is picked up.
    pub(crate) async fn authorized(
        &self,
        rb: RequestBuilder,
    ) -> Result<RequestBuilder, PythonRunError> {
        let token = match (&self.cfg.auth, &self.id_tokens) {
            (RemoteAuthConfig::BearerFile(p), _) => {
                let text = tokio::fs::read_to_string(p).await.unwrap_or_default();
                let file = "PythonExecutorError: the Python executor token file";
                match text.trim() {
                    "" => Err(format!("{file} is unreadable or empty")),
                    // `serve` takes visible ASCII only, as a header would.
                    t if !t.bytes().all(|b| b.is_ascii_graphic()) => {
                        Err(format!("{file} does not hold a valid token"))
                    }
                    t => Ok(t.to_string()),
                }
            }
            (_, Some(source)) => source
                .token()
                .await
                .map_err(|e| format!("PythonExecutorError: cannot obtain an identity token ({e})")),
            _ => return Ok(rb),
        };
        Ok(rb.bearer_auth(token.map_err(PythonRunError::Internal)?))
    }

    /// For the mounts transport (`tabular_run::remote_seam`), which sends its
    /// own streamed request with the same client and the same credentials as
    /// `/v1/run`: the HTTP client, the service's base URL, and the longest
    /// deadline a call may ask for.
    pub fn transport(&self) -> (&reqwest::Client, &Url, Duration) {
        (&self.http, &self.cfg.url, self.max_timeout)
    }

    /// The status of `rb` sent with this caller's credentials, if answered.
    async fn status(&self, rb: RequestBuilder) -> Result<Option<u16>, String> {
        let rb = self.authorized(rb.timeout(Duration::from_secs(10))).await;
        let sent = rb.map_err(|e| e.to_string())?.send().await;
        Ok(sent.ok().map(|r| r.status().as_u16()))
    }

    async fn post_once(&self, body: Bytes, deadline: Instant, request_id: &str) -> Attempt {
        let rb = self.http.post(endpoint(&self.cfg.url, "v1/run"));
        let rb = rb.header(CONTENT_TYPE, "application/json").body(body);
        let rb = rb
            .header(CONTENT_ENCODING, "zstd")
            .header(ACCEPT_ENCODING, "zstd");
        let rb = rb.header("x-colmena-request-id", request_id);
        let rb = match tokio::time::timeout_at(deadline, self.authorized(rb)).await {
            Ok(Ok(rb)) => rb.timeout(deadline.saturating_duration_since(Instant::now())),
            Ok(Err(e)) => return Attempt::Done(Err(e)),
            // The platform's delay, not the code's timeout: nothing has run.
            Err(_) => return Attempt::Done(Err(unavailable("credentials timed out"))),
        };
        let max = self.cfg.max_response_bytes;
        let mut resp = match rb.send().await.map(|r| (r.status().as_u16(), r)) {
            Ok((200, r)) => r,
            Ok((401 | 403, _)) => return Attempt::Done(Err(internal(REJECTED))),
            // This client's limit or a front's: the text holds for either.
            Ok((413, _)) => {
                let e = "Python execution error: the input exceeds what the isolated Python executor accepts";
                return Attempt::Done(Err(PythonRunError::Python(e.into())));
            }
            Ok((s @ (429 | 503), r)) => {
                // A bare 503 (not ready, or no usable slot) is sent again once.
                let in_line = s == 429 || r.headers().contains_key(RETRY_AFTER);
                return Attempt::Retry(format!("HTTP {s}"), in_line.then(|| retry_after(&r)));
            }
            Ok((s @ (502 | 504), _)) => {
                return Attempt::Done(Err(unavailable(format!("HTTP {s}"))))
            }
            Ok((s, _)) => {
                let e =
                    format!("PythonExecutorError: the isolated Python executor answered HTTP {s}");
                return Attempt::Done(Err(internal(e)));
            }
            // Before `is_timeout`: a connect timeout is both, and the call did not run.
            Err(e) if e.is_connect() => return Attempt::Retry("connection failed".into(), None),
            Err(e) if e.is_timeout() => return Attempt::Done(Err(PythonRunError::Timeout)),
            Err(_) => return Attempt::Done(Err(unavailable("request failed"))),
        };
        let announced = resp.content_length().unwrap_or(0);
        if announced > max as u64 {
            return Attempt::Done(Err(PythonRunError::Python(too_large(max))));
        }
        let zstd_body = resp.headers().get(CONTENT_ENCODING).map(|v| v == "zstd");
        let mut body = Vec::with_capacity(announced as usize);
        loop {
            match resp.chunk().await {
                Ok(Some(c)) if body.len() + c.len() > max => {
                    return Attempt::Done(Err(PythonRunError::Python(too_large(max))))
                }
                Ok(Some(c)) => {
                    // When full, doubling as a `Vec` does, but never past `max`.
                    let full = body.capacity() < body.len() + c.len();
                    let want = (body.len() + c.len()).next_power_of_two().min(max);
                    body.reserve_exact(if full { want - body.len() } else { 0 });
                    body.extend_from_slice(&c)
                }
                Ok(None) => break,
                Err(e) if e.is_timeout() => return Attempt::Done(Err(PythonRunError::Timeout)),
                Err(_) => return Attempt::Done(Err(unavailable("response interrupted"))),
            }
        }
        let zstd_body = zstd_body.unwrap_or(false);
        let decoded = tokio::task::spawn_blocking(move || decode(zstd_body, body, max)).await;
        Attempt::Done(decoded.unwrap_or_else(|_| Err(internal(MALFORMED))))
    }
}

#[async_trait::async_trait]
impl PythonExecutor for RemoteExecutor {
    fn kind(&self) -> ExecutorKind {
        ExecutorKind::Remote
    }

    async fn run(&self, req: PythonRunRequest) -> Result<PythonRunResult, PythonRunError> {
        let timeout = req.timeout.unwrap_or(self.max_timeout);
        // One deadline for credentials, attempts and waits (capped: no overflow).
        let budget = timeout.saturating_add(TRANSFER_GRACE);
        let deadline = Instant::now() + budget.min(Duration::from_secs(u32::MAX.into()));
        let json = serde_json::to_vec(&WireRequest::new(req, timeout))
            .map_err(|_| internal("PythonExecutorError: cannot encode the request"))?;
        let limit = self.cfg.max_request_bytes;
        if json.len() > limit {
            return Err(PythonRunError::Python(input_too_large_message(limit)));
        }
        let packed = tokio::task::spawn_blocking(move || zstd::bulk::compress(&json, 3)).await;
        let body = packed.ok().and_then(Result::ok).map(Bytes::from);
        let body = body.ok_or(internal("PythonExecutorError: cannot compress the request"))?;
        if let Some(cap) = self.cfg.max_wire_bytes.filter(|cap| body.len() > *cap) {
            return Err(PythonRunError::Python(format!(
                "Python execution error: the compressed input exceeds the Python executor transport limit of {} MiB",
                cap / (1024 * 1024)
            )));
        }
        let (request_id, mut reconnected) = (uuid::Uuid::new_v4().to_string(), false);
        let mut resent = false;
        loop {
            let (why, wait) = match self.post_once(body.clone(), deadline, &request_id).await {
                Attempt::Done(r) => return r,
                Attempt::Retry(why, wait) => (why, wait),
            };
            // Once, or as long as asked; only while the code would still get its whole timeout.
            let again = wait.is_some() || !std::mem::replace(&mut reconnected, true);
            let wait = wait.unwrap_or(RETRY_DELAY);
            let left = deadline.saturating_duration_since(Instant::now());
            if !again || left < wait.saturating_add(timeout) {
                return Err(unavailable(why));
            }
            if !std::mem::replace(&mut resent, true) {
                tracing::warn!(target: T_PYTHON_EXEC, request_id, reason = %why, "python remote call retried");
            }
            tokio::time::sleep(wait).await;
        }
    }

    /// Waits until `/readyz` answers 200 and the service takes this caller's
    /// credentials, asking every second, for up to 120 s in all. The
    /// credentials go with an empty `POST /v1/run`, which runs nothing: past
    /// the token check the service answers 400, taken as ready. 401 or 403,
    /// or credentials that cannot be obtained, end the wait at once.
    async fn warm(&self) -> Result<(), String> {
        let mut why = "no answer".to_string();
        let asking = tokio::time::timeout(self.ready_within, async {
            loop {
                let readyz = self.http.get(endpoint(&self.cfg.url, "readyz"));
                let run = self.http.post(endpoint(&self.cfg.url, "v1/run"));
                let status = match self.status(readyz).await? {
                    Some(200) => match self.status(run).await? {
                        Some(400) => return Ok(()),
                        s => s,
                    },
                    s => s,
                };
                why = match status {
                    Some(401 | 403) => return Err(REJECTED.to_string()),
                    Some(s) => format!("HTTP {s}"),
                    None => "no answer".to_string(),
                };
                tokio::time::sleep(READY_POLL).await;
            }
        });
        let e = "PythonExecutorError: the isolated Python executor is not ready";
        asking.await.unwrap_or_else(|_| Err(format!("{e} ({why})")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::infrastructure::python_exec::protocol::WireStatus;
    use serde_json::json;
    use wiremock::matchers::{header, header_exists, path};
    use wiremock::{Mock, MockServer, ResponseTemplate as Answer};

    fn config(uri: &str, auth: RemoteAuthConfig) -> RemoteConfig {
        let url = Url::parse(uri).unwrap();
        let (max_request_bytes, max_response_bytes, max_wire_bytes) = (1 << 20, 1 << 20, None);
        RemoteConfig {
            url,
            auth,
            max_request_bytes,
            max_response_bytes,
            max_wire_bytes,
        }
    }

    fn exec(uri: &str, auth: RemoteAuthConfig) -> RemoteExecutor {
        RemoteExecutor::new(config(uri, auth), Duration::from_secs(30)).unwrap()
    }

    fn req() -> PythonRunRequest {
        let (code, mode) = ("output = 1".into(), "restricted".into());
        let (timeout, inputs) = (Some(Duration::from_secs(5)), Default::default());
        PythonRunRequest {
            code,
            mode,
            timeout,
            inputs,
        }
    }

    fn ok_body() -> Vec<u8> {
        let mut r = WireResponse::status_only(WireStatus::Ok, None);
        (r.output_set, r.output) = (true, Some(json!(1)));
        serde_json::to_vec(&r).unwrap()
    }

    fn ok() -> Answer {
        Answer::new(200).set_body_bytes(ok_body())
    }

    /// Answers `a` on `at`, `n` times at most (then 404).
    async fn on(s: &MockServer, at: &str, a: Answer, n: u64) {
        let mock = Mock::given(path(at)).respond_with(a);
        mock.up_to_n_times(n).mount(s).await
    }

    async fn hits(s: &MockServer) -> usize {
        s.received_requests().await.unwrap().len()
    }

    async fn fails(ex: &RemoteExecutor, r: PythonRunRequest) -> String {
        ex.run(r).await.unwrap_err().to_string()
    }

    #[test]
    fn the_endpoint_keeps_the_path_of_the_url() {
        for (base, run) in [
            ("https://h.example/py", "https://h.example/py/v1/run"),
            ("https://h.example/py/", "https://h.example/py/v1/run"),
            ("https://h.example", "https://h.example/v1/run"),
        ] {
            assert_eq!(endpoint(&Url::parse(base).unwrap(), "v1/run").as_str(), run);
        }
    }

    #[tokio::test]
    async fn sends_zstd_and_parses_a_zstd_answer() {
        let s = MockServer::start().await;
        let packed = zstd::bulk::compress(&ok_body(), 3).unwrap();
        let answer = Answer::new(200).insert_header("content-encoding", "zstd");
        let given = Mock::given(path("/py/v1/run")).and(header("content-encoding", "zstd"));
        let given = given.and(header("accept-encoding", "zstd"));
        let given = given.and(header_exists("x-colmena-request-id"));
        let given = given.respond_with(answer.set_body_bytes(packed));
        given.mount(&s).await;
        let ex = exec(&format!("{}/py", s.uri()), RemoteAuthConfig::None);
        assert_eq!(ex.run(req()).await.unwrap().output, Some(json!(1)));
    }

    /// One retry, never a fallback to running the code in process.
    #[tokio::test]
    async fn a_busy_service_is_retried_once() {
        let s = MockServer::start().await;
        on(&s, "/v1/run", Answer::new(503), 1).await;
        on(&s, "/v1/run", ok(), 1).await;
        let ex = exec(&s.uri(), RemoteAuthConfig::None);
        assert!(ex.run(req()).await.is_ok());
        let s = MockServer::start().await;
        on(&s, "/v1/run", Answer::new(503), 9).await;
        let e = fails(&exec(&s.uri(), RemoteAuthConfig::None), req()).await;
        let unavailable = "PythonExecutorError: the isolated Python executor is unavailable";
        assert!(e.starts_with(unavailable) && hits(&s).await == 2, "{e}");
    }

    /// A full service (429, or 503 with `Retry-After`) is waited for as it says
    /// (at most 2 s), with the same request id and one warning. A bare 503 (not
    /// ready, no usable slot) is sent again once, a 429 as long as it lasts.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_full_service_is_waited_for() {
        let s = MockServer::start().await;
        let full = Answer::new(429).insert_header("retry-after", "3600");
        for answer in [Answer::new(503), Answer::new(429), full, ok()] {
            on(&s, "/v1/run", answer, 1).await;
        }
        let (ex, started) = (exec(&s.uri(), RemoteAuthConfig::None), Instant::now());
        let rt = tokio::runtime::Handle::current();
        let run = || tokio::task::block_in_place(|| rt.block_on(ex.run(req())));
        let (r, log) = crate::dag_engine::log_policy::capture_warnings(run);
        assert!(r.is_ok() && started.elapsed() >= Duration::from_secs(2));
        assert!(log.matches("call retried").count() == 1, "{log}");
        let sent = s.received_requests().await.unwrap();
        let id = |i: usize| sent[i].headers.get("x-colmena-request-id");
        assert!(sent.len() == 4 && id(0).is_some() && (1..4).all(|i| id(i) == id(0)));
    }

    /// One deadline, the timeout plus the grace (4 s in unit tests), covers the
    /// credentials, every attempt and every wait. Waits stop once the code would
    /// no longer get its whole timeout: a call kept in line past the grace is
    /// unavailable, not the code's timeout.
    #[tokio::test]
    async fn one_deadline_covers_the_whole_call() {
        let s = MockServer::start().await;
        let full = |after| Answer::new(503).insert_header("retry-after", after);
        on(&s, "/v1/run", full("1"), 1).await;
        on(&s, "/v1/run", full("2"), 2).await;
        on(&s, "/v1/run", ok().set_delay(Duration::from_secs(2)), 9).await;
        let (ex, mut r) = (exec(&s.uri(), RemoteAuthConfig::None), req());
        r.timeout = Some(Duration::from_secs(2)); // 6 s in all: waits of 1 + 2 s fit, 2 s more do not
        let e = fails(&ex, r.clone()).await;
        assert!(e.ends_with("(HTTP 503)") && hits(&s).await == 3, "{e}");
        s.reset().await;
        on(&s, "/v1/run", full("1"), 1).await;
        let slow = ok().set_delay(Duration::from_millis(3500));
        on(&s, "/v1/run", slow, 9).await;
        r.timeout = Some(Duration::ZERO); // after a wait of 1 s, under 3 s left
        assert_eq!(ex.run(r.clone()).await, Err(PythonRunError::Timeout));
        let jwt = "h.eyJleHAiOjk5OTk5OTk5OTl9.s"; // {"exp":9999999999}
        let late = Answer::new(200).set_body_string(jwt);
        on(&s, "/identity", late.set_delay(Duration::from_secs(6)), 9).await;
        let audience = "a".to_string();
        let mut ex = exec(&s.uri(), RemoteAuthConfig::GcpIdToken { audience });
        let metadata = format!("{}/identity", s.uri());
        let source = IdTokenSource::with_endpoint("a".into(), metadata, ex.http.clone());
        (ex.id_tokens, r.timeout) = (Some(source), Some(Duration::ZERO));
        let e = fails(&ex, r).await; // the platform's failure, not the code's timeout
        assert!(e.ends_with("unavailable (credentials timed out)"), "{e}");
    }

    /// A call made while the runtime of an earlier call ends: a connection
    /// kept from that call would be served by that runtime and end with it.
    #[tokio::test]
    async fn a_call_outlives_the_runtime_of_an_earlier_one() {
        let s = MockServer::start().await;
        on(&s, "/v1/run", ok().set_delay(Duration::from_secs(1)), 2).await;
        let ex = std::sync::Arc::new(exec(&s.uri(), RemoteAuthConfig::None));
        let (first, rt) = (ex.clone(), tokio::runtime::Runtime::new().unwrap());
        let rt = std::thread::spawn(move || rt.block_on(first.run(req())).map(|_| rt));
        let rt = rt.join().unwrap().unwrap(); // still alive
        std::thread::spawn(move || drop((std::thread::sleep(RETRY_DELAY), rt)));
        assert_eq!(ex.run(req()).await.map(|r| r.output), Ok(Some(json!(1))));
    }

    #[tokio::test]
    async fn a_huge_timeout_does_not_overflow() {
        let s = MockServer::start().await;
        on(&s, "/v1/run", ok(), 9).await;
        let mut r = req();
        r.timeout = Some(Duration::MAX);
        assert!(exec(&s.uri(), RemoteAuthConfig::None).run(r).await.is_ok());
    }

    /// A connect timeout (here, a TLS handshake nobody answers) is not the code's timeout.
    #[tokio::test]
    async fn a_silent_service_is_unavailable() {
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("https://{}", silent.local_addr().unwrap());
        let started = Instant::now(); // tried again once, 250 ms later
        let e = fails(&exec(&url, RemoteAuthConfig::None), req()).await;
        assert!(e.ends_with("unavailable (connection failed)"), "{e}");
        assert!((RETRY_DELAY..Duration::from_secs(2)).contains(&started.elapsed()));
    }

    /// Once sent, a call may have run: a lost or cut answer is not sent again
    /// (a retry would find the port closed and say `connection failed`). One
    /// announced over the limit is refused before it is read.
    #[tokio::test]
    async fn a_lost_answer_is_not_retried() {
        let cut = b"HTTP/1.1 200 OK\r\ncontent-length: 9\r\n\r\n{";
        let huge = &b"HTTP/1.1 200 OK\r\ncontent-length: 99999999\r\n\r\n"[..];
        let lost = [(&b""[..], "request failed"), (cut, "response interrupted")];
        for (answer, why) in lost.into_iter().chain([(huge, "result exceeds")]) {
            let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", port.local_addr().unwrap());
            std::thread::spawn(move || {
                let mut c = port.accept().unwrap().0;
                let _ = c.read(&mut [0; 64]); // answer only once the request has started
                let _ = std::io::Write::write_all(&mut c, answer);
                let _ = c.shutdown(std::net::Shutdown::Write);
                let _ = c.read_to_end(&mut vec![]);
            });
            let e = fails(&exec(&url, RemoteAuthConfig::None), req()).await;
            assert!(e.contains(why), "{e}");
        }
    }

    /// Each is sent once; a redirect is not followed.
    #[tokio::test]
    async fn refusals_are_reported_without_a_retry() {
        for (status, text) in [
            (401, "rejected this caller's credentials"),
            (403, "rejected this caller's credentials"),
            (413, "exceeds what the isolated Python executor accepts"),
            (500, "answered HTTP 500"),
            (307, "answered HTTP 307"),
            (502, "unavailable (HTTP 502)"),
            (504, "unavailable (HTTP 504)"),
        ] {
            let s = MockServer::start().await;
            let answer = Answer::new(status).insert_header("location", "/x");
            on(&s, "/v1/run", answer, 9).await;
            on(&s, "/x", ok(), 9).await;
            let e = fails(&exec(&s.uri(), RemoteAuthConfig::None), req()).await;
            assert!(e.contains(text) && hits(&s).await == 1, "{status}: {e}");
        }
    }

    #[tokio::test]
    async fn the_token_file_is_read_for_each_call() {
        let s = MockServer::start().await;
        let bearer = header("authorization", "Bearer abc");
        Mock::given(bearer).respond_with(ok()).mount(&s).await;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t");
        let ex = exec(&s.uri(), RemoteAuthConfig::BearerFile(file.clone()));
        std::fs::write(&file, "abc\n").unwrap();
        assert!(ex.run(req()).await.is_ok());
        std::fs::write(&file, " \n").unwrap();
        let e = fails(&ex, req()).await;
        assert!(e.contains("token file is unreadable or empty") && hits(&s).await == 1);
        std::fs::write(&file, "ab\ncd").unwrap();
        let e = fails(&ex, req()).await;
        assert!(e.ends_with("does not hold a valid token"), "{e}");
        assert_eq!(hits(&s).await, 1);
    }

    #[tokio::test]
    async fn an_identity_token_is_sent() {
        let s = MockServer::start().await;
        let jwt = "h.eyJleHAiOjk5OTk5OTk5OTl9.s"; // {"exp":9999999999}
        on(&s, "/identity", Answer::new(200).set_body_string(jwt), 9).await;
        let bearer = header("authorization", format!("Bearer {jwt}").as_str());
        let given = Mock::given(path("/v1/run")).and(bearer);
        given.respond_with(ok()).mount(&s).await;
        let audience = "https://svc.example".to_string();
        let mut ex = exec(&s.uri(), RemoteAuthConfig::GcpIdToken { audience });
        let metadata = format!("{}/identity", s.uri());
        let source = IdTokenSource::with_endpoint("a".into(), metadata, ex.http.clone());
        ex.id_tokens = Some(source);
        assert!(ex.run(req()).await.is_ok());
    }

    /// Nothing is sent over the request limit or, compressed, over the wire cap.
    #[tokio::test]
    async fn oversized_input_never_leaves() {
        let s = MockServer::start().await;
        on(&s, "/v1/run", ok(), 9).await;
        let mut cfg = config(&s.uri(), RemoteAuthConfig::None);
        (cfg.max_request_bytes, cfg.max_wire_bytes) = (8 << 20, Some(1 << 20));
        let ex = RemoteExecutor::new(cfg, Duration::from_secs(30)).unwrap();
        let with = |big: String| {
            let mut r = req();
            r.inputs.insert("big".into(), json!(big));
            r
        };
        assert!(ex.run(with("x".repeat(4 << 20))).await.is_ok());
        // Random hex barely compresses: 4 MiB stays above the 1 MiB wire cap.
        let hex = |_| uuid::Uuid::new_v4().simple().to_string();
        let noisy = (0..(128 << 10)).map(hex).collect();
        let e = fails(&ex, with(noisy)).await;
        assert!(e.contains("transport limit of 1 MiB"), "{e}");
        let e = fails(&ex, with("x".repeat(9 << 20))).await;
        assert!(e.contains("input exceeds"), "{e}");
        assert_eq!(hits(&s).await, 1);
    }

    #[tokio::test]
    async fn an_answer_over_the_limit_is_refused() {
        let big = vec![b' '; 2 << 20];
        let packed = zstd::bulk::compress(&big, 3).unwrap();
        let zstd = Answer::new(200).insert_header("content-encoding", "zstd");
        for answer in [
            Answer::new(200).set_body_bytes(big),
            zstd.set_body_bytes(packed),
        ] {
            let s = MockServer::start().await;
            on(&s, "/v1/run", answer, 9).await;
            let e = fails(&exec(&s.uri(), RemoteAuthConfig::None), req()).await;
            assert!(e.contains("result exceeds"), "{e}");
        }
    }

    /// `/readyz`, then an empty `POST /v1/run`: past the token check it is 400
    /// and runs nothing; 401 or 403 ends the wait at once.
    #[tokio::test]
    async fn warm_waits_for_readyz_and_checks_the_credentials() {
        let s = MockServer::start().await;
        on(&s, "/readyz", Answer::new(503), 1).await;
        on(&s, "/readyz", Answer::new(200), 9).await;
        on(&s, "/v1/run", Answer::new(400), 1).await;
        let mut ex = exec(&s.uri(), RemoteAuthConfig::None);
        assert_eq!((ex.warm().await, hits(&s).await), (Ok(()), 3));
        let sent = s.received_requests().await.unwrap();
        assert!(sent.iter().all(|r| r.body.is_empty()));
        on(&s, "/v1/run", Answer::new(401), 9).await;
        ex.ready_within = Duration::from_secs(60);
        let waited = tokio::time::timeout(Duration::from_secs(5), ex.warm()).await;
        assert_eq!(waited, Ok(Err(REJECTED.to_string())));
        s.reset().await;
        on(&s, "/readyz", Answer::new(503), 1).await;
        on(&s, "/readyz", ok().set_delay(Duration::from_secs(9)), 9).await;
        ex.ready_within = Duration::from_millis(1500); // in all, a slow probe included
        let e = "PythonExecutorError: the isolated Python executor is not ready (HTTP 503)";
        let waited = tokio::time::timeout(Duration::from_secs(2), ex.warm()).await;
        assert_eq!(waited, Ok(Err(e.to_string())));
    }
}
