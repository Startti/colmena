pub mod apply_excel_ops;
pub mod apply_patch;
pub mod apply_word_ops;
pub mod create_document;
pub mod get_head;
pub mod list_versions;
pub mod read_document;
pub mod rollback;
pub mod runtime;

pub use runtime::{DocumentRuntime, DEFAULT_RETENTION, DEFAULT_STORAGE_ROOT};
