//! Auto-injected system-message prelude for agents that have any
//! `gsheets_*` or `gdocs_*` tool exposed.
//!
//! Two responsibilities:
//!   1. Tell the LLM that it needs a Google document ID before it can
//!      operate (extract from URLs or ask the user) — fail fast instead
//!      of guessing.
//!   2. Surface the service-account email so the LLM can instruct the
//!      user to share the doc with it. Without this, every first turn
//!      ends in a `PermissionDenied` round-trip.
//!
//! The SA email is resolved at LlmCallNode build time via
//! `resolve_sa_email`, which checks in order:
//!   * `COLMENA_GOOGLE_SA_EMAIL` env var (operator override, also the
//!     only path that works under ADC since there is no JSON file).
//!   * The `client_email` field of the JSON pointed to by
//!     `GOOGLE_APPLICATION_CREDENTIALS`.
//!   * `None` — emits a degraded prelude that still demands the doc ID
//!     but tells the user to consult the operator for the SA email.
//!
//! Token cost: ~140 tokens with email, ~110 without. Always-on (every
//! turn) — the LLM will simply skip past it after turn 1 once the doc
//! ID is in scope. This trades a small per-turn overhead for
//! deterministic first-turn behavior.

use std::path::Path;

/// Resolve the service account email using the chain documented above.
/// Returns `None` only when both the env var is unset/empty AND the
/// JSON file is missing/unreadable/lacks `client_email`. The result is
/// safe to display in a system message (it's just an identifier; the
/// actual credential is the private key, never returned).
pub fn resolve_sa_email() -> Option<String> {
    // 1. Env-var override (wins so operators can swap without redeploying).
    if let Ok(email) = std::env::var("COLMENA_GOOGLE_SA_EMAIL") {
        let trimmed = email.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    // 2. Read `client_email` out of the SA JSON.
    let path = std::env::var("GOOGLE_APPLICATION_CREDENTIALS").ok()?;
    read_client_email_from_json(Path::new(&path))
}

fn read_client_email_from_json(path: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&contents).ok()?;
    v.get("client_email")
        .and_then(|e| e.as_str())
        .map(|s| s.to_string())
}

/// Build the prelude string. Pass `Some(email)` to embed the SA email
/// inline; `None` falls back to a degraded version that still enforces
/// the doc-ID gate but defers the share recipient to the operator.
pub fn build_google_workspace_prelude(sa_email: Option<&str>) -> String {
    match sa_email {
        Some(email) => format!(
            "## Acceso a Google Workspace\n\
             Tenés tools de Google Docs y/o Sheets habilitadas. Para operar sobre un \
             documento necesitás dos cosas:\n\
             1. El **ID del documento** (lo da el usuario, o se extrae de la URL: \
             `docs.google.com/document/d/<ID>/edit` o `docs.google.com/spreadsheets/d/<ID>/edit`).\n\
             2. Que el documento esté compartido como **Editor** con: `{email}`\n\n\
             Si en este turno NO tenés un doc ID confirmado en la conversación:\n\
             - Pedíselo al usuario explícitamente.\n\
             - Avisale que debe compartir el doc con `{email}` antes de continuar.\n\
             - NO intentes tool calls sobre IDs adivinados o inferidos.\n\n\
             Si el usuario YA mandó un doc ID en esta o en una conversación previa, \
             procedé directo a operar — confiá en que está compartido. Si una tool \
             falla con `PermissionDenied`, recién ahí pedile al usuario que verifique \
             el share con `{email}`.",
            email = email
        ),
        None => "## Acceso a Google Workspace\n\
             Tenés tools de Google Docs y/o Sheets habilitadas. Para operar sobre un \
             documento necesitás dos cosas:\n\
             1. El **ID del documento** (lo da el usuario, o se extrae de la URL: \
             `docs.google.com/document/d/<ID>/edit` o `docs.google.com/spreadsheets/d/<ID>/edit`).\n\
             2. Que el documento esté compartido como **Editor** con el service account \
             configurado para este agente (pedile al operador la dirección si no la sabés).\n\n\
             Si en este turno NO tenés un doc ID confirmado en la conversación, \
             pedíselo al usuario antes de cualquier tool call. NO adivines IDs.\n\n\
             Si una tool falla con `PermissionDenied`, pedile al usuario que verifique \
             el share con el service account."
            .to_string(),
    }
}

