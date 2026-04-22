use crate::documents::domain::ir::ExcelIR;
use crate::documents::domain::{DocumentError, IRValidator};
use std::collections::HashSet;

pub struct ExcelValidator;

impl IRValidator for ExcelValidator {
    fn validate(&self, ir_value: &serde_json::Value) -> Result<(), DocumentError> {
        let ir: ExcelIR = serde_json::from_value(ir_value.clone()).map_err(|e| {
            DocumentError::IRValidationFailed {
                path: "/".into(),
                reason: format!("not a valid Excel IR: {e}"),
            }
        })?;

        let mut seen_sheet_ids: HashSet<&str> = HashSet::new();
        for (i, sheet) in ir.workbook.sheets.iter().enumerate() {
            if !seen_sheet_ids.insert(&sheet.id) {
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/workbook/sheets/{i}/id"),
                    reason: format!("duplicate sheet ID: {}", sheet.id),
                });
            }
        }

        let mut seen_table_ids: HashSet<&str> = HashSet::new();
        for sheet in &ir.workbook.sheets {
            for (i, t) in sheet.tables.iter().enumerate() {
                if !seen_table_ids.insert(&t.id) {
                    return Err(DocumentError::IRValidationFailed {
                        path: format!("/workbook/sheets/{}/tables/{i}/id", sheet.id),
                        reason: format!("duplicate table ID: {}", t.id),
                    });
                }
            }
        }

        for sheet in &ir.workbook.sheets {
            for (addr, cell) in &sheet.cells {
                if let Some(sref) = &cell.style_ref {
                    if !ir.workbook.named_styles.contains_key(sref) {
                        return Err(DocumentError::IRValidationFailed {
                            path: format!("/workbook/sheets/{}/cells/{addr}/style_ref", sheet.id),
                            reason: format!("style_ref '{sref}' not defined in named_styles"),
                        });
                    }
                }
            }
        }

        for sheet in &ir.workbook.sheets {
            for (addr, cell) in &sheet.cells {
                if let Some(ct) = &cell.value_type {
                    use crate::documents::domain::ir::CellType;
                    let ok = match ct {
                        CellType::String | CellType::Formula => cell.value.is_string(),
                        CellType::Number => cell.value.is_number(),
                        CellType::Boolean => cell.value.is_boolean(),
                        CellType::Date => cell.value.is_string(),
                    };
                    if !ok {
                        return Err(DocumentError::IRValidationFailed {
                            path: format!("/workbook/sheets/{}/cells/{addr}", sheet.id),
                            reason: format!("value does not match declared type {:?}", ct),
                        });
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ir::{Cell, CellType, ExcelIR, Sheet};
    use std::collections::BTreeMap;

    fn base_ir() -> ExcelIR {
        let mut ir = ExcelIR::empty("art_x", "v1");
        ir.workbook.sheets.push(Sheet {
            id: "sheet_01".into(),
            name: "S".into(),
            order: 0,
            columns: vec![],
            cells: BTreeMap::new(),
            tables: vec![],
        });
        ir
    }

    #[test]
    fn empty_ir_is_valid() {
        let v = ExcelValidator;
        v.validate(&serde_json::to_value(&base_ir()).unwrap()).unwrap();
    }

    #[test]
    fn duplicate_sheet_ids_fail() {
        let mut ir = base_ir();
        ir.workbook.sheets.push(Sheet {
            id: "sheet_01".into(),
            name: "B".into(),
            order: 1,
            columns: vec![],
            cells: BTreeMap::new(),
            tables: vec![],
        });
        let v = ExcelValidator;
        let err = v.validate(&serde_json::to_value(&ir).unwrap()).unwrap_err();
        assert!(matches!(err, DocumentError::IRValidationFailed { .. }));
    }

    #[test]
    fn dangling_style_ref_fails() {
        let mut ir = base_ir();
        let mut cells = BTreeMap::new();
        cells.insert(
            "A1".into(),
            Cell {
                value: serde_json::json!("hi"),
                value_type: Some(CellType::String),
                format: None,
                style_ref: Some("missing".into()),
            },
        );
        ir.workbook.sheets[0].cells = cells;
        let v = ExcelValidator;
        assert!(v.validate(&serde_json::to_value(&ir).unwrap()).is_err());
    }

    #[test]
    fn type_mismatch_fails() {
        let mut ir = base_ir();
        let mut cells = BTreeMap::new();
        cells.insert(
            "A1".into(),
            Cell {
                value: serde_json::json!("notanumber"),
                value_type: Some(CellType::Number),
                format: None,
                style_ref: None,
            },
        );
        ir.workbook.sheets[0].cells = cells;
        let v = ExcelValidator;
        assert!(v.validate(&serde_json::to_value(&ir).unwrap()).is_err());
    }
}
