use super::common::NamedStyle;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordIR {
    pub kind: WordKindTag,
    pub artifact_id: String,
    pub version_id: String,
    pub schema_version: String,
    pub document: WordDocument,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WordKindTag {
    Word,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WordDocument {
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub named_styles: BTreeMap<String, NamedStyle>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Block {
    Heading {
        #[serde(default)]
        id: String,
        level: u8,
        runs: Vec<Run>,
    },
    Paragraph {
        #[serde(default)]
        id: String,
        runs: Vec<Run>,
    },
    List {
        #[serde(default)]
        id: String,
        #[serde(default = "default_list_style")]
        style: ListStyle,
        items: Vec<ListItem>,
    },
    Table {
        #[serde(default)]
        id: String,
        rows: Vec<TableRow>,
    },
}

fn default_list_style() -> ListStyle {
    ListStyle::Bullet
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListStyle {
    Bullet,
    Numbered,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    #[serde(default)]
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underline: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListItem {
    #[serde(default)]
    pub id: String,
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableRow {
    #[serde(default)]
    pub id: String,
    pub cells: Vec<TableCell>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableCell {
    pub runs: Vec<Run>,
}

impl Block {
    pub fn id(&self) -> &str {
        match self {
            Block::Heading { id, .. }
            | Block::Paragraph { id, .. }
            | Block::List { id, .. }
            | Block::Table { id, .. } => id,
        }
    }
}

impl WordIR {
    pub fn empty(artifact_id: impl Into<String>, version_id: impl Into<String>) -> Self {
        Self {
            kind: WordKindTag::Word,
            artifact_id: artifact_id.into(),
            version_id: version_id.into(),
            schema_version: super::common::SCHEMA_VERSION.to_string(),
            document: WordDocument::default(),
        }
    }

    pub fn block_mut(&mut self, block_id: &str) -> Option<&mut Block> {
        self.document.blocks.iter_mut().find(|b| b.id() == block_id)
    }

    pub fn block_index(&self, block_id: &str) -> Option<usize> {
        self.document.blocks.iter().position(|b| b.id() == block_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_ir_roundtrip() {
        let mut ir = WordIR::empty("art_x", "v1");
        ir.document.blocks.push(Block::Heading {
            id: "blk_01".into(),
            level: 1,
            runs: vec![Run {
                id: "run_01".into(),
                text: "Title".into(),
                bold: Some(true),
                italic: None,
                underline: None,
                size: None,
                color: None,
            }],
        });
        let j = serde_json::to_value(&ir).unwrap();
        assert_eq!(j["document"]["blocks"][0]["type"], "heading");
        assert_eq!(j["document"]["blocks"][0]["runs"][0]["text"], "Title");
        let back: WordIR = serde_json::from_value(j).unwrap();
        assert_eq!(back.document.blocks.len(), 1);
    }
}
