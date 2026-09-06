//! An OPTIONAL host allowlist for MCP servers.
//!
//! Any HTTPS URL a graph declares is reachable by default — that is unchanged
//! by this module. An operator who wants to close that surface (an SSRF risk:
//! a graph can point an MCP server at an internal endpoint) can opt in via
//! [`ALLOWLIST_ENV_VAR`]. Empty or unset means "allow everything", which is
//! the entire compatibility guarantee: nothing in production changes until an
//! operator sets the var.
//!
//! Deliberately strict rather than clever: exact, case-insensitive HOSTNAME
//! match only. No ports (a server on a non-default port is still the same
//! host you meant to allow), no wildcards, no subdomain matching, no CIDR
//! ranges. A permissive matcher is worse than none — it gives an operator
//! false confidence that `*.internal` is covered when it silently is not.
//! List every host you mean to allow.
//!
//! **A previous version of this module hand-rolled its own URL parser**
//! (`host_of`, since deleted) whose delimiter set did not include `\`. The
//! real HTTP client — `rmcp` over `reqwest`, whose `Url` type is the
//! WHATWG-compliant `url` crate — treats a backslash as an authority
//! terminator for special schemes such as `https`. That let
//! `https://evil.internal\@allowed.example.com/mcp` read as host
//! `allowed.example.com` to the hand-rolled parser while `reqwest` actually
//! dialled `evil.internal`: a complete SSRF bypass. The fix is not to patch
//! that one delimiter — punycode, normalisation, and other WHATWG rules would
//! still disagree with a second hand-rolled parser — but to have exactly ONE
//! notion of "host", taken from the same parser that decides where the
//! connection goes. See [`dialed_host`].

/// The env var an operator sets to enable the allowlist. Unset or empty keeps
/// today's behaviour: any host is reachable.
pub const ALLOWLIST_ENV_VAR: &str = "COLMENA_MCP_ALLOWED_HOSTS";

/// The host a URL will actually be dialled at — no scheme, no port, no path,
/// no query, no userinfo — lowercased.
///
/// This is the SECURITY decision input: it is derived from `reqwest::Url`
/// (the same WHATWG-compliant `url` crate `rmcp`'s HTTP transport uses to
/// open the connection), so the allowlist can never disagree with where the
/// request actually goes. `None` when the URL does not parse — callers must
/// treat that as "cannot vouch for this host", never as "allow it".
pub fn dialed_host(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_ascii_lowercase))
}

/// The host (and port, if the URL states one explicitly) a server URL points
/// at — for a LOG LINE only.
///
/// Uses the same `reqwest::Url` parser as [`dialed_host`] so the two never
/// tell conflicting stories about the same URL, but this function's output
/// must NEVER gate a security decision — that exact mistake (promoting a
/// display helper into a security boundary) is what caused the SSRF bypass
/// this module now guards against. Use [`url_is_allowed`] for the decision;
/// use this only to decorate a log line. Never panics: an unparseable URL
/// yields a stable placeholder rather than leaking scheme, path, query or
/// userinfo into the log.
pub fn host_for_log(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(parsed) => match parsed.host_str() {
            Some(host) => match parsed.port() {
                Some(port) => format!("{}:{port}", host.to_ascii_lowercase()),
                None => host.to_ascii_lowercase(),
            },
            None => "<unparseable>".to_string(),
        },
        Err(_) => "<unparseable>".to_string(),
    }
}

/// Turn a raw, comma-separated env value into a normalised allowlist.
///
/// Each entry is trimmed and lowercased; empty entries (from a stray comma, a
/// trailing separator, or all-whitespace input) are dropped. An all-whitespace
/// or empty string yields an empty `Vec` — which [`url_is_allowed`] treats as
/// "allow everything", so a blank env var is indistinguishable from an unset
/// one.
pub fn parse_allowlist(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|entry| entry.trim().to_ascii_lowercase())
        .filter(|entry| !entry.is_empty())
        .collect()
}

