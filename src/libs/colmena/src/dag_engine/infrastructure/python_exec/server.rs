//! HTTP front for the subprocess executor (`python_executor serve`). A request
//! body is a wire request and a response body a wire response, as JSON,
//! optionally zstd-compressed (`Content-Encoding` / `Accept-Encoding`). Every
//! call goes through [`SubprocessExecutor::run_raw`], so the jail and the
//! output policy apply as they do in a host. Logs carry fields only: never
//! code, inputs, outputs, stdout, tokens or headers.

use super::child::EXIT_NOT_READY;
use super::config::SubprocessConfig;
use super::jail::JailSpec;
use super::protocol::{
    result_too_large_message, WireResponse, WireStatus, REFUSED_MESSAGE, WIRE_VERSION,
};
use super::selftest;
use super::subprocess::{RawFailure, SubprocessExecutor};
use crate::dag_engine::log_policy::T_PYTHON_EXEC;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::{body::Bytes, routing::get, routing::post, Router};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::Semaphore;

pub struct ServeArgs {
    pub listen: SocketAddr,
    pub subprocess: SubprocessConfig,
    /// The longest deadline a request may ask for.
    pub max_timeout: Duration,
    /// Every request must carry `Authorization: Bearer <the file's content>`;
    /// required unless `listen` is a loopback address or `allow_no_token`.
    pub token_file: Option<PathBuf>,
    pub allow_no_token: bool,
    /// `host:port` targets that must not accept a connection for the server
    /// to be ready.
    pub closed_egress: Vec<String>,
}

#[derive(Clone)]
pub struct AppState {
    pub exec: Arc<SubprocessExecutor>,
    pub token: Option<Arc<String>>,
    pub ready: Arc<AtomicBool>,
    pub max_timeout: Duration,
}

pub fn router(state: AppState) -> Router {
    let limit = state.exec.config().max_request_bytes;
    // Calls run `slots` at a time; as many more may upload and wait, so no
    // slot idles between calls, and the bodies held in memory stay bounded.
    let in_flight = Arc::new(Semaphore::new(2 * state.exec.config().slots));
    let gate = from_fn_with_state((state.clone(), in_flight), guard);
    let run = post(run_call).route_layer(gate.clone());
    let readyz = |State(st): State<AppState>| async move {
        match st.ready.load(SeqCst) && st.exec.has_usable_slot() {
            true => StatusCode::OK,
            false => StatusCode::SERVICE_UNAVAILABLE,
        }
    };
    let mut app = Router::new().route("/v1/run", run);
    // A call with run mounts (dark): only a server with a staging root, which is
    // read only with COLMENA_LARGE_TABULAR on, has the route, behind the same gate.
    if state.exec.config().staging_root.is_some() {
        let mounts = post(crate::tabular_run::serve::run_mounts).route_layer(gate);
        app = app.route("/v2/run", mounts);
    }
    app.route("/healthz", get(|| async { StatusCode::OK }))
        .route("/readyz", get(readyz))
        .layer(DefaultBodyLimit::max(limit))
        .with_state(state)
}

