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
    crdt_backend::CrdtBackend, ArtifactId, CrdtDocumentsRuntime, DirectBackend, RestBackend,
    WsPeerArtifact,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use yrs::Doc;

/// Execution context bundled by `llm_call` and threaded into every
/// `crdt_doc_*` tool dispatcher.
pub enum CrdtDocsContext {
    /// Doc lives in this process's [`crate::crdt_documents::DocRegistry`]
    /// (autonomous OR shared in-process singleton). Mutations land on a
    /// `Y.Doc` we own; the snapshot writer eventually persists to storage.
    Local {
        runtime: Arc<CrdtDocumentsRuntime>,
        artifact_id: ArtifactId,
        /// agent_session_id captured from `llm_call` inputs. Used for
        /// origin attribution on recorded events and as the key for
        /// per-session cursor advancement (B-T12).
        session_id: Option<String>,
        /// Backend used to record/query change events. In Local mode this
        /// wraps the runtime's `ChangeTrackerStore` directly.
        backend: Arc<dyn CrdtBackend>,
        /// Highest event id observed during this turn. Tool dispatchers
        /// call `record_event_id` after every `backend.record_event`; the
        /// lifecycle owner (`llm.rs`) reads `max_event_id_observed` to
        /// advance the per-session cursor at end-of-turn.
        max_event_id: Arc<AtomicU64>,
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
        /// agent_session_id captured from `llm_call` inputs (see Local).
        session_id: Option<String>,
        /// Backend used to record/query change events. In WsPeer mode
        /// this is a REST client targeting the CRDT documents server.
        backend: Arc<dyn CrdtBackend>,
        /// Highest event id observed during this turn (see Local).
        max_event_id: Arc<AtomicU64>,
    },
}

impl CrdtDocsContext {
    /// Build a context that delegates to a locally-owned (or singleton)
    /// `CrdtDocumentsRuntime`.
    pub fn new_local(
        runtime: Arc<CrdtDocumentsRuntime>,
        artifact_id: ArtifactId,
        session_id: Option<String>,
    ) -> Self {
        let backend: Arc<dyn CrdtBackend> = Arc::new(DirectBackend {
            store: runtime.store.clone(),
        });
        Self::Local {
            runtime,
            artifact_id,
            session_id,
            backend,
            max_event_id: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Build a context backed by a live WS peer connection. Use this
    /// after constructing a `WsPeerArtifact` via
    /// [`crate::crdt_documents::WsPeerArtifact::connect`]. The peer
    /// handle (for graceful shutdown) is consumed by the caller via
    /// the returned tuple — the context only borrows `doc` + `alive`.
    ///
    /// `server_base_url` is the HTTP base URL for the CRDT documents
    /// service (e.g. `http://crdt-service:8090`) — derived from the
    /// caller's `ws_url` by stripping `/yjs` and swapping `ws[s]://`
    /// for `http[s]://`.
    pub fn new_ws_peer(
        peer: &WsPeerArtifact,
        session_id: Option<String>,
        server_base_url: impl Into<String>,
    ) -> Self {
        let backend: Arc<dyn CrdtBackend> = Arc::new(RestBackend::new(server_base_url));
        Self::WsPeer {
            artifact_id: peer.artifact_id.clone(),
            doc: peer.doc.clone(),
            alive: peer.alive.clone(),
            session_id,
            backend,
            max_event_id: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The artifact this context binds to.
    pub fn artifact_id(&self) -> &ArtifactId {
        match self {
            Self::Local { artifact_id, .. } | Self::WsPeer { artifact_id, .. } => artifact_id,
        }
    }

    /// The agent_session_id (if any) used to attribute events recorded
    /// during this turn.
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::Local { session_id, .. } | Self::WsPeer { session_id, .. } => {
                session_id.as_deref()
            }
        }
    }

    /// Backend for change events. Local mode talks to the runtime's
    /// `ChangeTrackerStore` directly; WsPeer mode does HTTP to the
    /// CRDT documents server.
    pub fn backend(&self) -> &dyn CrdtBackend {
        match self {
            Self::Local { backend, .. } | Self::WsPeer { backend, .. } => backend.as_ref(),
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
                ..
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
                ..
            } => {
                if let Some(e) = runtime.registry.get(artifact_id) {
                    e.mark_dirty();
                }
            }
            Self::WsPeer { .. } => {}
        }
    }

    /// Track the highest event_id observed during this turn. Called by
    /// tool dispatchers after `backend.record_event()`. The lifecycle
    /// owner (`llm.rs`) reads `max_event_id_observed()` to advance the
    /// cursor (B-T12).
    pub fn record_event_id(&self, id: u64) {
        let atomic = match self {
            Self::Local { max_event_id, .. } | Self::WsPeer { max_event_id, .. } => max_event_id,
        };
        let mut cur = atomic.load(Ordering::Acquire);
        while id > cur {
            match atomic.compare_exchange_weak(cur, id, Ordering::Release, Ordering::Acquire) {
                Ok(_) => break,
                Err(observed) => cur = observed,
            }
        }
    }

    /// Highest event id observed via `record_event_id` during this turn.
    /// Returns `0` when no event has been recorded.
    pub fn max_event_id_observed(&self) -> u64 {
        match self {
            Self::Local { max_event_id, .. } | Self::WsPeer { max_event_id, .. } => {
                max_event_id.load(Ordering::Acquire)
            }
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
