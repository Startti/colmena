use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Patch {
    /// ID of the artifact this patch targets (e.g. "art_abc123").
    pub artifact_id: String,

    /// Version the caller based this patch on (e.g. "v3"). Server rebases
    /// automatically when current version is newer and ops don't conflict.
    pub base_version: String,

    /// Who authored this patch. Only "user" patches generate narration for the LLM.
    #[serde(default = "default_source")]
    pub source: PatchSource,

    /// Ordered list of operations applied atomically.
    pub ops: Vec<PatchOp>,
}

fn default_source() -> PatchSource {
    PatchSource::Agent
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PatchSource {
    Agent,
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op")]
pub enum PatchOp {
    /// Set the value of a single cell. Creates it if missing, overwrites if
    /// present. Use for isolated changes. For contiguous bulk updates, prefer
    /// `set_range`.
    #[serde(rename = "set_cell")]
    SetCell {
        /// Stable sheet ID (e.g. "sheet_01"). NOT the display name.
        sheet_id: String,
        /// A1-style address (e.g. "B5", "AA120"). Case-insensitive.
        address: String,
        /// The value. Type inferred from JSON type unless `value_type` overrides.
        value: serde_json::Value,
        /// Optional: override the inferred type. Use for numbers stored as text,
        /// or formula strings (prefix "=").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value_type: Option<String>,
        /// Optional: Excel number format spec (e.g. "#,##0", "0.00%").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
        /// Optional: reference to a style defined in `named_styles`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style_ref: Option<String>,
    },

    /// Bulk write a rectangular region. Rows are outer array, columns inner.
    /// Existing cells in the range are overwritten.
    #[serde(rename = "set_range")]
    SetRange {
        sheet_id: String,
        /// Range in A1 notation (e.g. "A1:C10").
        range: String,
        /// 2D array of values, row-major. Missing/null cells are left untouched.
        values: Vec<Vec<serde_json::Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value_types: Option<Vec<Vec<Option<String>>>>,
    },

    /// Remove all cells in a range. Does NOT delete rows/columns.
    #[serde(rename = "clear_range")]
    ClearRange { sheet_id: String, range: String },

    /// Insert a row, shifting subsequent rows down. `before_row` is 1-indexed.
    #[serde(rename = "insert_row")]
    InsertRow {
        sheet_id: String,
        before_row: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<Vec<serde_json::Value>>,
    },

    /// Delete a row, shifting subsequent rows up. `row_index` is 1-indexed.
    #[serde(rename = "delete_row")]
    DeleteRow { sheet_id: String, row_index: u32 },

    /// Insert a column, shifting subsequent columns right. `before_col` is 0-indexed (A=0).
    #[serde(rename = "insert_column")]
    InsertColumn {
        sheet_id: String,
        before_col: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<Vec<serde_json::Value>>,
    },

    /// Delete a column, shifting subsequent columns left. `col_index` is 0-indexed.
    #[serde(rename = "delete_column")]
    DeleteColumn { sheet_id: String, col_index: u32 },

    /// Create a new sheet. Returns the generated sheet_id in the output.
    #[serde(rename = "add_sheet")]
    AddSheet {
        /// Display name for the sheet.
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at_index: Option<u32>,
    },

    /// Rename an existing sheet. sheet_id is stable.
    #[serde(rename = "rename_sheet")]
    RenameSheet { sheet_id: String, new_name: String },

    /// Delete a sheet and all its cells/tables.
    #[serde(rename = "delete_sheet")]
    DeleteSheet { sheet_id: String },

    /// Reorder sheets. `order` is the full list of sheet IDs in desired order.
    #[serde(rename = "reorder_sheets")]
    ReorderSheets { order: Vec<String> },

    /// Define a named table over a range.
    #[serde(rename = "create_table")]
    CreateTable {
        sheet_id: String,
        range: String,
        name: String,
        #[serde(default)]
        header_row: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style_preset: Option<String>,
    },

    /// Change the extent of a named table.
    #[serde(rename = "resize_table")]
    ResizeTable { table_id: String, new_range: String },

    /// Delete a named table (cells inside range persist).
    #[serde(rename = "delete_table")]
    DeleteTable { table_id: String },

    /// Set the width of a column. `col` is 0-indexed.
    #[serde(rename = "set_column_width")]
    SetColumnWidth { sheet_id: String, col: u32, width: f64 },

    /// Create or update a named style referenced by cells via `style_ref`.
    #[serde(rename = "define_style")]
    DefineStyle {
        style_ref: String,
        definition: serde_json::Value,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_op_serializes_with_tag() {
        let op = PatchOp::SetCell {
            sheet_id: "sheet_01".into(),
            address: "A1".into(),
            value: serde_json::json!("hi"),
            value_type: None,
            format: None,
            style_ref: None,
        };
        let j = serde_json::to_value(&op).unwrap();
        assert_eq!(j["op"], "set_cell");
        assert_eq!(j["sheet_id"], "sheet_01");
    }

    #[test]
    fn patch_op_schema_generates() {
        let schema = schemars::schema_for!(PatchOp);
        let s = serde_json::to_string(&schema).unwrap();
        assert!(s.contains("set_cell"));
        assert!(s.contains("set_range"));
        assert!(s.contains("A1-style address"));
    }

    #[test]
    fn patch_roundtrip() {
        let p = Patch {
            artifact_id: "art_x".into(),
            base_version: "v1".into(),
            source: PatchSource::Agent,
            ops: vec![PatchOp::DeleteSheet {
                sheet_id: "sheet_01".into(),
            }],
        };
        let j = serde_json::to_string(&p).unwrap();
        let back: Patch = serde_json::from_str(&j).unwrap();
        assert_eq!(back.ops.len(), 1);
    }
}