/// Whether `url` may be contacted.
///
/// An EMPTY `allowed` means allow everything — that is the whole compatibility
/// guarantee for an operator who never opts in. Otherwise this derives the
/// host from [`dialed_host`] — the SAME parser `reqwest` uses to decide where
/// to connect — and does an EXACT, case-insensitive, port-free match against
/// the entries: no wildcards, no subdomain matching. A `url` that does not
/// parse is refused when an allowlist is configured: a URL this function
/// cannot make sense of is one it cannot vouch for.
///
/// Deliberately takes the whole URL rather than a pre-extracted host, so no
/// caller can pass a host derived from a different (and possibly
/// disagreeing) parser — which is exactly how the previous bypass happened.
pub fn url_is_allowed(url: &str, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    match dialed_host(url) {
        Some(host) => allowed.iter().any(|entry| entry == &host),
        None => false,
    }
}

/// The allowlist this process was configured with, read from
/// [`ALLOWLIST_ENV_VAR`].
///
/// Split from the env read the same way `pool_size_from` is split from
/// `global_mcp_registry` in `mcp_registry/mod.rs`: a pure function that is
/// actually testable, plus a thin wrapper that touches the environment.
/// Reading a stray comma, extra spaces, or mixed case must never panic — a
/// malformed env var must not be the thing that takes MCP down, it must just
/// fail open the way an unset one does whenever every entry turns out empty.
pub fn allowed_hosts_from_env() -> Vec<String> {
    allowlist_from(std::env::var(ALLOWLIST_ENV_VAR).ok().as_deref())
}

