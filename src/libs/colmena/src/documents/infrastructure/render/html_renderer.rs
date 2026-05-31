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

fn render_body(ir: &HtmlIR, assets: &HashMap<String, String>) -> Markup {
    let layout_class = match ir.layout_mode {
        LayoutMode::Slides => "deck deck--slides",
        LayoutMode::Report => "deck deck--report",
    };
    html! {
        div class=(layout_class) {
            @for slide in &ir.slides {
                (render_slide(slide, assets, ir))
            }
            @if ir.footer.enabled {
                (render_footer(ir))
            }
        }
    }
}

fn render_slide(slide: &Slide, assets: &HashMap<String, String>, ir: &HtmlIR) -> Markup {
    use crate::documents::domain::ir::html::SlideLayout;
    let class = match slide.layout {
        SlideLayout::Title => "slide slide--title",
        SlideLayout::Content => "slide slide--content",
        SlideLayout::SectionDivider => "slide slide--section-divider",
        SlideLayout::Blank => "slide slide--blank",
    };
    html! {
        section class=(class) id=(slide.id) {
            @if matches!(slide.layout, SlideLayout::Title | SlideLayout::SectionDivider | SlideLayout::Content) {
                @if let Some(t) = &slide.title { h1 { (t) } }
                @if let Some(sub) = &slide.subtitle { p.subtitle { (sub) } }
            }
            @for block in &slide.blocks {
                (render_block(block, assets, ir))
            }
        }
    }
}

fn render_footer(ir: &HtmlIR) -> Markup {
    let custom = ir.footer.custom_text.clone().unwrap_or_default();
    html! {
        div.footer {
            @if !custom.is_empty() { span { (custom) " " } }
            @if ir.footer.page_numbers {
                span { (page_numbers_label(ir.doc_props.locale, ir.slides.len())) }
            }
        }
    }
}

fn page_numbers_label(locale: Locale, total: usize) -> String {
    match locale {
        Locale::Es => format!("Página 1 de {total}"),
        Locale::En => format!("Page 1 of {total}"),
    }
}

fn render_runs(runs: &[crate::documents::domain::ir::html::Run]) -> Markup {
    html! {
        @for r in runs {
            (render_run(r))
        }
    }
}

fn render_run(r: &crate::documents::domain::ir::html::Run) -> Markup {
    let body = html! {
        @if r.bold { strong { (r.text.clone()) } }
        @else if r.italic { em { (r.text.clone()) } }
        @else if r.code { code { (r.text.clone()) } }
        @else { (r.text.clone()) }
    };
    html! {
        @if let Some(href) = &r.link {
            a href=(href) target="_blank" rel="noopener" { (body) }
        } @else {
            (body)
        }
    }
}

