use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LlmProvider {
    OpenAi,
    Gemini,
    Anthropic,
}

impl Display for LlmProvider {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmProvider::OpenAi => write!(f, "openai"),
            LlmProvider::Gemini => write!(f, "gemini"),
            LlmProvider::Anthropic => write!(f, "anthropic"),
        }
    }
}

impl LlmProvider {
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(LlmProvider::OpenAi),
            "gemini" => Ok(LlmProvider::Gemini),
            "anthropic" => Ok(LlmProvider::Anthropic),
            _ => Err(format!("Unsupported provider: {}", s)),
        }
    }

    pub fn default_model(&self) -> &'static str {
        match self {
            LlmProvider::OpenAi => "gpt-4o",
            LlmProvider::Gemini => "gemini-1.5-flash",
            LlmProvider::Anthropic => "claude-3-sonnet",
        }
    }

    pub fn env_var_name(&self) -> &'static str {
        match self {
            LlmProvider::OpenAi => "OPENAI_API_KEY",
            LlmProvider::Gemini => "GEMINI_API_KEY",
            LlmProvider::Anthropic => "ANTHROPIC_API_KEY",
        }
    }
}