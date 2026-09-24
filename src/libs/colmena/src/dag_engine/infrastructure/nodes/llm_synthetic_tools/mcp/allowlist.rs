//! An OPTIONAL host allowlist for MCP servers.
//!
//! Any HTTPS URL at a PUBLIC address is reachable by default — that is unchanged
//! by this module. An operator who wants to close that surface (an SSRF risk:
//! a graph can point an MCP server at any public host) can opt in via
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
//!
//! The second control, ON by default: the MCP client dials only GLOBAL UNICAST
//! addresses ([`is_global_unicast`]), decided inside the DNS resolution the
//! socket uses (`rmcp_http_client`) and by [`non_public_literal`] for IP hosts.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The env var an operator sets to enable the allowlist. Unset or empty keeps
/// today's behaviour: any host is reachable.
pub const ALLOWLIST_ENV_VAR: &str = "COLMENA_MCP_ALLOWED_HOSTS";

/// `1` or `true` (any case) turns the non-public-address guard OFF, for local
/// development against a loopback server. Anything else keeps it ON. A
/// production deploy must never set it.
pub const ALLOW_PRIVATE_ENV_VAR: &str = "COLMENA_MCP_ALLOW_PRIVATE_HOSTS";

/// IPv4 prefixes that are NOT global unicast — ADP's `IPV4_NON_GLOBAL`
/// (`apps/api/src/shared/http/safe-fetch.ts`), so both layers agree.
const IPV4_NON_GLOBAL: [(Ipv4Addr, u32); 15] = [
    (Ipv4Addr::new(0, 0, 0, 0), 8),
    (Ipv4Addr::new(10, 0, 0, 0), 8),
    (Ipv4Addr::new(100, 64, 0, 0), 10),
    (Ipv4Addr::new(127, 0, 0, 0), 8),
    (Ipv4Addr::new(169, 254, 0, 0), 16),
    (Ipv4Addr::new(172, 16, 0, 0), 12),
    (Ipv4Addr::new(192, 0, 0, 0), 24),
    (Ipv4Addr::new(192, 0, 2, 0), 24),
    (Ipv4Addr::new(192, 88, 99, 0), 24),
    (Ipv4Addr::new(192, 168, 0, 0), 16),
    (Ipv4Addr::new(198, 18, 0, 0), 15),
    (Ipv4Addr::new(198, 51, 100, 0), 24),
    (Ipv4Addr::new(203, 0, 113, 0), 24),
    (Ipv4Addr::new(224, 0, 0, 0), 4),
    (Ipv4Addr::new(240, 0, 0, 0), 4),
];

/// Carved OUT of `2000::/3`: Teredo & co. (`2001::/23`), documentation, and
/// 6to4 — the first and last embed an IPv4 routing can unwrap to a private one.
const IPV6_NON_GLOBAL_INSIDE_2000: [(Ipv6Addr, u32); 3] = [
    (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23),
    (Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0), 32),
    (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16),
];

/// Whether the MCP client may dial `ip`: reject UNLESS global unicast (ADP's
/// `isGlobalUnicast`). An allowlist: IPv6 only inside `2000::/3`, so `::1`, ULA,
/// link-local, multicast, IPv4-mapped (even of a public v4) and NAT64 are
/// refused without being named — the forms a hand-written blocklist misses.
pub fn is_global_unicast(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !IPV4_NON_GLOBAL
            .iter()
            .any(|(net, bits)| (v4.to_bits() ^ net.to_bits()) >> (32 - bits) == 0),
        IpAddr::V6(v6) => {
            v6.to_bits() >> 125 == 0b001
                && !IPV6_NON_GLOBAL_INSIDE_2000
                    .iter()
                    .any(|(net, bits)| (v6.to_bits() ^ net.to_bits()) >> (128 - bits) == 0)
        }
    }
}

/// All of `addrs`, unchanged and in order, when every one is global unicast;
/// otherwise `Err` with the first that is not, or `Err(None)` for an empty
/// answer (as ADP). ONE refuses the whole answer: accepting because "some are
/// public" lets an attacker pick which one gets dialled.
pub fn filter_dialable(addrs: Vec<IpAddr>) -> Result<Vec<IpAddr>, Option<IpAddr>> {
    match addrs.iter().find(|ip| !is_global_unicast(**ip)) {
        Some(refused) => Err(Some(*refused)),
        None if addrs.is_empty() => Err(None),
        None => Ok(addrs),
    }
}

/// The non-public address `url` names LITERALLY, if any. `reqwest` asks no
/// resolver about an IP host, so this covers `http://10.0.0.5/`. Decided on the
/// PARSED host (WHATWG turns `0177.0.0.1`, `2130706433`, `127.1` into
/// `127.0.0.1`), never the text. A domain is the resolver's call: `None`.
pub fn non_public_literal(url: &str) -> Option<IpAddr> {
    let ip = match reqwest::Url::parse(url).ok()?.host()? {
        url::Host::Ipv4(v4) => IpAddr::V4(v4),
        url::Host::Ipv6(v6) => IpAddr::V6(v6),
        url::Host::Domain(_) => return None,
    };
    (!is_global_unicast(ip)).then_some(ip)
}

