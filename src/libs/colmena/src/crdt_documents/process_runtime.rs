//! Process-wide singleton for sharing a single [`CrdtDocumentsRuntime`]
//! between the WebSocket server and the LLM tool dispatcher.
//!
//! Why this exists
//! ---------------
//! Without a shared runtime, the WS server (started by `crdt-yws`) owns
//! one `CrdtDocumentsRuntime` with its own in-memory Y.Doc per artifact,
//! and every `llm_call` node that has a `crdt_documents` config block
//! builds a *separate* runtime via `CrdtDocumentsRuntime::from_config`.
//! They share the disk (same `storage_root`) but not the live Y.Doc, so
//! mutations the LLM agent issues never reach the WS server's RAM and the
//! browser sees a stale view until a server restart triggers a reload
//! from disk.
//!
//! In a production deployment (e.g. ADP's worker), the WS server and the
//! graph executor are in the same process. The bootstrap path installs
//! the runtime here via [`set_global`]; `llm_call` then reuses it via
//! [`get_global`] instead of building a fresh one.
//!
//! Lifecycle
//! ---------
//! * `set_global` is idempotent in the sense that re-setting is rejected
//!   with an error rather than silently overwriting. Callers that need
//!   re-installation should restart the process.
//! * Once installed, the runtime lives for the rest of the process.
//!   `llm_call` MUST NOT shut it down (the singleton is owned by the
//!   host process, not by any single graph execution).

use crate::crdt_documents::CrdtDocumentsRuntime;
use once_cell::sync::OnceCell;
use std::sync::Arc;

static GLOBAL_RUNTIME: OnceCell<Arc<CrdtDocumentsRuntime>> = OnceCell::new();

/// Install a process-wide runtime. Errors if one is already set — the
/// runtime is intended to be set exactly once during process bootstrap.
pub fn set_global(rt: Arc<CrdtDocumentsRuntime>) -> Result<(), &'static str> {
    GLOBAL_RUNTIME
        .set(rt)
        .map_err(|_| "global crdt_documents runtime already set")
}

/// Get a cloned `Arc` to the process-wide runtime, if installed.
pub fn get_global() -> Option<Arc<CrdtDocumentsRuntime>> {
    GLOBAL_RUNTIME.get().cloned()
}

/// Returns `true` if a global runtime has been installed.
pub fn is_installed() -> bool {
    GLOBAL_RUNTIME.get().is_some()
}
