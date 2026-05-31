//! `when` DSL — parser + evaluator. Full implementation in Task 7.

use serde_json::Value;

#[derive(Debug, Clone)]
pub enum WhenRule {
    // Real variants arrive in Task 7. This stub keeps `config.rs` compiling.
    Stub,
}

impl WhenRule {
    /// Stub parser — accepts anything as `Stub`. Replaced in Task 7.
    pub fn parse(_when: &Value, _schema: &Value) -> Result<Self, String> {
        Ok(WhenRule::Stub)
    }
}
