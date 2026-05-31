//! Router node — declarative branching with LLM-direct and extract+rules modes.
//!
//! See: docs/superpowers/specs/2026-05-31-router-and-output-parser-nodes-design.md

pub mod config;
pub mod when_dsl;
pub mod llm_direct;
pub mod extract_and_route;

mod node;
pub use node::RouterNode;
