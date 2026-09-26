//! The one client that fetches a URL whose bytes a node reads: an attachment's
//! URL (`files[].url`: both resolution paths of `llm_call`, byte persistence,
//! the auto-summary and the 24 h re-upload), `image_edit`'s http(s)
//! `source_url`/`mask_url`, `http_request`'s multipart URL parts and
//! `api_explorer`'s spec URL. http(s) only; it dials only global unicast
//! addresses (the MCP client's rule), checked inside the DNS resolution the
//! socket uses and, for an IP-literal host, on the URL and on every redirect
//! hop; no proxy; connect 10 s, whole request 600 s; a byte cap. No
//! `Authorization` header: a signed URL carries its signature in the query;
//! the only headers a caller adds are conditional-GET ones.
//!
//! [`DialGuard`] is that address rule on its own: `http_request` applies it
//! to a destination that comes from data.

use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use hyper::client::connect::dns::Name;
use reqwest::dns::{Addrs, Resolve, Resolving};
use reqwest::header::{HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH};
use reqwest::{redirect, Client, ClientBuilder, Url};

use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::mcp::allowlist::{
    is_global_unicast, private_block_from,
};
use crate::llm::domain::{BoxedByteStream, LlmError, SignedUrlFetcher};

/// `1` or `true` lets attachment fetches dial non-public addresses (local
/// development). A production deploy must never set it.
pub const ALLOW_PRIVATE_ENV_VAR: &str = "COLMENA_ATTACHMENT_ALLOW_PRIVATE_HOSTS";
/// Byte cap of one fetch, a positive integer; else [`DEFAULT_MAX_BYTES`].
pub const MAX_BYTES_ENV_VAR: &str = "COLMENA_ATTACHMENT_MAX_BYTES";
/// 100 MiB: the host application caps an uploaded file at 100 MB.
pub const DEFAULT_MAX_BYTES: u64 = 100 * 1024 * 1024;
/// URLs one request may visit, the first included: at most 4 redirects.
const MAX_URLS: usize = 5;

/// Whether an address may be dialled.
pub(crate) type Dialable = fn(IpAddr) -> bool;

fn any_ip(_: IpAddr) -> bool {
    true
}

/// A refusal, found again by type in the error chain; it never names the address.
#[derive(Debug, thiserror::Error)]
#[error("destination is not a public address")]
pub(crate) struct DialRefused;

fn refused(reason: impl ToString) -> LlmError {
    LlmError::AttachmentUrlRefused {
        reason: reason.to_string(),
    }
}

/// Where a client may connect: addresses `ok` accepts, and any address of
/// `exempt`, a host the author listed.
#[derive(Clone)]
pub(crate) struct DialGuard {
    ok: Dialable,
    exempt: Option<Arc<str>>,
}

impl DialGuard {
    pub(crate) fn new(ok: Dialable, exempt: Option<&str>) -> Self {
        let exempt = exempt.map(Arc::from);
        Self { ok, exempt }
    }

    /// Whether `url`'s host is a refused IP literal; a name is the resolver's call.
    pub(crate) fn refuses(&self, url: &Url) -> bool {
        self.exempt.as_deref() != url.host_str() && literal_refused(url, self.ok)
    }

    /// `builder` resolving through the guard, with no proxy (a proxy
    /// resolves the name itself).
    pub(crate) fn install(&self, builder: ClientBuilder) -> ClientBuilder {
        builder
            .dns_resolver(Arc::new(GuardedResolver(self.clone())))
            .no_proxy()
    }
}

/// Every answer dialable, or none is used (as `filter_dialable`).
struct GuardedResolver(DialGuard);

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let exempt = self.0.exempt.as_deref() == Some(name.as_str());
        let ok = if exempt { any_ip } else { self.0.ok };
        Box::pin(resolve_checked(name.as_str().to_string(), ok))
    }
}

async fn resolve_checked(
    host: String,
    ok: Dialable,
) -> Result<Addrs, Box<dyn Error + Send + Sync>> {
    let ips: Vec<IpAddr> = tokio::net::lookup_host((host.as_str(), 0))
        .await?
        .map(|a| a.ip())
        .collect();
    if ips.is_empty() {
        return Err("resolved to no address".into());
    }
    if !ips.iter().all(|ip| ok(*ip)) {
        tracing::warn!(target: "colmena::egress", event = "egress.dial_refused", host = %host,
            "refused to dial a non-public address");
        return Err(Box::new(DialRefused));
    }
    Ok(Box::new(ips.into_iter().map(|ip| SocketAddr::new(ip, 0))))
}

/// An IP-literal host reaches no resolver: it is checked on the URL.
fn literal_refused(url: &Url, ok: Dialable) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(v4)) => !ok(IpAddr::V4(v4)),
        Some(url::Host::Ipv6(v6)) => !ok(IpAddr::V6(v6)),
        _ => false,
    }
}

