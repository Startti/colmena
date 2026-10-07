//! Small workbooks for tests, assembled from XML strings as stored entries (so
//! every byte of every part is visible in the test that builds it).

use crate::tabular_prepare::zipfix::{build, Entry};

pub(crate) struct Wb {
    /// Name, `<row>` elements.
    sheets: Vec<(String, String)>,
    hidden: Vec<usize>,
    chartsheet: bool,
    date1904: bool,
    shared: Option<Vec<String>>,
    styles: Option<String>,
}

impl Wb {
    pub fn new() -> Self {
        Self {
            sheets: Vec::new(),
            hidden: Vec::new(),
            chartsheet: false,
            date1904: false,
            shared: None,
            styles: None,
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

    /// Adds a chart sheet after the worksheets: it has no cells.
    pub fn chartsheet(mut self) -> Self {
        self.chartsheet = true;
        self
    }

    pub fn date1904(mut self) -> Self {
        self.date1904 = true;
        self
    }

    pub fn shared(mut self, strings: &[&str]) -> Self {
        self.shared = Some(strings.iter().map(|s| s.to_string()).collect());
        self
    }

    /// `cellXfs` as the number-format ids of its `xf` elements, plus the custom
    /// formats (`id`, code).
    pub fn styles(mut self, xf_formats: &[u32], custom: &[(u32, &str)]) -> Self {
        let formats: String = custom
            .iter()
            .map(|(id, code)| format!("<numFmt numFmtId=\"{id}\" formatCode=\"{code}\"/>"))
            .collect();
        let xfs: String = xf_formats
            .iter()
            .map(|id| format!("<xf numFmtId=\"{id}\"/>"))
            .collect();
        self.styles = Some(format!(
            "<styleSheet><numFmts count=\"{}\">{formats}</numFmts><cellXfs count=\"{}\">{xfs}</cellXfs></styleSheet>",
            custom.len(),
            xf_formats.len()
        ));
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
            // A whole `worksheet` element is used as it is; anything else is rows.
            let xml = if rows.starts_with("<worksheet") {
                rows.clone()
            } else {
                format!("<worksheet><sheetData>{rows}</sheetData></worksheet>")
            };
            entries.push(Entry::stored(
                &format!("xl/worksheets/sheet{}.xml", i + 1),
                xml.as_bytes(),
            ));
        }
        if self.chartsheet {
            sheets_xml.push_str("<sheet name=\"Chart\" sheetId=\"90\" r:id=\"rId90\"/>");
            rels.push_str(&format!(
                "<Relationship Id=\"rId90\" Type=\"{rels_ns}/chartsheet\" Target=\"chartsheets/sheet1.xml\"/>"
            ));
            entries.push(Entry::stored("xl/chartsheets/sheet1.xml", b"<chartsheet/>"));
        }
        if let Some(strings) = &self.shared {
            let items: String = strings
                .iter()
                .map(|s| format!("<si><t>{s}</t></si>"))
                .collect();
            rels.push_str(&format!(
                "<Relationship Id=\"rId91\" Type=\"{rels_ns}/sharedStrings\" Target=\"sharedStrings.xml\"/>"
            ));
            entries.push(Entry::stored(
                "xl/sharedStrings.xml",
                format!(
                    "<sst count=\"{0}\" uniqueCount=\"{0}\">{items}</sst>",
                    strings.len()
                )
                .as_bytes(),
            ));
        }
        if let Some(styles) = &self.styles {
            rels.push_str(&format!(
                "<Relationship Id=\"rId92\" Type=\"{rels_ns}/styles\" Target=\"styles.xml\"/>"
            ));
            entries.push(Entry::stored("xl/styles.xml", styles.as_bytes()));
        }
        let pr = if self.date1904 {
            "<workbookPr date1904=\"1\"/>"
        } else {
            "<workbookPr/>"
        };
        let workbook =
            format!("<workbook xmlns:r=\"{rels_ns}\">{pr}<sheets>{sheets_xml}</sheets></workbook>");
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
