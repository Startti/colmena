//! HTML IR — schema for the `Html` artifact kind.
//!
//! Pure domain types. No imports from application/ or infrastructure/.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Executive,
    Minimal,
    Vibrant,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    En,
    Es,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    Report,
    Slides,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlideLayout {
    Title,
    Content,
    SectionDivider,
    Blank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ImagePosition {
    Inline,
    Full,
    Hero,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChartSize {
    Small,
    Medium,
    Large,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ColumnRatio {
    #[serde(rename = "50_50")]
    FiftyFifty,
    #[serde(rename = "40_60")]
    FortySixty,
    #[serde(rename = "60_40")]
    SixtyForty,
    #[serde(rename = "30_70")]
    ThirtySeventy,
    #[serde(rename = "70_30")]
    SeventyThirty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Gap {
    Small,
    Medium,
    Large,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CalloutVariant {
    Info,
    Warning,
    Success,
    Danger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DeltaDirection {
    Up,
    Down,
    Neutral,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HtmlIR {
    /// Discriminator — always the literal "html".
    pub kind: String,
    pub artifact_id: String,
    pub version_id: String,
    pub schema_version: String,

    pub doc_props: DocProps,
    pub theme: Theme,
    pub layout_mode: LayoutMode,
    pub footer: FooterConfig,
    pub slides: Vec<Slide>,

    /// IDs of assets reachable from any Image/Video block in the IR.
    /// Maintained by HtmlOpApplier; verified by HtmlValidator.
    #[serde(default)]
    pub assets_referenced: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DocProps {
    pub title: Option<String>,
    pub author: Option<String>,
    pub date: Option<String>,
    pub locale: Locale,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FooterConfig {
    pub enabled: bool,
    pub page_numbers: bool,
    pub custom_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Slide {
    pub id: String,
    pub layout: SlideLayout,
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub notes: Option<String>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChartSpec {
    pub chart_type: ChartType,
    pub series: Vec<ChartSeries>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_axis: Option<AxisSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_axis: Option<AxisSpec>,
    #[serde(default = "default_true")]
    pub legend: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChartType {
    Bar,
    Line,
    Pie,
    Doughnut,
    Area,
    Scatter,
    Radar,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChartSeries {
    pub name: String,
    pub data: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AxisSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum Block {
    #[serde(rename = "heading")]
    Heading {
        id: String,
        level: u8,
        runs: Vec<Run>,
    },
    #[serde(rename = "paragraph")]
    Paragraph { id: String, runs: Vec<Run> },
    #[serde(rename = "list")]
    List {
        id: String,
        ordered: bool,
        items: Vec<ListItem>,
    },
    #[serde(rename = "blockquote")]
    Blockquote {
        id: String,
        runs: Vec<Run>,
        cite: Option<String>,
    },
    #[serde(rename = "code")]
    Code {
        id: String,
        language: Option<String>,
        text: String,
    },
    #[serde(rename = "table")]
    Table {
        id: String,
        headers: Vec<String>,
        rows: Vec<TableRow>,
        caption: Option<String>,
    },
    #[serde(rename = "chart")]
    Chart {
        id: String,
        chart: ChartSpec,
        title: Option<String>,
        size: ChartSize,
    },
    #[serde(rename = "kpi_card")]
    KpiCard {
        id: String,
        label: String,
        value: String,
        delta: Option<KpiDelta>,
        icon: Option<String>,
    },
    #[serde(rename = "kpi_grid")]
    KpiGrid {
        id: String,
        columns: u8,
        cards: Vec<KpiCardInline>,
    },
    #[serde(rename = "image")]
    Image {
        id: String,
        src: ImageSrc,
        alt: String,
        caption: Option<String>,
        position: ImagePosition,
    },
    #[serde(rename = "two_columns")]
    TwoColumns {
        id: String,
        left: Vec<Block>,
        right: Vec<Block>,
        ratio: ColumnRatio,
        gap: Gap,
    },
    #[serde(rename = "three_columns")]
    ThreeColumns {
        id: String,
        left: Vec<Block>,
        middle: Vec<Block>,
        right: Vec<Block>,
        gap: Gap,
    },
    #[serde(rename = "comparison")]
    Comparison {
        id: String,
        left: ComparisonPanel,
        right: ComparisonPanel,
    },
    #[serde(rename = "callout")]
    Callout {
        id: String,
        variant: CalloutVariant,
        title: Option<String>,
        runs: Vec<Run>,
    },
    #[serde(rename = "divider")]
    Divider { id: String },
    #[serde(rename = "video")]
    Video {
        id: String,
        src: VideoSrc,
        caption: Option<String>,
    },
    #[serde(rename = "auto_toc")]
    AutoToc {
        id: String,
        title: Option<String>,
        depth: u8,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Run {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    #[serde(default)]
    pub code: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListItem {
    pub id: String,
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TableRow {
    pub id: String,
    pub cells: Vec<TableCell>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum TableCell {
    #[serde(rename = "text")]
    Text { value: String },
    #[serde(rename = "number")]
    Number {
        value: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    #[serde(rename = "runs")]
    Runs { runs: Vec<Run> },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KpiDelta {
    pub value: String,
    pub direction: DeltaDirection,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KpiCardInline {
    pub label: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<KpiDelta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum ImageSrc {
    #[serde(rename = "asset")]
    Asset { asset_id: String },
    #[serde(rename = "data_url")]
    DataUrl { data: String },
    #[serde(rename = "external")]
    External { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ComparisonPanel {
    pub header: String,
    pub runs: Vec<Run>,
    #[serde(default)]
    pub highlight: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "provider")]
pub enum VideoSrc {
    #[serde(rename = "youtube")]
    Youtube { video_id: String },
    #[serde(rename = "vimeo")]
    Vimeo { video_id: String },
    #[serde(rename = "asset")]
    Asset { asset_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_serializes_lowercase() {
        let s = serde_json::to_string(&Theme::Executive).unwrap();
        assert_eq!(s, "\"executive\"");
        let back: Theme = serde_json::from_str(&s).unwrap();
        assert_eq!(back, Theme::Executive);
    }

    #[test]
    fn theme_invalid_value_rejected_at_deserialize() {
        let r: Result<Theme, _> = serde_json::from_str("\"hot_pink\"");
        assert!(r.is_err(), "expected rejection of unknown theme");
    }

    #[test]
    fn locale_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Locale::Es).unwrap(), "\"es\"");
    }

    #[test]
    fn layout_mode_snake_case() {
        assert_eq!(
            serde_json::to_string(&LayoutMode::Slides).unwrap(),
            "\"slides\""
        );
    }

    #[test]
    fn slide_layout_section_divider_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&SlideLayout::SectionDivider).unwrap(),
            "\"section_divider\""
        );
    }

    #[test]
    fn column_ratio_uses_underscore() {
        assert_eq!(
            serde_json::to_string(&ColumnRatio::FortySixty).unwrap(),
            "\"40_60\""
        );
    }

    #[test]
    fn html_ir_minimal_roundtrip() {
        let ir = HtmlIR {
            kind: "html".to_string(),
            artifact_id: "art_x".to_string(),
            version_id: "v1".to_string(),
            schema_version: "1.0.0".to_string(),
            doc_props: DocProps {
                title: None,
                author: None,
                date: None,
                locale: Locale::En,
            },
            theme: Theme::Executive,
            layout_mode: LayoutMode::Report,
            footer: FooterConfig {
                enabled: false,
                page_numbers: false,
                custom_text: None,
            },
            slides: vec![Slide {
                id: "sl_1".to_string(),
                layout: SlideLayout::Blank,
                title: None,
                subtitle: None,
                notes: None,
                blocks: vec![],
            }],
            assets_referenced: vec![],
        };
        let s = serde_json::to_string(&ir).unwrap();
        let back: HtmlIR = serde_json::from_str(&s).unwrap();
        assert_eq!(back.theme, Theme::Executive);
        assert_eq!(back.slides.len(), 1);
    }

    #[test]
    fn block_heading_serializes_with_kind_tag() {
        let b = Block::Heading {
            id: "blk_1".into(),
            level: 2,
            runs: vec![Run {
                id: "run_1".into(),
                text: "Hello".into(),
                bold: true,
                italic: false,
                underline: false,
                code: false,
                link: None,
            }],
        };
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(v["kind"], "heading");
        assert_eq!(v["level"], 2);
        let back: Block = serde_json::from_value(v).unwrap();
        if let Block::Heading { level, .. } = back {
            assert_eq!(level, 2);
        } else {
            panic!("expected Heading variant");
        }
    }

    #[test]
    fn block_image_with_asset_src_roundtrips() {
        let b = Block::Image {
            id: "blk_2".into(),
            src: ImageSrc::Asset {
                asset_id: "asset_xyz".into(),
            },
            alt: "logo".into(),
            caption: None,
            position: ImagePosition::Hero,
        };
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(v["src"]["kind"], "asset");
        let back: Block = serde_json::from_value(v).unwrap();
        assert!(matches!(
            back,
            Block::Image {
                src: ImageSrc::Asset { .. },
                ..
            }
        ));
    }

    #[test]
    fn block_video_youtube_uses_provider_tag() {
        let b = Block::Video {
            id: "blk_3".into(),
            src: VideoSrc::Youtube {
                video_id: "dQw4w9WgXcQ".into(),
            },
            caption: None,
        };
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(v["src"]["provider"], "youtube");
    }

    #[test]
    fn block_callout_warning_roundtrips() {
        let b = Block::Callout {
            id: "blk_4".into(),
            variant: CalloutVariant::Warning,
            title: Some("Heads up".into()),
            runs: vec![],
        };
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(v["variant"], "warning");
        let _: Block = serde_json::from_value(v).unwrap();
    }

    #[test]
    fn chart_spec_bar_roundtrips() {
        let c = ChartSpec {
            chart_type: ChartType::Bar,
            series: vec![ChartSeries {
                name: "Sales".into(),
                data: vec![10.0, 20.0, 30.0],
            }],
            x_axis: Some(AxisSpec {
                title: Some("Quarter".into()),
                categories: Some(vec!["Q1".into(), "Q2".into(), "Q3".into()]),
                min: None,
                max: None,
            }),
            y_axis: None,
            legend: true,
            palette: None,
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["chart_type"], "bar");
        let back: ChartSpec = serde_json::from_value(v).unwrap();
        assert_eq!(back.series.len(), 1);
    }
}
