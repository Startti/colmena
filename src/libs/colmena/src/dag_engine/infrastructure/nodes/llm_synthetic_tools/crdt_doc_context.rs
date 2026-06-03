//! Execution context for CRDT document tools.
//!
//! Decouples the tool dispatchers from the underlying source of the `Doc`:
//! they only need to know how to fetch the doc, mark it dirty, and record
//! events on a `ChangeTracker`. The mode (local registry vs WS peer) is
//! invisible to them.
//!
//! Built by `llm_call` based on the `crdt_documents` config block:
//!
//! | Config                            | Mode             | When                      |
//! |-----------------------------------|------------------|---------------------------|
//! | `ws_url` set                      | [`CrdtDocsContext::WsPeer`]   | Agent in stateless worker; CRDT service runs separately (production split topology). |
//! | No `ws_url`, singleton installed  | [`CrdtDocsContext::Local`] using shared runtime | Colocated server + executor (`crdt-yws-graph` subcommand, monolithic deploy). |
//! | Neither                           | [`CrdtDocsContext::Local`] using fresh runtime  | Plain `dag_engine run`, autonomous CLI, no live server. |

use crate::crdt_documents::{
    ArtifactId, ChangeTracker, CrdtDocumentsRuntime, InMemoryChangeTrackerStore, WsPeerArtifact,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use yrs::Doc;

/// Execution context bundled by `llm_call` and threaded into every
/// `crdt_doc_*` tool dispatcher.
pub enum CrdtDocsContext {
    /// Doc lives in this process's [`DocRegistry`] (autonomous OR shared
    /// in-process singleton). Mutations land on a `Y.Doc` we own; the
    /// snapshot writer eventually persists to storage.
    Local {
        runtime: Arc<CrdtDocumentsRuntime>,
        artifact_id: ArtifactId,
    },
    /// Doc lives in a CRDT documents service reachable over WebSocket.
    /// We keep a local CRDT replica; mutations push to the service via
    /// the sync v1 protocol, and remote updates apply to our replica.
    /// Persistence is the service's responsibility.
    WsPeer {
        artifact_id: ArtifactId,
        /// Cloned from `WsPeerArtifact::doc` at construction. Cheap to
        /// pass around: it's just an `Arc<Doc>` and yrs makes Doc
        /// thread-safe via internal locking.
        doc: Arc<Doc>,
        /// Cloned from `WsPeerArtifact::alive`. Tool dispatchers can
        /// observe this after each mutation to detect a dead socket
        /// (v1 fail-fast policy).
        alive: Arc<AtomicBool>,
        /// Local tracker for this peer session. Scoped to the agent's
        /// execution: `get_recent_changes` sees only what THIS agent did
        /// (and what the server pushed back during the session).
        tracker: Arc<ChangeTracker>,
    },
}

impl CrdtDocsContext {
    /// Build a context that delegates to a locally-owned (or singleton)
    /// `CrdtDocumentsRuntime`.
    pub fn new_local(runtime: Arc<CrdtDocumentsRuntime>, artifact_id: ArtifactId) -> Self {
        Self::Local {
            runtime,
            artifact_id,
        }
    }

    /// Build a context backed by a live WS peer connection. Use this
    /// after constructing a `WsPeerArtifact` via
    /// [`crate::crdt_documents::WsPeerArtifact::connect`]. The peer
    /// handle (for graceful shutdown) is consumed by the caller via
    /// the returned tuple — the context only borrows `doc` + `alive`.
    pub fn new_ws_peer(peer: &WsPeerArtifact) -> Self {
        Self::WsPeer {
            artifact_id: peer.artifact_id.clone(),
            doc: peer.doc.clone(),
            alive: peer.alive.clone(),
            tracker: Arc::new(ChangeTracker::new(Arc::new(InMemoryChangeTrackerStore::new()))),
        }
    }

    /// The artifact this context binds to.
    pub fn artifact_id(&self) -> &ArtifactId {
        match self {
            Self::Local { artifact_id, .. } | Self::WsPeer { artifact_id, .. } => artifact_id,
        }
    }

    /// Fetch the `Doc` to operate on. Returns `None` if the artifact is
    /// not registered (local mode) or the peer socket is closed
    /// (ws_peer mode).
    pub fn doc(&self) -> Option<Arc<Doc>> {
        match self {
            Self::Local {
                runtime,
                artifact_id,
            } => runtime.registry.get(artifact_id).map(|e| e.doc.clone()),
            Self::WsPeer { doc, alive, .. } => {
                if alive.load(Ordering::Acquire) {
                    Some(doc.clone())
                } else {
                    None
                }
            }
        }
    }

    /// Mark the artifact dirty so the snapshot writer flushes it. In
    /// ws_peer mode this is a no-op: persistence is the server's
    /// responsibility, and our mutation has already been pushed over WS
    /// (or queued) by the time this is called.
    pub fn mark_dirty(&self) {
        match self {
            Self::Local {
                runtime,
                artifact_id,
            } => {
                if let Some(e) = runtime.registry.get(artifact_id) {
                    e.mark_dirty();
                }
            }
            Self::WsPeer { .. } => {}
        }
    }

    /// The change tracker used by `get_recent_changes`. In ws_peer mode
    /// this is per-session; in local mode it's the runtime's shared
    /// tracker.
    pub fn tracker(&self) -> Arc<ChangeTracker> {
        match self {
            Self::Local { runtime, .. } => runtime.tracker.clone(),
            Self::WsPeer { tracker, .. } => tracker.clone(),
        }
    }

    /// `true` if the context can still serve mutations. Local is always
    /// alive (the runtime lives as long as the context). WsPeer flips
    /// to false when the socket dies.
    pub fn is_alive(&self) -> bool {
        match self {
            Self::Local { .. } => true,
            Self::WsPeer { alive, .. } => alive.load(Ordering::Acquire),
        }
    }
}
