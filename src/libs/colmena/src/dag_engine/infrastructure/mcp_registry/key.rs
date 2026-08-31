//! Stable identity for one MCP server connection.
//!
//! Two tool configurations that resolve to the same remote endpoint under the
//! same credentials must share one connection; two that do not must never
//! share one. [`McpServerKey`] is that identity.

use sha2::{Digest, Sha256};

use crate::dag_engine::application::secure_value_service::is_secure_value_placeholder;
use crate::llm::domain::mcp::{McpServerConfig, McpTransport};

/// Absorb one field into the digest, framed by its own length.
///
/// Length framing, NOT a separator byte, is what makes the pre-image
/// unambiguous. A separator only works if it cannot occur inside a field, and
/// nothing here validates its inputs: URLs, header names and header references
/// are operator-authored strings read straight out of the graph JSON, and JSON
/// can encode any byte — including whatever separator we picked. With plain
/// separators, the two headers `{"A": "1", "B": "2"}` and the single header
/// `{"A": "1<SEP>B<SEP>2"}` hash identically. Two different credential sets,
/// one pooled connection: the second caller would send the first caller's
/// headers. A `u64` length prefix removes the ambiguity by construction rather
/// than by assuming something about the data.
fn absorb(hasher: &mut Sha256, field: &[u8]) {
    hasher.update((field.len() as u64).to_le_bytes());
    hasher.update(field);
}

/// The identity under which a config's credential references resolve.
///
/// Carries BOTH ids because [`SecureValueRepository::decrypt`] uses both: with
/// an agent id it resolves by agent, without one strictly by session. A scope
/// that tracked only the agent id would collapse every session-only run to one
/// key and let them share a credential — the exact bug this type exists to
/// prevent.
///
/// [`SecureValueRepository::decrypt`]: crate::dag_engine::domain::SecureValueRepository::decrypt
#[derive(Debug, Clone, Copy)]
pub struct CredentialScope<'a> {
    pub session_id: &'a str,
    pub agent_session_id: Option<&'a str>,
}

impl<'a> CredentialScope<'a> {
    pub fn new(session_id: &'a str, agent_session_id: Option<&'a str>) -> Self {
        Self {
            session_id,
            agent_session_id,
        }
    }

    /// For configs with no credential-bearing header, where the scope is
    /// ignored. Named so a caller cannot reach for it by accident.
    pub fn unscoped() -> Self {
        Self {
            session_id: "",
            agent_session_id: None,
        }
    }
}

/// Opaque, collision-resistant identity of one MCP server connection.
///
/// Derived from the URL, the transport, and a fingerprint of the header
/// **references** — never of their resolved values (design §7, spec R3.6).
/// That distinction is the whole point: two graphs referencing the same secret
/// under the same reference share a connection, and rotating the underlying
/// secret does not fragment the pool. A key built from resolved values would
/// also mean the plaintext credential decides cache placement, which is one
/// accident away from it being logged as a cache key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct McpServerKey(String);