/// True when the LLM's exposed-tools list contains any `gsheets_*` or
/// `gdocs_*` tool. Used as the gate for injecting the prelude.
pub fn has_google_workspace_tools<I, S>(tool_names: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    tool_names.into_iter().any(|name| {
        let n = name.as_ref();
        n.starts_with("gsheets_") || n.starts_with("gdocs_")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn prelude_with_email_includes_address_and_share_instruction() {
        let out = build_google_workspace_prelude(Some("agent@startti-dev.iam.gserviceaccount.com"));
        assert!(out.contains("agent@startti-dev.iam.gserviceaccount.com"));
        assert!(out.contains("Editor"));
        assert!(out.contains("doc ID") || out.contains("ID del documento"));
        // Negative: a None fallback shouldn't leak in
        assert!(!out.contains("operador la dirección"));
    }

    #[test]
    fn prelude_without_email_falls_back_gracefully() {
        let out = build_google_workspace_prelude(None);
        assert!(out.contains("operador la dirección"));
        // Email-bearing path should not appear in the degraded version.
        assert!(!out.contains("@"));
    }

    #[test]
    fn has_google_workspace_tools_detects_gsheets_prefix() {
        assert!(has_google_workspace_tools(["gsheets_read", "current_time"]));
        assert!(has_google_workspace_tools(["gdocs_create"]));
        assert!(has_google_workspace_tools(["foo", "gsheets_add_sheet"]));
        assert!(!has_google_workspace_tools(["current_time", "load_skill"]));
        assert!(!has_google_workspace_tools::<[&str; 0], &str>([]));
        // Underscore-free names that happen to start with `gsheets`
        // (none exist today) would NOT match — the prefix is `gsheets_`
        // including the underscore, by design.
        assert!(!has_google_workspace_tools(["gsheetsraw"]));
    }

    #[test]
    fn resolve_sa_email_env_var_wins_over_json() {
        // Write a fake JSON that would resolve to one email...
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(br#"{"client_email":"from-json@example.com"}"#)
            .unwrap();
        f.flush().unwrap();
        // ...but the env var should take priority. Use a unique env var
        // name lookup pattern: set both, expect env var to win.
        // (We cannot safely mutate process env vars in parallel tests,
        // so this is a focused single-env-var test; the suite runs it
        // in isolation if needed via #[serial].)
        std::env::set_var("COLMENA_GOOGLE_SA_EMAIL", "from-env@example.com");
        std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", path.to_str().unwrap());
        let out = resolve_sa_email();
        std::env::remove_var("COLMENA_GOOGLE_SA_EMAIL");
        std::env::remove_var("GOOGLE_APPLICATION_CREDENTIALS");
        assert_eq!(out.as_deref(), Some("from-env@example.com"));
    }

    #[test]
    fn read_client_email_from_json_returns_field() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(
            br#"{"type":"service_account","client_email":"svc@example.com","private_key":"..."}"#,
        )
        .unwrap();
        f.flush().unwrap();
        let out = read_client_email_from_json(&path);
        assert_eq!(out.as_deref(), Some("svc@example.com"));
    }

    #[test]
    fn read_client_email_from_json_missing_field_yields_none() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(br#"{"type":"service_account"}"#).unwrap();
        f.flush().unwrap();
        let out = read_client_email_from_json(&path);
        assert_eq!(out, None);
    }

    #[test]
    fn read_client_email_from_json_invalid_json_yields_none() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"not json").unwrap();
        f.flush().unwrap();
        let out = read_client_email_from_json(&path);
        assert_eq!(out, None);
    }

    #[test]
    fn read_client_email_from_json_missing_file_yields_none() {
        let out = read_client_email_from_json(Path::new("/nonexistent/path/credentials.json"));
        assert_eq!(out, None);
    }
}