fn render_block(b: &Block, assets: &HashMap<String, String>, ir: &HtmlIR) -> Markup {
    use crate::documents::domain::ir::html::{
        CalloutVariant, ChartSize, DeltaDirection, Gap, ImagePosition,
    };
    match b {
        Block::Heading { id, level, runs } => match level {
            1 => html! { h1.block-heading id=(id) { (render_runs(runs)) } },
            2 => html! { h2.block-heading id=(id) { (render_runs(runs)) } },
            3 => html! { h3.block-heading id=(id) { (render_runs(runs)) } },
            4 => html! { h4.block-heading id=(id) { (render_runs(runs)) } },
            5 => html! { h5.block-heading id=(id) { (render_runs(runs)) } },
            _ => html! { h6.block-heading id=(id) { (render_runs(runs)) } },
        },
        Block::Paragraph { id, runs } => html! {
            p.block-paragraph id=(id) { (render_runs(runs)) }
        },
        Block::List { id, ordered, items } => {
            if *ordered {
                html! { ol.block-list id=(id) { @for it in items { li id=(it.id) { (render_runs(&it.runs)) } } } }
            } else {
                html! { ul.block-list id=(id) { @for it in items { li id=(it.id) { (render_runs(&it.runs)) } } } }
            }
        }
        Block::Blockquote { id, runs, cite } => html! {
            blockquote.block-blockquote id=(id) cite=[cite.as_deref()] { (render_runs(runs)) }
        },
        Block::Code { id, language, text } => html! {
            pre.block-code id=(id) data-lang=[language.as_deref()] { code { (text) } }
        },
        Block::Table { id, headers, rows, caption } => html! {
            table.block-table id=(id) {
                @if let Some(c) = caption { caption { (c) } }
                thead { tr { @for h in headers { th { (h) } } } }
                tbody {
                    @for row in rows {
                        tr id=(row.id) {
                            @for cell in &row.cells {
                                td { (render_table_cell(cell)) }
                            }
                        }
                    }
                }
            }
        },
        Block::Chart { id, title, size, .. } => {
            let size_class = match size {
                ChartSize::Small => "chart-container chart-container--small",
                ChartSize::Medium => "chart-container chart-container--medium",
                ChartSize::Large => "chart-container chart-container--large",
            };
            html! {
                div class=(size_class) {
                    @if let Some(t) = title { h3 { (t) } }
                    canvas id=(format!("chart_{}", id)) {}
                }
            }
        }
        Block::KpiCard { id, label, value, delta, .. } => html! {
            div.block-kpi-card id=(id) {
                div.block-kpi-card__label { (label) }
                div.block-kpi-card__value { (value) }
                @if let Some(d) = delta {
                    div class=(format!("block-kpi-card__delta block-kpi-card__delta--{}",
                        match d.direction {
                            DeltaDirection::Up => "up",
                            DeltaDirection::Down => "down",
                            DeltaDirection::Neutral => "neutral",
                        })) { (d.value.clone()) }
                }
            }
        },
        Block::KpiGrid { id, columns, cards } => html! {
            div.block-kpi-grid id=(id) data-cols=(columns.to_string()) {
                @for c in cards {
                    div.block-kpi-card {
                        div.block-kpi-card__label { (c.label.clone()) }
                        div.block-kpi-card__value { (c.value.clone()) }
                        @if let Some(d) = &c.delta {
                            div class=(format!("block-kpi-card__delta block-kpi-card__delta--{}",
                                match d.direction {
                                    DeltaDirection::Up => "up",
                                    DeltaDirection::Down => "down",
                                    DeltaDirection::Neutral => "neutral",
                                })) { (d.value.clone()) }
                        }
                    }
                }
            }
        },
        Block::Image { id, src, alt, caption, position } => {
            let pos_class = match position {
                ImagePosition::Inline => "block-image block-image--inline",
                ImagePosition::Full => "block-image block-image--full",
                ImagePosition::Hero => "block-image block-image--hero",
            };
            let resolved_src = match src {
                ImageSrc::Asset { asset_id } => assets.get(asset_id).cloned().unwrap_or_default(),
                ImageSrc::DataUrl { data } => data.clone(),
                ImageSrc::External { url } => url.clone(),
            };
            html! {
                figure class=(pos_class) id=(id) {
                    img src=(resolved_src) alt=(alt);
                    @if let Some(c) = caption { figcaption.block-image__caption { (c) } }
                }
            }
        }
        Block::TwoColumns { id, left, right, ratio, gap } => {
            let ratio_str = serde_json::to_value(ratio).ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| "50_50".to_string());
            let gap_class = format!("gap-{}", match gap {
                Gap::Small => "small", Gap::Medium => "medium", Gap::Large => "large",
            });
            html! {
                div class=(format!("block-two-columns {gap_class}")) id=(id) data-ratio=(ratio_str) {
                    div.col { @for b in left { (render_block(b, assets, ir)) } }
                    div.col { @for b in right { (render_block(b, assets, ir)) } }
                }
            }
        }
        Block::ThreeColumns { id, left, middle, right, gap } => {
            let gap_class = format!("gap-{}", match gap {
                Gap::Small => "small", Gap::Medium => "medium", Gap::Large => "large",
            });
            html! {
                div class=(format!("block-three-columns {gap_class}")) id=(id) {
                    div.col { @for b in left { (render_block(b, assets, ir)) } }
                    div.col { @for b in middle { (render_block(b, assets, ir)) } }
                    div.col { @for b in right { (render_block(b, assets, ir)) } }
                }
            }
        }
        Block::Comparison { id, left, right } => html! {
            div.block-comparison id=(id) {
                div class=(if left.highlight { "panel highlight" } else { "panel" }) {
                    h3 { (left.header.clone()) } div { (render_runs(&left.runs)) }
                }
                div class=(if right.highlight { "panel highlight" } else { "panel" }) {
                    h3 { (right.header.clone()) } div { (render_runs(&right.runs)) }
                }
            }
        },
        Block::Callout { id, variant, title, runs } => {
            let var_class = format!("callout callout--{}", match variant {
                CalloutVariant::Info => "info",
                CalloutVariant::Warning => "warning",
                CalloutVariant::Success => "success",
                CalloutVariant::Danger => "danger",
            });
            html! {
                aside class=(var_class) id=(id) {
                    @if let Some(t) = title { strong.callout__title { (t) } }
                    div { (render_runs(runs)) }
                }
            }
        }
        Block::Divider { id } => html! { hr.divider id=(id); },
        Block::Video { id, src, caption } => {
            let embed_url = match src {
                VideoSrc::Youtube { video_id } => format!("https://www.youtube.com/embed/{video_id}"),
                VideoSrc::Vimeo { video_id } => format!("https://player.vimeo.com/video/{video_id}"),
                VideoSrc::Asset { asset_id } => assets.get(asset_id).cloned().unwrap_or_default(),
            };
            html! {
                figure.block-video id=(id) {
                    iframe src=(embed_url) allowfullscreen {}
                    @if let Some(c) = caption { figcaption { (c) } }
                }
            }
        }
        Block::AutoToc { id, title, depth } => {
            use crate::documents::domain::ir::html::SlideLayout;
            html! {
                nav.block-auto-toc id=(id) {
                    @if let Some(t) = title { strong { (t) } }
                    ol {
                        @for s in &ir.slides {
                            @if (*depth == 1 && matches!(s.layout, SlideLayout::SectionDivider))
                                || (*depth >= 2 && s.title.is_some()) {
                                @if let Some(t) = &s.title {
                                    li { a href=(format!("#{}", s.id)) { (t) } }
                                }
                            }
                        }
                    }
                }
            }
        },
    }
}

