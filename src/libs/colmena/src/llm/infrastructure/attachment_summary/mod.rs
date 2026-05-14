//! Adapters for `AttachmentSummaryGenerator`: local text extraction,
//! provider cheap-tier mapping, byte acquisition, and the LLM-backed
//! summary generator implementation.

pub mod text_extractor;

pub use text_extractor::truncate_chars;
