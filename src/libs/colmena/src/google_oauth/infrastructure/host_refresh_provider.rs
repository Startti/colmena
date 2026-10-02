//! [`AuthTokenProvider`] seeded by the host (the embedder) with an access token
//! plus an opaque, signed refresh `handle`. The engine never sees a client secret
//! or a refresh token: when the token is near expiry, or the API rejected it
//! (`invalidate` after a 401), it asks the host through [`HostTokenPort`].
//!
//! Never logs or prints the handle or a token; `Debug` is redacted.

use super::token_provider::EXPIRY_MARGIN_SECONDS;
use crate::dag_engine::application::ports::{HostTokenError, HostTokenPort, HostTokenRequest};
use crate::google_oauth::domain::{AccessToken, AuthTokenProvider, OAuthError};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::Mutex;

struct State {
    token: String,
    expires_at: i64,
    /// The token the API rejected, until the host replaces it.
    rejected: Option<String>,
}

pub struct HostRefreshTokenProvider {
    port: Arc<dyn HostTokenPort>,
    handle: String,
    agent_session_id: Option<String>,
    /// Held across the port call, so concurrent callers share one request.
    state: Mutex<State>,
}

impl std::fmt::Debug for HostRefreshTokenProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostRefreshTokenProvider { handle: <redacted>, token: <redacted> }")
    }
}

fn sha256_hex(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}

impl HostRefreshTokenProvider {
    /// `seed_expires_at` is in Unix seconds.
    pub fn new(
        port: Arc<dyn HostTokenPort>,
        handle: String,
        seed_token: String,
        seed_expires_at: i64,
        agent_session_id: Option<String>,
    ) -> Self {
        Self {
            port,
            handle,
            agent_session_id,
            state: Mutex::new(State {
                token: seed_token,
                expires_at: seed_expires_at,
                rejected: None,
            }),
        }
    }

    /// SHA-256 (lowercase hex) of the handle: an identity for caches and pools
    /// that does not change when the token does and does not expose the handle.
    pub fn handle_fingerprint(&self) -> String {
        sha256_hex(&self.handle)
    }
}

#[async_trait]
impl AuthTokenProvider for HostRefreshTokenProvider {
    async fn get_bearer_token(&self) -> Result<AccessToken, OAuthError> {
        let mut st = self.state.lock().await;
        let now = chrono::Utc::now().timestamp();
        if st.rejected.is_none() && st.expires_at - EXPIRY_MARGIN_SECONDS > now {
            return Ok(AccessToken(st.token.clone()));
        }
        let req = HostTokenRequest {
            handle: self.handle.clone(),
            agent_session_id: self.agent_session_id.clone(),
            stale_token_sha256: st.rejected.as_deref().map(sha256_hex),
        };
        match self.port.fresh_token(req).await {
            Ok(t) => {
                st.token = t.access_token;
                st.expires_at = t.expires_at;
                st.rejected = None;
                Ok(AccessToken(st.token.clone()))
            }
            // The host's fixed texts; neither carries the handle or a token.
            Err(HostTokenError::NeedsReconnect(m)) | Err(HostTokenError::Unauthorized(m)) => {
                Err(OAuthError::ClientCredsInvalid(m))
            }
            Err(HostTokenError::RateLimited) => Err(OAuthError::Transient(
                "host token refresh rate limited".into(),
            )),
            Err(HostTokenError::Unavailable(m)) => Err(OAuthError::Transient(m)),
        }
    }

