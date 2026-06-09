//! Domain errors for the Google Docs subsystem.
//!
//! Every variant maps to a stable JSON shape in the dispatcher layer
//! (see `gdocs_tools::error_to_json`). The content-addressed edit
//! errors (`AmbiguousMatch`, `TextNotFound`, `ConfirmManyMatches`,
//! `HumanChangesPending`) carry rich payloads the LLM uses to recover.

use crate::gdocs::domain::types::{HumanChange, MatchPreview};
use thiserror::Error;

/// Every error the gdocs subsystem can surface. Maps 1:1 to a JSON
/// envelope the LLM sees via `gdocs_tools::error_to_json`.
#[derive(Debug, Clone, Error)]
pub enum DocsError {
    /// No SA JSON or ADC available — operator has not configured auth.
    #[error("gdocs_not_configured: {0}")]
    NotConfigured(String),

    /// Token refresh failed after one retry.
    #[error("auth_failed: {0}")]
    AuthFailed(String),

    /// 404 on `documents.get` — `documentId` is wrong or the SA can't see it.
    #[error("document_not_found: {0}")]
    DocumentNotFound(String),

    /// A `tab_id` argument doesn't resolve to a tab in the document.
    #[error("tab_not_found: {0}")]
    TabNotFound(String),

    /// 403 — share the document/folder with the carried SA email.
    #[error("permission_denied: share with {0}")]
    PermissionDenied(String),

    /// `create_*` was called and no parent folder is configured
    /// (no env var, no per-call argument).
    #[error("no_parent_folder_configured")]
    NoParentFolder,

    /// `find` returned multiple matches and the caller did not pass
    /// `occurrence` or `anchor` to disambiguate.
    #[error("ambiguous_match: '{find}' has {} candidates", matches.len())]
    AmbiguousMatch {
        find: String,
        matches: Vec<MatchPreview>,
    },

    /// `find` returned zero matches. `fuzzy_suggestions` carries the
    /// nearest candidates by Levenshtein distance (may be empty).
    #[error("text_not_found: '{find}'")]
    TextNotFound {
        find: String,
        fuzzy_suggestions: Vec<String>,
    },

    /// `find` returned ≥5 matches and `confirm_many: true` was not set.
    /// The caller (LLM) must re-issue with explicit confirmation OR
    /// narrow the scope.
    #[error("confirm_many_matches: {count} matches in scope")]
    ConfirmManyMatches {
        find: String,
        count: u32,
        preview: Vec<MatchPreview>,
    },

    /// Human edits have landed since the agent's last known revision
    /// AND at least one overlaps the intended scope. The agent must
    /// call `gdocs_acknowledge_human_changes` before retrying the edit.
    #[error("human_changes_pending: {} overlapping, {} outside",
        changes_overlapping_scope.len(), changes_outside_scope.len())]
    HumanChangesPending {
        since: chrono::DateTime<chrono::Utc>,
        changes_overlapping_scope: Vec<HumanChange>,
        changes_outside_scope: Vec<HumanChange>,
    },

    /// The `find` text would cross a paragraph / table / structural
    /// boundary. v1 rejects these — caller can decompose into multiple
    /// in-paragraph operations.
    #[error("scope_crosses_boundary: {0}")]
    ScopeCrossesBoundary(String),

    /// `add_tab` would create a tab whose title already exists and the
    /// operator's `on_existing_tab` policy is `fail` (the default).
    #[error("tab_exists: {0}")]
    TabExists(String),

    /// 429 from Google after the dispatcher exhausted its retry budget.
    #[error("rate_limit: retry after {0}s")]
    RateLimit(u32),

    /// `writeControl.requiredRevisionId` was rejected (doc changed
    /// concurrently with our write) after the retry budget was spent.
    #[error("conflict: doc changed during write")]
    Conflict,

    /// 5xx / network / generic HTTP failure surfaces here.
    #[error("http_error: {0}")]
    Http(String),

    /// Caller passed bad input that the dispatcher caught before any
    /// network call (bad enum, malformed scope, etc.).
    #[error("invalid_args: {0}")]
    InvalidArgs(String),

    /// Catch-all for assertion failures inside the dispatcher / use cases.
    #[error("internal: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdocs::domain::types::{HumanChangeKind, MatchPreview};

    #[test]
    fn display_strings_render() {
        let e = DocsError::AmbiguousMatch {
            find: "cliente".into(),
            matches: vec![MatchPreview {
                n: 1,
                paragraph: 3,
                preview: "…".into(),
            }],
        };
        assert!(e.to_string().contains("'cliente' has 1 candidates"));

        let e2 = DocsError::HumanChangesPending {
            since: chrono::Utc::now(),
            changes_overlapping_scope: vec![],
            changes_outside_scope: vec![],
        };
        let _ = HumanChangeKind::Insert; // unused-import canary
        assert!(e2.to_string().contains("0 overlapping, 0 outside"));
    }

    #[test]
    fn errors_are_clone_and_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DocsError>();
        let e = DocsError::DocumentNotFound("abc".into());
        let _cloned = e.clone();
    }
}
