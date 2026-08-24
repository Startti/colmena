//! Central factory for the crate's `reqwest` HTTP clients.
//!
//! Every outbound HTTP client in Colmena is built here so a single place
//! governs TLS trust. The crate pins `rustls` with the built-in Mozilla
//! `webpki-roots` (see `reqwest` features in `Cargo.toml`): a fixed, portable
//! trust store compiled into the binary, so it behaves identically in a Cloud
//! Run container regardless of the host's certificate store.
//!
//! That portability is exactly why the client rejects a TLS-intercepting proxy
//! (e.g. Proxon) on a developer's machine: the proxy re-signs upstream
//! certificates with its own root CA, which is installed in the OS keychain but
//! is NOT in the embedded `webpki-roots` — so validation fails with
//! `invalid peer certificate: UnknownIssuer`.
//!
//! [`builder`] closes that gap without weakening anything: when the
//! [`EXTRA_CA_CERT_ENV`] environment variable points at a PEM file, its
//! certificate(s) are ADDED to the trusted roots (the built-in roots stay
//! trusted, and verification is never disabled — this is not
//! `danger_accept_invalid_certs`). When the variable is unset — the default in
//! production, CI, and any machine without such a proxy — the builder is byte
//! -for-byte a plain `reqwest::Client::builder()` with no behavioral change.

use reqwest::{Client, ClientBuilder};

/// Environment variable naming a PEM file with one or more extra root CA
/// certificates to trust, on top of the built-in `webpki-roots`.
///
/// Unset in production. A developer behind a TLS-intercepting proxy sets it to
/// that proxy's root CA, e.g. for Proxon:
/// `COLMENA_EXTRA_CA_CERT="$HOME/Library/Group Containers/group.com.proxon.observer/proxon-ca.cert.pem"`.
pub const EXTRA_CA_CERT_ENV: &str = "COLMENA_EXTRA_CA_CERT";

/// A [`reqwest::ClientBuilder`] pre-seeded with any extra root CA from
/// [`EXTRA_CA_CERT_ENV`]. Use this instead of `reqwest::Client::builder()` so
/// per-client configuration (timeouts, `http1_only`, …) can be chained on top
/// while still honoring the extra-CA env var.
///
/// With the env var unset this is exactly `reqwest::Client::builder()`.
pub fn builder() -> ClientBuilder {
    let mut b = Client::builder();
    for cert in load_extra_ca_certs() {
        b = b.add_root_certificate(cert);
    }
    b
}

/// A default [`reqwest::Client`] with any extra CA applied — the drop-in
/// replacement for `reqwest::Client::new()`.
///
/// Falls back to a plain `Client::new()` only if the builder fails, which for
/// the default configuration should never happen.
pub fn client() -> Client {
    builder().build().unwrap_or_else(|e| {
        tracing::warn!(
            target: "colmena::http",
            event = "http_client.build_failed",
            error = %e,
            "falling back to a default reqwest client without extra CA"
        );
        Client::new()
    })
}

/// Load the extra CA certificate(s) named by [`EXTRA_CA_CERT_ENV`].
///
/// Returns an empty vec (and leaves the built-in roots as the only trust
/// anchors) when the var is unset/empty, the file is unreadable, or it holds no
/// valid PEM certificate — a misconfiguration must not silently break every
/// request, so it degrades to the default trust store with a warning.
fn load_extra_ca_certs() -> Vec<reqwest::Certificate> {
    let path = match std::env::var(EXTRA_CA_CERT_ENV) {
        Ok(p) if !p.trim().is_empty() => p,
        _ => return Vec::new(),
    };

    let pem = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!(
                target: "colmena::http",
                event = "extra_ca.read_failed",
                path = %path,
                error = %e,
                "COLMENA_EXTRA_CA_CERT is set but the file could not be read; using built-in roots only"
            );
            return Vec::new();
        }
    };

    let certs = parse_pem_certificates(&pem);
    if certs.is_empty() {
        tracing::warn!(
            target: "colmena::http",
            event = "extra_ca.none_parsed",
            path = %path,
            "COLMENA_EXTRA_CA_CERT file contained no valid PEM certificate; using built-in roots only"
        );
    } else {
        tracing::info!(
            target: "colmena::http",
            event = "extra_ca.loaded",
            path = %path,
            count = certs.len(),
            "trusting {} extra root CA certificate(s) from COLMENA_EXTRA_CA_CERT",
            certs.len()
        );
    }
    certs
}

