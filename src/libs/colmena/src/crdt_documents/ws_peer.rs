//! WebSocket-peer client for a CRDT documents server.
//!
//! Establishes a long-lived peer connection to `ws://<server>/yjs/<artifact_id>`,
//! maintains a **local** Y.Doc replica synced via the Yjs sync v1 protocol, and
//! forwards every local mutation to the server while applying every remote
//! update to the local replica.
//!
//! Why this exists
//! ---------------
//! Previously, `llm_call` could only operate on a CRDT artifact if the
//! `CrdtDocumentsRuntime` was either constructed locally (`from_config`,
//! disconnected from any live server) or shared in-process with a server
//! (`process_runtime` singleton, requires worker = server colocation). Neither
//! works for the production topology where the WS server is its own service
//! and the worker that runs the graph is stateless.
//!
//! This module is the third operating mode: the agent is a peer just like a
//! browser. The tool dispatchers continue to operate on a `Doc` reference —
//! they do not know whether the doc is locally-owned (`from_config`),
//! singleton (`shared`), or a peer replica (this module). The CRDT layer
//! handles propagation.
//!
//! Lifecycle
//! ---------
//! ```ignore
//! let peer = WsPeerArtifact::connect(
//!     "ws://crdt-service:8090",
//!     artifact_id,
//!     "agent",
//!     Some("session_abc123"),
//! ).await?;
//! // mutate peer.doc through the tool dispatchers …
//! peer.shutdown().await; // flush pending updates and close cleanly
//! ```
//!
//! Failure mode (v1 policy: fail-fast)
//! -----------------------------------
//! If the underlying socket dies mid-operation, the background sync task
//! exits and any further mutations are written to the local replica only —
//! they will NOT reach the server. Callers are expected to detect this via
//! [`WsPeerArtifact::is_alive`] after each tool call and surface the failure
//! to the LLM. Automatic reconnect is intentionally deferred to v1.1.

use crate::crdt_documents::yjs_protocol::{
    decode_sync_step1_sv, decode_sync_step2_update, encode_sync_step1, encode_sync_step2,
    encode_update, parse_msgs, SyncMsg,
};
use crate::crdt_documents::ArtifactId;
use futures::{SinkExt, StreamExt};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message as TMsg;
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

#[derive(Debug, thiserror::Error)]
pub enum WsPeerError {
    #[error("connect: {0}")]
    Connect(String),
    #[error("sync handshake: {0}")]
    Sync(String),
    #[error("ws send: {0}")]
    Send(String),
    #[error("ws closed unexpectedly")]
    Closed,
}

/// A live peer connection to a CRDT documents server for one artifact.
///
/// Holds the local `Doc` replica + a background task that bidirectionally
/// syncs updates with the server. Drop or call [`Self::shutdown`] to close.
pub struct WsPeerArtifact {
    pub doc: Arc<Doc>,
    pub artifact_id: ArtifactId,
    pub alive: Arc<AtomicBool>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    done_rx: Option<oneshot::Receiver<()>>,
}

