//! Shared collision-policy + error-envelope helpers for sheet write
//! dispatchers (`gsheets_run_python` and `crdt_doc_run_python`).
//!
//! This module is data-only: no async, no I/O. The dispatchers fetch
//! the live metadata and call into here to (a) parse the operator-
//! supplied policy and (b) build the structured "SheetExists" error
//! returned to the LLM when the policy is `fail`.

use serde::{Deserialize, Serialize};

/// What the dispatcher does when a target tab already exists.
/// Configurable per-tool via `fixed_config.on_existing_sheet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CollisionPolicy {
    /// Default. Dispatcher returns a structured `SheetExists` error
    /// without writing anything. LLM must explicitly choose
    /// `update_in_place`, `overwrite`, or rename.
    #[default]
    Fail,
    /// Legacy behavior — write as `"Name (2)"` silently.
    AutoSuffix,
    /// Replace the existing tab. Operator accepts the risk; the LLM
    /// doesn't get a round-trip.
    Overwrite,
}

/// Parse the operator-supplied policy string. Unknown values yield a
/// clear error listing valid options. Implemented as a thin wrapper
/// over `serde::Deserialize` so the variant list stays single-sourced
/// to the enum definition.
pub fn parse_policy(s: &str) -> Result<CollisionPolicy, String> {
    let quoted = format!(r#""{s}""#);
    serde_json::from_str::<CollisionPolicy>(&quoted).map_err(|_| {
        format!("unknown on_existing_sheet value '{s}'; valid: fail, auto_suffix, overwrite")
    })
}

/// Metadata about an existing tab that the LLM tried to write to.
/// Surfaced inside the `SheetExists` error so the LLM can decide
/// what to do (rename / update / overwrite) without another round-trip.
#[derive(Debug, Clone)]
pub struct TabMeta {
    pub n_rows: u64,
    pub n_cols: u64,
    pub columns: Vec<String>,
    /// Spreadsheet-level `modifiedTime` (RFC 3339) from Drive, when
    /// available. Surfaced as `current_state.last_modified` so the LLM
    /// can tell fresh data from stale before overwriting. `None` when
    /// the source has no Drive concept (CRDT) or the lookup failed.
    pub last_modified: Option<String>,
}

/// Build the structured `SheetExists` error payload that the
/// dispatcher returns to the LLM when `policy == Fail`.
pub fn build_sheet_exists_error(
    tab: &str,
    spreadsheet_id: Option<&str>,
    meta: &TabMeta,
) -> serde_json::Value {
    let advice = format!(
        "The tab '{tab}' already exists with data. Recommended: use a different name \
         (e.g., '{tab}_analysis'). If you must touch the existing tab, choose \
         update_in_place (patch specific rows) or overwrite (replace everything — \
         destructive)."
    );
    let mut current_state = serde_json::json!({
        "n_rows": meta.n_rows,
        "n_cols": meta.n_cols,
        "columns": meta.columns,
    });
    if let Some(modified) = &meta.last_modified {
        current_state["last_modified"] = serde_json::Value::String(modified.clone());
    }
    let mut payload = serde_json::json!({
        "error": "SheetExists",
        "tab": tab,
        "current_state": current_state,
        "advice": advice,
        "valid_next_moves": [
            {
                "action": "rename",
                "example_code": format!("output_sheets = {{'{tab}_review': df}}"),
            },
            {
                "action": "update_in_place",
                "example_code": format!(
                    "output_sheets = {{'{tab}': {{'mode':'update_in_place','df':df,'key':'<unique_col>'}}}}"
                ),
            },
            {
                "action": "overwrite",
                "example_code": format!(
                    "output_sheets = {{'{tab}': {{'mode':'overwrite','df':df}}}}"
                ),
            },
        ],
    });
    if let Some(sid) = spreadsheet_id {
        payload["spreadsheet_id"] = serde_json::Value::String(sid.to_string());
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_parses_known_values() {
        assert_eq!(parse_policy("fail").unwrap(), CollisionPolicy::Fail);
        assert_eq!(
            parse_policy("auto_suffix").unwrap(),
            CollisionPolicy::AutoSuffix
        );
        assert_eq!(
            parse_policy("overwrite").unwrap(),
            CollisionPolicy::Overwrite
        );
    }

    #[test]
    fn policy_default_is_fail() {
        assert_eq!(CollisionPolicy::default(), CollisionPolicy::Fail);
    }

    #[test]
    fn policy_parse_error_lists_valid_values() {
        let err = parse_policy("clobber").unwrap_err();
        assert!(err.contains("clobber"));
        assert!(err.contains("fail"));
        assert!(err.contains("auto_suffix"));
        assert!(err.contains("overwrite"));
    }

    #[test]
    fn sheet_exists_error_has_required_fields() {
        let meta = TabMeta {
            n_rows: 4998,
            n_cols: 12,
            columns: vec!["product_id".into(), "name".into(), "price".into()],
            last_modified: Some("2026-06-04T10:23:00Z".into()),
        };
        let err = build_sheet_exists_error("Sales", Some("1xyz"), &meta);
        assert_eq!(err["error"], "SheetExists");
        assert_eq!(err["tab"], "Sales");
        assert_eq!(err["spreadsheet_id"], "1xyz");
        assert_eq!(err["current_state"]["n_rows"], 4998);
        assert_eq!(err["current_state"]["n_cols"], 12);
        assert_eq!(err["current_state"]["columns"][0], "product_id");
        assert_eq!(
            err["current_state"]["last_modified"],
            "2026-06-04T10:23:00Z"
        );

        let advice = err["advice"].as_str().unwrap();
        assert!(advice.contains("Sales"));
        assert!(advice.contains("update_in_place"));
        assert!(advice.contains("overwrite"));

        let moves = err["valid_next_moves"].as_array().unwrap();
        assert_eq!(moves.len(), 3);
        let actions: Vec<&str> = moves
            .iter()
            .map(|m| m["action"].as_str().unwrap())
            .collect();
        assert!(actions.contains(&"rename"));
        assert!(actions.contains(&"update_in_place"));
        assert!(actions.contains(&"overwrite"));
    }

    #[test]
    fn sheet_exists_error_omits_spreadsheet_id_when_absent() {
        let meta = TabMeta {
            n_rows: 1,
            n_cols: 1,
            columns: vec!["x".into()],
            last_modified: None,
        };
        let err = build_sheet_exists_error("S", None, &meta);
        assert!(err.get("spreadsheet_id").is_none());
        // last_modified omitted when the source can't provide it.
        assert!(err["current_state"].get("last_modified").is_none());
    }
}
