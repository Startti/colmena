use crate::documents::domain::ir::{Block, Run, WordIR};
use crate::documents::domain::{IRRenderer, RenderError};
use async_trait::async_trait;
use docx_rs::{
    Docx, Paragraph, Run as DocxRun, RunFonts, Table as DocxTable, TableCell as DocxCell,
    TableRow as DocxRow,
};

pub struct WordRenderer;

impl WordRenderer {
    fn render_sync(ir: &WordIR) -> Result<Vec<u8>, RenderError> {
        let mut doc = Docx::new();
        for block in &ir.document.blocks {
            match block {
                Block::Heading { level, runs, .. } => {
                    let mut p = Paragraph::new().style(&format!("Heading{level}"));
                    for run in runs {
                        p = p.add_run(build_run(run));
                    }
                    doc = doc.add_paragraph(p);
                }
                Block::Paragraph { runs, .. } => {
                    let mut p = Paragraph::new();
                    for run in runs {
                        p = p.add_run(build_run(run));
                    }
                    doc = doc.add_paragraph(p);
                }
                Block::List { items, style, .. } => {
                    let num_id = match style {
                        crate::documents::domain::ir::ListStyle::Bullet => 1,
                        crate::documents::domain::ir::ListStyle::Numbered => 2,
                    };
                    for it in items {
                        let mut p = Paragraph::new().numbering(
                            docx_rs::NumberingId::new(num_id),
                            docx_rs::IndentLevel::new(0),
                        );
                        for run in &it.runs {
                            p = p.add_run(build_run(run));
                        }
                        doc = doc.add_paragraph(p);
                    }
                }
                Block::Table { rows, .. } => {
                    let mut drows = Vec::new();
                    for row in rows {
                        let mut dcells = Vec::new();
                        for cell in &row.cells {
                            let mut p = Paragraph::new();
                            for run in &cell.runs {
                                p = p.add_run(build_run(run));
                            }
                            dcells.push(DocxCell::new().add_paragraph(p));
                        }
                        drows.push(DocxRow::new(dcells));
                    }
                    let tbl = DocxTable::new(drows);
                    doc = doc.add_table(tbl);
                }
            }
        }
        let mut buf: Vec<u8> = Vec::new();
        doc.build()
            .pack(std::io::Cursor::new(&mut buf))
            .map_err(|e| RenderError::Failed(format!("pack docx: {e}")))?;
        Ok(buf)
    }
}

fn build_run(run: &Run) -> DocxRun {
    let mut r = DocxRun::new().add_text(&run.text);
    if run.bold.unwrap_or(false) {
        r = r.bold();
    }
    if run.italic.unwrap_or(false) {
        r = r.italic();
    }
    if run.underline.unwrap_or(false) {
        r = r.underline("single");
    }
    if let Some(sz) = run.size {
        r = r.size((sz * 2.0) as usize);
    }
    if let Some(color) = &run.color {
        r = r.color(color.trim_start_matches('#').to_string());
    }
    if run.size.is_some() {
        r = r.fonts(RunFonts::new().ascii("Calibri"));
    }
    r
}

#[async_trait]
impl IRRenderer for WordRenderer {
    async fn render(&self, ir: &serde_json::Value) -> Result<Vec<u8>, RenderError> {
        let ir: WordIR = serde_json::from_value(ir.clone())
            .map_err(|e| RenderError::Failed(format!("parse word IR: {e}")))?;
        Self::render_sync(&ir)
    }
    fn target_extension(&self) -> &'static str {
        "docx"
    }
    fn target_mime(&self) -> &'static str {
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ir::{Block, Run};

    #[tokio::test]
    async fn renders_minimal_docx() {
        let mut ir = WordIR::empty("x", "v1");
        ir.document.blocks.push(Block::Heading {
            id: "h1".into(),
            level: 1,
            runs: vec![Run {
                id: "r1".into(),
                text: "Title".into(),
                bold: Some(true),
                italic: None,
                underline: None,
                size: None,
                color: None,
            }],
        });
        ir.document.blocks.push(Block::Paragraph {
            id: "p1".into(),
            runs: vec![Run {
                id: "r1".into(),
                text: "body".into(),
                bold: None,
                italic: None,
                underline: None,
                size: None,
                color: None,
            }],
        });
        let bytes = WordRenderer
            .render(&serde_json::to_value(&ir).unwrap())
            .await
            .unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[..2], b"PK");
    }
}
