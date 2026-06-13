//! Auto-injected system-message prelude for agents that have any
//! `gsheets_*` or `gdocs_*` tool exposed.
//!
//! Two responsibilities:
//!   1. Tell the LLM that it needs a Google document ID before it can
//!      operate (extract from URLs or ask the user) — fail fast instead
//!      of guessing.
//!   2. Surface the email the agent ACTS AS so the LLM can instruct the
//!      user to share the doc with it. Without this, every first turn
//!      ends in a `PermissionDenied` round-trip.
//!
//! Resolution order (post-OAuth migration 2026-06-10) — first non-empty
//! value wins:
//!   1. `COLMENA_GOOGLE_SHARE_EMAIL` — the OAuth-flow identity (e.g.
//!      `agents@startti.co`). New canonical var; what production
//!      uses.
//!   2. `COLMENA_GOOGLE_SA_EMAIL` — legacy SA email var, retained for
//!      tests / local dev that still run against the SA flow.
//!   3. `client_email` field in the JSON at
//!      `GOOGLE_APPLICATION_CREDENTIALS` — legacy SA JSON fallback,
//!      same audience as (2).
//!   4. `None` — emits a degraded prelude that still demands the doc
//!      ID but tells the user to consult the operator for the email.
//!
//! Token cost: ~215 tokens with email, ~150 without (incluye el bloque
//! de preferencia "compartir > crear" agregado 2026-06-11). Always-on
//! (every turn) — the LLM will simply skip past it after turn 1 once the
//! doc ID is in scope. This trades a small per-turn overhead for
//! deterministic first-turn behavior.

use std::path::Path;

/// Resolve the share email using the chain documented at the top of
/// this module. Returns `None` only when every source is missing or
/// empty. The result is safe to display in a system message — it's
/// just an identifier; the actual credential (refresh_token or
/// private key) is never returned.
///
/// The function is named `resolve_share_email` post-migration. The
/// historical `resolve_sa_email` name is kept as a deprecated alias
/// for any external callers; new callers should use the new name.
pub fn resolve_share_email() -> Option<String> {
    // 1. New canonical var — the OAuth identity.
    if let Some(s) = read_env_nonempty("COLMENA_GOOGLE_SHARE_EMAIL") {
        return Some(s);
    }

    // 2. Legacy SA-flow override (tests / local dev).
    if let Some(s) = read_env_nonempty("COLMENA_GOOGLE_SA_EMAIL") {
        return Some(s);
    }

    // 3. Legacy SA JSON path (tests / local dev).
    let path = std::env::var("GOOGLE_APPLICATION_CREDENTIALS").ok()?;
    read_client_email_from_json(Path::new(&path))
}

/// Deprecated alias preserved for any external callers that imported
/// the pre-OAuth name. New code should call [`resolve_share_email`].
#[deprecated(
    since = "0.4.0",
    note = "Use `resolve_share_email` instead — the name was generalised \
            when OAuth user-scoped auth replaced the SA-only flow."
)]
pub fn resolve_sa_email() -> Option<String> {
    resolve_share_email()
}

