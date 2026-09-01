//! Stable identity for one MCP server connection.
//!
//! Two tool configurations that resolve to the same remote endpoint under the
//! same credentials must share one connection; two that do not must never
//! share one. [`McpServerKey`] is that identity.

use std::collections::BTreeMap;

use once_cell::sync::Lazy;
use sha2::{Digest, Sha256};
use uuid::Uuid;

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

/// A fingerprint of the credential VALUES a connection will carry.
///
/// The pool must split exactly where the credential splits. This used to be
/// approximated by the session or agent id, because the key was computed
/// before the secrets were resolved and the id was all that existed. That
/// proxy was wrong in both directions: it separated two sessions holding the
/// SAME credential, which the server cannot tell apart, and it kept ONE key
/// across a secret rotation, so the pool went on handing back a connection
/// carrying the retired value until something else happened to evict it.
///
/// Fingerprinting the resolved values removes the proxy. A rotation changes
/// the fingerprint, which changes the key, which yields a new connection; the
/// stale one ages out through the LRU. Correct by construction, with no
/// invalidation path to get wrong.
///
/// SALTED, per process. This digest lands in the pool key, and that key is
/// printed by the eviction `tracing::debug`. An unsalted `sha256` of a secret
/// is not reversible, but it is a stable oracle: anyone who can read a log and
/// guess a value can confirm the guess. A random per-process salt keeps the
/// digest stable for exactly as long as pooling needs it — the life of the
/// process — and meaningless outside it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialFingerprint(String);

/// Random per process. Never persisted, never logged.
static CREDENTIAL_SALT: Lazy<String> = Lazy::new(|| Uuid::new_v4().to_string());

impl CredentialFingerprint {
    /// Fingerprint the RESOLVED header values.
    ///
    /// Names are absorbed alongside values, so the same bytes sent as
    /// `Authorization` and as `X-Api-Key` are not the same credential.
    pub fn of(resolved_headers: &BTreeMap<String, String>) -> Self {
        let mut hasher = Sha256::new();
        absorb(&mut hasher, CREDENTIAL_SALT.as_bytes());
        for (name, value) in resolved_headers {
            absorb(&mut hasher, name.as_bytes());
            absorb(&mut hasher, value.as_bytes());
        }
        Self(hex_digest(hasher))
    }

    /// A server reached with no headers at all.
    pub fn none() -> Self {
        Self::of(&BTreeMap::new())
    }
}

/// Opaque, collision-resistant identity of one MCP server connection.
///
/// Derived from the URL, the transport, the header NAMES, and a
/// [`CredentialFingerprint`] of the RESOLVED header values.
///
/// Keying on the references instead would leave the key unchanged across a
/// rotation, and an unchanged key means the pool keeps handing back the
/// connection built with the retired value. The fingerprint is what makes a
/// rotation produce a new connection.
///
/// Resolved values do carry a risk the references did not: the credential
/// would decide cache placement and could reach a log. That is why the
/// fingerprint is SALTED — see its docs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct McpServerKey(String);

