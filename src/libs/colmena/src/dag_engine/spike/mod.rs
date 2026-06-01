//! Documents CRDT spike (Phase 0). See
//! `docs/superpowers/specs/2026-05-31-documents-crdt-spike-design.md`.
//!
//! This module is SPIKE code. It is intentionally isolated so it can be
//! removed in one commit after the GO/NO-GO verdict.

pub mod agent_peer;
pub mod doc_registry;
pub mod projection;
pub mod server;
pub mod yjs_protocol;

pub use doc_registry::DocRegistry;