fn read_env_nonempty(name: &str) -> Option<String> {
    let raw = std::env::var(name).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
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
///
/// **Wording note (2026-06-10):** the "in turn 1 without a doc_id" branch
/// is intentionally repetitive — the email appears twice, with explicit
/// emphasis that BOTH instructions are mandatory in the same reply. This
/// counters a behaviour we observed with low-temperature `gpt-4o-mini`
/// where the model would compress the prelude's two bullets into a
/// single "give me the ID" reply, dropping the share instruction. The
/// share instruction is the load-bearing part of this prelude — without
/// it the user has no way to know the agent needs to be granted access,
/// so they paste an ID and get `PermissionDenied`. Don't soften this.
/// Workflow guidance appended to the prelude whenever any gsheets/gdocs tool is
/// active: how to understand a sheet's shape BEFORE reading or comparing it.
/// Scoped to spreadsheet work ("una planilla"); harmless for gdocs-only agents.
const SHEET_WORKFLOW_PRELUDE: &str = "### Entender una hoja ANTES de leerla o compararla\n\
     Antes de operar sobre una planilla, entendé su forma: leéla primero con \
     `gsheets_read` (devuelve una tabla markdown + `dimensions` {filas, columnas}). \
     El encabezado NO siempre está en la primera fila — puede haber filas de título \
     arriba, o ser una tabla clave→valor sin encabezados clásicos. Usá ese preview \
     para ubicar dónde empiezan el header y los datos reales.\n\
     Para COMPARAR o analizar datos, hacelo en CÓDIGO con `gsheets_run_python` \
     (nunca comparando a ojo): cargá cada tabla como un binding (una hoja, o datos \
     inline via `data:` — p.ej. una tabla que extrajiste de una imagen) y arrancá \
     el `range` de cada binding de hoja en la fila del header/datos real, para que \
     los registros tomen las claves correctas.\n\
     Celdas combinadas (merged): Google deja el valor SOLO en la celda superior \
     izquierda; las demás del bloque vienen vacías.";

pub fn build_google_workspace_prelude(sa_email: Option<&str>) -> String {
    let base = match sa_email {
        Some(email) => format!(
            "## Acceso a Google Workspace\n\
             Tenés tools de Google Docs y/o Sheets habilitadas. Para operar sobre \
             un documento NECESITÁS DOS COSAS, no una:\n\
             1. El **ID del documento** (lo da el usuario, o se extrae de la URL: \
             `docs.google.com/document/d/<ID>/edit` o `docs.google.com/spreadsheets/d/<ID>/edit`).\n\
             2. Que el documento esté compartido como **Editor** con: `{email}`\n\n\
             ### Preferí COMPARTIR un doc existente antes que CREAR uno nuevo\n\
             Cuando el usuario necesita una planilla o un documento, la opción por \
             defecto es pedirle que **comparta uno EXISTENTE** con vos (como Editor, \
             con `{email}`): ese archivo queda bajo el ownership y control del \
             usuario, en SU Drive. Crear uno nuevo con `gsheets_create_spreadsheet` \
             o `gdocs_create*` lo deja en la cuenta del agente (`{email}`), NO en la \
             del usuario — reservá esa opción para cuando el usuario lo pida \
             explícitamente o no tenga nada que compartir, y avisale que el archivo \
             va a vivir en la cuenta del agente.\n\n\
             ### Si el usuario YA mandó un doc ID en este turno o en uno previo\n\
             Procedé directo a operar — confiá en que el ID que pasó es real y que \
             está compartido. NO le vuelvas a pedir el ID. Si la tool falla con \
             `permission_denied`, el tool result va a traer un `hint` actionable: \
             paraphraseálo al usuario citando el email `{email}` literal, y pedile \
             que verifique que el share está hecho como Editor (no Viewer).\n\n\
             ### Si el usuario NO mandó un doc ID en ningún turno\n\
             Tu respuesta DEBE incluir AMBAS instrucciones siguientes en el mismo \
             mensaje, no una sola:\n\
             1. Pedile el doc ID al usuario (mostrale dónde sacarlo de la URL).\n\
             2. Decile explícitamente que ANTES de seguir tiene que compartir el doc \
             como Editor con `{email}` (poné el email literal, no escribas \"el agent\" \
             o \"el service account\" como sustituto).\n\n\
             NO mandes una respuesta que solo pida el ID — el usuario no va a saber \
             que también necesita compartir.",
            email = email
        ),
        None => "## Acceso a Google Workspace\n\
             Tenés tools de Google Docs y/o Sheets habilitadas. Para operar sobre \
             un documento NECESITÁS DOS COSAS, no una:\n\
             1. El **ID del documento** (lo da el usuario, o se extrae de la URL: \
             `docs.google.com/document/d/<ID>/edit` o `docs.google.com/spreadsheets/d/<ID>/edit`).\n\
             2. Que el documento esté compartido como **Editor** con el service account \
             configurado para este agente (pedile al operador la dirección si no la sabés).\n\n\
             Preferí que el usuario COMPARTA un doc EXISTENTE (queda bajo su ownership, \
             en su Drive) antes que crear uno nuevo con `gsheets_create_spreadsheet` / \
             `gdocs_create*` (que lo dejaría en la cuenta del agente). Creá uno nuevo \
             solo si el usuario lo pide o no tiene nada que compartir.\n\n\
             Si el usuario ya mandó un doc ID, procedé directo. Si la tool falla con \
             `permission_denied`, el tool result trae un `hint` accionable que debés \
             paraphrasear al usuario.\n\n\
             Si el usuario NO mandó un doc ID, tu respuesta DEBE incluir AMBAS \
             instrucciones: pedirle el ID y avisarle que debe compartir el doc como \
             Editor."
            .to_string(),
    };
    format!("{base}\n\n{SHEET_WORKFLOW_PRELUDE}")
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
    use serial_test::serial;
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

    /// The "prefer sharing an existing doc over creating one in the agent
    /// account" guidance must be present in BOTH prelude variants. This is
    /// a product requirement (2026-06-11): created docs live in the agent
    /// account, not the user's Drive, so sharing is the default path.
    #[test]
    fn prelude_prefers_share_over_create_in_both_variants() {
        for out in [
            build_google_workspace_prelude(Some("agents@startti.co")),
            build_google_workspace_prelude(None),
        ] {
            let lower = out.to_lowercase();
            assert!(
                lower.contains("comparta") || lower.contains("compartir"),
                "prelude must steer toward the user sharing a doc"
            );
            assert!(
                lower.contains("existente"),
                "prelude must mention preferring an EXISTING doc"
            );
            assert!(
                lower.contains("crear") || lower.contains("create"),
                "prelude must contrast against creating a new doc"
            );
            // The create tools must be named so the LLM knows what to avoid by default.
            assert!(out.contains("gsheets_create_spreadsheet") || out.contains("gdocs_create"));
        }
    }

    #[test]
    fn prelude_includes_understand_sheet_workflow_in_both_variants() {
        for out in [
            build_google_workspace_prelude(Some("agents@startti.co")),
            build_google_workspace_prelude(None),
        ] {
            let lower = out.to_lowercase();
            // Read first to understand the sheet shape.
            assert!(
                out.contains("gsheets_read"),
                "prelude must tell the LLM to read the sheet first"
            );
            // The header is not always the first row.
            assert!(
                lower.contains("no siempre está en la primera fila"),
                "prelude must warn the header is not always row 1"
            );
            // Compare in code, not by eye.
            assert!(
                out.contains("gsheets_run_python") && lower.contains("a ojo"),
                "prelude must steer comparisons to code (gsheets_run_python), not eyeballing"
            );
            // Merged-cell behavior.
            assert!(
                lower.contains("merged") || lower.contains("combinadas"),
                "prelude must note merged-cell behavior"
            );
        }
    }

    /// Regression pin: the email MUST appear at least twice when the
    /// prelude is fired with a known share email. Once for the
    /// always-present instructions block ("debe estar compartido con
    /// {email}") and once for the first-turn explicit reminder. The
    /// duplication is the load-bearing fix for `gpt-4o-mini` compressing
    /// the prelude into a single "give me the ID" reply.
    ///
    /// If you ever DRY this and the email appears only once, models
    /// will silently drop the share instruction.
    #[test]
    fn prelude_with_email_repeats_email_for_mandatory_first_turn_instructions() {
        let email = "agents@startti.co";
        let out = build_google_workspace_prelude(Some(email));
        let occurrences = out.matches(email).count();
        assert!(
            occurrences >= 2,
            "email must appear at least twice in prelude (intentional repetition); \
             got {occurrences}. Did someone DRY the prelude?"
        );
        // The "two things, not one" anti-compression language must be
        // present — it's the load-bearing prompt-engineering bit.
        assert!(
            out.contains("DOS COSAS") || out.contains("dos cosas"),
            "prelude lost the explicit 'two things' anti-compression hint"
        );
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
    #[serial]
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
        std::env::remove_var("COLMENA_GOOGLE_SHARE_EMAIL");
        std::env::set_var("COLMENA_GOOGLE_SA_EMAIL", "from-env@example.com");
        std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", path.to_str().unwrap());
        let out = resolve_share_email();
        std::env::remove_var("COLMENA_GOOGLE_SA_EMAIL");
        std::env::remove_var("GOOGLE_APPLICATION_CREDENTIALS");
        assert_eq!(out.as_deref(), Some("from-env@example.com"));
    }

    /// Pins the precedence: `COLMENA_GOOGLE_SHARE_EMAIL` (OAuth-era
    /// canonical var) MUST win when set alongside both the legacy SA
    /// env var and the SA JSON file. Catches regressions where someone
    /// reorders the resolution chain.
    #[test]
    #[serial]
    fn resolve_share_email_oauth_var_wins_over_everything() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(br#"{"client_email":"from-json@example.com"}"#)
            .unwrap();
        f.flush().unwrap();

        std::env::set_var("COLMENA_GOOGLE_SHARE_EMAIL", "agents@startti.co");
        std::env::set_var("COLMENA_GOOGLE_SA_EMAIL", "legacy-sa@example.com");
        std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", path.to_str().unwrap());
        let out = resolve_share_email();
        std::env::remove_var("COLMENA_GOOGLE_SHARE_EMAIL");
        std::env::remove_var("COLMENA_GOOGLE_SA_EMAIL");
        std::env::remove_var("GOOGLE_APPLICATION_CREDENTIALS");
        assert_eq!(out.as_deref(), Some("agents@startti.co"));
    }

    /// When only the legacy SA-flow vars are set, those must still
    /// resolve — local-dev and test environments that haven't
    /// migrated to the OAuth canonical name should keep working.
    #[test]
    #[serial]
    fn resolve_share_email_falls_back_to_legacy_sa_var() {
        std::env::remove_var("COLMENA_GOOGLE_SHARE_EMAIL");
        std::env::set_var("COLMENA_GOOGLE_SA_EMAIL", "fallback@example.com");
        std::env::remove_var("GOOGLE_APPLICATION_CREDENTIALS");
        let out = resolve_share_email();
        std::env::remove_var("COLMENA_GOOGLE_SA_EMAIL");
        assert_eq!(out.as_deref(), Some("fallback@example.com"));
    }

    /// Empty/whitespace-only values must NOT win — they should be
    /// treated as unset so the fallback chain proceeds. Catches the
    /// case where a Secret Manager mount resolves to an empty string
    /// because the secret version is missing.
    #[test]
    #[serial]
    fn resolve_share_email_treats_empty_share_email_as_unset() {
        std::env::set_var("COLMENA_GOOGLE_SHARE_EMAIL", "   ");
        std::env::set_var("COLMENA_GOOGLE_SA_EMAIL", "fallback@example.com");
        std::env::remove_var("GOOGLE_APPLICATION_CREDENTIALS");
        let out = resolve_share_email();
        std::env::remove_var("COLMENA_GOOGLE_SHARE_EMAIL");
        std::env::remove_var("COLMENA_GOOGLE_SA_EMAIL");
        assert_eq!(out.as_deref(), Some("fallback@example.com"));
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
