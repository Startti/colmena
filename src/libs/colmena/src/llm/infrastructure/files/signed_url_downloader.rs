//! The one client that fetches an attachment's URL (`files[].url`): both
//! resolution paths of `llm_call`, byte persistence, the auto-summary and the
//! 24 h re-upload go through it. http(s) only; it dials only global unicast
//! addresses (the MCP client's rule), checked inside the DNS resolution the
//! socket uses and, for an IP-literal host, on the URL and on every redirect
//! hop; no proxy; connect 10 s, whole request 600 s; a byte cap. No
//! `Authorization` header: a signed URL carries its signature in the query.

use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use hyper::client::connect::dns::Name;
use reqwest::dns::{Addrs, Resolve, Resolving};
use reqwest::{redirect, Client, Url};

use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::mcp::allowlist::{
    is_global_unicast, private_block_from,
};
use crate::llm::domain::{BoxedByteStream, LlmError, SignedUrlFetcher};

/// `1` or `true` lets attachment fetches dial non-public addresses (local
/// development). A production deploy must never set it.
pub const ALLOW_PRIVATE_ENV_VAR: &str = "COLMENA_ATTACHMENT_ALLOW_PRIVATE_HOSTS";
/// Byte cap of one fetch, a positive integer; else [`DEFAULT_MAX_BYTES`].
pub const MAX_BYTES_ENV_VAR: &str = "COLMENA_ATTACHMENT_MAX_BYTES";
/// 512 MiB, the largest single file the providers' Files APIs accept.
pub const DEFAULT_MAX_BYTES: u64 = 512 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

/// Whether an address may be dialled.
type Dialable = fn(IpAddr) -> bool;

fn any_ip(_: IpAddr) -> bool {
    true
}

/// A refusal, found again by type in the error chain; it never names the address.
#[derive(Debug, thiserror::Error)]
#[error("destination is not a public address")]
struct DialRefused;

fn refused(reason: impl ToString) -> LlmError {
    LlmError::AttachmentUrlRefused {
        reason: reason.to_string(),
    }
}

/// Every answer dialable, or none is used (as `filter_dialable`).
struct GuardedResolver(Dialable);

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(resolve_checked(name.as_str().to_string(), self.0))
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
        tracing::warn!(target: "colmena::attachment", event = "attachment.dial_refused", host = %host,
            "refused to fetch an attachment from a non-public address");
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

fn is_dial_refused(e: &reqwest::Error) -> bool {
    std::iter::successors(Some(e as &(dyn Error + 'static)), |&e| e.source())
        .any(|e| e.is::<DialRefused>())
}

fn guarded_client(ok: Dialable) -> Client {
    crate::shared::http_client::builder()
        .dns_resolver(Arc::new(GuardedResolver(ok)))
        .redirect(redirect::Policy::custom(move |hop| {
            if hop.previous().len() >= MAX_REDIRECTS {
                hop.error("too many redirects")
            } else if literal_refused(hop.url(), ok) {
                hop.error(DialRefused)
            } else {
                hop.follow()
            }
        }))
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        // 600 s: generous for files up to ~500 MB on slow connections.
        .timeout(Duration::from_secs(600))
        .user_agent(concat!("colmena/", env!("CARGO_PKG_VERSION")))
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

/// The guarded HTTP client for attachment URLs (see the module doc).
pub struct SignedUrlDownloader {
    client: Client,
    dialable: Dialable,
    max_bytes: u64,
}

impl SignedUrlDownloader {
    /// Public addresses only unless [`ALLOW_PRIVATE_ENV_VAR`] is set; byte cap
    /// from [`MAX_BYTES_ENV_VAR`]. Both are read once per process.
    pub fn new() -> Self {
        static POLICY: OnceLock<(bool, u64)> = OnceLock::new();
        let (block, max) = *POLICY.get_or_init(|| {
            let env = |k: &str| std::env::var(k).ok();
            let max = env(MAX_BYTES_ENV_VAR).and_then(|v| v.trim().parse().ok());
            let block = private_block_from(env(ALLOW_PRIVATE_ENV_VAR).as_deref());
            (block, max.filter(|n| *n > 0).unwrap_or(DEFAULT_MAX_BYTES))
        });
        Self::with_policy(if block { is_global_unicast } else { any_ip }, max)
    }

    fn with_policy(dialable: Dialable, max_bytes: u64) -> Self {
        let client = guarded_client(dialable);
        Self {
            client,
            dialable,
            max_bytes,
        }
    }

    /// For tests against a loopback server: every address is dialable.
    #[cfg(test)]
    pub(crate) fn allowing_private_hosts() -> Self {
        Self::with_policy(any_ip, DEFAULT_MAX_BYTES)
    }

    /// Streams the response body of an attachment URL.
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
    pub async fn stream(&self, url: &str) -> Result<BoxedByteStream, LlmError> {
        let parsed = Url::parse(url).map_err(|_| refused("not a valid URL"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(refused("only http and https URLs are fetched"));
        }
        if literal_refused(&parsed, self.dialable) {
            return Err(refused(DialRefused));
        }
        let response = match self.client.get(parsed).send().await {
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
        let body = response.bytes_stream().map_err(std::io::Error::other);
        Ok(Box::pin(body.scan(Some(0), move |seen, c| {
            futures::future::ready(cap(seen, c, max))
        })))
    }
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

    /// The production client never dials a loopback server.
    #[tokio::test]
    async fn a_non_public_address_is_never_dialed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"x"))
            .mount(&server)
            .await;
        let by_name = server.uri().replace("127.0.0.1", "localhost");
        for url in [server.uri(), by_name] {
            let r = SignedUrlDownloader::new().stream(&format!("{url}/f")).await;
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

    /// A transport error does not carry the URL: a signed URL's query is its
    /// signature.
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
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = socket.read(&mut [0u8; 4096]).await;
            let chunk = format!("258\r\n{}\r\n", "x".repeat(0x258));
            let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
            let reply = format!("{head}{chunk}{chunk}{chunk}0\r\n\r\n");
            let _ = socket.write_all(reply.as_bytes()).await;
        });
        let d = SignedUrlDownloader::with_policy(any_ip, 1024);
        let stream = d.stream(&format!("http://{addr}/f")).await.unwrap();
        let items: Vec<_> = stream.collect().await;
        let (last, read) = items.split_last().expect("at least the error");
        let err = last.as_ref().expect_err("the stream ends with the error");
        let inner = err.get_ref().and_then(|e| e.downcast_ref());
        assert!(matches!(
            inner,
            Some(LlmError::AttachmentTooLarge { limit: 1024 })
        ));
        let read: usize = read.iter().map(|c| c.as_ref().map_or(0, |b| b.len())).sum();
        assert!(read <= 1024 && items[..items.len() - 1].iter().all(Result::is_ok));
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
        let result = downloader.stream(&url).await;
        assert!(result.is_ok());
        // Validar via received requests:
        let received = server.received_requests().await.unwrap();
        let req = received
            .iter()
            .find(|r| r.url.path() == "/no-auth.pdf")
            .unwrap();
        assert!(req.headers.get("authorization").is_none());
    }
}
