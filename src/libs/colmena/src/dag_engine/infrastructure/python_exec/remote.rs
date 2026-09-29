//! Sends each Python call to a `python_executor serve` endpoint, with zstd
//! bodies. One retry, only when the call did not run: a connection error, 429,
//! or 503 (`serve` answers it before running a call). It never falls back to
//! running the code in process. Requests go only to the configured URL: proxy
//! variables are ignored and redirects are not followed.

use super::config::{ExecutorConfigError, RemoteAuthConfig, RemoteConfig};
use super::id_token::IdTokenSource;
use super::protocol::{input_too_large_message, result_too_large_message as too_large};
use super::protocol::{WireRequest, WireResponse};
use crate::dag_engine::domain::python_executor::PythonRunResult;
use crate::dag_engine::domain::python_executor::{ExecutorKind, PythonExecutor};
use crate::dag_engine::domain::python_executor::{PythonRunError, PythonRunRequest};
use crate::dag_engine::log_policy::T_PYTHON_EXEC;
use bytes::Bytes;
use reqwest::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE};
use reqwest::{RequestBuilder, Url};
use std::io::Read;
use std::time::Duration;

/// Time allowed beyond a call's deadline for sending and receiving it.
const TRANSFER_GRACE: Duration = Duration::from_secs(30);
const RETRY_DELAY: Duration = Duration::from_millis(250);
/// Short in unit tests, which wait for it to expire.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(if cfg!(test) { 200 } else { 10_000 });
const REJECTED: &str =
    "PythonExecutorError: the isolated Python executor rejected this caller's credentials";
const MALFORMED: &str = "PythonExecutorError: malformed response from the isolated Python executor";

pub struct RemoteExecutor {
    cfg: RemoteConfig,
    max_timeout: Duration,
    http: reqwest::Client,
    id_tokens: Option<IdTokenSource>,
}

enum Attempt {
    Done(Result<PythonRunResult, PythonRunError>),
    Retry(String),
}

fn internal(m: impl Into<String>) -> PythonRunError {
    PythonRunError::Internal(m.into())
}

fn unavailable(why: impl std::fmt::Display) -> PythonRunError {
    let m = format!("PythonExecutorError: the isolated Python executor is unavailable ({why})");
    internal(m)
}

