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

// Placeholder — replaced in Task 2.2.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum Block {
    #[serde(rename = "_placeholder")]
    Placeholder { id: String },
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
}
