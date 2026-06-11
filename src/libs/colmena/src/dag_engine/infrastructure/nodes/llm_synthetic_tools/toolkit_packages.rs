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
pub static TOOLKIT_PACKAGES: &[ToolkitPackage] = &[
    ToolkitPackage {
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
    },
    ToolkitPackage {
        alias: "gdocs",
        description: "Read, write, edit, share, discover, and comment on Google Docs (28 tools)",
        tools: &[
            "gdocs_create",
            "gdocs_create_from_markdown",
            "gdocs_create_from_docx",
            "gdocs_share",
            "gdocs_export",
            "gdocs_list_tabs",
            "gdocs_add_tab",
            "gdocs_read_as_markdown",
            "gdocs_read_outline",
            "gdocs_list_named_ranges",
            "gdocs_replace_text",
            "gdocs_insert_after_text",
            "gdocs_insert_before_text",
            "gdocs_insert_between",
            "gdocs_delete_text",
            "gdocs_replace_section",
            "gdocs_append_markdown",
            "gdocs_apply_edits",
            "gdocs_style_text",
            "gdocs_create_named_range",
            "gdocs_replace_named_range",
            "gdocs_acknowledge_human_changes",
            // Bundle 2A (2026-06-11): Drive discovery
            "gdocs_list_documents",
            // Bundle 2B (2026-06-11): permissions
            "gdocs_list_permissions",
            "gdocs_unshare",
            // Bundle 4A (2026-06-11): Drive comments
            "gdocs_add_comment",
            "gdocs_list_comments",
            "gdocs_resolve_comment",
        ],
    },
    ToolkitPackage {
        // Alias has no `_` per the package-vs-tool naming convention enforced
        // by `package_aliases_have_no_underscore`. `gdocsread` is the
        // read-only subset (9 tools — no writes; comments/permission/document
        // discovery listings are all reads).
        alias: "gdocsread",
        description: "Read-only Google Docs access (9 tools — no writes)",
        tools: &[
            "gdocs_export",
            "gdocs_list_tabs",
            "gdocs_read_as_markdown",
            "gdocs_read_outline",
            "gdocs_list_named_ranges",
            "gdocs_acknowledge_human_changes",
            // Bundle 2A/2B/4A listings are reads — safe to ship in read-only.
            "gdocs_list_documents",
            "gdocs_list_permissions",
            "gdocs_list_comments",
        ],
    },
];

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

    #[test]
    fn gdocs_package_has_all_tools() {
        let pkg = find_package("gdocs").expect("gdocs package must exist");
        assert_eq!(pkg.tools.len(), 28, "gdocs package must list 28 tools");
        for required in &[
            "gdocs_create",
            "gdocs_create_from_markdown",
            "gdocs_create_from_docx",
            "gdocs_share",
            "gdocs_export",
            "gdocs_list_tabs",
            "gdocs_add_tab",
            "gdocs_read_as_markdown",
            "gdocs_read_outline",
            "gdocs_list_named_ranges",
            "gdocs_replace_text",
            "gdocs_insert_after_text",
            "gdocs_insert_before_text",
            "gdocs_insert_between",
            "gdocs_delete_text",
            "gdocs_replace_section",
            "gdocs_append_markdown",
            "gdocs_apply_edits",
            "gdocs_style_text",
            "gdocs_create_named_range",
            "gdocs_replace_named_range",
            "gdocs_acknowledge_human_changes",
            "gdocs_list_documents",
            "gdocs_list_permissions",
            "gdocs_unshare",
            "gdocs_add_comment",
            "gdocs_list_comments",
            "gdocs_resolve_comment",
        ] {
            assert!(
                pkg.tools.contains(required),
                "gdocs package missing tool: {required}"
            );
        }
    }

    #[test]
    fn gdocsread_readonly_package_subset() {
        let pkg = find_package("gdocsread").expect("gdocsread package must exist");
        assert_eq!(pkg.tools.len(), 9);
        // Every entry must be a gdocs_* tool.
        for t in pkg.tools {
            assert!(t.starts_with("gdocs_"), "gdocsread non-gdocs tool: {t}");
        }
        // No write/mutation tools should leak in (acknowledge_human_changes
        // is read-only: it just resets the revision cursor; list_permissions
        // and list_comments are reads).
        let write_substrings = [
            "create_",
            "delete_",
            "insert_",
            "style_",
            "append_",
            "apply_",
            "share",
            "add_tab",
            "add_comment",
            "resolve_comment",
            "unshare",
        ];
        for t in pkg.tools {
            // Also catch `replace_*` writes — exclude `replace_*` rather
            // than substring "replace_" so the test stays explicit.
            assert!(
                !t.starts_with("gdocs_replace_"),
                "gdocsread should not contain write tool: {t} (replace_*)"
            );
            for write in &write_substrings {
                assert!(
                    !t.contains(write),
                    "gdocsread should not contain write tool: {t} (matched '{write}')"
                );
            }
        }
        // Every tool listed must also exist in the full `gdocs` package.
        let full = find_package("gdocs").expect("gdocs package");
        for t in pkg.tools {
            assert!(
                full.tools.contains(t),
                "gdocsread tool {t} missing from full gdocs package"
            );
        }
    }
}
