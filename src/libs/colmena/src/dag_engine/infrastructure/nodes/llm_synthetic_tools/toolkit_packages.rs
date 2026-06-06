//! Toolkit-package registry. Lets a user write `enabled_tools: ["gsheets"]`
//! to enable every gsheets_* tool at once, with optional `!toolname`
//! exclusion entries.
//!
//! Naming convention (enforced by test): package aliases MUST NOT contain
//! `_`. Individual tool names MUST contain `_` after the package namespace
//! (e.g. `gsheets_read`). The single-underscore boundary is how a human
//! reading a graph JSON disambiguates "package" from "tool" at a glance.

/// A curated bundle of tools exposed under a single alias.
pub struct ToolkitPackage {
    /// Alias used in `enabled_tools`. Must not contain `_`.
    pub alias: &'static str,
    /// One-line human description shown in docs / future introspection tools.
    pub description: &'static str,
    /// Exact names of every tool this package activates. Order is preserved
    /// in the expansion.
    pub tools: &'static [&'static str],
}

/// The registry. New packages append here as a single struct literal.
pub static TOOLKIT_PACKAGES: &[ToolkitPackage] = &[ToolkitPackage {
    alias: "gsheets",
    description: "Read, write, and analyze Google Sheets workbooks (10 tools)",
    tools: &[
        "gsheets_create_spreadsheet",
        "gsheets_create_from_xlsx",
        "gsheets_export_xlsx",
        "gsheets_list_sheets",
        "gsheets_add_sheet",
        "gsheets_delete_sheet",
        "gsheets_read",
        "gsheets_set_cell",
        "gsheets_set_range",
        "gsheets_run_python",
    ],
}];

/// Linear-scan lookup. The registry is small (≪ 50 entries) so a HashMap
/// would be over-engineering.
pub fn find_package(alias: &str) -> Option<&'static ToolkitPackage> {
    TOOLKIT_PACKAGES.iter().find(|p| p.alias == alias)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_aliases_have_no_underscore() {
        for pkg in TOOLKIT_PACKAGES {
            assert!(
                !pkg.alias.contains('_'),
                "Package alias '{}' must not contain '_' — reserved for tool names",
                pkg.alias
            );
        }
    }

    #[test]
    fn gsheets_package_has_all_ten_tools() {
        let pkg = find_package("gsheets").expect("gsheets package must exist");
        assert_eq!(pkg.tools.len(), 10, "gsheets package must list 10 tools");
        for required in &[
            "gsheets_create_spreadsheet",
            "gsheets_create_from_xlsx",
            "gsheets_export_xlsx",
            "gsheets_list_sheets",
            "gsheets_add_sheet",
            "gsheets_delete_sheet",
            "gsheets_read",
            "gsheets_set_cell",
            "gsheets_set_range",
            "gsheets_run_python",
        ] {
            assert!(
                pkg.tools.contains(required),
                "gsheets package missing tool: {}",
                required
            );
        }
    }

    #[test]
    fn find_package_returns_some_for_known_alias() {
        assert!(find_package("gsheets").is_some());
    }

    #[test]
    fn find_package_returns_none_for_unknown() {
        assert!(find_package("gsheetz").is_none());
        assert!(find_package("").is_none());
        assert!(find_package("gsheets_read").is_none());
    }
}
