//! Application layer — high-level use cases shared across dispatchers.
//!
//! Submodules `replace_text`, `insert`, `delete_text`,
//! `replace_section`, `style`, `apply_edits`, `named_range` are added
//! by Tasks 16-18 as they land.

pub mod co_edit_guard;
pub mod scope_resolver;
