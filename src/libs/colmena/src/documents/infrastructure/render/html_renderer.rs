//! HtmlRenderer — implements IRRenderer for HtmlIR.
//!
//! Uses maud (type-safe HTML macros) + asset bundling via include_str!.
//! Depends on the AssetStore port to resolve `asset_id` to bytes at render
//! time. Output is a single self-contained .html file with CSS + Chart.js
//! (when needed) + slides_runtime.js (when needed) + images base64-inlined.

use crate::documents::domain::ids::AssetId;
use crate::documents::domain::ir::html::{
    Block, HtmlIR, ImageSrc, LayoutMode, Locale, Slide, Theme, VideoSrc,
};
use crate::documents::domain::ports::AssetStore;
use crate::documents::domain::{IRRenderer, RenderError};
use async_trait::async_trait;
use base64::Engine;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use std::collections::HashMap;
use std::sync::Arc;

const THEME_EXECUTIVE: &str = include_str!("html_assets/themes/executive.css");
const THEME_MINIMAL: &str = include_str!("html_assets/themes/minimal.css");
const THEME_VIBRANT: &str = include_str!("html_assets/themes/vibrant.css");
const THEME_DARK: &str = include_str!("html_assets/themes/dark.css");
const CHART_JS: &str = include_str!("html_assets/chart_js.min.js");
const SLIDES_RUNTIME: &str = include_str!("html_assets/slides_runtime.js");

pub struct HtmlRenderer {
    asset_store: Arc<dyn AssetStore>,
}

impl HtmlRenderer {
    pub fn new(asset_store: Arc<dyn AssetStore>) -> Self {
        Self { asset_store }
    }
}

fn load_theme(theme: Theme) -> &'static str {
    match theme {
        Theme::Executive => THEME_EXECUTIVE,
        Theme::Minimal => THEME_MINIMAL,
        Theme::Vibrant => THEME_VIBRANT,
        Theme::Dark => THEME_DARK,
    }
}

fn locale_lang(locale: Locale) -> &'static str {
    match locale {
        Locale::En => "en",
        Locale::Es => "es",
    }
}

fn doc_has_chart(ir: &HtmlIR) -> bool {
    ir.slides.iter().any(|s| slide_has_chart(s))
}

fn slide_has_chart(s: &Slide) -> bool {
    s.blocks.iter().any(block_is_or_contains_chart)
}

fn block_is_or_contains_chart(b: &Block) -> bool {
    match b {
        Block::Chart { .. } => true,
        Block::TwoColumns { left, right, .. } => {
            left.iter().any(block_is_or_contains_chart)
                || right.iter().any(block_is_or_contains_chart)
        }
        Block::ThreeColumns {
            left,
            middle,
            right,
            ..
        } => {
            left.iter().any(block_is_or_contains_chart)
                || middle.iter().any(block_is_or_contains_chart)
                || right.iter().any(block_is_or_contains_chart)
        }
        _ => false,
    }
}

fn collect_asset_ids(ir: &HtmlIR) -> Vec<AssetId> {
    fn walk(blocks: &[Block], out: &mut Vec<AssetId>) {
        for b in blocks {
            match b {
                Block::Image {
                    src: ImageSrc::Asset { asset_id },
                    ..
                }
                | Block::Video {
                    src: VideoSrc::Asset { asset_id },
                    ..
                } => out.push(AssetId::new(asset_id.clone())),
                Block::TwoColumns { left, right, .. } => {
                    walk(left, out);
                    walk(right, out);
                }
                Block::ThreeColumns {
                    left,
                    middle,
                    right,
                    ..
                } => {
                    walk(left, out);
                    walk(middle, out);
                    walk(right, out);
                }
                _ => {}
            }
        }
    }
    let mut out = vec![];
    for s in &ir.slides {
        walk(&s.blocks, &mut out);
    }
    out
}