/// Parse every `CERTIFICATE` block in a PEM blob. `reqwest::Certificate::from_pem`
/// reads only the first block, so a bundle is split first; a malformed block is
/// skipped (with a warning) rather than discarding the whole file.
fn parse_pem_certificates(pem: &[u8]) -> Vec<reqwest::Certificate> {
    split_pem_blocks(pem)
        .into_iter()
        .filter_map(
            |block| match reqwest::Certificate::from_pem(block.as_bytes()) {
                Ok(cert) => Some(cert),
                Err(e) => {
                    tracing::warn!(
                        target: "colmena::http",
                        event = "extra_ca.parse_failed",
                        error = %e,
                        "skipping a malformed certificate block in COLMENA_EXTRA_CA_CERT"
                    );
                    None
                }
            },
        )
        .collect()
}

/// Split a PEM blob into individual `-----BEGIN/END CERTIFICATE-----` blocks
/// (markers included). Non-certificate text and a trailing unterminated block
/// are ignored. Pure string handling — no certificate validation here.
fn split_pem_blocks(pem: &[u8]) -> Vec<String> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";

    let text = String::from_utf8_lossy(pem);
    let mut blocks = Vec::new();
    let mut rest = text.as_ref();
    while let Some(bpos) = rest.find(BEGIN) {
        let after = &rest[bpos..];
        match after.find(END) {
            Some(epos) => {
                let end = epos + END.len();
                blocks.push(after[..end].to_string());
                rest = &after[end..];
            }
            None => break,
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_finds_each_certificate_block() {
        let two = "noise before\n\
                   -----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n\
                   junk between\n\
                   -----BEGIN CERTIFICATE-----\nBBBB\n-----END CERTIFICATE-----\n\
                   trailing";
        let blocks = split_pem_blocks(two.as_bytes());
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].contains("AAAA") && blocks[0].starts_with("-----BEGIN"));
        assert!(blocks[1].contains("BBBB") && blocks[1].ends_with("-----END CERTIFICATE-----"));
    }

    #[test]
    fn split_ignores_an_unterminated_block() {
        let one = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n\
                   -----BEGIN CERTIFICATE-----\nno end marker here";
        assert_eq!(split_pem_blocks(one.as_bytes()).len(), 1);
    }

    #[test]
    fn split_returns_nothing_for_non_pem() {
        assert!(split_pem_blocks(b"not a certificate at all").is_empty());
    }

    #[test]
    fn text_with_no_certificate_block_parses_to_zero_certs() {
        // `reqwest::Certificate::from_pem` validates lazily (at handshake time),
        // so it may accept a malformed block — but with NO block at all there is
        // nothing to hand it, so the result is reliably empty and never panics.
        assert!(parse_pem_certificates(b"just some logs, no PEM here").is_empty());
    }

    #[test]
    #[serial_test::serial(colmena_extra_ca_env)]
    fn unset_env_yields_no_extra_certs_and_a_usable_builder() {
        // NOTE: relies on COLMENA_EXTRA_CA_CERT being unset in the test env,
        // which is the default. `builder()` must always produce a valid client.
        std::env::remove_var(EXTRA_CA_CERT_ENV);
        assert!(load_extra_ca_certs().is_empty());
        assert!(
            builder().build().is_ok(),
            "default builder must always build"
        );
    }

    #[test]
    #[serial_test::serial(colmena_extra_ca_env)]
    fn a_missing_file_degrades_to_builtin_roots() {
        std::env::set_var(EXTRA_CA_CERT_ENV, "/nonexistent/proxon-ca.cert.pem");
        // Must not panic or error — just no extra certs.
        assert!(load_extra_ca_certs().is_empty());
        assert!(builder().build().is_ok());
        std::env::remove_var(EXTRA_CA_CERT_ENV);
    }
}