impl McpServerKey {
    /// Derive the key for a server configuration under a credential scope.
    ///
    /// `credential_scope` is the `agent_session_id`. It participates in the
    /// key ONLY when a header value is a secure-value reference, and here is
    /// why that is not optional:
    ///
    /// Secure-value handles are `<value_1>`, `<sv_admin_token>` — counters and
    /// names, carrying nothing unique per session. `decrypt` resolves the SAME
    /// handle to DIFFERENT secrets depending on the session. So two agent
    /// sessions running the same graph produce identical `header_refs`, and
    /// without the scope they would produce an identical key, share one pooled
    /// connection, and the second session would send the first session's
    /// credential.
    ///
    /// A config whose headers are all literals is genuinely the same server
    /// everywhere, so it stays globally pooled; so does one with no headers at
    /// all. Only credential-bearing configs fragment, and they fragment per
    /// agent session rather than per run, so a conversation still reuses its
    /// connection across turns.
    pub fn from_config(config: &McpServerConfig, scope: CredentialScope<'_>) -> Self {
        let mut hasher = Sha256::new();
        absorb(&mut hasher, config.url.as_bytes());
        absorb(&mut hasher, transport_tag(config.transport).as_bytes());

        // Absorbed before the headers so a scope can never be confused with
        // header content. Always two fields — a discriminant and an id — so
        // ("agent", "") and ("none", "") stay distinct pre-images and an empty
        // id cannot collide with the unscoped case.
        let (tag, id) = if config
            .header_refs
            .values()
            .any(|v| is_secure_value_placeholder(v))
        {
            // Mirror `decrypt`'s OWN partitioning exactly. The key must split
            // the pool the same way decryption splits secrets, or the pool
            // hands one caller another's credential.
            //
            // The authority is the PRODUCTION impl,
            // `PostgresSecureValueRepository::decrypt`: two pure branches, an
            // agent-only `WHERE` when an agent id is present and a
            // session-only `WHERE` otherwise, with NO fallback between them.
            // Do not reason from `MockSecureValueRepository` in
            // `secure_value_service`'s tests — it implements agent-first WITH
            // a session fallback, which production does not, and copying that
            // shape here would silently merge two identities into one key.
            match scope.agent_session_id {
                Some(agent) => ("agent", agent),
                None => ("session", scope.session_id),
            }
        } else {
            // No credential-bearing header: the config is the same server for
            // everyone, so it stays globally pooled.
            ("none", "")
        };
        absorb(&mut hasher, tag.as_bytes());
        absorb(&mut hasher, id.as_bytes());
        // `header_refs` is a BTreeMap, so iteration order is already
        // deterministic. Each name and reference is absorbed under its own
        // length, so ("ab", "c") cannot collide with ("a", "bc") and no byte
        // inside a value can shift a field boundary.
        for (name, reference) in &config.header_refs {
            absorb(&mut hasher, name.as_bytes());
            absorb(&mut hasher, reference.as_bytes());
        }
        // Same hex idiom as `llm::domain::mcp::hex_sha256_prefix`; no new
        // dependency for eight lines of formatting.
        Self(
            hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        )
    }

