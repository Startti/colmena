//! One-time OAuth consent flow for the colmena agent.
//!
//! Run on an operator's laptop to obtain the `refresh_token` that
//! production needs. The refresh_token is printed to stdout — the
//! operator is expected to copy it into Google Secret Manager
//! immediately and clear their terminal history afterwards.
//!
//! Usage:
//!
//! ```text
//! cargo run --bin colmena_oauth_setup -- --client-secret ~/.colmena/oauth_client_secret.json
//! ```
//!
//! Flow:
//!   1. Reads `client_id` + `client_secret` from the JSON file
//!      downloaded from GCP console (OAuth Client — Desktop type).
//!   2. Opens the operator's default browser to Google's consent URL
//!      with `access_type=offline` + `prompt=consent` (mandatory to
//!      receive a refresh_token).
//!   3. Spins up an axum server on `localhost:<port>` (default 8080)
//!      to receive the auth_code redirect.
//!   4. Exchanges the auth_code for an access_token + refresh_token
//!      via `POST oauth2.googleapis.com/token`.
//!   5. Prints the refresh_token to stdout, shows a small "you can
//!      close this tab" HTML in the browser, exits 0.
//!
//! Scopes hard-coded:
//!   - `drive.file`     (per-file access to created/shared docs)
//!   - `documents`      (Google Docs API)
//!   - `spreadsheets`   (Google Sheets API)
//!
//! These match what production needs. Operators who want different
//! scopes should edit this file (rare).

use axum::extract::{Query, State};
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use clap::Parser;
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::oneshot;

const OAUTH_AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const OAUTH_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

const REQUESTED_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/drive.file",
    "https://www.googleapis.com/auth/documents",
    "https://www.googleapis.com/auth/spreadsheets",
];

#[derive(Parser, Debug)]
#[command(
    name = "colmena_oauth_setup",
    version,
    about = "One-time OAuth consent flow that produces a refresh_token \
             for production deployment.",
    long_about = None,
)]
struct Args {
    /// Path to the OAuth client_secret JSON file downloaded from
    /// GCP console (APIs & Services → Credentials → OAuth Client).
    /// Must be type "Desktop app".
    #[arg(long = "client-secret", short = 'c')]
    client_secret_path: std::path::PathBuf,

    /// TCP port for the local redirect URI. Must match a URI
    /// registered on the OAuth client (defaults to 8080 which Google
    /// allows for "Desktop app" clients without explicit registration).
    #[arg(long = "port", default_value_t = 8080)]
    port: u16,
}

/// What we read out of the downloaded client_secret.json. Google ships
/// two top-level shapes — `installed` for Desktop clients, `web` for
/// Web clients. We support both for robustness.
#[derive(Debug, Deserialize)]
struct ClientSecretFile {
    #[serde(rename = "installed")]
    installed: Option<ClientSecretFields>,
    #[serde(rename = "web")]
    web: Option<ClientSecretFields>,
}

#[derive(Debug, Deserialize)]
struct ClientSecretFields {
    client_id: String,
    client_secret: String,
}

impl ClientSecretFile {
    fn into_fields(self) -> Result<ClientSecretFields, String> {
        self.installed
            .or(self.web)
            .ok_or_else(|| "client_secret.json missing both `installed` and `web` blocks".into())
    }
}

#[derive(Debug, Deserialize)]
struct AuthRedirectQuery {
    /// The authorization code Google appends after consent. Present
    /// on success.
    code: Option<String>,
    /// Set on user-cancel / denied consent.
    error: Option<String>,
    /// Human-readable error description from Google.
    error_description: Option<String>,
}

/// Shared state passed into the axum handler. Holds the oneshot
/// sender so the handler can wake up `main` once the redirect lands.
struct AppState {
    sender: tokio::sync::Mutex<Option<oneshot::Sender<Result<String, String>>>>,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args = Args::parse();

