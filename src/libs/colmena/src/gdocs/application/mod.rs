//! Application layer — high-level use cases shared across dispatchers.

#[cfg(test)]
pub mod _test_helpers;

pub mod apply_edits;
pub mod co_edit_guard;
pub mod delete_text;
pub mod diff;
pub mod insert;
pub mod named_range;
pub mod replace_section;
pub mod replace_text;
pub mod scope_resolver;
pub mod style;
pub mod table;
pub mod util;