fn render_table_cell(c: &crate::documents::domain::ir::html::TableCell) -> Markup {
    use crate::documents::domain::ir::html::TableCell;
    match c {
        TableCell::Text { value } => html! { (value.clone()) },
        TableCell::Number { value, .. } => html! { (value.to_string()) },
        TableCell::Runs { runs } => html! { (render_runs(runs)) },
    }
}

fn render_chart_init_scripts(ir: &HtmlIR) -> String {
    let mut chunks = vec![];
    for slide in &ir.slides {
        collect_chart_inits(&slide.blocks, &mut chunks);
    }
    if chunks.is_empty() {
        return String::new();
    }
    let body = chunks.join("\n");
    format!("(function(){{\n{}\n}})();", body)
}

fn collect_chart_inits(blocks: &[Block], out: &mut Vec<String>) {
    for b in blocks {
        match b {
            Block::Chart { id, chart, .. } => {
                out.push(chart_init_for(id, chart));
            }
            Block::TwoColumns { left, right, .. } => {
                collect_chart_inits(left, out);
                collect_chart_inits(right, out);
            }
            Block::ThreeColumns {
                left,
                middle,
                right,
                ..
            } => {
                collect_chart_inits(left, out);
                collect_chart_inits(middle, out);
                collect_chart_inits(right, out);
            }
            _ => {}
        }
    }
}