/// Whether the guard is ON for a raw [`ALLOW_PRIVATE_ENV_VAR`] value.
pub fn private_block_from(raw: Option<&str>) -> bool {
    !matches!(raw, Some(v) if v == "1" || v.eq_ignore_ascii_case("true"))
}

/// [`private_block_from`] over this process's environment, read ONCE (first MCP
/// connect): in-process code that edits the env later (a Python node) cannot
/// turn the guard off for the rest of the process.
pub fn private_block_from_env() -> bool {
    static BLOCK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *BLOCK.get_or_init(|| private_block_from(std::env::var(ALLOW_PRIVATE_ENV_VAR).ok().as_deref()))
}

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

    // -- the non-public-address guard ---------------------------------------
    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// ADP's tables (`safe-fetch.spec.ts`, "rechaza/acepta una IP literal"),
    /// through the parser the client dials with. A NAME is the resolver's call.
    #[test]
    fn adps_literal_table_is_refused_and_public_literals_or_names_are_not() {
        let table = "https://127.0.0.1/ https://0177.0.0.1/ https://0x7f.0.0.1/
            https://2130706433/ https://127.1/ https://[::1]/ https://0.0.0.0/ https://10.0.0.5/
            https://172.20.1.1/ https://192.168.1.1/ https://169.254.169.254/ https://100.64.0.1/
            https://198.19.0.1/ https://192.0.2.1/ https://239.1.2.3/ https://250.1.2.3/
            https://255.255.255.255/ https://[::ffff:10.0.0.1]/ https://[::ffff:169.254.169.254]/
            https://[::ffff:a9fe:a9fe]/ https://[64:ff9b::a00:1]/ https://[fd00::1]/
            https://[fe80::1]/ https://[ff02::1]/ https://[2002:0a00:0001::1]/
            https://[2001::1]/ https://[2001:db8::1]/";
        for url in table.split_whitespace() {
            assert!(non_public_literal(url).is_some(), "{url} must be refused");
        }
        let table = "https://93.184.216.34/token https://[2606:4700:4700::1111]/token
            https://mcp.deepwiki.com/mcp https://localhost/mcp not-a-url";
        for url in table.split_whitespace() {
            assert_eq!(non_public_literal(url), None, "{url}");
        }
    }

    #[test]
    fn non_global_addresses_are_refused() {
        let table = "10.0.0.5 172.16.0.1 172.31.255.255 192.168.1.1 127.0.0.1 169.254.169.254
            0.0.0.0 224.0.0.1 100.64.0.1 100.127.255.255 192.0.0.1 192.88.99.1 198.18.0.1
            198.51.100.1 203.0.113.1 255.255.255.255 ::1 :: fd00::1 fc00::1 fe80::1 ff02::1
            ::ffff:10.0.0.5 ::ffff:1.1.1.1 64:ff9b::a9fe:a9fe 2001:db8::1 2002::1 2001::1";
        for s in table.split_whitespace() {
            assert!(!is_global_unicast(ip(s)), "{s} must be refused");
        }
    }

    /// The edges of the refused ranges are public, so a mask off by one bit
    /// shows up here rather than in production.
    #[test]
    fn global_unicast_addresses_and_range_edges_are_dialable() {
        let table = "1.1.1.1 8.8.8.8 93.184.216.34 172.15.255.255 172.32.0.1 100.63.255.255
            100.128.0.1 198.17.255.255 198.20.0.1 223.255.255.255 2606:4700:4700::1111
            2001:200::1 2003::1";
        for s in table.split_whitespace() {
            assert!(is_global_unicast(ip(s)), "{s} must be dialable");
        }
    }

    #[test]
    fn one_private_address_refuses_the_answer_and_all_public_passes_intact() {
        let one_private = vec![ip("1.1.1.1"), ip("10.0.0.5")];
        assert_eq!(filter_dialable(one_private), Err(Some(ip("10.0.0.5"))));
        // ADP's case: the private one arrives as IPv6, the public one as IPv4.
        let mixed = vec![ip("93.184.216.34"), ip("::ffff:a9fe:a9fe")];
        assert_eq!(filter_dialable(mixed), Err(Some(ip("::ffff:a9fe:a9fe"))));
        assert_eq!(filter_dialable(vec![]), Err(None), "as ADP");
        // All public: unchanged, in order.
        let addrs = vec![ip("8.8.8.8"), ip("2606:4700:4700::1111"), ip("1.1.1.1")];
        assert_eq!(filter_dialable(addrs.clone()), Ok(addrs));
    }

    /// Unlike `COLMENA_MCP_ALLOWED_HOSTS`, this guard defaults ON: if it came
    /// off too it would fix nothing in production. Only `1`/`true` turn it off.
    #[test]
    fn the_guard_is_on_unless_explicitly_turned_off() {
        assert!(private_block_from(None));
        for raw in ["", "0", "false", "yes", " 1"] {
            assert!(private_block_from(Some(raw)), "{raw:?} must keep it ON");
        }
        for raw in ["1", "true", "TRUE"] {
            assert!(!private_block_from(Some(raw)), "{raw:?}");
        }
    }
}
