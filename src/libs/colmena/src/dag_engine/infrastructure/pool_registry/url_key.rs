//! Normalized representation of a Postgres connection URL used as a registry key.
//!
//! Conservative normalization: lowercase the scheme and host, strip a single
//! trailing slash on the path, preserve query parameters and credentials.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UrlKey(String);

impl UrlKey {
    pub fn normalize(raw: &str) -> Self {
        // Split into "scheme://rest" and "rest" without parsing with `url` crate
        // (avoids a new dep and keeps behavior predictable for custom query params).
        let (scheme, rest) = match raw.split_once("://") {
            Some((s, r)) => (s.to_ascii_lowercase(), r),
            None => return UrlKey(raw.to_string()),
        };

        // Split rest into "authority" and "path+query" around the first '/'
        // (after the "://"). Authority may contain "user:pass@host:port".
        let (authority, path_q) = match rest.find('/') {
            Some(idx) => (&rest[..idx], &rest[idx..]),
            None => (rest, ""),
        };

        // Split authority into "credentials@host" — lowercase only the host part.
        let (creds, hostport) = match authority.rfind('@') {
            Some(idx) => (Some(&authority[..idx]), &authority[idx + 1..]),
            None => (None, authority),
        };
        let hostport_lower = hostport.to_ascii_lowercase();

        let mut out = String::with_capacity(raw.len());
        out.push_str(&scheme);
        out.push_str("://");
        if let Some(c) = creds {
            out.push_str(c);
            out.push('@');
        }
        out.push_str(&hostport_lower);

        // Strip one trailing slash from the path, but only if there's no query string.
        if let Some(q_idx) = path_q.find('?') {
            let (path, query) = path_q.split_at(q_idx);
            let path = path.strip_suffix('/').unwrap_or(path);
            out.push_str(path);
            out.push_str(query);
        } else {
            let path = path_q.strip_suffix('/').unwrap_or(path_q);
            out.push_str(path);
        }

        UrlKey(out)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UrlKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases_scheme_and_host() {
        let a = UrlKey::normalize("POSTGRES://User:Pass@HOST.example.COM:5432/db");
        let b = UrlKey::normalize("postgres://User:Pass@host.example.com:5432/db");
        assert_eq!(a, b);
    }

    #[test]
    fn preserves_credentials_case_sensitive() {
        let a = UrlKey::normalize("postgres://User:Pass@host/db");
        let b = UrlKey::normalize("postgres://user:pass@host/db");
        assert_ne!(a, b, "credentials must not be normalized");
    }

    #[test]
    fn strips_single_trailing_slash() {
        let a = UrlKey::normalize("postgres://host/db/");
        let b = UrlKey::normalize("postgres://host/db");
        assert_eq!(a, b);
    }

    #[test]
    fn preserves_query_parameters() {
        let a = UrlKey::normalize("postgres://host/db?sslmode=require");
        let b = UrlKey::normalize("postgres://host/db");
        assert_ne!(
            a, b,
            "query parameters can change connection behavior and must be preserved"
        );
    }

    #[test]
    fn distinct_users_are_distinct_keys() {
        let a = UrlKey::normalize("postgres://alice:pw@host/db");
        let b = UrlKey::normalize("postgres://bob:pw@host/db");
        assert_ne!(a, b);
    }

    #[test]
    fn handles_url_without_path() {
        let a = UrlKey::normalize("postgres://user:pass@host:5432");
        assert_eq!(a.as_str(), "postgres://user:pass@host:5432");
    }
}