fn chart_init_for(block_id: &str, c: &crate::documents::domain::ir::html::ChartSpec) -> String {
    use crate::documents::domain::ir::html::ChartType;
    let type_str = match c.chart_type {
        ChartType::Bar => "bar",
        ChartType::Line => "line",
        ChartType::Pie => "pie",
        ChartType::Doughnut => "doughnut",
        ChartType::Area => "line", // Chart.js: area = line with fill
        ChartType::Scatter => "scatter",
        ChartType::Radar => "radar",
    };
    let labels = c
        .x_axis
        .as_ref()
        .and_then(|a| a.categories.as_ref())
        .map(|cats| {
            format!(
                "[{}]",
                cats.iter()
                    .map(|s| format!("{:?}", s))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .unwrap_or_else(|| "[]".to_string());

    let fill = matches!(c.chart_type, ChartType::Area);
    let datasets = c
        .series
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                r#"{{label:{name},data:[{data}],backgroundColor:getCss("--chart-palette-{i}"),borderColor:getCss("--chart-palette-{i}"),fill:{fill}}}"#,
                name = format!("{:?}", s.name),
                data = s.data
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    format!(
        r#"
function getCss(name){{return getComputedStyle(document.documentElement).getPropertyValue(name).trim();}}
new Chart(document.getElementById("chart_{id}"),{{
  type: "{ty}",
  data: {{ labels: {labels}, datasets: [{ds}] }},
  options: {{ responsive: true, maintainAspectRatio: false, plugins: {{ legend: {{ display: {legend} }} }} }}
}});"#,
        id = block_id,
        ty = type_str,
        labels = labels,
        ds = datasets,
        legend = c.legend,
    )
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

    #[test]
    fn heading_renders_with_correct_level() {
        let mut ir = minimal_ir();
        ir["slides"][0]["blocks"] = json!([
            {"kind":"heading","id":"blk_h","level":2,
             "runs":[{"id":"run_1","text":"Hello","bold":false,"italic":false,
                      "underline":false,"code":false}]}
        ]);
        let html = render_blocking(ir);
        assert!(html.contains("<h2"), "expected h2: {html}");
        assert!(html.contains("Hello"));
    }

    #[test]
    fn paragraph_with_link_renders_anchor() {
        let mut ir = minimal_ir();
        ir["slides"][0]["blocks"] = json!([{
            "kind":"paragraph","id":"blk_p",
            "runs":[{"id":"run_1","text":"site","bold":false,"italic":false,
                     "underline":false,"code":false,"link":"https://example.com"}]
        }]);
        let html = render_blocking(ir);
        assert!(html.contains(r#"href="https://example.com""#), "missing link: {html}");
    }

    #[test]
    fn script_text_is_escaped() {
        let mut ir = minimal_ir();
        ir["slides"][0]["blocks"] = json!([{
            "kind":"paragraph","id":"blk_p",
            "runs":[{"id":"run_1","text":"<script>alert(1)</script>","bold":false,
                     "italic":false,"underline":false,"code":false}]
        }]);
        let html = render_blocking(ir);
        assert!(html.contains("&lt;script&gt;"), "script tag not escaped");
        assert!(!html.contains("<script>alert(1)"), "raw script leaked");
    }

    #[test]
    fn table_renders_thead_and_tbody() {
        let mut ir = minimal_ir();
        ir["slides"][0]["blocks"] = json!([{
            "kind":"table","id":"blk_t",
            "headers":["A","B"],
            "rows":[
                {"id":"row_1","cells":[{"type":"text","value":"x"},{"type":"number","value":1.5}]}
            ],
            "caption":null
        }]);
        let html = render_blocking(ir);
        assert!(html.contains("<thead>") && html.contains("<tbody>"));
        assert!(html.contains("1.5"));
    }

    #[test]
    fn kpi_grid_uses_data_cols() {
        let mut ir = minimal_ir();
        ir["slides"][0]["blocks"] = json!([{
            "kind":"kpi_grid","id":"blk_g","columns":3,
            "cards":[
                {"label":"A","value":"100"},{"label":"B","value":"200"},{"label":"C","value":"300"}
            ]
        }]);
        let html = render_blocking(ir);
        assert!(html.contains(r#"data-cols="3""#));
    }

    #[test]
    fn video_youtube_iframe() {
        let mut ir = minimal_ir();
        ir["slides"][0]["blocks"] = json!([{
            "kind":"video","id":"blk_v",
            "src":{"provider":"youtube","video_id":"dQw4w9WgXcQ"},
            "caption":null
        }]);
        let html = render_blocking(ir);
        assert!(html.contains("youtube.com/embed/dQw4w9WgXcQ"));
    }

    #[test]
    fn chart_block_generates_init_script_with_correct_type() {
        let mut ir = minimal_ir();
        ir["slides"][0]["blocks"] = json!([{
            "kind":"chart","id":"blk_c",
            "chart": {
                "chart_type":"bar",
                "series":[{"name":"A","data":[1,2,3]}],
                "x_axis":{"categories":["x","y","z"]},
                "legend":true
            },
            "title":"Sales", "size":"medium"
        }]);
        let html = render_blocking(ir);
        assert!(html.contains("new Chart"), "no Chart init: {html}");
        assert!(html.contains(r#"type: "bar""#), "wrong chart type");
        assert!(html.contains("chart_blk_c"), "wrong canvas id reference");
    }

    #[test]
    fn auto_toc_depth_1_lists_section_dividers_only() {
        let mut ir = minimal_ir();
        ir["layout_mode"] = json!("slides");
        ir["slides"] = json!([
            {"id":"sl_a","layout":"section_divider","title":"Part I","subtitle":null,"notes":null,"blocks":[]},
            {"id":"sl_b","layout":"content","title":"Detail","subtitle":null,"notes":null,
             "blocks":[{"kind":"auto_toc","id":"blk_toc","title":"Agenda","depth":1}]},
            {"id":"sl_c","layout":"section_divider","title":"Part II","subtitle":null,"notes":null,"blocks":[]}
        ]);
        let html = render_blocking(ir);
        assert!(html.contains("Part I"));
        assert!(html.contains("Part II"));
        assert!(html.contains(r##"href="#sl_a""##));
    }
}
