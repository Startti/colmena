//! Redaction of credentials in the `config` and `inputs` a `node-start` frame
//! echoes (defense in depth, CX7).
//!
//! Those frames travel to the embedder and can sit in its event store for
//! hours. The engine masks decrypted secure values elsewhere, but a literal
//! token, a `${VAR}`-resolved one or a host-issued handle written into a
//! node's config would otherwise go out verbatim. Only the VALUES of the keys
//! below are replaced; every other field (labels, provider, model, urls) stays
//! so the frame is still useful for debugging.

use serde_json::{Map, Value};

/// What a redacted value becomes.
pub const REDACTED: &str = "[redacted]";

/// Keys whose value is a credential, written NORMALIZED (see [`normalize`]):
/// lowercase, `-` as `_`, so `X-Goog-Api-Key`, `x_goog_api_key` and
/// `X_GOOG_API_KEY` are one entry. A matching key's whole value is replaced,
/// an object included (`auth`, `bearer_refresh`, `auth_refresh`,
/// `google_workspace_auth`).
pub const SECRET_KEYS: &[&str] = &[
    "access_token",
    "api_key",
    "apikey",
    "auth",
    "auth_refresh",
    "authorization",
    "aws_secret_access_key",
    "bearer_refresh",
    "bearer_token",
    "client_secret",
    "connection_url",
    "cookie",
    "credentials",
    "google_workspace_auth",
    "id_token",
    "ocp_apim_subscription_key",
    "passwd",
    "password",
    "private_key",
    "private_token",
    "proxy_authorization",
    "refresh_token",
    "secret",
    "secret_key",
    "session_token",
    "set_cookie",
    "token",
    "x_api_key",
    "x_auth_token",
    "x_goog_api_key",
];

/// Extra names masked ONLY as URL query parameters (normalized): too generic
/// to hide as a config field (`key`, `code`), but a credential in a query
/// string (an API key, a presigned signature, an OAuth code).
pub const SECRET_QUERY_PARAMS: &[&str] = &[
    "access_token",
    "code",
    "key",
    "sig",
    "signature",
    "token",
    "x_amz_signature",
];

/// Lowercase, `-` as `_`: the form both lists are written in.
fn normalize(key: &str) -> String {
    key.to_ascii_lowercase().replace('-', "_")
}

fn is_secret_key(key: &str) -> bool {
    SECRET_KEYS.contains(&normalize(key).as_str())
}

fn is_secret_query_param(key: &str) -> bool {
    let k = normalize(key);
    SECRET_KEYS.contains(&k.as_str()) || SECRET_QUERY_PARAMS.contains(&k.as_str())
}

/// `value` with every secret key's value replaced by [`REDACTED`], at any
/// depth, and secret query parameters masked in URL strings.
pub fn redact_secrets(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let v = match is_secret_key(k) {
                        true => Value::String(REDACTED.into()),
                        false => redact_secrets(v),
                    };
                    (k.clone(), v)
                })
                .collect::<Map<_, _>>(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact_secrets).collect()),
        Value::String(s) => Value::String(redact_url_query(s)),
        other => other.clone(),
    }
}

/// An `http(s)://` URL keeps its path and non-secret parameters; the value of a
/// query parameter named in [`SECRET_KEYS`] or [`SECRET_QUERY_PARAMS`] becomes
/// [`REDACTED`].
fn redact_url_query(s: &str) -> String {
    let is_url = s.starts_with("https://") || s.starts_with("http://");
    let Some((base, query)) = s.split_once('?').filter(|_| is_url) else {
        return s.to_string();
    };
    let pairs = query.split('&').map(|pair| match pair.split_once('=') {
        Some((k, _)) if is_secret_query_param(k) => format!("{k}={REDACTED}"),
        _ => pair.to_string(),
    });
    format!("{base}?{}", pairs.collect::<Vec<_>>().join("&"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn secret_values_are_replaced_at_any_depth_and_the_rest_stays() {
        let v = json!({
            "node_label": "Get", "provider": "openai", "model": "m",
            "headers": { "AUTHORIZATION": "Bearer tok-cx7-a", "Accept": "json" },
            "steps": [{ "query_params": { "Api_Key": "tok-cx7-b", "q": "x" } }],
            "url": "https://api.example.com/x?token=tok-cx7-c&page=2",
        });
        let out = redact_secrets(&v);
        assert!(!out.to_string().contains("tok-cx7-"), "{out}");
        assert_eq!(out["headers"]["Accept"], "json");
        assert_eq!(out["steps"][0]["query_params"]["q"], "x");
        assert_eq!(
            out["url"],
            "https://api.example.com/x?token=[redacted]&page=2"
        );
        assert_eq!(
            (out["node_label"].as_str(), out["model"].as_str()),
            (Some("Get"), Some("m"))
        );
    }

    /// Hyphenated and mixed-case names normalize to one entry; `key`, `sig`
    /// and a presigned `X-Amz-Signature` are masked in a URL query, while a
    /// config FIELD named `key` stays.
    #[test]
    fn names_normalize_and_url_only_params_stay_in_config() {
        let v = json!({
            "headers": { "X-Goog-Api-Key": "tok-cx7-a", "Ocp-Apim-Subscription-Key": "tok-cx7-b" },
            "key": "sheet-id",
            "a": "https://h.example/x?key=tok-cx7-c&alt=json",
            "b": "https://s3.example/o?X-Amz-Signature=tok-cx7-d&X-Amz-Date=1",
        });
        let out = redact_secrets(&v);
        assert!(!out.to_string().contains("tok-cx7-"), "{out}");
        assert_eq!(out["key"], "sheet-id");
        assert_eq!(out["a"], "https://h.example/x?key=[redacted]&alt=json");
        assert_eq!(
            out["b"],
            "https://s3.example/o?X-Amz-Signature=[redacted]&X-Amz-Date=1"
        );
    }
}