    match run(args).await {
        Ok(refresh_token) => {
            // The big visual marker before printing the token tells
            // the operator exactly which line to copy. Keep this format
            // stable — docs reference it.
            println!();
            println!("════════════════════════════════════════════════════════════════════");
            println!("  ✓ Consent successful. Refresh token (copy into Secret Manager):");
            println!("════════════════════════════════════════════════════════════════════");
            println!();
            println!("{refresh_token}");
            println!();
            println!("Next steps:");
            println!("  1. Upload to Secret Manager:");
            println!("     gcloud secrets create colmena-oauth-refresh-token --data-file=-");
            println!("     (paste the token above, then Ctrl-D)");
            println!("  2. Clear terminal history immediately:");
            println!("     history -c   # bash/zsh");
            println!("  3. See docs/developer_guide/47_google_oauth.md for the rest");
            println!("     (client_id / client_secret as secrets, deploy_gcp.sh updates).");
            std::process::ExitCode::SUCCESS
        }
        Err(msg) => {
            eprintln!("✗ Consent flow failed: {msg}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<String, String> {
    // 1. Load client credentials from the downloaded JSON.
    let raw = std::fs::read_to_string(&args.client_secret_path)
        .map_err(|e| format!("cannot read {:?}: {e}", args.client_secret_path))?;
    let file: ClientSecretFile = serde_json::from_str(&raw)
        .map_err(|e| format!("cannot parse {:?} as JSON: {e}", args.client_secret_path))?;
    let fields = file.into_fields()?;

    // 2. Build the authorization URL.
    let redirect_uri = format!("http://localhost:{}/", args.port);
    let scopes = REQUESTED_SCOPES.join(" ");
    let auth_url = build_auth_url(&fields.client_id, &redirect_uri, &scopes);

    // 3. Spin up the redirect server and obtain the auth code.
    let (tx, rx) = oneshot::channel::<Result<String, String>>();
    let state = Arc::new(AppState {
        sender: tokio::sync::Mutex::new(Some(tx)),
    });
    let app = Router::new()
        .route("/", get(handle_redirect))
        .with_state(state.clone());
    let addr: SocketAddr = ([127, 0, 0, 1], args.port).into();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("cannot bind {addr}: {e}"))?;
    let server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // 4. Open the browser. If the operator's environment has no
    // default browser, fall back to printing the URL.
    eprintln!("→ Opening browser to consent screen…");
    eprintln!("  (If the browser does not open, visit this URL manually:)");
    eprintln!("  {auth_url}");
    eprintln!();
    if let Err(e) = webbrowser::open(&auth_url) {
        eprintln!("Warning: failed to open browser automatically: {e}");
        eprintln!("Paste the URL above into a browser logged in as agents@startti.co.");
    }

    // 5. Wait for the auth code.
    eprintln!("→ Waiting for consent (the browser will redirect here when you click Permitir)…");
    let auth_code = rx
        .await
        .map_err(|e| format!("auth-code channel closed without sending: {e}"))??;

    server_handle.abort();

    // 6. Exchange the code for a refresh_token.
    eprintln!("→ Exchanging auth_code for refresh_token…");
    let refresh_token = exchange_code_for_refresh_token(&fields, &redirect_uri, &auth_code).await?;

    Ok(refresh_token)
}

fn build_auth_url(client_id: &str, redirect_uri: &str, scopes: &str) -> String {
    let encoded_redirect = urlencoding::encode(redirect_uri);
    let encoded_scopes = urlencoding::encode(scopes);
    format!(
        "{OAUTH_AUTHORIZE_URL}\
         ?response_type=code\
         &client_id={client_id}\
         &redirect_uri={encoded_redirect}\
         &scope={encoded_scopes}\
         &access_type=offline\
         &prompt=consent\
         &include_granted_scopes=false"
    )
}

/// Axum handler — Google redirects the browser here after the user
/// approves consent (or denies). Send the result through the channel
/// so `main` can proceed.
async fn handle_redirect(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AuthRedirectQuery>,
) -> Html<&'static str> {
    let result = match (q.code, q.error) {
        (Some(code), _) => Ok(code),
        (_, Some(err)) => Err(format!(
            "Google returned error: {err} ({})",
            q.error_description
                .unwrap_or_else(|| "no description".into())
        )),
        (None, None) => Err("redirect missing both `code` and `error`".into()),
    };

    let mut sender_guard = state.sender.lock().await;
    if let Some(sender) = sender_guard.take() {
        let _ = sender.send(result.clone());
    }
    drop(sender_guard);

    match result {
        Ok(_) => Html(SUCCESS_HTML),
        Err(_) => Html(FAILURE_HTML),
    }
}

const SUCCESS_HTML: &str = r#"<!doctype html>
<html><head><title>Colmena consent OK</title></head>
<body style="font-family: system-ui, sans-serif; padding: 2em; max-width: 600px;">
<h2>✓ Consent successful</h2>
<p>You can close this tab. Return to your terminal — the refresh token
will be printed there.</p>
</body></html>"#;

const FAILURE_HTML: &str = r#"<!doctype html>
<html><head><title>Colmena consent failed</title></head>
<body style="font-family: system-ui, sans-serif; padding: 2em; max-width: 600px;">
<h2>✗ Consent failed</h2>
<p>Return to your terminal for the error message. You can close this tab.</p>
</body></html>"#;

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn exchange_code_for_refresh_token(
    fields: &ClientSecretFields,
    redirect_uri: &str,
    code: &str,
) -> Result<String, String> {
    let http = reqwest::Client::new();
    let resp = http
        .post(OAUTH_TOKEN_URL)
        .form(&[
            ("code", code),
            ("client_id", fields.client_id.as_str()),
            ("client_secret", fields.client_secret.as_str()),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| format!("token exchange request: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("read token response body: {e}"))?;
    let parsed: TokenResponse = serde_json::from_str(&body)
        .map_err(|e| format!("parse token response (status {status}, body: {body}): {e}"))?;

    if !status.is_success() {
        return Err(format!(
            "Google rejected the code exchange ({status}): {} — {}",
            parsed.error.unwrap_or_default(),
            parsed.error_description.unwrap_or_default()
        ));
    }

    let _ = parsed.access_token; // not used — we want the refresh_token only.
    parsed.refresh_token.ok_or_else(|| {
        "Google response had no refresh_token. Ensure access_type=offline + \
         prompt=consent were used (they are by default in this tool)."
            .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_url_contains_required_params() {
        let url = build_auth_url(
            "test-client-id",
            "http://localhost:8080/",
            "https://www.googleapis.com/auth/drive.file",
        );
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=test-client-id"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
        // Redirect URI is URL-encoded.
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A8080%2F"));
        // Scope is URL-encoded (`:` and `/` get percent-encoded).
        assert!(url.contains("scope=https%3A%2F%2F"));
    }

    #[test]
    fn parses_installed_client_secret_shape() {
        let raw = r#"{
          "installed": {
            "client_id": "installed-cid",
            "client_secret": "installed-csec",
            "redirect_uris": ["http://localhost"]
          }
        }"#;
        let file: ClientSecretFile = serde_json::from_str(raw).unwrap();
        let fields = file.into_fields().unwrap();
        assert_eq!(fields.client_id, "installed-cid");
        assert_eq!(fields.client_secret, "installed-csec");
    }

    #[test]
    fn parses_web_client_secret_shape() {
        let raw = r#"{
          "web": {
            "client_id": "web-cid",
            "client_secret": "web-csec"
          }
        }"#;
        let file: ClientSecretFile = serde_json::from_str(raw).unwrap();
        let fields = file.into_fields().unwrap();
        assert_eq!(fields.client_id, "web-cid");
        assert_eq!(fields.client_secret, "web-csec");
    }

    #[test]
    fn rejects_client_secret_with_neither_block() {
        let raw = r#"{}"#;
        let file: ClientSecretFile = serde_json::from_str(raw).unwrap();
        let err = file.into_fields().unwrap_err();
        assert!(err.contains("missing both"));
    }
}
