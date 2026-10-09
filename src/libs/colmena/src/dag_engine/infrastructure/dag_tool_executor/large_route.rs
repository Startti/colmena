//! Where a tool call over a large file is routed (dark behind
//! `COLMENA_LARGE_TABULAR`).
//!
//! A call goes to the large path only when a runtime is wired (the switch is on
//! and the host gave the engine what it needs) AND the file's catalog row is a
//! reference to an object the HOST owns, which is what a large `storage_key`-only
//! entry is registered as. Everything else, small files included, takes the path
//! it always took: with no runtime nothing here looks at anything, and the
//! decision reads only the start-of-turn catalog snapshot, so it adds no lookup
//! and no storage call to a small file's call.

use super::DagToolExecutor;
use crate::tabular_run::runtime::LargeTabularRuntime;
use std::sync::Arc;

/// A large file the model wants to run code over, as the catalog row says.
pub(crate) struct LargeTarget {
    pub runtime: Arc<LargeTabularRuntime>,
    /// The host's key: from the session's own row, never from the model.
    pub source_key: String,
    pub mime_type: String,
    pub filename: String,
    pub size_bytes: u64,
    pub session_id: Option<String>,
    pub agent_session_id: Option<String>,
}

impl DagToolExecutor {
    /// Builder: wire the runtime that runs code over prepared large files.
    pub fn with_large_tabular(mut self, runtime: Arc<LargeTabularRuntime>) -> Self {
        self.large_tabular = Some(runtime);
        self
    }

    /// Builder: a shorter clock for a large-file call than the tool's own 900 s, so a
    /// test can prove the cut-off end to end. Not for production wiring.
    #[doc(hidden)]
    pub fn with_large_call_budget(mut self, budget: std::time::Duration) -> Self {
        self.large_call_budget = Some(budget);
        self
    }

    pub(crate) fn large_call_budget(&self) -> Option<std::time::Duration> {
        self.large_call_budget
    }

    /// The large path for `document_id`, or `None` when the call keeps its
    /// usual path: no runtime is wired, the row is not in the catalog snapshot,
    /// or it is not a host-owned reference.
    pub(crate) fn large_target(&self, document_id: &str) -> Option<LargeTarget> {
        let runtime = self.large_tabular.clone()?;
        let row = self
            .attachment_catalog
            .as_ref()?
            .iter()
            .find(|a| a.document_id == document_id)?;
        if !row.is_host_storage_ref() {
            return None;
        }
        Some(LargeTarget {
            runtime,
            source_key: row.storage_key.clone()?,
            mime_type: row.mime_type.clone(),
            filename: row.filename.clone(),
            size_bytes: row.size_bytes.unwrap_or(0),
            session_id: self.session_id.clone(),
            agent_session_id: self.agent_session_id.clone(),
        })
    }
}
