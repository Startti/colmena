//! HtmlValidator — implements IRValidator for HtmlIR.
//!
//! Lives in infrastructure because the implementation uses regex and JSON
//! traversal. The CONTRACT (what is validated, what errors arise) belongs
//! to the domain via `IRValidator` and `DocumentError`.

use crate::documents::domain::ir::html::{Block, HtmlIR, ImageSrc, VideoSrc};
use crate::documents::domain::ir::SlideLayout;
use crate::documents::domain::{DocumentError, IRValidator};
use std::collections::HashSet;

pub struct HtmlValidator;

impl IRValidator for HtmlValidator {
    fn validate(&self, ir: &serde_json::Value) -> Result<(), DocumentError> {
        let ir: HtmlIR =
            serde_json::from_value(ir.clone()).map_err(|e| DocumentError::IRValidationFailed {
                path: "/".into(),
                reason: format!("schema parse: {e}"),
            })?;

        validate_cardinality(&ir)?;
        validate_unique_ids(&ir)?;
        validate_layouts(&ir)?;
        validate_assets_referenced_consistency(&ir)?;
        Ok(())
    }
}

fn validate_cardinality(ir: &HtmlIR) -> Result<(), DocumentError> {
    if ir.slides.is_empty() {
        return Err(DocumentError::IRValidationFailed {
            path: "/slides".into(),
            reason: "must contain at least one slide".into(),
        });
    }
    Ok(())
}

fn validate_unique_ids(ir: &HtmlIR) -> Result<(), DocumentError> {
    let mut slide_ids = HashSet::new();
    let mut block_ids = HashSet::new();
    for slide in &ir.slides {
        if !slide_ids.insert(&slide.id) {
            return Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{}", slide.id),
                reason: "duplicate slide id".into(),
            });
        }
        for block in &slide.blocks {
            let id = block_id(block);
            if !block_ids.insert(id.clone()) {
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/slides/{}/blocks/{}", slide.id, id),
                    reason: "duplicate block id (across all slides)".into(),
                });
            }
        }
    }
    Ok(())
}

fn block_id(b: &Block) -> String {
    match b {
        Block::Heading { id, .. }
        | Block::Paragraph { id, .. }
        | Block::List { id, .. }
        | Block::Blockquote { id, .. }
        | Block::Code { id, .. }
        | Block::Table { id, .. }
        | Block::Chart { id, .. }
        | Block::KpiCard { id, .. }
        | Block::KpiGrid { id, .. }
        | Block::Image { id, .. }
        | Block::TwoColumns { id, .. }
        | Block::ThreeColumns { id, .. }
        | Block::Comparison { id, .. }
        | Block::Callout { id, .. }
        | Block::Divider { id }
        | Block::Video { id, .. }
        | Block::AutoToc { id, .. } => id.clone(),
    }
}

fn validate_layouts(ir: &HtmlIR) -> Result<(), DocumentError> {
    for slide in &ir.slides {
        match slide.layout {
            SlideLayout::Title | SlideLayout::SectionDivider => {
                if slide.title.as_deref().unwrap_or("").trim().is_empty() {
                    return Err(DocumentError::IRValidationFailed {
                        path: format!("/slides/{}", slide.id),
                        reason: format!("layout {:?} requires non-empty title", slide.layout),
                    });
                }
            }
            _ => {}
        }
        for block in &slide.blocks {
            if let Block::Heading { level, .. } = block {
                if !(1..=6).contains(level) {
                    return Err(DocumentError::IRValidationFailed {
                        path: format!("/slides/{}/blocks/{}", slide.id, block_id(block)),
                        reason: format!("heading level {level} out of range 1..=6"),
                    });
                }
            }
        }
    }
    Ok(())
}

fn validate_assets_referenced_consistency(ir: &HtmlIR) -> Result<(), DocumentError> {
    let declared: HashSet<&String> = ir.assets_referenced.iter().collect();
    let mut actual: HashSet<String> = HashSet::new();
    for slide in &ir.slides {
        collect_asset_refs(&slide.blocks, &mut actual);
    }
    for a in &actual {
        if !declared.contains(a) {
            return Err(DocumentError::IRValidationFailed {
                path: "/assets_referenced".into(),
                reason: format!("asset {a} referenced by block but not in assets_referenced"),
            });
        }
    }
    Ok(())
}

