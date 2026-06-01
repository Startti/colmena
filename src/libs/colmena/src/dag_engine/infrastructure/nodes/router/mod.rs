//! Router node — declarative branching with LLM-direct and extract+rules modes.
//!
//! See: docs/superpowers/specs/2026-05-31-router-and-output-parser-nodes-design.md

pub mod config;
pub mod extract_and_route;
pub mod llm_direct;
pub mod when_dsl;

mod node;
pub use node::RouterNode;
