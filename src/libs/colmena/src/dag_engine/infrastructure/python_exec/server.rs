//! Pieces of the HTTP front for the subprocess executor (`python_executor
//! serve`): the bearer token and the file it is read from, which the jail
//! hides, and the version and deadline of a request.

use super::config::SubprocessConfig;
use super::protocol::WIRE_VERSION;
use axum::http::{header, HeaderMap, StatusCode};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

/// Whether the request carries the token. Digests are compared to the last
/// byte, so the time taken says nothing about the token, its length included.
pub fn authorized(headers: &HeaderMap, token: Option<&str>) -> bool {
    let Some(token) = token else { return true };
    let sent = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let Some(sent) = sent.and_then(|v| v.strip_prefix("Bearer ")) else {
        return false;
    };
    let (a, b) = (Sha256::digest(sent), Sha256::digest(token));
    a.iter().zip(b.iter()).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(serde::Deserialize)]
struct Peek {
    v: u32,
    timeout_ms: u64,
}

/// The deadline the request asks for, capped at `max`; 400 unless the body is
/// a wire request of this version.
pub fn deadline(raw: &[u8], max: Duration) -> Result<Duration, StatusCode> {
    match serde_json::from_slice::<Peek>(raw) {
        Ok(p) if p.v == WIRE_VERSION => Ok(Duration::from_millis(p.timeout_ms).min(max)),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

/// The token of `--token-file`; without one, only loopback is served.
pub fn token(file: Option<&Path>, listen: SocketAddr) -> Result<Option<String>, String> {
    let Some(file) = file else {
        return match listen.ip().is_loopback() {
            true => Ok(None),
            false => Err(format!(
                "{listen} is not a loopback address: set --token-file"
            )),
        };
    };
    let text =
        std::fs::read_to_string(file).map_err(|e| format!("cannot read the token file: {e}"))?;
    match text.trim() {
        "" => Err("the token file is empty".into()),
        token => Ok(Some(token.to_string())),
    }
}

/// The shortest token accepted.
const MIN_TOKEN_BYTES: usize = 32;

/// The executor settings to serve with: `cfg` with the token file hidden in
/// the jail, by its canonical path (absolute, without `..`, through any
/// symlink), so the code the server runs cannot read it and the self-test
/// proves so. The token must be visible ASCII, [`MIN_TOKEN_BYTES`] or more.
pub fn executor_config(
    cfg: &SubprocessConfig,
    token: Option<&str>,
    file: Option<&Path>,
) -> Result<SubprocessConfig, String> {
    let mut cfg = cfg.clone();
    let (Some(token), Some(file)) = (token, file) else {
        return Ok(cfg);
    };
    if token.len() < MIN_TOKEN_BYTES || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(format!(
            "the token must be {MIN_TOKEN_BYTES} or more visible ASCII characters"
        ));
    }
    let path = std::fs::canonicalize(file);
    let path = path.map_err(|e| format!("cannot resolve the token file: {e}"))?;
    cfg.hide_paths.push(path);
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_bearer_token_is_authorized() {
        let bearer = |v: &str| HeaderMap::from_iter([(header::AUTHORIZATION, v.parse().unwrap())]);
        assert!(authorized(&HeaderMap::new(), None));
        assert!(authorized(&bearer("Bearer s3cret"), Some("s3cret")));
        assert!(!authorized(&HeaderMap::new(), Some("s3cret")));
        for wrong in "Bearer s3cre|Bearer s3cret2|s3cret|Basic s3cret|Bearer ".split('|') {
            assert!(!authorized(&bearer(wrong), Some("s3cret")), "{wrong}");
        }
    }

    #[test]
    fn the_token_is_long_enough_and_its_file_hidden_by_its_canonical_path() {
        let cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("token");
        std::fs::write(&file, "t").unwrap();
        std::os::unix::fs::symlink(&file, dir.path().join("link")).unwrap();
        let link = Some(dir.path().join("link"));
        let long = "t".repeat(MIN_TOKEN_BYTES);
        assert_eq!(executor_config(&cfg, None, None), Ok(cfg.clone()));
        let hidden = executor_config(&cfg, Some(&long), link.as_deref())
            .unwrap()
            .hide_paths;
        assert_eq!(hidden, [std::fs::canonicalize(&file).unwrap()]);
        let short = &long[1..];
        assert!(executor_config(&cfg, Some(short), link.as_deref()).is_err());
        assert!(executor_config(&cfg, Some(&format!("{short}\u{e9}")), link.as_deref()).is_err());
    }

    #[test]
    fn the_deadline_is_capped_at_the_maximum() {
        let max = Duration::from_secs(60);
        let asked = |ms: u64| deadline(format!(r#"{{"v":1,"timeout_ms":{ms}}}"#).as_bytes(), max);
        assert_eq!(asked(2500), Ok(Duration::from_millis(2500)));
        assert_eq!(asked(90_000), Ok(max));
        let other_version = deadline(br#"{"v":2,"timeout_ms":1}"#, max);
        assert_eq!(other_version, Err(StatusCode::BAD_REQUEST));
    }

    #[test]
    fn a_token_is_required_beyond_loopback_and_is_never_empty() {
        let (lo, any) = ("127.0.0.1:80".parse().unwrap(), "[::]:80".parse().unwrap());
        assert_eq!(token(None, lo), Ok(None));
        assert!(token(None, any).is_err());
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("token");
        std::fs::write(&file, "  s3cret\n").unwrap();
        assert_eq!(token(Some(&file), any), Ok(Some("s3cret".into())));
        std::fs::write(&file, " \n").unwrap();
        assert!(token(Some(&file), lo).is_err());
        assert!(token(Some(&dir.path().join("missing")), lo).is_err());
    }
}
