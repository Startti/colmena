//! Value types for the OAuth subsystem.
//!
//! The newtypes around `String` serve two purposes:
//!   1. Type-level distinction between a long-lived `refresh_token`
//!      and a short-lived `access_token` so the wrong one can't be
//!      passed to a function by accident.
//!   2. A `Display` impl on `RefreshTokenSecret` that redacts the
//!      value, preventing accidental disclosure in `eprintln!` /
//!      logging paths.

use chrono::{DateTime, Utc};
use std::fmt;

/// A short-lived OAuth 2.0 bearer token (Google issues with TTL
/// ~3600s). Pass directly in `Authorization: Bearer <token>` headers.
///
/// Display is NOT redacted because access tokens are emitted in HTTP
/// headers anyway — there is no privacy benefit to hiding the value in
/// debug output, and visibility helps debugging auth issues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessToken(pub String);

impl AccessToken {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A long-lived refresh token. NEVER expires by default; only dies
/// when the user revokes consent or Google rotates aggressively.
///
/// Display IS redacted because this is a write-once-read-many
/// credential — its value should only appear in the explicit refresh
/// HTTP request body and in the operator's terminal during the
/// consent flow. Any other appearance (logs, panics, debug dumps) is
/// a leak risk.
#[derive(Clone, PartialEq, Eq)]
pub struct RefreshTokenSecret(String);

impl RefreshTokenSecret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Reveal the raw value. Use only when building the refresh
    /// request body. NEVER pass the result of this method to a logger.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RefreshTokenSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RefreshTokenSecret(<redacted>)")
    }
}

impl fmt::Display for RefreshTokenSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// A cached access token together with the absolute moment it stops
/// being valid. The provider uses a 60-second margin when deciding
/// whether to reuse vs refresh — anything older than `expires_at -
/// 60s` is treated as expired so an in-flight request can't race
/// the actual expiry.
#[derive(Debug, Clone)]
pub struct CachedToken {
    pub access_token: AccessToken,
    pub expires_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_token_redacts_in_debug_format() {
        let t = RefreshTokenSecret::new("1//0g_AbCdEfGh");
        // The literal value MUST NOT appear when the type is debug
        // printed — protects against accidental leaks via
        // `dbg!(provider)` or panic-message inclusion.
        let s = format!("{:?}", t);
        assert!(
            !s.contains("0g_AbCdEfGh"),
            "Debug impl leaked refresh token value: {s}"
        );
        assert!(s.contains("redacted"));
    }

    #[test]
    fn refresh_token_redacts_in_display_format() {
        let t = RefreshTokenSecret::new("1//0g_AbCdEfGh");
        let s = format!("{}", t);
        assert!(!s.contains("0g_AbCdEfGh"));
        assert!(s.contains("redacted"));
    }

    #[test]
    fn refresh_token_expose_returns_raw_value() {
        let t = RefreshTokenSecret::new("1//SECRET_VALUE");
        assert_eq!(t.expose(), "1//SECRET_VALUE");
    }

    #[test]
    fn access_token_displays_normally() {
        // Access tokens are not redacted — they live in HTTP headers
        // anyway, and being readable in debug output helps
        // troubleshooting.
        let t = AccessToken("ya29.a0Af.short.lived".into());
        assert_eq!(t.to_string(), "ya29.a0Af.short.lived");
    }

    #[test]
    fn access_token_into_string_consumes() {
        let t = AccessToken("ya29.foo".into());
        let s: String = t.into_string();
        assert_eq!(s, "ya29.foo");
    }
}
