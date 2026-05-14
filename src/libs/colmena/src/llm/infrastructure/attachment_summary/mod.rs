//! Adapters for `AttachmentSummaryGenerator`: local text extraction,
//! provider cheap-tier mapping, byte acquisition, and the LLM-backed
//! summary generator implementation.

pub mod byte_acquisition;
pub mod cheap_tier;
pub mod text_extractor;

pub use byte_acquisition::{acquire_bytes, AcquireError};
pub use cheap_tier::provider_cheap_tier;
pub use text_extractor::{extract_text, truncate_chars, ExtractError};
