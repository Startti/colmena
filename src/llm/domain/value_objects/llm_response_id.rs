use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LlmResponseId {
    value: String,
}

impl LlmResponseId {
    pub fn new() -> Self {
        Self {
            value: Uuid::new_v4().to_string(),
        }
    }

    pub fn from_string(value: String) -> Result<Self, String> {
        if value.is_empty() {
            return Err("LlmResponseId cannot be empty".to_string());
        }
        Ok(Self { value })
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

impl Display for LlmResponseId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}

impl Default for LlmResponseId {
    fn default() -> Self {
        Self::new()
    }
}