    async fn invalidate(&self) {
        let mut st = self.state.lock().await;
        st.rejected = Some(st.token.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::application::ports::{
        HostToken, HostTokenError, HostTokenPort, HostTokenRequest,
    };
    use crate::google_oauth::domain::AuthTokenProvider;
    use async_trait::async_trait;
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, Mutex};

    struct FakePort {
        calls: Mutex<Vec<HostTokenRequest>>,
        reply: Mutex<Result<HostToken, HostTokenError>>,
    }

    impl FakePort {
        fn ok(token: &str, expires_at: i64) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                reply: Mutex::new(Ok(HostToken {
                    access_token: token.into(),
                    expires_at,
                })),
            }
        }
        fn err(e: HostTokenError) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                reply: Mutex::new(Err(e)),
            }
        }
    }

    #[async_trait]
    impl HostTokenPort for FakePort {
        async fn fresh_token(&self, req: HostTokenRequest) -> Result<HostToken, HostTokenError> {
            self.calls.lock().unwrap().push(req);
            self.reply.lock().unwrap().clone()
        }
    }

    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    fn provider(
        port: Arc<FakePort>,
        seed_expires_at: i64,
        sid: Option<&str>,
    ) -> HostRefreshTokenProvider {
        HostRefreshTokenProvider::new(
            port,
            "cth1-cx7-h".into(),
            "tok-cx7-seed".into(),
            seed_expires_at,
            sid.map(str::to_string),
        )
    }

    #[tokio::test]
    async fn seed_token_is_served_without_calling_the_port_while_fresh() {
        let port = Arc::new(FakePort::ok("tok-cx7-new", now() + 3600));
        let p = provider(port.clone(), now() + 3000, Some("sess_1"));
        assert_eq!(p.get_bearer_token().await.unwrap().as_str(), "tok-cx7-seed");
        assert!(port.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn near_expiry_calls_the_port_without_stale_hash() {
        let port = Arc::new(FakePort::ok("tok-cx7-new", now() + 3600));
        let p = provider(port.clone(), now() + 30, Some("sess_1"));
        assert_eq!(p.get_bearer_token().await.unwrap().as_str(), "tok-cx7-new");
        let calls = port.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].handle, "cth1-cx7-h");
        assert_eq!(calls[0].stale_token_sha256, None);
        assert_eq!(calls[0].agent_session_id.as_deref(), Some("sess_1"));
    }

    /// The margin is strict: a token with exactly `EXPIRY_MARGIN_SECONDS` left
    /// is already too close to expiry to be served.
    #[tokio::test]
    async fn token_at_exactly_the_margin_is_refreshed() {
        let port = Arc::new(FakePort::ok("tok-cx7-new", now() + 3600));
        let p = provider(port.clone(), now() + EXPIRY_MARGIN_SECONDS, None);
        assert_eq!(p.get_bearer_token().await.unwrap().as_str(), "tok-cx7-new");
        assert_eq!(port.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn invalidate_sends_the_sha256_of_the_rejected_token() {
        let port = Arc::new(FakePort::ok("tok-cx7-new", now() + 3600));
        let p = provider(port.clone(), now() + 3000, None);
        p.invalidate().await;
        assert_eq!(p.get_bearer_token().await.unwrap().as_str(), "tok-cx7-new");
        let expected = format!("{:x}", Sha256::digest(b"tok-cx7-seed"));
        assert_eq!(
            port.calls.lock().unwrap()[0].stale_token_sha256.as_deref(),
            Some(expected.as_str())
        );
    }

    /// After a successful refresh the new token is served from the cache: the
    /// rejection mark does not survive the refresh.
    #[tokio::test]
    async fn refreshed_token_is_cached_after_invalidate() {
        let port = Arc::new(FakePort::ok("tok-cx7-new", now() + 3600));
        let p = provider(port.clone(), now() + 3000, None);
        p.invalidate().await;
        p.get_bearer_token().await.unwrap();
        assert_eq!(p.get_bearer_token().await.unwrap().as_str(), "tok-cx7-new");
        assert_eq!(port.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn concurrent_callers_share_one_port_call() {
        let port = Arc::new(FakePort::ok("tok-cx7-new", now() + 3600));
        let p = Arc::new(provider(port.clone(), now(), None));
        let (a, b) = tokio::join!(p.get_bearer_token(), p.get_bearer_token());
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(port.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn port_error_maps_to_oauth_error_without_handle_or_token() {
        let port = Arc::new(FakePort::err(HostTokenError::NeedsReconnect(
            "The app connection needs to be reconnected.".into(),
        )));
        let p = provider(port, now(), None);
        let e = p.get_bearer_token().await.unwrap_err().to_string();
        assert!(e.contains("reconnected"));
        assert!(!e.contains("cth1-cx7-h") && !e.contains("tok-cx7-seed"));
    }

    #[tokio::test]
    async fn rate_limited_and_unavailable_are_transient() {
        for err in [
            HostTokenError::RateLimited,
            HostTokenError::Unavailable("host down".into()),
        ] {
            let p = provider(Arc::new(FakePort::err(err)), now(), None);
            assert!(matches!(
                p.get_bearer_token().await.unwrap_err(),
                OAuthError::Transient(_)
            ));
        }
    }

    #[test]
    fn handle_fingerprint_is_the_sha256_hex_of_the_handle() {
        let p = provider(Arc::new(FakePort::ok("x", 0)), 0, None);
        assert_eq!(
            p.handle_fingerprint(),
            format!("{:x}", Sha256::digest(b"cth1-cx7-h"))
        );
    }

    #[test]
    fn debug_is_redacted() {
        let p = provider(Arc::new(FakePort::ok("x", 0)), 0, None);
        let d = format!("{p:?}");
        assert!(!d.contains("cth1-cx7-h") && !d.contains("tok-cx7-seed"));
        let req = HostTokenRequest {
            handle: "cth1-cx7-h".into(),
            agent_session_id: Some("sess_1".into()),
            stale_token_sha256: None,
        };
        assert!(!format!("{req:?}").contains("cth1-cx7-h"));
        let tok = HostToken {
            access_token: "tok-cx7-new".into(),
            expires_at: 1,
        };
        assert!(!format!("{tok:?}").contains("tok-cx7-new"));
    }
}