impl McpServerKey {
    /// The pool identity of a server reached with these resolved credentials.
    ///
    /// Takes the fingerprint, not the raw values, so this function never holds
    /// a secret and the salting decision lives in exactly one place.
    pub fn from_resolved(config: &McpServerConfig, credential: &CredentialFingerprint) -> Self {
        let mut hasher = Sha256::new();
        absorb(&mut hasher, config.url.as_bytes());
        absorb(&mut hasher, transport_tag(config.transport).as_bytes());
        absorb(&mut hasher, credential.0.as_bytes());
        // Header NAMES still count: two configs sending the same value under
        // different names are different requests. The VALUES are already
        // inside the fingerprint and must not be absorbed again here, where
        // they would be unsalted.
        for name in config.header_refs.keys() {
            absorb(&mut hasher, name.as_bytes());
        }
        Self(hex_digest(hasher))
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

/// The hex form both digests use. Same idiom as
/// `llm::domain::mcp::hex_sha256_prefix`; no new dependency for eight lines.
fn hex_digest(hasher: Sha256) -> String {
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn cfg_with(headers: &[(&str, &str)]) -> McpServerConfig {
        McpServerConfig {
            url: "https://mcp.example.com/mcp".to_string(),
            transport: McpTransport::StreamableHttp,
            header_refs: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            timeout: Duration::from_secs(30),
            cache_ttl: Duration::from_secs(300),
        }
    }

    fn resolved(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn key_for(headers: &[(&str, &str)], values: &[(&str, &str)]) -> McpServerKey {
        McpServerKey::from_resolved(
            &cfg_with(headers),
            &CredentialFingerprint::of(&resolved(values)),
        )
    }

    // --- What the pool must never merge ---

    /// The property the credential scope was introduced to protect, now stated
    /// directly instead of through a session-id proxy: two principals whose
    /// secret resolves to different bytes must never share a connection.
    #[test]
    fn two_principals_with_different_secrets_do_not_share_a_connection() {
        assert_ne!(
            key_for(
                &[("Authorization", "<sv_token>")],
                &[("Authorization", "Bearer alice")]
            ),
            key_for(
                &[("Authorization", "<sv_token>")],
                &[("Authorization", "Bearer bob")]
            ),
            "same reference, different resolved secret: different connections"
        );
    }

    /// The bug the session-id proxy could not see. A rotation leaves the
    /// config and the session untouched, so the OLD key was unchanged and the
    /// pool kept serving a connection holding the retired credential.
    #[test]
    fn rotating_a_secret_yields_a_new_connection() {
        let before = key_for(
            &[("Authorization", "<sv_token>")],
            &[("Authorization", "Bearer old")],
        );
        let after = key_for(
            &[("Authorization", "<sv_token>")],
            &[("Authorization", "Bearer new")],
        );
        assert_ne!(
            before, after,
            "a rotated secret must not keep serving the connection built with the old one"
        );
    }

    /// Moving the same bytes to a different header is a different request, so
    /// it must be a different identity.
    #[test]
    fn the_same_secret_under_a_different_header_name_is_a_different_key() {
        assert_ne!(
            key_for(&[("Authorization", "<sv_t>")], &[("Authorization", "tok")]),
            key_for(&[("X-Api-Key", "<sv_t>")], &[("X-Api-Key", "tok")]),
        );
    }

    /// Field framing, at the fingerprint layer: ("ab","c") and ("a","bc")
    /// concatenate identically, so only length framing keeps them apart.
    #[test]
    fn credential_field_boundaries_cannot_be_shifted_into_a_collision() {
        assert_ne!(
            CredentialFingerprint::of(&resolved(&[("ab", "c")])),
            CredentialFingerprint::of(&resolved(&[("a", "bc")])),
        );
    }

    /// Role order, not just field length. The surviving boundary tests compare
    /// two DIFFERENT string pairs, so a digest that combined name and value
    /// commutatively would pass every one of them. This is the successor to
    /// the old `a_header_name_and_its_reference_are_not_interchangeable`,
    /// re-expressed at the fingerprint layer where the values now live.
    #[test]
    fn a_header_name_and_its_value_are_not_interchangeable() {
        assert_ne!(
            CredentialFingerprint::of(&resolved(&[("A", "B")])),
            CredentialFingerprint::of(&resolved(&[("B", "A")])),
            "the same two strings in swapped roles are a different credential"
        );
    }

    /// Per-ENTRY pairing, which the single-header tests cannot show.
    ///
    /// A digest that hashed the sorted names and the sorted values as two
    /// independent sets would pass every other test in this module — the name
    /// set and the value set are each unchanged here — and yet would equate
    /// two genuinely different credential sets, letting the pool hand one
    /// principal's connection to the other. Only a fixture with TWO headers
    /// whose values are swapped between them can catch that.
    #[test]
    fn swapping_values_between_two_headers_is_a_different_credential() {
        assert_ne!(
            CredentialFingerprint::of(&resolved(&[("Authorization", "X"), ("X-Api-Key", "Y")])),
            CredentialFingerprint::of(&resolved(&[("Authorization", "Y"), ("X-Api-Key", "X")])),
            "same names, same values, different pairing: a different credential"
        );
    }

    /// Every entry counts, not just the first.
    ///
    /// The swap test above shares its name set and value set between the two
    /// fixtures, but its two sides also differ in the FIRST sorted key, so a
    /// digest that hashed only the first pair would still tell them apart —
    /// passing for the wrong reason. These two fixtures agree on the first
    /// entry and differ only in the second, which is the shape that catches
    /// it. Cardinality counts too: one header is not the same credential as
    /// that header plus another.
    #[test]
    fn entries_past_the_first_participate_in_the_fingerprint() {
        let alice = CredentialFingerprint::of(&resolved(&[
            ("Authorization", "tok"),
            ("X-Api-Key", "alice"),
        ]));
        let bob =
            CredentialFingerprint::of(&resolved(&[("Authorization", "tok"), ("X-Api-Key", "bob")]));
        assert_ne!(
            alice, bob,
            "same first entry, different second: two different principals"
        );

        let one = CredentialFingerprint::of(&resolved(&[("Authorization", "tok")]));
        assert_ne!(
            one, alice,
            "adding a second header is a different credential, not the same one"
        );
    }

    /// EVERY entry, at EVERY position.
    ///
    /// Three ad-hoc fixtures in a row were defeated by a different
    /// position-blind implementation each time: hash only the first pair, then
    /// hash only the last. Each new fixture happened to vary the position the
    /// next mutant ignored, so it passed for the wrong reason. Naming positions
    /// one at a time is a losing game — this pins the property instead: change
    /// any single entry's value, at any position, and the fingerprint changes.
    /// A digest that consults only some positions fails here whichever ones it
    /// picks.
    #[test]
    fn every_entry_participates_regardless_of_position() {
        // Names chosen so BTreeMap order is first < middle < last.
        let base = [("a-first", "1"), ("m-middle", "2"), ("z-last", "3")];
        let baseline = CredentialFingerprint::of(&resolved(&base));

        for position in 0..base.len() {
            let mut varied = base;
            varied[position].1 = "changed";
            assert_ne!(
                baseline,
                CredentialFingerprint::of(&resolved(&varied)),
                "changing '{}' (position {position}) left the fingerprint unchanged, so \
                 that entry does not participate",
                base[position].0
            );
        }
    }

    /// And at the key layer, over header names.
    #[test]
    fn header_name_boundaries_cannot_be_shifted_into_a_collision() {
        let none = CredentialFingerprint::none();
        assert_ne!(
            McpServerKey::from_resolved(&cfg_with(&[("ab", ""), ("c", "")]), &none),
            McpServerKey::from_resolved(&cfg_with(&[("a", ""), ("bc", "")]), &none),
        );
    }

    #[test]
    fn adding_a_header_changes_the_key() {
        let none = CredentialFingerprint::none();
        assert_ne!(
            McpServerKey::from_resolved(&cfg_with(&[("A", "1")]), &none),
            McpServerKey::from_resolved(&cfg_with(&[("A", "1"), ("B", "2")]), &none),
        );
    }

    #[test]
    fn url_and_transport_both_participate_in_the_key() {
        let none = CredentialFingerprint::none();
        let base = cfg_with(&[]);
        let mut other_url = base.clone();
        other_url.url = "https://other.example.com/mcp".to_string();
        let mut other_transport = base.clone();
        other_transport.transport = McpTransport::Sse;

        assert_ne!(
            McpServerKey::from_resolved(&base, &none),
            McpServerKey::from_resolved(&other_url, &none),
            "a different endpoint is a different server"
        );
        assert_ne!(
            McpServerKey::from_resolved(&base, &none),
            McpServerKey::from_resolved(&other_transport, &none),
            "the same endpoint over a different transport is a different connection"
        );
    }

    /// A separator-looking byte inside a URL must not be able to forge the
    /// pre-image of another config.
    #[test]
    fn a_separator_byte_inside_the_url_cannot_shift_field_boundaries() {
        let none = CredentialFingerprint::none();
        let mut sneaky = cfg_with(&[]);
        sneaky.url = "https://mcp.example.com/mcp\u{1f}streamable_http".to_string();
        assert_ne!(
            McpServerKey::from_resolved(&sneaky, &none),
            McpServerKey::from_resolved(&cfg_with(&[]), &none),
        );
    }

    // --- What the pool must merge ---

    /// The improvement the proxy could not express. Two sessions holding the
    /// SAME credential are indistinguishable to the server, so separating them
    /// bought nothing and cost a connection each.
    #[test]
    fn two_sessions_with_the_same_secret_share_one_connection() {
        assert_eq!(
            key_for(
                &[("Authorization", "<sv_token>")],
                &[("Authorization", "Bearer shared")]
            ),
            key_for(
                &[("Authorization", "<sv_token>")],
                &[("Authorization", "Bearer shared")]
            ),
            "the server cannot tell these apart; the pool should not either"
        );
    }

    /// No headers at all: one connection for everyone, which is what makes a
    /// public server like DeepWiki cheap to talk to.
    #[test]
    fn a_server_without_headers_pools_globally() {
        assert_eq!(
            McpServerKey::from_resolved(&cfg_with(&[]), &CredentialFingerprint::none()),
            McpServerKey::from_resolved(&cfg_with(&[]), &CredentialFingerprint::none()),
        );
    }

    #[test]
    fn the_same_inputs_always_yield_the_same_identity() {
        assert_eq!(
            key_for(&[("Authorization", "<sv_t>")], &[("Authorization", "tok")]),
            key_for(&[("Authorization", "<sv_t>")], &[("Authorization", "tok")]),
        );
    }

    // --- What the key must not leak ---

    /// The key is printed by the eviction log. It must be a digest, and in
    /// particular must not carry the RESOLVED secret.
    #[test]
    fn the_key_never_carries_its_inputs_verbatim() {
        let k = key_for(
            &[("Authorization", "<sv_token>")],
            &[("Authorization", "Bearer super-secret-value")],
        );
        let s = k.as_str();
        assert!(
            !s.contains("super-secret-value"),
            "the resolved secret leaked into the key"
        );
        assert!(!s.contains("sv_token"), "the reference leaked into the key");
        assert!(
            !s.contains("mcp.example.com"),
            "the url leaked into the key"
        );
        assert_eq!(s.len(), 64, "a sha256 hex digest");
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// The fingerprint is salted, so it is not the bare digest of the secret
    /// an attacker would compute to confirm a guess from a log line.
    #[test]
    fn the_fingerprint_is_salted_not_a_bare_digest_of_the_secret() {
        let fp = CredentialFingerprint::of(&resolved(&[("Authorization", "tok")]));

        let mut unsalted = Sha256::new();
        absorb(&mut unsalted, b"Authorization");
        absorb(&mut unsalted, b"tok");
        assert_ne!(
            fp.0,
            hex_digest(unsalted),
            "an unsalted digest lets anyone with the log confirm a guessed secret"
        );
    }
}
