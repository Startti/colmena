//! Small workbooks for tests, assembled from XML strings as stored entries (so
//! every byte of every part is visible in the test that builds it).

use crate::tabular_prepare::zipfix::{build, Entry};

pub(crate) struct Wb {
    /// Name, `<row>` elements.
    sheets: Vec<(String, String)>,
    hidden: Vec<usize>,
}

impl Wb {
    pub fn new() -> Self {
        Self {
            sheets: Vec::new(),
            hidden: Vec::new(),
        }
    }

    pub fn sheet(mut self, name: &str, rows: &str) -> Self {
        self.sheets.push((name.to_string(), rows.to_string()));
        self
    }

    pub fn hidden(mut self, index: usize) -> Self {
        self.hidden.push(index);
        self
    }

    pub fn build(self) -> Vec<u8> {
        let rels_ns = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        let mut sheets_xml = String::new();
        let mut rels = String::new();
        let mut entries = Vec::new();
        for (i, (name, rows)) in self.sheets.iter().enumerate() {
            let state = if self.hidden.contains(&i) {
                " state=\"hidden\""
            } else {
                ""
            };
            sheets_xml.push_str(&format!(
                "<sheet name=\"{name}\" sheetId=\"{}\"{state} r:id=\"rId{}\"/>",
                i + 1,
                i + 1
            ));
            rels.push_str(&format!(
                "<Relationship Id=\"rId{}\" Type=\"{rels_ns}/worksheet\" Target=\"worksheets/sheet{}.xml\"/>",
                i + 1,
                i + 1
            ));
            entries.push(Entry::stored(
                &format!("xl/worksheets/sheet{}.xml", i + 1),
                format!("<worksheet><sheetData>{rows}</sheetData></worksheet>").as_bytes(),
            ));
        }
        let workbook =
            format!("<workbook xmlns:r=\"{rels_ns}\"><sheets>{sheets_xml}</sheets></workbook>");
        let mut all = vec![
            Entry::stored(
                "_rels/.rels",
                format!(
                    "<Relationships><Relationship Id=\"rId1\" Type=\"{rels_ns}/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>"
                )
                .as_bytes(),
            ),
            Entry::stored("xl/workbook.xml", workbook.as_bytes()),
            Entry::stored(
                "xl/_rels/workbook.xml.rels",
                format!("<Relationships>{rels}</Relationships>").as_bytes(),
            ),
        ];
        all.extend(entries);
        build(&all)
    }
}
