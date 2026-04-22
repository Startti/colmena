use crate::documents::domain::ir::{Block, WordIR};
use crate::documents::domain::{DocumentError, IRValidator};
use std::collections::HashSet;

pub struct WordValidator;

impl IRValidator for WordValidator {
    fn validate(&self, ir_value: &serde_json::Value) -> Result<(), DocumentError> {
        let ir: WordIR =
            serde_json::from_value(ir_value.clone()).map_err(|e| DocumentError::IRValidationFailed {
                path: "/".into(),
                reason: format!("not a valid Word IR: {e}"),
            })?;

        let mut block_ids: HashSet<&str> = HashSet::new();
        for (i, block) in ir.document.blocks.iter().enumerate() {
            if !block_ids.insert(block.id()) {
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/document/blocks/{i}/id"),
                    reason: format!("duplicate block ID: {}", block.id()),
                });
            }
            validate_block(block, i)?;
        }
        Ok(())
    }
}

fn validate_block(block: &Block, idx: usize) -> Result<(), DocumentError> {
    match block {
        Block::Heading { level, runs, .. } => {
            if !(1..=6).contains(level) {
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/document/blocks/{idx}/level"),
                    reason: format!("heading level must be 1..=6, got {level}"),
                });
            }
            check_run_ids(runs, &format!("/document/blocks/{idx}"))?;
        }
        Block::Paragraph { runs, .. } => {
            check_run_ids(runs, &format!("/document/blocks/{idx}"))?;
        }
        Block::List { items, .. } => {
            let mut seen: HashSet<&str> = HashSet::new();
            for (i, it) in items.iter().enumerate() {
                if !seen.insert(&it.id) {
                    return Err(DocumentError::IRValidationFailed {
                        path: format!("/document/blocks/{idx}/items/{i}/id"),
                        reason: format!("duplicate list item ID: {}", it.id),
                    });
                }
                check_run_ids(&it.runs, &format!("/document/blocks/{idx}/items/{i}"))?;
            }
        }
        Block::Table { rows, .. } => {
            let mut seen: HashSet<&str> = HashSet::new();
            for (i, row) in rows.iter().enumerate() {
                if !seen.insert(&row.id) {
                    return Err(DocumentError::IRValidationFailed {
                        path: format!("/document/blocks/{idx}/rows/{i}/id"),
                        reason: format!("duplicate row ID: {}", row.id),
                    });
                }
                for (c, cell) in row.cells.iter().enumerate() {
                    check_run_ids(
                        &cell.runs,
                        &format!("/document/blocks/{idx}/rows/{i}/cells/{c}"),
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn check_run_ids(
    runs: &[crate::documents::domain::ir::Run],
    scope: &str,
) -> Result<(), DocumentError> {
    let mut seen: HashSet<&str> = HashSet::new();
    for (i, r) in runs.iter().enumerate() {
        if !seen.insert(&r.id) {
            return Err(DocumentError::IRValidationFailed {
                path: format!("{scope}/runs/{i}/id"),
                reason: format!("duplicate run ID in scope: {}", r.id),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ir::{Block, Run, WordIR};

    #[test]
    fn empty_word_is_valid() {
        let ir = WordIR::empty("x", "v1");
        WordValidator
            .validate(&serde_json::to_value(&ir).unwrap())
            .unwrap();
    }

    #[test]
    fn duplicate_block_ids_fail() {
        let mut ir = WordIR::empty("x", "v1");
        ir.document.blocks.push(Block::Paragraph {
            id: "b1".into(),
            runs: vec![],
        });
        ir.document.blocks.push(Block::Paragraph {
            id: "b1".into(),
            runs: vec![],
        });
        assert!(WordValidator
            .validate(&serde_json::to_value(&ir).unwrap())
            .is_err());
    }

    #[test]
    fn heading_level_out_of_range_fails() {
        let mut ir = WordIR::empty("x", "v1");
        ir.document.blocks.push(Block::Heading {
            id: "h".into(),
            level: 9,
            runs: vec![],
        });
        assert!(WordValidator
            .validate(&serde_json::to_value(&ir).unwrap())
            .is_err());
    }

    #[test]
    fn same_run_id_in_different_blocks_ok() {
        let mut ir = WordIR::empty("x", "v1");
        ir.document.blocks.push(Block::Paragraph {
            id: "b1".into(),
            runs: vec![Run {
                id: "r1".into(),
                text: "a".into(),
                bold: None,
                italic: None,
                underline: None,
                size: None,
                color: None,
            }],
        });
        ir.document.blocks.push(Block::Paragraph {
            id: "b2".into(),
            runs: vec![Run {
                id: "r1".into(),
                text: "b".into(),
                bold: None,
                italic: None,
                underline: None,
                size: None,
                color: None,
            }],
        });
        WordValidator
            .validate(&serde_json::to_value(&ir).unwrap())
            .unwrap();
    }
}
