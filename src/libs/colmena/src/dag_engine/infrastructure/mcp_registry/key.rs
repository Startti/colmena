//! Stable identity for one MCP server connection.
//!
//! Two tool configurations that resolve to the same remote endpoint under the
//! same credentials must share one connection; two that do not must never
//! share one. [`McpServerKey`] is that identity.

use sha2::{Digest, Sha256};

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
    /// Derive the key for a server configuration.
    pub fn from_config(config: &McpServerConfig) -> Self {
        let mut hasher = Sha256::new();
        absorb(&mut hasher, config.url.as_bytes());
        absorb(&mut hasher, transport_tag(config.transport).as_bytes());
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
        let key = McpServerKey::from_config(&a);

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
            McpServerKey::from_config(&mk()),
            McpServerKey::from_config(&mk())
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
        assert_ne!(McpServerKey::from_config(&a), McpServerKey::from_config(&b));
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
            McpServerKey::from_config(&base),
            McpServerKey::from_config(&other_url)
        );
        assert_ne!(
            McpServerKey::from_config(&base),
            McpServerKey::from_config(&other_transport)
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
        assert_ne!(McpServerKey::from_config(&a), McpServerKey::from_config(&b));
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
            McpServerKey::from_config(&two_headers),
            McpServerKey::from_config(&smuggled),
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
            McpServerKey::from_config(&plain),
            McpServerKey::from_config(&smuggled)
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
            McpServerKey::from_config(&bare),
            McpServerKey::from_config(&with_header)
        );
    }

    /// The name and the reference are distinct inputs: swapping which is which
    /// must change the identity, or a header named after a value would collide
    /// with a value named after a header.
    #[test]
    fn a_header_name_and_its_reference_are_not_interchangeable() {
        let a = config("https://x/mcp", McpTransport::StreamableHttp, &[("A", "B")]);
        let b = config("https://x/mcp", McpTransport::StreamableHttp, &[("B", "A")]);
        assert_ne!(McpServerKey::from_config(&a), McpServerKey::from_config(&b));
    }
}
