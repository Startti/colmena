//! Web toolkit nodes — shared foundation.
//!
//! See `docs/superpowers/specs/2026-04-23-web-nodes-unified-design.md`.
//!
//! This module hosts the ports, use cases, and adapters for three toolkit nodes
//! (`tavily_client`, `api_explorer`, `browser`) plus cross-cutting pieces that
//! all of them use: [`session::SessionRegistry`], [`errors::WebDomainError`].

pub mod domain;
pub mod application;
pub mod infrastructure;
