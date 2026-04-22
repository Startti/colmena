use super::common::NamedStyle;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExcelIR {
    pub kind: ExcelKindTag,
    pub artifact_id: String,
    pub version_id: String,
    pub schema_version: String,
    pub workbook: Workbook,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExcelKindTag {
    Excel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Workbook {
    pub sheets: Vec<Sheet>,
    #[serde(default)]
    pub named_styles: BTreeMap<String, NamedStyle>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    pub id: String,
    pub name: String,
    pub order: u32,
    #[serde(default)]
    pub columns: Vec<ColumnSpec>,
    #[serde(default)]
    pub cells: BTreeMap<String, Cell>,
    #[serde(default)]
    pub tables: Vec<NamedTable>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnSpec {
    pub index: u32,
    pub width: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub value: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub value_type: Option<CellType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CellType {
    String,
    Number,
    Boolean,
    Date,
    Formula,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedTable {
    pub id: String,
    pub name: String,
    pub range: String,
    #[serde(default)]
    pub header_row: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_preset: Option<String>,
}

impl ExcelIR {
    pub fn empty(artifact_id: impl Into<String>, version_id: impl Into<String>) -> Self {
        Self {
            kind: ExcelKindTag::Excel,
            artifact_id: artifact_id.into(),
            version_id: version_id.into(),
            schema_version: super::common::SCHEMA_VERSION.to_string(),
            workbook: Workbook::default(),
        }
    }

    pub fn sheet_mut(&mut self, sheet_id: &str) -> Option<&mut Sheet> {
        self.workbook.sheets.iter_mut().find(|s| s.id == sheet_id)
    }

    pub fn sheet(&self, sheet_id: &str) -> Option<&Sheet> {
        self.workbook.sheets.iter().find(|s| s.id == sheet_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_empty_excel_ir() {
        let ir = ExcelIR::empty("art_x", "v1");
        let json = serde_json::to_string(&ir).unwrap();
        let back: ExcelIR = serde_json::from_str(&json).unwrap();
        assert_eq!(back.artifact_id, "art_x");
        assert_eq!(back.version_id, "v1");
        assert!(back.workbook.sheets.is_empty());
    }

    #[test]
    fn roundtrip_ir_with_cells() {
        let mut ir = ExcelIR::empty("art_x", "v1");
        let mut cells = BTreeMap::new();
        cells.insert(
            "A1".into(),
            Cell {
                value: serde_json::json!("Hello"),
                value_type: Some(CellType::String),
                format: None,
                style_ref: None,
            },
        );
        ir.workbook.sheets.push(Sheet {
            id: "sheet_01".into(),
            name: "Ventas".into(),
            order: 0,
            columns: vec![],
            cells,
            tables: vec![],
        });
        let json = serde_json::to_value(&ir).unwrap();
        assert_eq!(json["workbook"]["sheets"][0]["cells"]["A1"]["value"], "Hello");
    }
}