/// The allowlist a raw env value asks for, or the empty (allow-everything)
/// list when it asks for nothing usable.
///
/// Split out so [`allowed_hosts_from_env`]'s only untestable line is the env
/// read itself.
fn allowlist_from(raw: Option<&str>) -> Vec<String> {
    raw.map(parse_allowlist).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- parse_allowlist -----------------------------------------------

    #[test]
    fn parse_allowlist_splits_trims_and_lowercases() {
        assert_eq!(
            parse_allowlist("Mcp.Example.Com, other.host ,  third.io"),
            vec!["mcp.example.com", "other.host", "third.io"],
        );
    }

    #[test]
    fn parse_allowlist_drops_empty_entries_from_stray_commas() {
        assert_eq!(parse_allowlist("host.a,,host.b,"), vec!["host.a", "host.b"],);
    }

    #[test]
    fn parse_allowlist_of_empty_string_is_empty() {
        assert!(parse_allowlist("").is_empty());
    }

    #[test]
    fn parse_allowlist_of_all_whitespace_is_empty() {
        assert!(parse_allowlist("   ,\t, \n ").is_empty());
    }

    // -- url_is_allowed ----------------------------------------------------

    /// THE REGRESSION TEST. The previous hand-rolled `host_of` parser split
    /// the authority on `['/', '?', '#']` only — never on `\` — so it read
    /// this URL's host as `allowed.example.com`. `reqwest::Url` (WHATWG),
    /// which is what `rmcp` actually dials with, treats `\` as an authority
    /// terminator for special schemes and resolves the userinfo-delimited
    /// host `evil.internal` instead. That divergence was a complete SSRF
    /// bypass: the allowlist approved a URL the HTTP client sent somewhere
    /// else entirely. `url_is_allowed` must refuse it.
    #[test]
    fn refuses_the_whatwg_backslash_authority_bypass() {
        let allowed = vec!["allowed.example.com".to_string()];
        assert!(
            !url_is_allowed("https://evil.internal\\@allowed.example.com/mcp", &allowed),
            "must be refused: reqwest actually dials evil.internal, not allowed.example.com"
        );
    }

    #[test]
    fn empty_allowlist_allows_everything_including_the_crafted_bypass_url() {
        assert!(url_is_allowed("https://anything.internal/x", &[]));
        assert!(url_is_allowed(
            "https://evil.internal\\@allowed.example.com/mcp",
            &[]
        ));
    }

    #[test]
    fn a_host_not_in_a_nonempty_allowlist_is_refused() {
        let allowed = vec!["mcp.example.com".to_string()];
        assert!(!url_is_allowed("https://other.example.com/x", &allowed));
    }

    #[test]
    fn a_matching_host_is_allowed() {
        let allowed = vec!["mcp.example.com".to_string()];
        assert!(url_is_allowed("https://mcp.example.com/mcp", &allowed));
    }

    #[test]
    fn matching_is_case_insensitive() {
        let allowed = vec!["allowed.example.com".to_string()];
        assert!(url_is_allowed("HTTPS://Allowed.Example.COM/x", &allowed));
    }

    #[test]
    fn port_is_ignored_by_the_match() {
        let allowed = vec!["allowed.example.com".to_string()];
        assert!(url_is_allowed(
            "https://allowed.example.com:8443/x",
            &allowed
        ));
    }

    #[test]
    fn userinfo_does_not_decide_the_match() {
        // reqwest dials `evil.internal` here — the part before `@` is
        // userinfo, not host. An allowlist for `allowed.example.com` must
        // refuse this even though that hostname appears in the URL text.
        let allowed = vec!["allowed.example.com".to_string()];
        assert!(!url_is_allowed(
            "https://allowed.example.com@evil.internal/x",
            &allowed
        ));
    }

    #[test]
    fn no_subdomain_matching() {
        let allowed = vec!["example.com".to_string()];
        assert!(
            !url_is_allowed("https://sub.example.com/x", &allowed),
            "a subdomain must not match its parent domain — no wildcard behaviour"
        );
    }

    #[test]
    fn no_wildcard_matching() {
        let allowed = vec!["*.example.com".to_string()];
        assert!(
            !url_is_allowed("https://mcp.example.com/x", &allowed),
            "a literal '*' entry is not a wildcard — it is just a string that never matches a real host"
        );
    }

    #[test]
    fn an_unparseable_url_is_refused_when_an_allowlist_is_set() {
        let allowed = vec!["allowed.example.com".to_string()];
        assert!(!url_is_allowed("not a url at all", &allowed));
    }

    #[test]
    fn an_unparseable_url_is_allowed_when_the_allowlist_is_empty() {
        assert!(url_is_allowed("not a url at all", &[]));
    }

    // -- allowlist_from (the pure half of the env wrapper) ----------------

    #[test]
    fn allowlist_from_none_is_empty() {
        assert!(allowlist_from(None).is_empty());
    }

    #[test]
    fn allowlist_from_junk_is_empty_rather_than_panicking() {
        assert!(allowlist_from(Some(" , ,, ")).is_empty());
    }

    #[test]
    fn allowlist_from_mixed_case_and_spacing_normalises() {
        assert_eq!(
            allowlist_from(Some(" Host.A , HOST.B")),
            vec!["host.a", "host.b"],
        );
    }

    // -- dialed_host ---------------------------------------------------------

    #[test]
    fn dialed_host_has_no_port() {
        assert_eq!(
            dialed_host("https://host.example.com:8443/x"),
            Some("host.example.com".to_string())
        );
    }

    #[test]
    fn dialed_host_ignores_userinfo() {
        assert_eq!(
            dialed_host("https://user:pass@host.example.com/mcp"),
            Some("host.example.com".to_string())
        );
    }

    #[test]
    fn dialed_host_of_unparseable_url_is_none() {
        assert_eq!(dialed_host("not a url at all"), None);
    }

    // -- host_for_log ------------------------------------------------------

    #[test]
    fn host_for_log_strips_scheme_path_query_and_userinfo() {
        assert_eq!(
            host_for_log("https://mcp.context7.com/mcp"),
            "mcp.context7.com"
        );
        assert_eq!(
            host_for_log("https://user:pass@host.example.com/mcp?x=1#y"),
            "host.example.com",
            "userinfo, query and fragment must not reach the log"
        );
    }

    #[test]
    fn host_for_log_keeps_an_explicit_port() {
        assert_eq!(
            host_for_log("https://host.example.com:8443/x"),
            "host.example.com:8443"
        );
    }

    #[test]
    fn host_for_log_never_panics_on_empty_input() {
        assert_eq!(host_for_log(""), "<unparseable>");
    }

    #[test]
    fn host_for_log_placeholder_leaks_nothing_from_an_unparseable_url() {
        let out = host_for_log("not a url at all");
        assert_eq!(out, "<unparseable>");
    }
}