/// `path` under `base`, keeping the path of `base` whether or not it ends in `/`.
fn endpoint(base: &Url, path: &str) -> Url {
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
    /// Refuses a URL with user info: credentials travel only as `auth` says.
    pub fn new(cfg: RemoteConfig, max_timeout: Duration) -> Result<Self, ExecutorConfigError> {
        if !cfg.url.username().is_empty() || cfg.url.password().is_some() {
            let e = "the Python executor URL must not carry user info";
            return Err(ExecutorConfigError(e.into()));
        }
        let http = reqwest::Client::builder()
            .use_rustls_tls()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
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
            http,
            id_tokens,
        })
    }

    /// `rb` with this caller's credentials. The token file is read on each
    /// call, so a rotated token is picked up.
    async fn authorized(&self, rb: RequestBuilder) -> Result<RequestBuilder, PythonRunError> {
        let token = match (&self.cfg.auth, &self.id_tokens) {
            (RemoteAuthConfig::BearerFile(p), _) => {
                let text = tokio::fs::read_to_string(p).await.unwrap_or_default();
                let e =
                    "PythonExecutorError: the Python executor token file is unreadable or empty";
                Some(text.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .ok_or(e.to_string())
            }
            (_, Some(source)) => source
                .token()
                .await
                .map_err(|e| format!("PythonExecutorError: cannot obtain an identity token ({e})")),
            _ => return Ok(rb),
        };
        Ok(rb.bearer_auth(token.map_err(PythonRunError::Internal)?))
    }

    async fn post_once(&self, body: Bytes, timeout: Duration, request_id: &str) -> Attempt {
        let rb = self.http.post(endpoint(&self.cfg.url, "v1/run"));
        let rb = rb.header(CONTENT_TYPE, "application/json").body(body);
        let rb = rb
            .header(CONTENT_ENCODING, "zstd")
            .header(ACCEPT_ENCODING, "zstd");
        let rb = rb.header("x-colmena-request-id", request_id);
        let rb = rb.timeout(timeout.saturating_add(TRANSFER_GRACE));
        let rb = match self.authorized(rb).await {
            Ok(rb) => rb,
            Err(e) => return Attempt::Done(Err(e)),
        };
        let (max, limit) = (self.cfg.max_response_bytes, self.cfg.max_request_bytes);
        let mut resp = match rb.send().await.map(|r| (r.status().as_u16(), r)) {
            Ok((200, r)) => r,
            Ok((401 | 403, _)) => return Attempt::Done(Err(internal(REJECTED))),
            Ok((413, _)) => {
                return Attempt::Done(Err(PythonRunError::Python(input_too_large_message(limit))))
            }
            Ok((s @ (429 | 503), _)) => return Attempt::Retry(format!("HTTP {s}")),
            Ok((s @ (502 | 504), _)) => {
                return Attempt::Done(Err(unavailable(format!("HTTP {s}"))))
            }
            Ok((s, _)) => {
                let e =
                    format!("PythonExecutorError: the isolated Python executor answered HTTP {s}");
                return Attempt::Done(Err(internal(e)));
            }
            // Before `is_timeout`: a connect timeout is both, and the call did not run.
            Err(e) if e.is_connect() => return Attempt::Retry("connection failed".into()),
            Err(e) if e.is_timeout() => return Attempt::Done(Err(PythonRunError::Timeout)),
            Err(_) => return Attempt::Done(Err(unavailable("request failed"))),
        };
        let zstd_body = resp.headers().get(CONTENT_ENCODING).map(|v| v == "zstd");
        let mut body = Vec::new();
        loop {
            match resp.chunk().await {
                Ok(Some(c)) if body.len() + c.len() > max => {
                    return Attempt::Done(Err(PythonRunError::Python(too_large(max))))
                }
                Ok(Some(c)) => body.extend_from_slice(&c),
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
        let request_id = uuid::Uuid::new_v4().to_string();
        let why = match self.post_once(body.clone(), timeout, &request_id).await {
            Attempt::Done(r) => return r,
            Attempt::Retry(why) => why,
        };
        tracing::warn!(target: T_PYTHON_EXEC, request_id, reason = %why, "python remote call retried");
        tokio::time::sleep(RETRY_DELAY).await;
        match self.post_once(body, timeout, &request_id).await {
            Attempt::Done(r) => r,
            Attempt::Retry(why) => Err(unavailable(why)),
        }
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

    #[test]
    fn a_url_with_user_info_is_refused() {
        let cfg = config("http://u:p@127.0.0.1:1", RemoteAuthConfig::None);
        assert!(RemoteExecutor::new(cfg, Duration::from_secs(1)).is_err());
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
        let e = fails(&exec(&url, RemoteAuthConfig::None), req()).await;
        assert!(e.ends_with("unavailable (connection failed)"), "{e}");
    }

    /// Once sent, a call may have run: a lost or cut answer is not sent again
    /// (a retry would find the port closed and say `connection failed`).
    #[tokio::test]
    async fn a_lost_answer_is_not_retried() {
        let cut = b"HTTP/1.1 200 OK\r\ncontent-length: 9\r\n\r\n{";
        for (answer, why) in [(&b""[..], "request failed"), (cut, "response interrupted")] {
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
            assert!(e.ends_with(&format!("unavailable ({why})")), "{e}");
        }
    }

    /// Each is sent once; a redirect is not followed.
    #[tokio::test]
    async fn refusals_are_reported_without_a_retry() {
        for (status, text) in [
            (401, "rejected this caller's credentials"),
            (403, "rejected this caller's credentials"),
            (413, "the input exceeds the Python executor limit of 1 MiB"),
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
}
