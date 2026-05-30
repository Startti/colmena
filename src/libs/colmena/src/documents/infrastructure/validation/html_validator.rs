//! HtmlValidator — implements IRValidator for HtmlIR.
//!
//! Lives in infrastructure because the implementation uses regex and JSON
//! traversal. The CONTRACT (what is validated, what errors arise) belongs
//! to the domain via `IRValidator` and `DocumentError`.

use crate::documents::domain::ir::html::{
    AxisSpec, Block, ChartSpec, ChartType, HtmlIR, ImageSrc, Run, VideoSrc,
};
use crate::documents::domain::ir::SlideLayout;
use crate::documents::domain::{DocumentError, IRValidator};
use once_cell::sync::Lazy;
use regex::Regex;
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
        validate_security(&ir)?;
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

// ── Security statics ────────────────────────────────────────────────────────

static ALLOWED_LINK_SCHEMES: &[&str] = &["http", "https", "mailto", "tel"];
static DATA_URL_IMAGE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^data:image/(png|jpeg|jpg|gif|webp);base64,").unwrap()
});
static EXTERNAL_HTTP_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^https?://").unwrap());
static VIDEO_ID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z0-9_-]+$").unwrap());

const MAX_NESTING_DEPTH: u8 = 3;

fn link_scheme_ok(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    ALLOWED_LINK_SCHEMES
        .iter()
        .any(|s| lower.starts_with(&format!("{s}:")))
}

fn validate_security(ir: &HtmlIR) -> Result<(), DocumentError> {
    for slide in &ir.slides {
        validate_blocks_security(&slide.blocks, &slide.id, 0)?;
    }
    Ok(())
}

fn validate_blocks_security(
    blocks: &[Block],
    slide_id: &str,
    depth: u8,
) -> Result<(), DocumentError> {
    for b in blocks {
        match b {
            Block::Heading { runs, .. }
            | Block::Paragraph { runs, .. }
            | Block::Blockquote { runs, .. }
            | Block::Callout { runs, .. } => {
                check_runs_links(runs, slide_id, &block_id(b))?;
            }
            Block::List { items, id, .. } => {
                for item in items {
                    check_runs_links(&item.runs, slide_id, id)?;
                }
            }
            Block::Table { rows, id, .. } => {
                for row in rows {
                    for cell in &row.cells {
                        if let crate::documents::domain::ir::html::TableCell::Runs { runs } = cell {
                            check_runs_links(runs, slide_id, id)?;
                        }
                    }
                }
            }
            Block::Image { src, id, .. } => {
                check_image_src(src, slide_id, id)?;
            }
            Block::Video { src, id, .. } => {
                check_video_src(src, slide_id, id)?;
            }
            Block::Chart { chart, id, .. } => {
                check_chart(chart, slide_id, id)?;
            }
            Block::KpiGrid { columns, cards, id } => {
                if !matches!(columns, 2..=4) {
                    return Err(DocumentError::IRValidationFailed {
                        path: format!("/slides/{slide_id}/blocks/{id}"),
                        reason: "kpi_grid columns must be 2..=4".into(),
                    });
                }
                if cards.len() as u8 != *columns {
                    return Err(DocumentError::IRValidationFailed {
                        path: format!("/slides/{slide_id}/blocks/{id}"),
                        reason: "kpi_grid cards.len() must equal columns".into(),
                    });
                }
            }
            Block::TwoColumns { left, right, id, .. } => {
                if depth >= MAX_NESTING_DEPTH {
                    return Err(nesting_err(slide_id, id));
                }
                validate_blocks_security(left, slide_id, depth + 1)?;
                validate_blocks_security(right, slide_id, depth + 1)?;
            }
            Block::ThreeColumns {
                left,
                middle,
                right,
                id,
                ..
            } => {
                if depth >= MAX_NESTING_DEPTH {
                    return Err(nesting_err(slide_id, id));
                }
                validate_blocks_security(left, slide_id, depth + 1)?;
                validate_blocks_security(middle, slide_id, depth + 1)?;
                validate_blocks_security(right, slide_id, depth + 1)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn nesting_err(slide_id: &str, block_id: &str) -> DocumentError {
    DocumentError::IRValidationFailed {
        path: format!("/slides/{slide_id}/blocks/{block_id}"),
        reason: format!("nested layouts exceed max depth {MAX_NESTING_DEPTH}"),
    }
}

fn check_runs_links(runs: &[Run], slide_id: &str, block_id: &str) -> Result<(), DocumentError> {
    for r in runs {
        if let Some(link) = &r.link {
            if !link_scheme_ok(link) {
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/slides/{slide_id}/blocks/{block_id}/runs/{}", r.id),
                    reason: format!("URL scheme not allowed in link: {link}"),
                });
            }
        }
    }
    Ok(())
}

fn check_image_src(src: &ImageSrc, slide_id: &str, block_id: &str) -> Result<(), DocumentError> {
    match src {
        ImageSrc::Asset { .. } => Ok(()),
        ImageSrc::External { url } => {
            if !EXTERNAL_HTTP_RE.is_match(url) {
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/slides/{slide_id}/blocks/{block_id}/src"),
                    reason: format!("image external src must be http(s): {url}"),
                });
            }
            Ok(())
        }
        ImageSrc::DataUrl { data } => {
            if !DATA_URL_IMAGE_RE.is_match(data) {
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/slides/{slide_id}/blocks/{block_id}/src"),
                    reason: "image data url must be data:image/(png|jpeg|jpg|gif|webp);base64,..."
                        .into(),
                });
            }
            Ok(())
        }
    }
}