#[async_trait]
impl IRRenderer for HtmlRenderer {
    async fn render(&self, ir: &serde_json::Value) -> Result<Vec<u8>, RenderError> {
        let ir: HtmlIR = serde_json::from_value(ir.clone())
            .map_err(|e| RenderError::Failed(format!("schema parse: {e}")))?;

        // Resolve assets to base64 data URLs
        let asset_ids = collect_asset_ids(&ir);
        let mut resolved: HashMap<String, String> = HashMap::new();
        for id in asset_ids {
            let (bytes, mime) = self
                .asset_store
                .read(&id)
                .await
                .map_err(|e| RenderError::Failed(format!("asset {}: {e}", id.as_str())))?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            resolved.insert(id.as_str().to_string(), format!("data:{mime};base64,{b64}"));
        }

        let needs_chart = doc_has_chart(&ir);
        let needs_slides_runtime = matches!(ir.layout_mode, LayoutMode::Slides);
        let theme_css = load_theme(ir.theme);

        let chart_init = render_chart_init_scripts(&ir);

        let doc = html! {
            (DOCTYPE)
            html lang=(locale_lang(ir.doc_props.locale)) {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width, initial-scale=1";
                    @if let Some(title) = &ir.doc_props.title {
                        title { (title) }
                    }
                    style { (PreEscaped(theme_css)) }
                    @if needs_chart {
                        script { (PreEscaped(CHART_JS)) }
                    }
                }
                body {
                    (render_body(&ir, &resolved))
                    @if needs_slides_runtime {
                        div id="slide-counter" { "1 / 1" }
                        script { (PreEscaped(SLIDES_RUNTIME)) }
                    }
                    @if !chart_init.is_empty() {
                        script { (PreEscaped(chart_init)) }
                    }
                }
            }
        };

        Ok(doc.into_string().into_bytes())
    }

    fn target_extension(&self) -> &'static str {
        "html"
    }
    fn target_mime(&self) -> &'static str {
        "text/html; charset=utf-8"
    }
}

// Placeholders — filled in Task 9.2 (render_body) and Task 9.3 (render_chart_init_scripts).
fn render_body(_ir: &HtmlIR, _assets: &HashMap<String, String>) -> Markup {
    html! { div.deck { p { "(empty)" } } }
}
fn render_chart_init_scripts(_ir: &HtmlIR) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::infrastructure::storage::LocalFsAssetStore;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn render_blocking(ir: serde_json::Value) -> String {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let tmp = tempdir().unwrap();
            let store: Arc<dyn AssetStore> =
                Arc::new(LocalFsAssetStore::new(tmp.path()));
            let renderer = HtmlRenderer::new(store);
            let bytes = renderer.render(&ir).await.unwrap();
            String::from_utf8(bytes).unwrap()
        })
    }

    fn minimal_ir() -> serde_json::Value {
        json!({
            "kind": "html",
            "artifact_id": "art_x",
            "version_id": "v1",
            "schema_version": "1.0.0",
            "doc_props": { "title": "Sample", "author": null, "date": null, "locale": "en" },
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
    fn render_starts_with_doctype() {
        let html = render_blocking(minimal_ir());
        assert!(html.starts_with("<!DOCTYPE html>"), "got: {}", &html[..60]);
    }

    #[test]
    fn render_includes_theme_css() {
        let html = render_blocking(minimal_ir());
        assert!(html.contains("--font-heading"), "theme CSS missing");
    }

    #[test]
    fn report_mode_excludes_slides_runtime() {
        let html = render_blocking(minimal_ir());
        // Check for the slide-counter *div element* — the CSS selector #slide-counter
        // is always present in the embedded theme CSS, so we test the HTML tag.
        assert!(!html.contains("<div id=\"slide-counter\""), "should not include slide counter div");
    }

    #[test]
    fn slides_mode_includes_runtime() {
        let mut ir = minimal_ir();
        ir["layout_mode"] = json!("slides");
        let html = render_blocking(ir);
        assert!(html.contains("<div id=\"slide-counter\""), "should include slide counter div");
    }

    #[test]
    fn no_chart_no_chartjs() {
        let html = render_blocking(minimal_ir());
        assert!(!html.contains("Chart.register"), "Chart.js leaked when no chart");
    }
}