pub(crate) fn is_dial_refused(e: &reqwest::Error) -> bool {
    std::iter::successors(Some(e as &(dyn Error + 'static)), |&e| e.source())
        .any(|e| e.is::<DialRefused>())
}

fn guarded_client(ok: Dialable) -> Client {
    let guard = DialGuard::new(ok, None);
    guard
        .install(crate::shared::http_client::builder())
        .redirect(redirect::Policy::custom(move |hop| {
            if hop.previous().len() >= MAX_URLS {
                hop.error("too many redirects")
            } else if guard.refuses(hop.url()) {
                hop.error(DialRefused)
            } else {
                hop.follow()
            }
        }))
        .connect_timeout(Duration::from_secs(10))
        // 600 s: generous for files up to ~500 MB on slow connections.
        .timeout(Duration::from_secs(600))
        // With a contact: some hosts refuse an anonymous User-Agent.
        .user_agent(concat!(
            "colmena/",
            env!("CARGO_PKG_VERSION"),
            " (+",
            env!("CARGO_PKG_REPOSITORY"),
            ")"
        ))
        .build()
        .expect("the attachment fetch client should build")
}

/// Chunks pass until `max` bytes; past it, one error and the end of the stream.
fn cap(
    seen: &mut Option<u64>,
    chunk: std::io::Result<Bytes>,
    max: u64,
) -> Option<std::io::Result<Bytes>> {
    let total = seen.as_mut()?;
    let Ok(bytes) = chunk else { return Some(chunk) };
    *total += bytes.len() as u64;
    if *total > max {
        *seen = None;
        let e = LlmError::AttachmentTooLarge { limit: max };
        return Some(Err(std::io::Error::other(e)));
    }
    Some(Ok(bytes))
}

/// A 2xx response: its headers, and its body under the byte cap.
pub struct Fetched {
    pub headers: HeaderMap,
    pub body: BoxedByteStream,
}

/// The guarded HTTP client (see the module doc).
#[derive(Clone)]
pub struct SignedUrlDownloader {
    client: Client,
    dialable: Dialable,
    max_bytes: u64,
    timeout: Option<Duration>,
}

impl SignedUrlDownloader {
    /// Public addresses only unless [`ALLOW_PRIVATE_ENV_VAR`] is set; byte cap
    /// from [`MAX_BYTES_ENV_VAR`]. Both are read once per process, and every
    /// instance shares one client (one connection pool).
    pub fn new() -> Self {
        static SHARED: OnceLock<(Dialable, u64, Client)> = OnceLock::new();
        let (dialable, max, client) = SHARED.get_or_init(|| {
            let max = std::env::var(MAX_BYTES_ENV_VAR).ok();
            let max = max.and_then(|v| v.trim().parse().ok());
            let max = max.filter(|n| *n > 0).unwrap_or(DEFAULT_MAX_BYTES);
            (process_dialable(), max, guarded_client(process_dialable()))
        });
        Self::from_parts(client.clone(), *dialable, *max)
    }

    /// This client's address rule, `exempt` aside (see [`DialGuard`]).
    pub(crate) fn guard(&self, exempt: Option<&str>) -> DialGuard {
        DialGuard::new(self.dialable, exempt)
    }

    #[cfg(test)]
    pub(crate) fn with_policy(dialable: Dialable, max_bytes: u64) -> Self {
        Self::from_parts(guarded_client(dialable), dialable, max_bytes)
    }

    /// Public addresses only, whatever the environment says.
    #[cfg(test)]
    pub(crate) fn public_only() -> Self {
        Self::with_policy(is_global_unicast, DEFAULT_MAX_BYTES)
    }

    fn from_parts(client: Client, dialable: Dialable, max_bytes: u64) -> Self {
        Self {
            client,
            dialable,
            max_bytes,
            timeout: None,
        }
    }

    /// The same client with a lower byte cap (`max_bytes` never raises it).
    pub fn capped_at(mut self, max_bytes: u64) -> Self {
        self.max_bytes = self.max_bytes.min(max_bytes);
        self
    }

    /// The same client with a whole-request deadline of `total`, at most 600 s.
    pub fn with_timeout(mut self, total: Duration) -> Self {
        self.timeout = Some(total.min(Duration::from_secs(600)));
        self
    }

    /// For tests against a loopback server: every address is dialable.
    #[cfg(test)]
    pub(crate) fn allowing_private_hosts() -> Self {
        Self::with_policy(any_ip, DEFAULT_MAX_BYTES)
    }

    /// Streams the response body of an attachment URL (see [`Self::fetch`]).
    pub async fn stream(&self, url: &str) -> Result<BoxedByteStream, LlmError> {
        Ok(self.fetch(url).await?.body)
    }

    /// GETs `url` and returns the response headers and its streamed body.
    ///
    /// Dropping the returned stream early aborts the underlying request
    /// and releases the HTTP connection. The downloader does not retry —
    /// retry policy belongs in the use-case layer.
    ///
    /// # Errors
    /// - [`LlmError::AttachmentUrlRefused`]: not http(s), or not a public
    ///   address (nothing is dialled).
    /// - [`LlmError::AttachmentTooLarge`]: `Content-Length` past the cap; a
    ///   body that grows past it ends the stream with this error.
    /// - [`LlmError::NetworkError`] on transport failure (DNS, TCP, TLS, timeout).
    /// - [`LlmError::SignedUrlFetchFailed`] on any non-2xx HTTP status.
    pub async fn fetch(&self, url: &str) -> Result<Fetched, LlmError> {
        self.fetch_conditional(url, None, None).await
    }

    /// [`Self::fetch`] as a conditional GET: `If-None-Match` and
    /// `If-Modified-Since` are the only headers a caller adds.
    pub async fn fetch_conditional(
        &self,
        url: &str,
        if_none_match: Option<&str>,
        if_modified_since: Option<&str>,
    ) -> Result<Fetched, LlmError> {
        let parsed = Url::parse(url).map_err(|_| refused("not a valid URL"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(refused("only http and https URLs are fetched"));
        }
        if literal_refused(&parsed, self.dialable) {
            return Err(refused(DialRefused));
        }
        let mut request = self.client.get(parsed);
        let conditional = [
            (IF_NONE_MATCH, if_none_match),
            (IF_MODIFIED_SINCE, if_modified_since),
        ];
        for (name, value) in conditional {
            if let Some(v) = value.and_then(|v| HeaderValue::from_str(v).ok()) {
                request = request.header(name, v);
            }
        }
        if let Some(total) = self.timeout {
            request = request.timeout(total);
        }
        let response = match request.send().await {
            Ok(r) => r,
            Err(e) if is_dial_refused(&e) => return Err(refused(DialRefused)),
            Err(e) => {
                return Err(LlmError::NetworkError {
                    message: format!("signed URL fetch failed: {}", e.without_url()),
                })
            }
        };

        let status = response.status();
        if !status.is_success() {
            return Err(LlmError::SignedUrlFetchFailed {
                status: status.as_u16(),
            });
        }
        let max = self.max_bytes;
        if response.content_length().is_some_and(|n| n > max) {
            return Err(LlmError::AttachmentTooLarge { limit: max });
        }
        let headers = response.headers().clone();
        let body = response.bytes_stream().map_err(std::io::Error::other);
        let body = Box::pin(body.scan(Some(0), move |seen, c| {
            futures::future::ready(cap(seen, c, max))
        }));
        Ok(Fetched { headers, body })
    }
}

/// Public addresses only unless [`ALLOW_PRIVATE_ENV_VAR`] is set; read once
/// per process.
pub(crate) fn process_dialable() -> Dialable {
    static RULE: OnceLock<Dialable> = OnceLock::new();
    *RULE.get_or_init(|| {
        let raw = std::env::var(ALLOW_PRIVATE_ENV_VAR).ok();
        if private_block_from(raw.as_deref()) {
            is_global_unicast
        } else {
            any_ip
        }
    })
}

impl Default for SignedUrlDownloader {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SignedUrlFetcher for SignedUrlDownloader {
    async fn stream(&self, url: &str) -> Result<BoxedByteStream, LlmError> {
        SignedUrlDownloader::stream(self, url).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn stream_returns_body_chunks_on_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/file.pdf"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello world"))
            .mount(&server)
            .await;

        let downloader = SignedUrlDownloader::allowing_private_hosts();
        let url = format!("{}/file.pdf?sig=x", server.uri());
        let mut stream = downloader.stream(&url).await.unwrap();

        let mut all = Vec::new();
        while let Some(chunk) = stream.next().await {
            all.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(all, b"hello world");
    }

    #[tokio::test]
    async fn stream_errors_on_403() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/expired.pdf"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let downloader = SignedUrlDownloader::allowing_private_hosts();
        let url = format!("{}/expired.pdf", server.uri());
        let err = match downloader.stream(&url).await {
            Ok(_) => panic!("expected error, got Ok"),
            Err(e) => e,
        };
        assert!(matches!(
            err,
            LlmError::SignedUrlFetchFailed { status: 403 }
        ));
    }

    #[tokio::test]
    async fn stream_errors_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing.pdf"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let downloader = SignedUrlDownloader::allowing_private_hosts();
        let url = format!("{}/missing.pdf", server.uri());
        let err = match downloader.stream(&url).await {
            Ok(_) => panic!("expected error, got Ok"),
            Err(e) => e,
        };
        assert!(matches!(
            err,
            LlmError::SignedUrlFetchFailed { status: 404 }
        ));
    }

    /// `public_only()`, which `new()` is with the variable unset, never dials loopback.
    #[tokio::test]
    async fn a_non_public_address_is_never_dialed() {
        assert!(private_block_from(None), "unset, the variable blocks");
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"x"))
            .mount(&server)
            .await;
        let by_name = server.uri().replace("127.0.0.1", "localhost");
        let d = SignedUrlDownloader::public_only();
        for url in [server.uri(), by_name] {
            let r = d.stream(&format!("{url}/f")).await;
            assert!(
                matches!(r, Err(LlmError::AttachmentUrlRefused { .. })),
                "{url}"
            );
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// A redirect hop that names a non-public IP is refused before it is
    /// dialled (a dial would end in a connect error, not a refusal).
    #[tokio::test]
    async fn a_redirect_to_a_non_public_address_is_not_followed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "http://10.255.0.1/f"),
            )
            .mount(&server)
            .await;
        let loopback_only: Dialable = |ip| ip.is_loopback();
        let d = SignedUrlDownloader::with_policy(loopback_only, DEFAULT_MAX_BYTES);
        let r = d.stream(&format!("{}/f", server.uri())).await;
        assert!(matches!(r, Err(LlmError::AttachmentUrlRefused { .. })));
    }

    /// A transport error does not carry the URL (a signed URL's query is its signature).
    #[tokio::test]
    async fn a_transport_error_does_not_carry_the_url() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/f?sig=query-value", closed.local_addr().unwrap());
        drop(closed);
        let d = SignedUrlDownloader::allowing_private_hosts();
        let Err(LlmError::NetworkError { message }) = d.stream(&url).await else {
            panic!("expected a transport error")
        };
        assert!(!message.contains("query-value"), "{message}");
    }

    /// `with_timeout` never sets a deadline past the client's own 600 s.
    #[test]
    fn a_deadline_is_never_raised_past_600_s() {
        let d = SignedUrlDownloader::public_only().with_timeout(Duration::from_secs(3600));
        assert_eq!(d.timeout, Some(Duration::from_secs(600)));
    }

    #[tokio::test]
    async fn only_http_urls_are_fetched() {
        let r = SignedUrlDownloader::new().stream("file:///tmp/a.pdf").await;
        assert!(matches!(r, Err(LlmError::AttachmentUrlRefused { .. })));
    }

    /// A `Content-Length` past the byte cap is refused before the body.
    #[tokio::test]
    async fn a_body_past_the_byte_cap_is_not_read() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 2048]))
            .mount(&server)
            .await;
        let d = SignedUrlDownloader::with_policy(any_ip, 1024);
        let r = d.stream(&format!("{}/f", server.uri())).await;
        assert!(matches!(
            r,
            Err(LlmError::AttachmentTooLarge { limit: 1024 })
        ));
    }

    /// A chunked body (no `Content-Length`) that grows past the byte cap ends
    /// the stream with one error, after at most `cap` bytes.
    #[tokio::test]
    async fn a_streamed_body_past_the_byte_cap_ends_in_an_error() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/f", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = socket.read(&mut [0u8; 4096]).await;
            let chunk = format!("258\r\n{}\r\n", "x".repeat(0x258));
            let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
            let reply = format!("{head}{chunk}{chunk}{chunk}0\r\n\r\n");
            let _ = socket.write_all(reply.as_bytes()).await;
        });
        let d = SignedUrlDownloader::with_policy(any_ip, 1024);
        let items: Vec<_> = d.stream(&url).await.unwrap().collect().await;
        let (last, read) = items.split_last().expect("at least the error");
        let err = last.as_ref().expect_err("the stream ends with the error");
        let inner = err.get_ref().and_then(|e| e.downcast_ref());
        assert!(matches!(inner, Some(LlmError::AttachmentTooLarge { .. })));
        let read: usize = read.iter().map(|c| c.as_ref().unwrap().len()).sum();
        assert!(read <= 1024, "{read} bytes before the error");
    }

    #[tokio::test]
    async fn stream_does_not_send_authorization() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/no-auth.pdf"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok"))
            .mount(&server)
            .await;

        let downloader = SignedUrlDownloader::allowing_private_hosts();
        let url = format!("{}/no-auth.pdf", server.uri());
        let etag = Some("\"e\"");
        let result = downloader.fetch_conditional(&url, etag, None).await;
        assert!(result.is_ok());
        // Validar via received requests:
        let received = server.received_requests().await.unwrap();
        let req = received
            .iter()
            .find(|r| r.url.path() == "/no-auth.pdf")
            .unwrap();
        assert!(req.headers.get("authorization").is_none());
        assert!(req.headers.contains_key("if-none-match"));
        let ua = req.headers.get("user-agent").unwrap().to_str().unwrap();
        assert!(ua.contains(" (+https://"), "{ua}");
    }
}