/// Whether the request carries the token. Digests are compared to the last
/// byte, so the time taken says nothing about the token, its length included.
fn authorized(headers: &HeaderMap, token: Option<&str>) -> bool {
    let Some(token) = token else { return true };
    let sent = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let Some(sent) = sent.and_then(|v| v.strip_prefix("Bearer ")) else {
        return false;
    };
    let (a, b) = (Sha256::digest(sent), Sha256::digest(token));
    a.iter().zip(b.iter()).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Refuses a request without the token (401), while not ready (503) or
/// beyond the in-flight bound (503 with `Retry-After`), before its body is
/// read.
async fn guard(
    State((st, in_flight)): State<(AppState, Arc<Semaphore>)>,
    req: Request,
    next: Next,
) -> Response {
    if !authorized(req.headers(), st.token.as_deref().map(String::as_str)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !st.ready.load(SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let Ok(_permit) = in_flight.try_acquire_owned() else {
        let retry = [(header::RETRY_AFTER, "1")];
        return (StatusCode::SERVICE_UNAVAILABLE, retry).into_response();
    };
    next.run(req).await
}

#[derive(serde::Deserialize)]
struct Peek {
    v: u32,
    timeout_ms: u64,
}

/// The deadline the request asks for, capped at `max`; 400 unless the body is
/// a wire request of this version.
fn deadline(raw: &[u8], max: Duration) -> Result<Duration, StatusCode> {
    match serde_json::from_slice::<Peek>(raw) {
        Ok(p) if p.v == WIRE_VERSION => Ok(Duration::from_millis(p.timeout_ms).min(max)),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

/// A call's result as a wire response, or an HTTP error without a body; with
/// its outcome for the log.
fn answer(
    result: Result<Vec<u8>, RawFailure>,
    max_response: usize,
) -> (StatusCode, Vec<u8>, &'static str) {
    let wire = |status, message: Option<String>, outcome| {
        let body = serde_json::to_vec(&WireResponse::status_only(status, message));
        (StatusCode::OK, body.unwrap_or_default(), outcome)
    };
    let refused = || Some(REFUSED_MESSAGE.to_string());
    match result {
        Ok(body) => (StatusCode::OK, body, "completed"),
        Err(RawFailure::Timeout) => wire(WireStatus::Timeout, None, "timeout"),
        Err(RawFailure::Crashed) => wire(WireStatus::Crashed, None, "crashed"),
        Err(RawFailure::ResponseTooLarge) => {
            let message = Some(result_too_large_message(max_response));
            wire(WireStatus::TooLarge, message, "result_too_large")
        }
        Err(RawFailure::Refused) => wire(WireStatus::PythonError, refused(), "output_refused"),
        Err(RawFailure::RequestTooLarge) => {
            (StatusCode::PAYLOAD_TOO_LARGE, vec![], "request_too_large")
        }
        Err(RawFailure::Unavailable(_)) => (StatusCode::SERVICE_UNAVAILABLE, vec![], "unavailable"),
    }
}

/// The body with its `Content-Encoding` undone; 413 once it exceeds `max`.
fn decode(encoding: Option<&str>, body: Bytes, max: usize) -> Result<Bytes, StatusCode> {
    let raw = match encoding {
        None | Some("identity") => body,
        Some("zstd") => {
            let unpack = zstd::stream::read::Decoder::new(&body[..]);
            let mut out = Vec::new();
            let read = unpack.and_then(|u| u.take(max as u64 + 1).read_to_end(&mut out));
            read.map_err(|_| StatusCode::BAD_REQUEST)?;
            out.into()
        }
        Some(_) => return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE),
    };
    match raw.len() > max {
        true => Err(StatusCode::PAYLOAD_TOO_LARGE),
        false => Ok(raw),
    }
}

/// The caller's correlation id for the log: at most 64 of `[A-Za-z0-9._:-]`.
fn request_id(value: Option<&str>) -> String {
    let allowed = |c: &char| c.is_ascii_alphanumeric() || "._:-".contains(*c);
    value
        .unwrap_or("-")
        .chars()
        .filter(allowed)
        .take(64)
        .collect()
}

async fn run_call(State(st): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let text = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let (wire_in_bytes, max) = (body.len(), st.exec.config().max_request_bytes);
    let encoding = text("content-encoding").map(str::to_string);
    let decoded = tokio::task::spawn_blocking(move || decode(encoding.as_deref(), body, max));
    let raw = match decoded
        .await
        .unwrap_or(Err(StatusCode::INTERNAL_SERVER_ERROR))
    {
        Ok(raw) => raw,
        Err(code) => return code.into_response(),
    };
    let timeout = match deadline(&raw, st.max_timeout) {
        Ok(timeout) => timeout,
        Err(code) => return code.into_response(),
    };
    let (started, max_response) = (Instant::now(), st.exec.config().max_response_bytes);
    let (code, out, outcome) = answer(st.exec.run_raw(timeout, &raw).await, max_response);
    let (in_bytes, out_bytes) = (raw.len(), out.len());
    let duration_ms = started.elapsed().as_millis() as u64;
    let request_id = request_id(text("x-colmena-request-id"));
    tracing::info!(target: T_PYTHON_EXEC, request_id, outcome, in_bytes, wire_in_bytes, out_bytes, duration_ms, "python serve run");
    let json = (header::CONTENT_TYPE, "application/json");
    let accept = text("accept-encoding").unwrap_or("");
    if out.is_empty() || !accept.split(',').any(|e| e.trim().starts_with("zstd")) {
        return (code, [json], out).into_response();
    }
    match tokio::task::spawn_blocking(move || zstd::bulk::compress(&out, 3)).await {
        Ok(Ok(packed)) => {
            (code, [json, (header::CONTENT_ENCODING, "zstd")], packed).into_response()
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// A `--require-closed-egress` target: `host:port`, the host a name or an
/// address (an IPv6 one in brackets), the port from 1 to 65535.
pub fn egress_target(s: &str) -> Result<String, String> {
    let (host, port) = s.rsplit_once(':').unwrap_or((s, ""));
    let name = |c: char| c.is_ascii_alphanumeric() || ".-_".contains(c);
    let v6 = host.strip_prefix('[').and_then(|h| h.strip_suffix(']'));
    let host_ok = match v6 {
        Some(v6) => v6.parse::<std::net::Ipv6Addr>().is_ok(),
        None => !host.is_empty() && host.chars().all(name),
    };
    match (host_ok, port.parse::<u16>()) {
        (true, Ok(port)) if port > 0 => Ok(s.to_string()),
        _ => Err("expected host:port (an IPv6 host in brackets, a port from 1 to 65535)".into()),
    }
}

/// Whether every one of `targets` is proven closed: it resolves, and each of
/// its addresses refuses a TCP connection or lets it time out in 2 s. A
/// target that does not resolve, or fails to connect otherwise, proves
/// nothing.
async fn egress_closed(targets: &[String]) -> bool {
    let wait = Duration::from_secs(2);
    for t in targets {
        let resolved = tokio::time::timeout(wait, tokio::net::lookup_host(t.as_str())).await;
        let addrs: Vec<SocketAddr> = match resolved {
            Ok(Ok(addrs)) => addrs.collect(),
            _ => Vec::new(),
        };
        if addrs.is_empty() {
            tracing::warn!(target: T_PYTHON_EXEC, addr = %t, "python serve egress target not resolved");
            return false;
        }
        for a in addrs {
            let error_kind =
                match tokio::time::timeout(wait, tokio::net::TcpStream::connect(a)).await {
                    Err(_) => continue,
                    Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => continue,
                    Ok(Ok(_)) => None,
                    Ok(Err(e)) => Some(e.kind()),
                };
            tracing::warn!(target: T_PYTHON_EXEC, addr = %t, error_kind = ?error_kind, "python serve egress target not proven closed");
            return false;
        }
    }
    true
}

/// Keeps `ready` true only while the template runs and no egress target
/// accepts a connection; checked every minute, every 5 s while not ready.
pub async fn readiness(
    exec: Arc<SubprocessExecutor>,
    targets: Vec<String>,
    ready: Arc<AtomicBool>,
) {
    loop {
        let template_ok = exec.warm().await.is_ok();
        let ok = template_ok && egress_closed(&targets).await;
        if ready.swap(ok, SeqCst) != ok {
            tracing::info!(target: T_PYTHON_EXEC, ready = ok, template_ok, "python serve readiness");
        }
        tokio::time::sleep(Duration::from_secs(if ok { 60 } else { 5 })).await;
    }
}

/// The token of `--token-file`; without one, only loopback is served unless
/// `allow_no_token`.
fn token(
    file: Option<&Path>,
    listen: SocketAddr,
    allow_no_token: bool,
) -> Result<Option<String>, String> {
    let Some(file) = file else {
        if allow_no_token && !listen.ip().is_loopback() {
            tracing::warn!(target: T_PYTHON_EXEC, addr = %listen, "python serve without a token on a non-loopback address");
        }
        return match allow_no_token || listen.ip().is_loopback() {
            true => Ok(None),
            false => Err(format!("{listen} is not a loopback address: set --token-file, or --allow-no-token to serve without one")),
        };
    };
    let text =
        std::fs::read_to_string(file).map_err(|e| format!("cannot read the token file: {e}"))?;
    match text.trim() {
        "" => Err("the token file is empty".into()),
        token => Ok(Some(token.to_string())),
    }
}

/// The shortest token accepted.
const MIN_TOKEN_BYTES: usize = 32;

/// The executor settings to serve with: `cfg` with the token file hidden in
/// the jail, by its canonical path (absolute, without `..`, through any
/// symlink), so the code the server runs cannot read it and the self-test
/// proves so. The token must be visible ASCII, [`MIN_TOKEN_BYTES`] or more.
fn executor_config(
    cfg: &SubprocessConfig,
    token: Option<&str>,
    file: Option<&Path>,
) -> Result<SubprocessConfig, String> {
    let mut cfg = cfg.clone();
    let (Some(token), Some(file)) = (token, file) else {
        return Ok(cfg);
    };
    if token.len() < MIN_TOKEN_BYTES || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(format!(
            "the token must be {MIN_TOKEN_BYTES} or more visible ASCII characters"
        ));
    }
    let path = std::fs::canonicalize(file);
    let path = path.map_err(|e| format!("cannot resolve the token file: {e}"))?;
    cfg.hide_paths.push(path);
    Ok(cfg)
}

/// `python_executor serve`: checks the configuration, proves the jail once
/// (each template start proves it again) and serves until SIGTERM or Ctrl-C.
/// Exits 2 for a bad configuration, [`EXIT_NOT_READY`] when the jail does not
/// hold, 1 when serving fails.
pub fn run(args: ServeArgs) -> i32 {
    let Err((code, reason)) = start(args) else {
        return 0;
    };
    tracing::error!(target: T_PYTHON_EXEC, reason = %reason, "python serve stopped");
    code
}

fn start(args: ServeArgs) -> Result<(), (i32, String)> {
    let token = token(args.token_file.as_deref(), args.listen, args.allow_no_token);
    let token = token.map_err(|e| (2, e))?;
    let cfg = &args.subprocess;
    let file = args.token_file.as_deref();
    let cfg = executor_config(cfg, token.as_deref(), file).map_err(|e| (2, e))?;
    let spec = JailSpec {
        uid_base: cfg.uid_base,
        tmp_mb: cfg.tmp_mb,
        hide_paths: cfg.hide_paths.clone(),
        staging_root: cfg.staging_root.clone(),
    };
    // First, while this is the only thread: the self-test forks.
    if let Err(checks) = selftest::run(&spec) {
        for c in checks.iter().filter(|c| !c.ok) {
            tracing::error!(target: T_PYTHON_EXEC, layer = %c.layer, reason = %c.reason, errno = c.errno, "python jail self-test failed");
        }
        return Err((EXIT_NOT_READY, "the jail self-test failed".into()));
    }
    let exec =
        SubprocessExecutor::new_for_serving(cfg.clone(), args.max_timeout).map_err(|e| (2, e.0))?;
    let (exec, token) = (Arc::new(exec), token.map(Arc::new));
    let (ready, max_timeout) = (Arc::default(), args.max_timeout);
    let state = AppState {
        exec,
        token,
        ready,
        max_timeout,
    };
    let rt = tokio::runtime::Runtime::new().map_err(|e| (1, e.to_string()))?;
    let served = rt.block_on(serve(args.listen, args.closed_egress, state));
    served.map_err(|e| (1, e))
}

async fn serve(listen: SocketAddr, egress: Vec<String>, state: AppState) -> Result<(), String> {
    // SIGTERM is how a container platform stops the server.
    let mut term = signal(SignalKind::terminate()).map_err(|e| e.to_string())?;
    tokio::spawn(readiness(state.exec.clone(), egress, state.ready.clone()));
    let listener = tokio::net::TcpListener::bind(listen).await;
    let listener = listener.map_err(|e| format!("cannot listen on {listen}: {e}"))?;
    tracing::info!(target: T_PYTHON_EXEC, addr = %listen, "python serve listening");
    let served = axum::serve(listener, router(state)).with_graceful_shutdown(async move {
        tokio::select! { _ = term.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
    });
    served.await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;

    #[test]
    fn only_the_exact_bearer_token_is_authorized() {
        let bearer = |v: &str| HeaderMap::from_iter([(header::AUTHORIZATION, v.parse().unwrap())]);
        assert!(authorized(&HeaderMap::new(), None));
        assert!(authorized(&bearer("Bearer s3cret"), Some("s3cret")));
        assert!(!authorized(&HeaderMap::new(), Some("s3cret")));
        for wrong in "Bearer s3cre|Bearer s3cret2|s3cret|Basic s3cret|Bearer ".split('|') {
            assert!(!authorized(&bearer(wrong), Some("s3cret")), "{wrong}");
        }
    }

    #[test]
    fn the_token_is_long_enough_and_its_file_hidden_by_its_canonical_path() {
        let cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("token");
        std::fs::write(&file, "t").unwrap();
        std::os::unix::fs::symlink(&file, dir.path().join("link")).unwrap();
        let link = Some(dir.path().join("link"));
        let long = "t".repeat(MIN_TOKEN_BYTES);
        assert_eq!(executor_config(&cfg, None, None), Ok(cfg.clone()));
        let hidden = executor_config(&cfg, Some(&long), link.as_deref())
            .unwrap()
            .hide_paths;
        assert_eq!(hidden, [std::fs::canonicalize(&file).unwrap()]);
        let short = &long[1..];
        assert!(executor_config(&cfg, Some(short), link.as_deref()).is_err());
        assert!(executor_config(&cfg, Some(&format!("{short}\u{e9}")), link.as_deref()).is_err());
    }

    /// A router whose template cannot start: nothing in it runs Python.
    fn app(ready: bool, token: Option<&str>) -> Router {
        let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
        (cfg.bin, cfg.uid_base, cfg.max_request_bytes) = ("/nonexistent".into(), 40000, 1024);
        cfg.slots = 1;
        let exec = Arc::new(SubprocessExecutor::unchecked(cfg, Duration::from_secs(5)).unwrap());
        let (token, ready) = (token.map(|t| Arc::new(t.into())), Arc::new(ready.into()));
        let max_timeout = Duration::from_secs(5);
        router(AppState {
            exec,
            token,
            ready,
            max_timeout,
        })
    }

    async fn status(app: Router, headers: &[(&str, &str)], body: Vec<u8>) -> StatusCode {
        let mut req = axum::http::Request::post("/v1/run");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let req = req.body(Body::from(body)).unwrap();
        app.oneshot(req).await.unwrap().status()
    }

    /// Each check before a call, in its order (the token before the body is
    /// read); a call that passes them all fails as unavailable, since the
    /// template cannot start, and runs nowhere else.
    #[tokio::test]
    async fn a_request_is_checked_before_any_call() {
        use StatusCode as S;
        let wire =
            |v| format!(r#"{{"v":{v},"code":"","mode":"none","timeout_ms":1,"inputs":{{}}}}"#);
        let (wire, big) = (|v| wire(v).into_bytes(), || vec![b' '; 1025]);
        let zipped = |b: Vec<u8>| zstd::bulk::compress(&b, 3).unwrap();
        let (t, auth) = (Some("s3cret"), &[("authorization", "Bearer s3cret")][..]);
        let (gzip, zstd) = (
            &[("content-encoding", "gzip")][..],
            &[("content-encoding", "zstd")][..],
        );
        for (ready, token, headers, body, expected) in [
            (true, t, &[][..], big(), S::UNAUTHORIZED),
            (false, t, auth, wire(2), S::SERVICE_UNAVAILABLE),
            (true, t, auth, big(), S::PAYLOAD_TOO_LARGE),
            (true, None, zstd, zipped(big()), S::PAYLOAD_TOO_LARGE),
            (true, None, gzip, wire(1), S::UNSUPPORTED_MEDIA_TYPE),
            (true, None, zstd, b"not zstd".to_vec(), S::BAD_REQUEST),
            (true, t, auth, wire(2), S::BAD_REQUEST),
            (true, None, &[][..], b"{}".to_vec(), S::BAD_REQUEST),
            (true, None, &[][..], wire(1), S::SERVICE_UNAVAILABLE),
            (true, None, zstd, zipped(wire(1)), S::SERVICE_UNAVAILABLE),
        ] {
            let text = String::from_utf8_lossy(&body[..body.len().min(20)]).into_owned();
            let got = status(app(ready, token), headers, body).await;
            assert_eq!(got, expected, "{headers:?} {text:?}");
        }
        let probe = |path| axum::http::Request::get(path).body(Body::empty()).unwrap();
        let readyz = app(false, None).oneshot(probe("/readyz")).await.unwrap();
        assert_eq!(readyz.status(), S::SERVICE_UNAVAILABLE);
        let healthz = app(false, None).oneshot(probe("/healthz")).await.unwrap();
        assert_eq!(healthz.status(), S::OK);
    }

    #[test]
    fn the_deadline_is_capped_at_the_maximum() {
        let max = Duration::from_secs(60);
        let asked = |ms: u64| deadline(format!(r#"{{"v":1,"timeout_ms":{ms}}}"#).as_bytes(), max);
        assert_eq!(asked(2500), Ok(Duration::from_millis(2500)));
        assert_eq!(asked(90_000), Ok(max));
        let other_version = deadline(br#"{"v":2,"timeout_ms":1}"#, max);
        assert_eq!(other_version, Err(StatusCode::BAD_REQUEST));
    }

    #[test]
    fn a_token_is_required_beyond_loopback_and_is_never_empty() {
        let (lo, any) = ("127.0.0.1:80".parse().unwrap(), "[::]:80".parse().unwrap());
        assert_eq!(token(None, lo, false), Ok(None));
        assert_eq!(token(None, any, true), Ok(None));
        assert!(token(None, any, false).is_err());
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("token");
        std::fs::write(&file, "  s3cret\n").unwrap();
        assert_eq!(token(Some(&file), any, false), Ok(Some("s3cret".into())));
        std::fs::write(&file, " \n").unwrap();
        assert!(token(Some(&file), lo, false).is_err());
        assert!(token(Some(&dir.path().join("missing")), lo, false).is_err());
    }

    #[test]
    fn the_logged_request_id_keeps_only_name_characters() {
        assert_eq!(request_id(None), "-");
        assert_eq!(request_id(Some("run-7:a.b_c")), "run-7:a.b_c");
        assert_eq!(request_id(Some("a b\"c=d")), "abcd");
        assert_eq!(request_id(Some(&"x".repeat(100))).len(), 64);
    }

    #[test]
    fn a_body_is_decoded_up_to_the_limit() {
        let (fits, max) = (Bytes::from(vec![b' '; 1024]), 1024);
        let packed = Bytes::from(zstd::bulk::compress(&fits, 3).unwrap());
        assert_eq!(decode(None, fits.clone(), max), Ok(fits.clone()));
        assert_eq!(
            decode(Some("identity"), fits.clone(), max),
            Ok(fits.clone())
        );
        assert_eq!(decode(Some("zstd"), packed.clone(), max), Ok(fits));
        assert_eq!(
            decode(Some("zstd"), packed, max - 1),
            Err(StatusCode::PAYLOAD_TOO_LARGE)
        );
    }

    #[tokio::test]
    async fn egress_is_closed_only_when_no_target_accepts_a_connection() {
        let open = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed = listener.local_addr().unwrap().to_string();
        drop(listener);
        let open = open.local_addr().unwrap().to_string();
        assert!(egress_closed(&[]).await);
        assert!(egress_closed(std::slice::from_ref(&closed)).await);
        assert!(!egress_closed(&[closed.clone(), open]).await);
        // A target that does not resolve proves nothing.
        assert!(!egress_closed(&[closed, "unresolved.invalid:80".into()]).await);
    }

    #[test]
    fn an_egress_target_is_a_host_and_a_port() {
        for ok in ["127.0.0.1:80", "[::1]:443", "example.com:443"] {
            assert_eq!(egress_target(ok).as_deref(), Ok(ok), "{ok}");
        }
        for bad in [
            "127.0.0.1",
            "127.0.0.1:0",
            ":80",
            "host:port",
            "::1:80",
            "a:70000",
            "a b:80",
        ] {
            assert!(egress_target(bad).is_err(), "{bad}");
        }
    }

    /// At most twice as many requests as slots are in flight; one more is
    /// turned away before its body is read, and asked to come back.
    #[tokio::test]
    async fn requests_beyond_the_in_flight_bound_are_turned_away() {
        let app = app(true, None);
        let stalled = || {
            let body =
                Body::from_stream(futures::stream::pending::<Result<Bytes, std::io::Error>>());
            let req = axum::http::Request::post("/v1/run").body(body).unwrap();
            tokio::spawn(app.clone().oneshot(req))
        };
        let held = [stalled(), stalled()];
        tokio::time::sleep(Duration::from_millis(100)).await;
        let req = axum::http::Request::post("/v1/run")
            .body(Body::from("{}"))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(resp.headers()[header::RETRY_AFTER], "1");
        for h in held {
            h.abort();
            let _ = h.await;
        }
        assert_eq!(
            status(app, &[], b"{}".to_vec()).await,
            StatusCode::BAD_REQUEST
        );
    }
}