impl WsPeerArtifact {
    /// Open a WS connection to `<server_url>/<artifact_id>` and complete the
    /// sync v1 handshake. The server's WS route is `/yjs/:id`, so a typical
    /// `server_url` is `ws://host:port/yjs`.
    ///
    /// `peer_type` (typically `"agent"` or `"browser"`) and the optional
    /// `session_id` are passed as URL query params so the server can attribute
    /// inbound updates to the correct origin (see B-T13). Omitted params
    /// default to `peer:browser` on the server side.
    pub async fn connect(
        server_url: &str,
        artifact_id: ArtifactId,
        peer_type: &str,
        session_id: Option<&str>,
    ) -> Result<Self, WsPeerError> {
        let mut full_url = format!(
            "{}/{}?peer_type={}",
            server_url.trim_end_matches('/'),
            artifact_id.as_str(),
            peer_type,
        );
        if let Some(sid) = session_id {
            full_url.push_str(&format!("&session_id={sid}"));
        }

        let (mut ws, _resp) = tokio_tungstenite::connect_async(&full_url)
            .await
            .map_err(|e| WsPeerError::Connect(format!("{full_url}: {e}")))?;

        let doc = Arc::new(Doc::new());

        // ── Sync v1 handshake ────────────────────────────────────────────
        // The server speaks first with its sync_step1 (its state vector).
        // We capture it (needed below to compute our outbound diff).
        let server_sv: StateVector = loop {
            match ws.next().await {
                Some(Ok(TMsg::Binary(bytes))) => {
                    if let Some(sv_bytes) = decode_sync_step1_sv(&bytes) {
                        break StateVector::decode_v1(&sv_bytes)
                            .map_err(|e| WsPeerError::Sync(format!("decode server sv: {e:?}")))?;
                    }
                    // Non-sync frame (awareness, auth) — keep reading.
                    continue;
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(WsPeerError::Sync(format!("recv step1: {e}"))),
                None => return Err(WsPeerError::Sync("ws closed before sync_step1".into())),
            }
        };

        // Reply with our sync_step1 (our state vector — empty for a fresh doc).
        let our_sv = doc.transact().state_vector();
        ws.send(TMsg::Binary(encode_sync_step1(&our_sv).into()))
            .await
            .map_err(|e| WsPeerError::Sync(format!("send step1: {e}")))?;

        // Receive server's sync_step2 (the full state diff we need).
        let server_state_bytes: Vec<u8> = loop {
            match ws.next().await {
                Some(Ok(TMsg::Binary(ref bytes))) => {
                    if let Some(state) = decode_sync_step2_update(bytes) {
                        break state;
                    }
                    continue;
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(WsPeerError::Sync(format!("recv step2: {e}"))),
                None => return Err(WsPeerError::Sync("ws closed before sync_step2".into())),
            }
        };

        // Apply server's state to our local replica. After this, our doc
        // shares the same CRDT object IDs as the server's, so updates flow
        // cleanly in both directions.
        {
            let update = Update::decode_v1(&server_state_bytes)
                .map_err(|e| WsPeerError::Sync(format!("decode server state: {e:?}")))?;
            doc.transact_mut()
                .apply_update(update)
                .map_err(|e| WsPeerError::Sync(format!("apply server state: {e:?}")))?;
        }

        // ── Background sync task ─────────────────────────────────────────
        let (update_tx, mut update_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let (done_tx, done_rx) = oneshot::channel::<()>();
        let alive = Arc::new(AtomicBool::new(true));

        // Background sync task. `yrs::Subscription` (Arc<dyn Drop>) is
        // !Send, so we can't `tokio::spawn` a future that holds it across
        // awaits AND we can't create the subscription outside the thread
        // either (the closure passed to std::thread::spawn must also be
        // Send). The server runs into the same constraint and uses the
        // same workaround: dedicate a thread with its own single-threaded
        // tokio runtime, create the subscription INSIDE the thread, and
        // drive everything there.
        //
        // `ready_tx` lets us block `connect()` until the thread has
        // registered its subscription — without it, the caller could
        // race a `transact_mut` against `observe_update_v1`'s exclusive
        // lock acquisition and trigger `ExclusiveAcqFailed`.
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), String>>();
        let doc_task = doc.clone();
        let alive_task = alive.clone();
        let _server_sv_for_task = server_sv; // captured for completeness; unused now
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("ws_peer thread rt");
            rt.block_on(async move {
                // Subscribe HERE (inside the thread) so the !Send
                // Subscription never crosses thread boundaries.
                let subscription_result = doc_task.observe_update_v1({
                    let update_tx = update_tx.clone();
                    move |_txn, evt| {
                        let msg = encode_update(&evt.update);
                        let _ = update_tx.send(msg);
                    }
                });
                let _subscription = match subscription_result {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("observe_update_v1: {e:?}")));
                        alive_task.store(false, Ordering::Release);
                        let _ = done_tx.send(());
                        return;
                    }
                };
                // Signal connect() that we're ready to receive mutations.
                let _ = ready_tx.send(Ok(()));
                let mut ws = ws;

                loop {
                    tokio::select! {
                        biased;

                        _ = &mut shutdown_rx => {
                            // Drain any pending outbound updates before closing.
                            while let Ok(bytes) = update_rx.try_recv() {
                                let _ = ws.send(TMsg::Binary(bytes.into())).await;
                            }
                            let _ = ws.send(TMsg::Close(None)).await;
                            break;
                        }

                        outbound = update_rx.recv() => {
                            match outbound {
                                Some(bytes) => {
                                    if ws.send(TMsg::Binary(bytes.into())).await.is_err() {
                                        break;
                                    }
                                }
                                None => break, // all senders dropped
                            }
                        }

                        incoming = ws.next() => {
                            let Some(Ok(msg)) = incoming else { break };
                            let bytes = match msg {
                                TMsg::Binary(b) => b,
                                TMsg::Close(_) => break,
                                _ => continue,
                            };
                            let Ok(msgs) = parse_msgs(&bytes) else { continue };
                            for m in msgs {
                                match m {
                                    SyncMsg::Step1 { sv_bytes } => {
                                        // Server requested our diff; reply with
                                        // a step2 of our updates since their sv.
                                        let Ok(sv) = StateVector::decode_v1(&sv_bytes)
                                        else {
                                            continue;
                                        };
                                        let diff = doc_task.transact().encode_diff_v1(&sv);
                                        let _ = ws
                                            .send(TMsg::Binary(encode_sync_step2(&diff).into()))
                                            .await;
                                    }
                                    SyncMsg::Step2OrUpdate { update_bytes } => {
                                        if let Ok(update) = Update::decode_v1(&update_bytes) {
                                            let _ = doc_task
                                                .transact_mut()
                                                .apply_update(update);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                alive_task.store(false, Ordering::Release);
                let _ = done_tx.send(());
            });
        });

        // Wait until the background thread has registered its subscription
        // — only then is it safe for callers to mutate the doc.
        match ready_rx.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(WsPeerError::Sync(e)),
            Err(_) => return Err(WsPeerError::Sync("ws_peer thread died during setup".into())),
        }

        Ok(Self {
            doc,
            artifact_id,
            alive,
            shutdown_tx: Some(shutdown_tx),
            done_rx: Some(done_rx),
        })
    }

    /// `true` if the background sync task is still running. Becomes `false`
    /// after the socket closes (graceful or otherwise) and after
    /// [`Self::shutdown`] completes.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Signal the background task to flush pending outbound updates and
    /// close the socket, then wait for it to confirm. Idempotent.
    pub async fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(rx) = self.done_rx.take() {
            let _ = rx.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::{server::router as server_router, CrdtDocumentsRuntime};
    use serde_json::json;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use yrs::{Any, Map, Out, ReadTxn, Transact, WriteTxn};

    /// Spin up a real `crdt_documents` WS server on a random port and
    /// connect a `WsPeerArtifact` to it. Mutate the peer's local replica;
    /// the server's authoritative Y.Doc should converge to match.
    #[tokio::test]
    async fn peer_mutation_propagates_to_server() {
        // Server runtime + axum router on a random port.
        let dump = std::env::temp_dir().join(format!("ws_peer_test_{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dump).unwrap();
        let cfg = json!({
            "storage_backend": "localfs",
            "storage_root": dump.to_str().unwrap(),
        });
        let runtime = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());

        // Seed an artifact on the server so the peer has something to sync.
        let aid = ArtifactId::new();
        let _seed_entry = runtime.registry.get_or_create(&aid, "test");

        let app = server_router(runtime.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        // Tiny pause so the server is ready to accept WS upgrades.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Connect a peer.
        let server_url = format!("ws://{}/yjs", addr);
        let mut peer = WsPeerArtifact::connect(&server_url, aid.clone(), "agent", None)
            .await
            .expect("peer connect");
        assert!(peer.is_alive());

        // Mutate the peer's local doc.
        {
            let mut txn = peer.doc.transact_mut();
            let m = txn.get_or_insert_map("workbook");
            m.insert(&mut txn, "marker", "from-peer");
        }

        // Wait a moment for the subscription to fire + WS to propagate +
        // server to apply. We poll up to 1s.
        let server_entry = runtime.registry.get(&aid).unwrap();
        let server_doc = server_entry.doc.clone();
        let mut got = None;
        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let txn = server_doc.transact();
            if let Some(m) = txn.get_map("workbook") {
                if let Some(v) = m.get(&txn, "marker") {
                    got = Some(v);
                    break;
                }
            }
        }

        peer.shutdown().await;
        assert!(!peer.is_alive());
        runtime.shutdown().await;

        match got {
            Some(Out::Any(Any::String(s))) => assert_eq!(s.as_ref(), "from-peer"),
            other => panic!("server didn't receive peer mutation: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dump);
    }

    /// Inverse direction: when the server's doc is mutated (e.g. via a
    /// directly-attached process), the peer's local replica should converge.
    #[tokio::test]
    async fn server_mutation_propagates_to_peer() {
        let dump = std::env::temp_dir().join(format!("ws_peer_test_{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dump).unwrap();
        let cfg = json!({
            "storage_backend": "localfs",
            "storage_root": dump.to_str().unwrap(),
        });
        let runtime = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
        let aid = ArtifactId::new();
        let server_entry = runtime.registry.get_or_create(&aid, "test");

        let app = server_router(runtime.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let server_url = format!("ws://{}/yjs", addr);
        let mut peer = WsPeerArtifact::connect(&server_url, aid.clone(), "agent", None)
            .await
            .expect("peer connect");

        // Mutate the server-side doc directly (simulates a co-resident
        // singleton or another peer hitting the server). The peer should
        // see it via inbound WS update.
        {
            let mut txn = server_entry.doc.transact_mut();
            let m = txn.get_or_insert_map("workbook");
            m.insert(&mut txn, "marker", "from-server");
        }
        server_entry.mark_dirty();

        // Poll peer's local replica.
        let peer_doc = peer.doc.clone();
        let mut got = None;
        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let txn = peer_doc.transact();
            if let Some(m) = txn.get_map("workbook") {
                if let Some(v) = m.get(&txn, "marker") {
                    got = Some(v);
                    break;
                }
            }
        }

        peer.shutdown().await;
        runtime.shutdown().await;

        match got {
            Some(Out::Any(Any::String(s))) => assert_eq!(s.as_ref(), "from-server"),
            other => panic!("peer didn't receive server mutation: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dump);
    }
}
