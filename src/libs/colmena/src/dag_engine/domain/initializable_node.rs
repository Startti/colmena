//! Optional initialization trait for nodes that need pre-execution setup.
//!
//! Nodes that implement this trait get `initialize()` called once before the first
//! execution within a DAG run. Use this for creating connection pools, loading metadata,
//! or any expensive one-time setup.

use serde_json::Value;
use std::error::Error as StdError;

/// Context returned by `InitializableNode::initialize()`.
/// Contains metadata that can enrich the tool description sent to the LLM.
#[derive(Debug, Clone, Default)]
pub struct InitContext {
    /// Additional text to append to the tool's description.
    /// Used to inject database schema info, available functions, etc.
    pub description_supplement: Option<String>,
}

/// Optional trait for nodes that require one-time initialization before execution.
///
/// The DAG engine checks if a node implements this trait (via downcast) and calls
/// `initialize()` once before the first `execute()` call in a given DAG run.
#[async_trait::async_trait]
pub trait InitializableNode: Send + Sync {
    /// Perform one-time setup. Called before the first `execute()`.
    ///
    /// # Arguments
    /// * `config` - The node's static configuration from the graph JSON.
    ///
    /// # Returns
    /// An `InitContext` whose `description_supplement` is appended to the tool description.
    async fn initialize(
        &self,
        config: &Value,
    ) -> Result<InitContext, Box<dyn StdError + Send + Sync>>;
}