fn collect_asset_refs(blocks: &[Block], out: &mut HashSet<String>) {
    for b in blocks {
        match b {
            Block::Image {
                src: ImageSrc::Asset { asset_id },
                ..
            } => {
                out.insert(asset_id.clone());
            }
            Block::Video {
                src: VideoSrc::Asset { asset_id },
                ..
            } => {
                out.insert(asset_id.clone());
            }
            Block::TwoColumns { left, right, .. } => {
                collect_asset_refs(left, out);
                collect_asset_refs(right, out);
            }
            Block::ThreeColumns {
                left,
                middle,
                right,
                ..
            } => {
                collect_asset_refs(left, out);
                collect_asset_refs(middle, out);
                collect_asset_refs(right, out);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn minimal_ir() -> serde_json::Value {
        json!({
            "kind": "html",
            "artifact_id": "art_x",
            "version_id": "v1",
            "schema_version": "1.0.0",
            "doc_props": { "title": null, "author": null, "date": null, "locale": "en" },
            "theme": "executive",
            "layout_mode": "report",
            "footer": { "enabled": false, "page_numbers": false, "custom_text": null },
            "slides": [{
                "id": "sl_1", "layout": "blank",
                "title": null, "subtitle": null, "notes": null, "blocks": []
            }],
            "assets_referenced": []
        })
    }

    #[test]
    fn valid_minimal_ir_passes() {
        HtmlValidator.validate(&minimal_ir()).unwrap();
    }

    #[test]
    fn empty_slides_rejected() {
        let mut ir = minimal_ir();
        ir["slides"] = json!([]);
        let err = HtmlValidator.validate(&ir).unwrap_err();
        assert!(matches!(err, DocumentError::IRValidationFailed { .. }));
    }

    #[test]
    fn duplicate_slide_ids_rejected() {
        let mut ir = minimal_ir();
        ir["slides"] = json!([
            {"id": "sl_dup", "layout": "blank", "title": null, "subtitle": null, "notes": null, "blocks": []},
            {"id": "sl_dup", "layout": "blank", "title": null, "subtitle": null, "notes": null, "blocks": []}
        ]);
        assert!(HtmlValidator.validate(&ir).is_err());
    }

    #[test]
    fn title_layout_without_title_rejected() {
        let mut ir = minimal_ir();
        ir["slides"][0]["layout"] = json!("title");
        let err = HtmlValidator.validate(&ir).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("requires non-empty title"), "got: {msg}");
    }

    #[test]
    fn heading_level_7_rejected() {
        let mut ir = minimal_ir();
        ir["slides"][0]["blocks"] = json!([
            {"kind": "heading", "id": "blk_h", "level": 7, "runs": []}
        ]);
        assert!(HtmlValidator.validate(&ir).is_err());
    }

    #[test]
    fn duplicate_block_ids_across_slides_rejected() {
        let mut ir = minimal_ir();
        ir["slides"] = json!([
            {"id": "sl_a", "layout": "blank", "title": null, "subtitle": null, "notes": null,
             "blocks": [{"kind":"divider","id":"blk_d"}]},
            {"id": "sl_b", "layout": "blank", "title": null, "subtitle": null, "notes": null,
             "blocks": [{"kind":"divider","id":"blk_d"}]}
        ]);
        assert!(HtmlValidator.validate(&ir).is_err());
    }

    #[test]
    fn assets_referenced_mismatch_rejected() {
        let mut ir = minimal_ir();
        ir["slides"][0]["blocks"] = json!([
            {"kind":"image","id":"blk_i","src":{"kind":"asset","asset_id":"asset_z"},
             "alt":"x","caption":null,"position":"inline"}
        ]);
        // assets_referenced stays empty → mismatch
        assert!(HtmlValidator.validate(&ir).is_err());
    }
}