fn check_video_src(src: &VideoSrc, slide_id: &str, block_id: &str) -> Result<(), DocumentError> {
    match src {
        VideoSrc::Youtube { video_id } | VideoSrc::Vimeo { video_id } => {
            if !VIDEO_ID_RE.is_match(video_id) {
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/slides/{slide_id}/blocks/{block_id}/src/video_id"),
                    reason: "video_id must match [A-Za-z0-9_-]+".into(),
                });
            }
            Ok(())
        }
        VideoSrc::Asset { .. } => Ok(()),
    }
}

fn check_chart(c: &ChartSpec, slide_id: &str, block_id: &str) -> Result<(), DocumentError> {
    if c.series.is_empty() {
        return Err(DocumentError::IRValidationFailed {
            path: format!("/slides/{slide_id}/blocks/{block_id}/chart/series"),
            reason: "chart requires at least one series".into(),
        });
    }
    match c.chart_type {
        ChartType::Pie | ChartType::Doughnut => {
            if c.series.len() != 1 {
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/slides/{slide_id}/blocks/{block_id}/chart"),
                    reason: format!("{:?} chart requires exactly 1 series", c.chart_type),
                });
            }
        }
        ChartType::Bar | ChartType::Line | ChartType::Area | ChartType::Radar => {
            if let Some(AxisSpec {
                categories: Some(cats),
                ..
            }) = &c.x_axis
            {
                for s in &c.series {
                    if s.data.len() != cats.len() {
                        return Err(DocumentError::IRValidationFailed {
                            path: format!(
                                "/slides/{slide_id}/blocks/{block_id}/chart/series/{}",
                                s.name
                            ),
                            reason: format!(
                                "series '{}' data length {} != categories length {}",
                                s.name,
                                s.data.len(),
                                cats.len()
                            ),
                        });
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────────

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

    #[test]
    fn link_javascript_scheme_rejected() {
        let mut ir = minimal_ir();
        ir["slides"][0]["blocks"] = json!([{
            "kind": "paragraph", "id": "blk_p",
            "runs": [{"id":"run_1","text":"click","bold":false,"italic":false,
                      "underline":false,"code":false,"link":"javascript:alert(1)"}]
        }]);
        let err = HtmlValidator.validate(&ir).unwrap_err();
        assert!(err.to_string().contains("scheme"), "got: {err}");
    }

    #[test]
    fn image_external_ftp_rejected() {
        let mut ir = minimal_ir();
        ir["slides"][0]["blocks"] = json!([{
            "kind":"image","id":"blk_i",
            "src":{"kind":"external","url":"ftp://x.com/y.png"},
            "alt":"x","caption":null,"position":"inline"
        }]);
        assert!(HtmlValidator.validate(&ir).is_err());
    }

    #[test]
    fn image_data_url_svg_rejected_v1() {
        let mut ir = minimal_ir();
        ir["slides"][0]["blocks"] = json!([{
            "kind":"image","id":"blk_i",
            "src":{"kind":"data_url","data":"data:image/svg+xml;base64,PHN2Zz48L3N2Zz4="},
            "alt":"x","caption":null,"position":"inline"
        }]);
        assert!(HtmlValidator.validate(&ir).is_err());
    }

    #[test]
    fn chart_bar_mismatched_series_categories_rejected() {
        let mut ir = minimal_ir();
        ir["slides"][0]["blocks"] = json!([{
            "kind":"chart","id":"blk_c",
            "chart": {
                "chart_type": "bar",
                "series": [{"name":"A","data":[1,2,3]}],
                "x_axis": {"categories":["x","y"]},
                "legend": true
            },
            "title": null,
            "size": "medium"
        }]);
        let err = HtmlValidator.validate(&ir).unwrap_err();
        assert!(err.to_string().contains("categories"), "got: {err}");
    }

    #[test]
    fn chart_pie_two_series_rejected() {
        let mut ir = minimal_ir();
        ir["slides"][0]["blocks"] = json!([{
            "kind":"chart","id":"blk_c",
            "chart": {
                "chart_type": "pie",
                "series": [
                    {"name":"A","data":[1.0]},
                    {"name":"B","data":[2.0]}
                ],
                "legend": true
            },
            "title": null, "size":"small"
        }]);
        assert!(HtmlValidator.validate(&ir).is_err());
    }

    #[test]
    fn nested_grids_depth_4_rejected() {
        let mut ir = minimal_ir();
        fn two_col(id: &str, inner: serde_json::Value) -> serde_json::Value {
            json!({
                "kind": "two_columns", "id": id, "ratio": "50_50", "gap": "medium",
                "left": [inner],
                "right": []
            })
        }
        let leaf = json!({"kind":"divider","id":"blk_leaf"});
        let level3 = two_col("blk_3", leaf);
        let level2 = two_col("blk_2", level3);
        let level1 = two_col("blk_1", level2);
        let level0 = two_col("blk_0", level1);
        ir["slides"][0]["blocks"] = json!([level0]);
        assert!(HtmlValidator.validate(&ir).is_err());
    }
}