    /// The hex digest, for log lines and metrics. Safe to print: it is derived
    /// from references, never from resolved secret values.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn transport_tag(transport: McpTransport) -> &'static str {
    match transport {
        McpTransport::StreamableHttp => "streamable_http",
        McpTransport::Sse => "sse",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::time::Duration;

    fn config(url: &str, transport: McpTransport, headers: &[(&str, &str)]) -> McpServerConfig {
        McpServerConfig {
            url: url.to_string(),
            transport,
            header_refs: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
            timeout: Duration::from_secs(30),
            cache_ttl: Duration::from_secs(300),
        }
    }

    /// THE bug this scope exists for. Secure-value handles are counters and
    /// names (`<value_1>`, `<sv_admin_token>`) with nothing unique per
    /// session, and `decrypt` resolves the same handle to a different secret
    /// per session. Two agent sessions running the SAME graph therefore build
    /// identical `header_refs`. Without the scope they would key identically,
    /// share one pooled connection, and the second session would send the
    /// first session's credential.
    #[test]
    fn two_agent_sessions_sharing_a_reference_do_not_share_a_connection() {
        let cfg = config(
            "https://mcp.example.com/mcp",
            McpTransport::StreamableHttp,
            &[("Authorization", "<value_1>")],
        );
        assert_ne!(
            McpServerKey::from_config(&cfg, CredentialScope::new("s", Some("agent-a"))),
            McpServerKey::from_config(&cfg, CredentialScope::new("s", Some("agent-b"))),
            "identical refs under different agent sessions resolve to DIFFERENT \
             secrets, so they must not share a pooled connection"
        );
    }

    /// The other half: one conversation must still reuse its connection across
    /// turns, or the scope would have traded a credential leak for a handshake
    /// on every turn.
    #[test]
    fn the_same_agent_session_keeps_one_connection_across_turns() {
        let cfg = config(
            "https://mcp.example.com/mcp",
            McpTransport::StreamableHttp,
            &[("Authorization", "<sv_admin_token>")],
        );
        // DIFFERENT session ids, same agent. That is what "across turns"
        // actually means: each run of a conversation gets a fresh session id
        // while the agent id persists. Comparing two identical scopes would
        // only have re-proved determinism, which
        // `the_same_reference_always_yields_the_same_identity` already covers.
        // This mirrors `decrypt`'s agent branch, which filters on the agent id
        // alone and never reads session_id.
        assert_eq!(
            McpServerKey::from_config(&cfg, CredentialScope::new("run-1", Some("agent-a"))),
            McpServerKey::from_config(&cfg, CredentialScope::new("run-2", Some("agent-a")))
        );
    }

    /// A server with no headers is genuinely the same server for everyone, so
    /// it must stay globally pooled — otherwise every agent session would
    /// re-handshake against a public server for no reason.
    #[test]
    fn a_server_without_headers_pools_globally_across_sessions() {
        let cfg = config(
            "https://mcp.deepwiki.com/mcp",
            McpTransport::StreamableHttp,
            &[],
        );
        assert_eq!(
            McpServerKey::from_config(&cfg, CredentialScope::new("s", Some("agent-a"))),
            McpServerKey::from_config(&cfg, CredentialScope::new("s", Some("agent-b"))),
            "an unauthenticated server must not fragment per session"
        );
    }

    /// A LITERAL header is the same secret in every session, so sharing is
    /// correct. Only references — whose resolved value is session-dependent —
    /// force isolation.
    #[test]
    fn a_literal_header_is_the_same_secret_everywhere_and_still_pools() {
        let cfg = config(
            "https://mcp.example.com/mcp",
            McpTransport::StreamableHttp,
            &[("X-Api-Key", "literal-key-not-a-placeholder")],
        );
        assert_eq!(
            McpServerKey::from_config(&cfg, CredentialScope::new("s", Some("agent-a"))),
            McpServerKey::from_config(&cfg, CredentialScope::new("s", Some("agent-b")))
        );
    }

    /// The half the first version of this fix left open, and the reason the
    /// scope carries BOTH ids. With no agent id, `decrypt` resolves strictly
    /// by `session_id`, so two session-only runs of the same graph resolve the
    /// same handle to different secrets and must not share a connection.
    #[test]
    fn two_session_only_runs_sharing_a_reference_do_not_share_a_connection() {
        let cfg = config(
            "https://mcp.example.com/mcp",
            McpTransport::StreamableHttp,
            &[("Authorization", "<value_1>")],
        );
        assert_ne!(
            McpServerKey::from_config(&cfg, CredentialScope::new("run-1", None)),
            McpServerKey::from_config(&cfg, CredentialScope::new("run-2", None)),
            "without an agent id, decrypt partitions by session_id, so the key must too"
        );
    }

    /// The discriminant must participate: an agent named `x` and a session
    /// named `x` are different identities, and `decrypt` looks them up in
    /// different columns. BOTH ids are `"x"` here on purpose — if the two
    /// scopes differed in any second field, that difference could carry the
    /// assertion and an implementation that ignored the discriminant entirely
    /// would still pass.
    #[test]
    fn an_agent_id_and_a_session_id_with_the_same_text_are_different_scopes() {
        let cfg = config(
            "https://mcp.example.com/mcp",
            McpTransport::StreamableHttp,
            &[("Authorization", "<value_1>")],
        );
        assert_ne!(
            McpServerKey::from_config(&cfg, CredentialScope::new("x", Some("x"))),
            McpServerKey::from_config(&cfg, CredentialScope::new("x", None))
        );
    }

    /// An empty agent id must not collapse into the session case. ONE config,
    /// only the scope varies — comparing two different configs would let the
    /// differing headers carry the assertion and prove nothing about the
    /// discriminant.
    #[test]
    fn an_empty_agent_id_is_a_different_scope_from_an_empty_session_id() {
        let cfg = config(
            "https://mcp.example.com/mcp",
            McpTransport::StreamableHttp,
            &[("Authorization", "<value_1>")],
        );
        assert_ne!(
            McpServerKey::from_config(&cfg, CredentialScope::new("", Some(""))),
            McpServerKey::from_config(&cfg, CredentialScope::new("", None)),
            "both ids are empty, so only the discriminant can tell an agent-scoped \
             secret from a session-scoped one"
        );
    }

    /// R3.6 — the key is a function of the REFERENCE, not of the resolved
    /// value. Two deployments pointing the same reference at different secrets
    /// still share one connection, and rotating a secret does not fragment the
    /// pool. It also keeps the plaintext credential out of anything that gets
    /// logged as a cache key.
    #[test]
    fn the_key_is_a_digest_and_never_carries_its_inputs_verbatim() {
        let a = config(
            "https://mcp.example.com/mcp",
            McpTransport::StreamableHttp,
            &[("Authorization", "$DYNAMIC")],
        );
        let key = McpServerKey::from_config(&a, CredentialScope::unscoped());

        // 64 lowercase hex characters and nothing else. This is the assertion
        // that can actually fail: a key built by concatenating its inputs —
        // `format!("{url}|{refs}")`, the obvious shortcut — would carry the
        // URL and the header reference verbatim into every log line and metric
        // label that prints it. Asserting "the rendered key does not contain
        // '$DYNAMIC'" would NOT catch that, because it holds vacuously for any
        // hex digest whatever the implementation hashed.
        assert_eq!(key.as_str().len(), 64, "sha256 hex is 64 chars: {key:?}");
        assert!(
            key.as_str()
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "the key must be pure lowercase hex, carrying no input material: {key:?}"
        );
    }

    /// R3.6 is enforced by the TYPE, not by the hash: `McpServerConfig` holds
    /// `header_refs`, and a resolved secret value never enters that struct at
    /// all (it is read at connect time and moved straight into the transport's
    /// header map). What IS testable here is the consequence operators depend
    /// on — the same reference always yields the same identity, so rotating
    /// the secret behind it does not fragment the connection pool.
    #[test]
    fn the_same_reference_always_yields_the_same_identity() {
        let mk = || {
            config(
                "https://mcp.example.com/mcp",
                McpTransport::StreamableHttp,
                &[("Authorization", "$DYNAMIC")],
            )
        };
        assert_eq!(
            McpServerKey::from_config(&mk(), CredentialScope::unscoped()),
            McpServerKey::from_config(&mk(), CredentialScope::unscoped())
        );
    }

    /// A different reference is a different credential, so it must be a
    /// different connection — otherwise one tool would silently ride another
    /// tool's authorization.
    #[test]
    fn a_different_header_reference_is_a_different_key() {
        let a = config(
            "https://mcp.example.com/mcp",
            McpTransport::StreamableHttp,
            &[("Authorization", "$DYNAMIC")],
        );
        let b = config(
            "https://mcp.example.com/mcp",
            McpTransport::StreamableHttp,
            &[("Authorization", "<sv_other>")],
        );
        assert_ne!(
            McpServerKey::from_config(&a, CredentialScope::unscoped()),
            McpServerKey::from_config(&b, CredentialScope::unscoped())
        );
    }

    /// URL and transport are part of the identity too.
    #[test]
    fn url_and_transport_both_participate_in_the_key() {
        let base = config(
            "https://a.example.com/mcp",
            McpTransport::StreamableHttp,
            &[],
        );
        let other_url = config(
            "https://b.example.com/mcp",
            McpTransport::StreamableHttp,
            &[],
        );
        let other_transport = config("https://a.example.com/mcp", McpTransport::Sse, &[]);
        assert_ne!(
            McpServerKey::from_config(&base, CredentialScope::unscoped()),
            McpServerKey::from_config(&other_url, CredentialScope::unscoped())
        );
        assert_ne!(
            McpServerKey::from_config(&base, CredentialScope::unscoped()),
            McpServerKey::from_config(&other_transport, CredentialScope::unscoped())
        );
    }

    /// The separator earns its place here: without it, ("ab","c") and
    /// ("a","bc") would hash the same pre-image and two DIFFERENT credentials
    /// would share one connection.
    #[test]
    fn header_field_boundaries_cannot_be_shifted_into_a_collision() {
        let a = config(
            "https://x/mcp",
            McpTransport::StreamableHttp,
            &[("ab", "c")],
        );
        let b = config(
            "https://x/mcp",
            McpTransport::StreamableHttp,
            &[("a", "bc")],
        );
        assert_ne!(
            McpServerKey::from_config(&a, CredentialScope::unscoped()),
            McpServerKey::from_config(&b, CredentialScope::unscoped())
        );
    }

    /// The separator alone is NOT enough, and this is the test that proves it.
    ///
    /// Nothing validates header names or references — they are operator-authored
    /// strings from the graph JSON, and JSON can encode any byte, `\u001F`
    /// included. With plain separators, `{"A":"1","B":"2"}` and the single
    /// header `{"A":"1\u001FB\u001F2"}` produce the SAME pre-image: two
    /// different header sets, therefore two different credentials, sharing one
    /// pooled connection — the second caller would send the first caller's
    /// headers. Length-prefixed framing is what actually makes the pre-image
    /// unambiguous.
    #[test]
    fn a_separator_byte_inside_a_header_cannot_forge_another_configs_identity() {
        let two_headers = config(
            "https://x/mcp",
            McpTransport::StreamableHttp,
            &[("A", "1"), ("B", "2")],
        );
        let smuggled = config(
            "https://x/mcp",
            McpTransport::StreamableHttp,
            &[("A", "1\u{1F}B\u{1F}2")],
        );
        assert_ne!(
            McpServerKey::from_config(&two_headers, CredentialScope::unscoped()),
            McpServerKey::from_config(&smuggled, CredentialScope::unscoped()),
            "a header value carrying the separator must not be able to impersonate \
             a different header set"
        );
    }

    /// The same smuggling attempt through the URL field.
    #[test]
    fn a_separator_byte_inside_the_url_cannot_shift_field_boundaries() {
        let plain = config("https://x/mcp", McpTransport::StreamableHttp, &[("A", "1")]);
        let smuggled = config(
            "https://x/mcp\u{1F}streamable_http\u{1F}A\u{1F}1",
            McpTransport::StreamableHttp,
            &[],
        );
        assert_ne!(
            McpServerKey::from_config(&plain, CredentialScope::unscoped()),
            McpServerKey::from_config(&smuggled, CredentialScope::unscoped())
        );
    }

    // Header ORDER is deliberately NOT tested: `header_refs` is a `BTreeMap`,
    // so two configs written in different orders are already the same map
    // before `from_config` is ever called. A test comparing them could not
    // fail for any deterministic implementation — it would assert a property
    // of `BTreeMap`, not of this module. Order-independence here is
    // structural, guaranteed by the type.

    /// Adding a header is a different credential set, so it must be a
    /// different connection. Covers the empty-to-non-empty boundary that the
    /// pair-swap cases do not reach.
    #[test]
    fn adding_a_header_changes_the_key() {
        let bare = config("https://x/mcp", McpTransport::StreamableHttp, &[]);
        let with_header = config(
            "https://x/mcp",
            McpTransport::StreamableHttp,
            &[("Authorization", "$DYNAMIC")],
        );
        assert_ne!(
            McpServerKey::from_config(&bare, CredentialScope::unscoped()),
            McpServerKey::from_config(&with_header, CredentialScope::unscoped())
        );
    }

    /// The name and the reference are distinct inputs: swapping which is which
    /// must change the identity, or a header named after a value would collide
    /// with a value named after a header.
    #[test]
    fn a_header_name_and_its_reference_are_not_interchangeable() {
        let a = config("https://x/mcp", McpTransport::StreamableHttp, &[("A", "B")]);
        let b = config("https://x/mcp", McpTransport::StreamableHttp, &[("B", "A")]);
        assert_ne!(
            McpServerKey::from_config(&a, CredentialScope::unscoped()),
            McpServerKey::from_config(&b, CredentialScope::unscoped())
        );
    }
}
