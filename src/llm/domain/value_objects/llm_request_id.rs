use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LlmRequestId {
    value: String,
}

impl LlmRequestId {
    pub fn new() -> Self {
        Self {
            value: Uuid::new_v4().to_string(),
        }
    }

    pub fn from_string(value: String) -> Result<Self, String> {
        if value.is_empty() {
            return Err("LlmRequestId cannot be empty".to_string());
        }
        Ok(Self { value })
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

impl Display for LlmRequestId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}

impl Default for LlmRequestId {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_id_creation() {
        let id = LlmRequestId::new();
        assert!(!id.value().is_empty());
    }

    #[test]
    fn test_id_from_string() {
        let id_str = "test-id".to_string();
        let id = LlmRequestId::from_string(id_str.clone()).unwrap();
        assert_eq!(id.value(), id_str);
    }

    #[test]
    fn test_id_from_empty_string_fails() {
        let result = LlmRequestId::from_string("".to_string());
        assert!(result.is_err());
    }
}
