//! Adapters for `AttachmentSummaryGenerator`: local text extraction,
//! provider cheap-tier mapping, byte acquisition, and the LLM-backed
//! summary generator implementation.

pub mod cheap_tier;
pub mod text_extractor;

pub use cheap_tier::provider_cheap_tier;
pub use text_extractor::{extract_text, truncate_chars, ExtractError};
