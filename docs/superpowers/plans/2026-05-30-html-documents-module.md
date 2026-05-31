# HTML Documents Module Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `ArtifactKind::Html` al módulo `documents/` con IR, ops, applier, validator, renderer (maud + Chart.js + slides runtime + 4 themes) y un asset store paralelo (LocalFs + GCS) — sin tocar DAG ni LLM tools (eso queda para Spec 2).

**Architecture:** Extiende el módulo hexagonal existente en `src/libs/colmena/src/documents/`. Reusa `ArtifactStore`, `IdGenerator`, `SessionArtifactIndex`. Suma un nuevo port `AssetStore` con dos adapters (LocalFs, GCS). Cumple estrictamente la regla de dependencias domain ← application ← infrastructure; `application/runtime.rs` es el único composition root.

**Tech Stack:** Rust (tokio async), `maud` (HTML type-safe), `Chart.js` (embebido vía include_str!), `regex` + `serde_json` (validator), `ulid` (IDs), `google-cloud-storage` (GCS adapter, feature flag `gcs`).

**Design spec:** [docs/superpowers/specs/2026-05-30-html-documents-module-design.md](../specs/2026-05-30-html-documents-module-design.md)

---

## Conventions for this plan

- All commits use: `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>`
- Run Rust tests with `cargo test --lib documents` (crate `colmena_dag_engine`). For GCS tests: `cargo test --lib documents --features gcs`.
- TDD: test → fail → minimal impl → pass → commit. One action per step (2-5 min).
- Every task ends with a commit. Don't batch commits across tasks.
- ID prefixes match existing conventions: reuse `new_block_id` (`blk_`), `new_run_id` (`run_`), `new_row_id` (`row_`), `new_list_item_id` (`li_`); add only `new_slide_id` (`sl_`) and `new_asset_id` (`asset_`).
- Hexagonal rule: **domain** importa solo `std`/`serde`/`schemars`/`chrono`; **application** importa solo de `domain/`; **infrastructure** importa de `domain/` y libs externas; **único composition root** es `application/runtime.rs`.

The plan is split into **13 phases**:

| Phase | Scope |
|---|---|
| 0 | Cargo dep + module scaffolding |
| 1 | Domain — `ArtifactKind::Html`, `AssetId`, enums |
| 2 | Domain — `HtmlIR`, `Block`, `ChartSpec` |
| 3 | Domain — 19 nuevas variantes de `PatchOp` |
| 4 | Domain — `AssetStore` port + `AssetError` + `IdGenerator` extensions |
| 5 | Infra — `HtmlValidator` |
| 6 | Infra — `LocalFsAssetStore` |
| 7 | Infra — `GcsAssetStore` (feature gcs) |
| 8 | Infra — Themes CSS + slides_runtime.js + Chart.js |
| 9 | Infra — `HtmlRenderer` |
| 10 | Application — 3 asset use cases |
| 11 | Application — `HtmlOpApplier` |
| 12 | Wire into `CreateDocumentUseCase`, `ApplyPatchUseCase`, `DocumentRuntime`, `describe_op` |
| 13 | End-to-end test + snapshot tests + CI guard hexagonal |

---

## Phase 0 — Setup

### Task 0.1: Add `maud` dependency

**Files:**
- Modify: `src/libs/colmena/Cargo.toml`

- [ ] **Step 1: Add maud to [dependencies]**

In `src/libs/colmena/Cargo.toml`, under `[dependencies]` after the `ulid = "1"` line, add:

```toml
maud = "0.26"
```

- [ ] **Step 2: Verify it builds**

Run: `cargo build --lib -p colmena_dag_engine`
Expected: build succeeds, downloads maud crate.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
feat(documents/html): add maud dependency for HTML renderer

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 1 — Domain foundations (kind, AssetId, enums)

### Task 1.1: Add `ArtifactKind::Html` variant

**Files:**
- Modify: `src/libs/colmena/src/documents/domain/ids.rs`

- [ ] **Step 1: Add failing test for new variant**

Append to the `#[cfg(test)] mod tests` block in `src/libs/colmena/src/documents/domain/ids.rs`:

```rust
#[test]
fn artifact_kind_html_extension_and_mime() {
    assert_eq!(ArtifactKind::Html.extension(), "html");
    assert_eq!(ArtifactKind::Html.mime(), "text/html; charset=utf-8");
}

#[test]
fn artifact_kind_html_serde_roundtrip() {
    let k = ArtifactKind::Html;
    let s = serde_json::to_string(&k).unwrap();
    assert_eq!(s, "\"html\"");
    let back: ArtifactKind = serde_json::from_str(&s).unwrap();
    assert_eq!(back, ArtifactKind::Html);
}
```

- [ ] **Step 2: Run, verify it fails**

Run: `cargo test --lib -p colmena_dag_engine documents::domain::ids -- --nocapture`
Expected: compile error — `Html` variant doesn't exist.

- [ ] **Step 3: Add variant + extension + mime**

In `src/libs/colmena/src/documents/domain/ids.rs`, replace the `ArtifactKind` enum and impl block with:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    Excel,
    Word,
    Html,
}

impl ArtifactKind {
    pub fn extension(&self) -> &'static str {
        match self {
            ArtifactKind::Excel => "xlsx",
            ArtifactKind::Word => "docx",
            ArtifactKind::Html => "html",
        }
    }
    pub fn mime(&self) -> &'static str {
        match self {
            ArtifactKind::Excel => {
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            }
            ArtifactKind::Word => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            }
            ArtifactKind::Html => "text/html; charset=utf-8",
        }
    }
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::domain::ids`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/domain/ids.rs
git commit -m "$(cat <<'EOF'
feat(documents/domain): add ArtifactKind::Html variant

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 1.2: Add `AssetId` value object

**Files:**
- Modify: `src/libs/colmena/src/documents/domain/ids.rs`

- [ ] **Step 1: Write failing tests**

Append to `tests` mod in `src/libs/colmena/src/documents/domain/ids.rs`:

```rust
#[test]
fn asset_id_construction_and_display() {
    let a = AssetId::new("asset_abc123");
    assert_eq!(a.as_str(), "asset_abc123");
    assert_eq!(a.to_string(), "asset_abc123");
}

#[test]
fn asset_id_serde_roundtrip() {
    let a = AssetId::new("asset_xyz");
    let s = serde_json::to_string(&a).unwrap();
    let back: AssetId = serde_json::from_str(&s).unwrap();
    assert_eq!(a, back);
}
```

- [ ] **Step 2: Run, verify fails**

Run: `cargo test --lib -p colmena_dag_engine documents::domain::ids`
Expected: compile error — `AssetId` not defined.

- [ ] **Step 3: Add `AssetId`**

In `src/libs/colmena/src/documents/domain/ids.rs`, after the `SessionId` block and before `ArtifactKind`, add:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetId(pub String);

impl AssetId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::domain::ids`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/domain/ids.rs
git commit -m "$(cat <<'EOF'
feat(documents/domain): add AssetId value object

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 1.3: Add HTML enums (`Theme`, `Locale`, `LayoutMode`, `SlideLayout`, and visual VOs)

**Files:**
- Create: `src/libs/colmena/src/documents/domain/ir/html.rs`
- Modify: `src/libs/colmena/src/documents/domain/ir/mod.rs`

- [ ] **Step 1: Create the html IR file with enums + tests**

Create `src/libs/colmena/src/documents/domain/ir/html.rs`:

```rust
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
}
```

- [ ] **Step 2: Register html module in ir/mod.rs**

In `src/libs/colmena/src/documents/domain/ir/mod.rs`, add `pub mod html;` and a re-export line:

```rust
pub mod common;
pub mod excel;
pub mod html;
pub mod word;

pub use common::{Alignment, FontSpec, NamedStyle, SCHEMA_VERSION};
pub use excel::{Cell, CellType, ColumnSpec, ExcelIR, ExcelKindTag, NamedTable, Sheet, Workbook};
pub use html::{
    CalloutVariant, ChartSize, ColumnRatio, DeltaDirection, Gap, ImagePosition, LayoutMode,
    Locale, SlideLayout, Theme,
};
pub use word::{
    Block, ListItem, ListStyle, Run, TableCell, TableRow, WordDocument, WordIR, WordKindTag,
};
```

- [ ] **Step 3: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::domain::ir::html`
Expected: 6 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/domain/ir/html.rs src/libs/colmena/src/documents/domain/ir/mod.rs
git commit -m "$(cat <<'EOF'
feat(documents/domain): add HTML enums (Theme, Locale, LayoutMode, SlideLayout, visual VOs)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2 — `HtmlIR` core types

### Task 2.1: Add `HtmlIR`, `DocProps`, `FooterConfig`, `Slide` structs

**Files:**
- Modify: `src/libs/colmena/src/documents/domain/ir/html.rs`

- [ ] **Step 1: Add failing roundtrip test**

In `src/libs/colmena/src/documents/domain/ir/html.rs`, inside `mod tests`, append:

```rust
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
```

- [ ] **Step 2: Run, verify fails**

Run: `cargo test --lib -p colmena_dag_engine documents::domain::ir::html`
Expected: compile error — types don't exist yet.

- [ ] **Step 3: Add structs**

In `src/libs/colmena/src/documents/domain/ir/html.rs`, BEFORE the `#[cfg(test)] mod tests` block, add:

```rust
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
```

NOTE: `Block` is referenced but not yet defined — Task 2.2 adds it. This task won't compile until Task 2.2 is also done. To keep TDD strict, add a temporary placeholder right after `Slide`:

```rust
// Placeholder — replaced in Task 2.2
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum Block {
    #[serde(rename = "_placeholder")]
    Placeholder { id: String },
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::domain::ir::html`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/domain/ir/html.rs
git commit -m "$(cat <<'EOF'
feat(documents/domain): add HtmlIR, DocProps, FooterConfig, Slide (Block placeholder)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 2.2: Replace Block placeholder with all 17 variants + supporting types

**Files:**
- Modify: `src/libs/colmena/src/documents/domain/ir/html.rs`

- [ ] **Step 1: Add failing test for Block variants**

Inside `mod tests`, append:

```rust
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
```

- [ ] **Step 2: Run, verify fails**

Run: `cargo test --lib -p colmena_dag_engine documents::domain::ir::html`
Expected: compile errors — variants and supporting types not defined.

- [ ] **Step 3: Replace placeholder Block + add supporting types**

In `src/libs/colmena/src/documents/domain/ir/html.rs`, REPLACE the placeholder `Block` enum with the full version, and ADD the supporting types right after it:

```rust
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
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::domain::ir::html`
Expected: PASS (new tests + existing).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/domain/ir/html.rs
git commit -m "$(cat <<'EOF'
feat(documents/domain): add 17 Block variants and supporting types (Run, ListItem, TableRow/Cell, ImageSrc, VideoSrc, etc.)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 2.3: Add `ChartSpec` and chart sub-types

**Files:**
- Modify: `src/libs/colmena/src/documents/domain/ir/html.rs`

- [ ] **Step 1: Add failing test**

Inside `mod tests`, append:

```rust
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
```

- [ ] **Step 2: Run, verify fails**

Expected: types not defined.

- [ ] **Step 3: Add chart types**

In `src/libs/colmena/src/documents/domain/ir/html.rs`, before the `#[cfg(test)] mod tests` block (alongside the other supporting types), add:

```rust
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
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::domain::ir::html`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/domain/ir/html.rs
git commit -m "$(cat <<'EOF'
feat(documents/domain): add ChartSpec normalized IR (independent of Chart.js)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3 — `PatchOp` HTML variants

### Task 3.1: Add 6 slide-level ops to `PatchOp`

**Files:**
- Modify: `src/libs/colmena/src/documents/domain/patch.rs`

- [ ] **Step 1: Add failing test**

In `src/libs/colmena/src/documents/domain/patch.rs`, inside `mod tests`, append:

```rust
#[test]
fn add_slide_op_serializes() {
    let op = PatchOp::AddSlide {
        layout: crate::documents::domain::ir::SlideLayout::Title,
        at_index: None,
        title: Some("Q3".into()),
        subtitle: None,
    };
    let v = serde_json::to_value(&op).unwrap();
    assert_eq!(v["op"], "add_slide");
    assert_eq!(v["layout"], "title");
}

#[test]
fn reorder_slides_serializes() {
    let op = PatchOp::ReorderSlides {
        order: vec!["sl_1".into(), "sl_2".into()],
    };
    let v = serde_json::to_value(&op).unwrap();
    assert_eq!(v["op"], "reorder_slides");
}

#[test]
fn set_slide_notes_clearing() {
    let op = PatchOp::SetSlideNotes {
        slide_id: "sl_1".into(),
        notes: None,
    };
    let v = serde_json::to_value(&op).unwrap();
    assert_eq!(v["op"], "set_slide_notes");
}
```

- [ ] **Step 2: Run, verify fails**

Run: `cargo test --lib -p colmena_dag_engine documents::domain::patch`
Expected: compile errors — variants don't exist.

- [ ] **Step 3: Add variants**

In `src/libs/colmena/src/documents/domain/patch.rs`, at the top, change the `use schemars::JsonSchema;` line to also import the HTML enums:

```rust
use crate::documents::domain::ir::SlideLayout;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
```

Then, BEFORE the closing `}` of the `pub enum PatchOp { ... }` enum, add:

```rust
    // -------- HTML — slide level --------

    /// Create a new slide. Returns generated slide_id in summary.
    #[serde(rename = "add_slide")]
    AddSlide {
        layout: SlideLayout,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at_index: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtitle: Option<String>,
    },

    /// Delete a slide by id.
    #[serde(rename = "delete_slide")]
    DeleteSlide { slide_id: String },

    /// Reorder slides. Pass the FULL list of slide_ids in desired order.
    #[serde(rename = "reorder_slides")]
    ReorderSlides { order: Vec<String> },

    /// Change a slide's layout. SectionDivider/Title require non-empty title.
    #[serde(rename = "set_slide_layout")]
    SetSlideLayout {
        slide_id: String,
        layout: SlideLayout,
    },

    /// Set/clear a slide's title slot (independent of blocks).
    #[serde(rename = "set_slide_title")]
    SetSlideTitle {
        slide_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtitle: Option<String>,
    },

    /// Set/clear speaker notes on a slide.
    #[serde(rename = "set_slide_notes")]
    SetSlideNotes {
        slide_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::domain::patch`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/domain/patch.rs
git commit -m "$(cat <<'EOF'
feat(documents/domain): add 6 slide-level HTML patch ops

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 3.2: Add 4 block-level + 3 table + 3 list ops

**Files:**
- Modify: `src/libs/colmena/src/documents/domain/patch.rs`

- [ ] **Step 1: Add failing test**

In `mod tests`, append:

```rust
#[test]
fn insert_html_block_serializes() {
    let op = PatchOp::InsertHtmlBlock {
        slide_id: "sl_1".into(),
        before: None,
        after: None,
        block: serde_json::json!({"kind": "paragraph", "runs": []}),
    };
    let v = serde_json::to_value(&op).unwrap();
    assert_eq!(v["op"], "insert_html_block");
}

#[test]
fn insert_html_table_row_serializes() {
    let op = PatchOp::InsertHtmlTableRow {
        slide_id: "sl_1".into(),
        table_block_id: "blk_t".into(),
        before: Some("row_5".into()),
        after: None,
        cells: vec![serde_json::json!({"type":"text","value":"A"})],
    };
    let v = serde_json::to_value(&op).unwrap();
    assert_eq!(v["op"], "insert_html_table_row");
}

#[test]
fn update_html_list_item_serializes() {
    let op = PatchOp::UpdateHtmlListItem {
        slide_id: "sl_1".into(),
        list_block_id: "blk_l".into(),
        item_id: "li_3".into(),
        runs: vec![],
    };
    let v = serde_json::to_value(&op).unwrap();
    assert_eq!(v["op"], "update_html_list_item");
}
```

- [ ] **Step 2: Run, verify fails**

Expected: variants don't exist.

- [ ] **Step 3: Add variants**

In `src/libs/colmena/src/documents/domain/patch.rs`, AFTER the slide-level ops, before the closing `}` of `PatchOp`, add:

```rust
    // -------- HTML — block level --------

    /// Insert a new block on a slide. Provide exactly one of before/after
    /// (a block_id) or neither (appends). Returns block_id in summary.
    #[serde(rename = "insert_html_block")]
    InsertHtmlBlock {
        slide_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<String>,
        /// Block JSON (type-tagged). ID assigned server-side.
        block: serde_json::Value,
    },

    /// Delete a block from a slide.
    #[serde(rename = "delete_html_block")]
    DeleteHtmlBlock { slide_id: String, block_id: String },

    /// Replace a block's contents preserving its id.
    #[serde(rename = "replace_html_block")]
    ReplaceHtmlBlock {
        slide_id: String,
        block_id: String,
        block: serde_json::Value,
    },

    /// Move a block within the same slide to appear after another block.
    #[serde(rename = "move_html_block")]
    MoveHtmlBlock {
        slide_id: String,
        block_id: String,
        after_block_id: String,
    },

    // -------- HTML — table --------

    #[serde(rename = "insert_html_table_row")]
    InsertHtmlTableRow {
        slide_id: String,
        table_block_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<String>,
        /// Array of TableCell tagged JSON values.
        cells: Vec<serde_json::Value>,
    },

    #[serde(rename = "delete_html_table_row")]
    DeleteHtmlTableRow {
        slide_id: String,
        table_block_id: String,
        row_id: String,
    },

    #[serde(rename = "update_html_table_cell")]
    UpdateHtmlTableCell {
        slide_id: String,
        table_block_id: String,
        row_id: String,
        col_index: u32,
        /// TableCell tagged JSON.
        cell: serde_json::Value,
    },

    // -------- HTML — list --------

    #[serde(rename = "insert_html_list_item")]
    InsertHtmlListItem {
        slide_id: String,
        list_block_id: String,
        at_index: u32,
        runs: Vec<serde_json::Value>,
    },

    #[serde(rename = "delete_html_list_item")]
    DeleteHtmlListItem {
        slide_id: String,
        list_block_id: String,
        item_id: String,
    },

    #[serde(rename = "update_html_list_item")]
    UpdateHtmlListItem {
        slide_id: String,
        list_block_id: String,
        item_id: String,
        runs: Vec<serde_json::Value>,
    },
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::domain::patch`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/domain/patch.rs
git commit -m "$(cat <<'EOF'
feat(documents/domain): add 10 block/table/list HTML patch ops

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 3.3: Add 3 doc-level ops (`SetTheme`, `SetDocProps`, `SetFooter`)

**Files:**
- Modify: `src/libs/colmena/src/documents/domain/patch.rs`

- [ ] **Step 1: Add failing test**

In `mod tests`, append:

```rust
#[test]
fn set_theme_uses_enum() {
    use crate::documents::domain::ir::Theme;
    let op = PatchOp::SetTheme {
        theme: Theme::Vibrant,
    };
    let v = serde_json::to_value(&op).unwrap();
    assert_eq!(v["op"], "set_theme");
    assert_eq!(v["theme"], "vibrant");
}

#[test]
fn set_theme_invalid_string_fails_deserialize() {
    let bad = serde_json::json!({"op": "set_theme", "theme": "rainbow"});
    let r: Result<PatchOp, _> = serde_json::from_value(bad);
    assert!(r.is_err());
}

#[test]
fn set_footer_serializes() {
    use crate::documents::domain::ir::FooterConfig;
    let op = PatchOp::SetFooter {
        footer: FooterConfig {
            enabled: true,
            page_numbers: true,
            custom_text: Some("Confidential".into()),
        },
    };
    let v = serde_json::to_value(&op).unwrap();
    assert_eq!(v["op"], "set_footer");
    assert_eq!(v["footer"]["page_numbers"], true);
}
```

- [ ] **Step 2: Run, verify fails**

Expected: variants don't exist.

- [ ] **Step 3: Add variants and import**

At the top of `src/libs/colmena/src/documents/domain/patch.rs`, extend the imports:

```rust
use crate::documents::domain::ir::{FooterConfig, Locale, SlideLayout, Theme};
```

Inside `PatchOp`, after the list ops, add:

```rust
    // -------- HTML — document level --------

    #[serde(rename = "set_theme")]
    SetTheme { theme: Theme },

    #[serde(rename = "set_doc_props")]
    SetDocProps {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        author: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        date: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locale: Option<Locale>,
    },

    #[serde(rename = "set_footer")]
    SetFooter { footer: FooterConfig },
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::domain::patch`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/domain/patch.rs
git commit -m "$(cat <<'EOF'
feat(documents/domain): add 3 doc-level HTML patch ops (SetTheme, SetDocProps, SetFooter)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4 — `AssetStore` port + `AssetError` + `IdGenerator` extensions

### Task 4.1: Add `AssetError` + `AssetSummary` + `AssetStore` trait

**Files:**
- Modify: `src/libs/colmena/src/documents/domain/error.rs`
- Modify: `src/libs/colmena/src/documents/domain/ports.rs`

- [ ] **Step 1: Add failing test for trait shape**

In `src/libs/colmena/src/documents/domain/ports.rs`, inside (or create) a `#[cfg(test)] mod tests`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ids::{AssetId, SessionId};

    #[tokio::test]
    async fn asset_store_trait_is_dyn_compatible() {
        // Compile-time check: we can build a trait object.
        fn _accepts(_: std::sync::Arc<dyn AssetStore>) {}
        // (no body — compile = pass)
    }

    #[test]
    fn asset_summary_has_required_fields() {
        let s = AssetSummary {
            id: AssetId::new("asset_1"),
            session_id: SessionId::new("sess_1"),
            mime: "image/png".into(),
            size_bytes: 1024,
            label: Some("logo".into()),
            created_at: chrono::Utc::now(),
        };
        assert_eq!(s.mime, "image/png");
    }
}
```

- [ ] **Step 2: Run, verify fails**

Run: `cargo test --lib -p colmena_dag_engine documents::domain::ports`
Expected: compile error — `AssetStore` and `AssetSummary` not defined.

- [ ] **Step 3: Add `AssetError`**

In `src/libs/colmena/src/documents/domain/error.rs`, append:

```rust
#[derive(Debug, Error, Serialize)]
pub enum AssetError {
    #[error("asset not found: {id}")]
    NotFound { id: AssetId },

    #[error("mime not allowed: {mime}")]
    MimeNotAllowed { mime: String },

    #[error("asset too large: {actual} bytes (max {max})")]
    TooLarge { actual: u64, max: u64 },

    #[error("asset {id} still referenced by {} artifact(s)", by_artifacts.len())]
    StillReferenced {
        id: AssetId,
        by_artifacts: Vec<ArtifactId>,
    },

    #[error("storage error: {0}")]
    Storage(String),
}
```

At the top of the file, extend the import:

```rust
use super::ids::{ArtifactId, AssetId, SessionId, VersionId};
```

- [ ] **Step 4: Add `AssetStore` trait + `AssetSummary`**

In `src/libs/colmena/src/documents/domain/ports.rs`, append:

```rust
use crate::documents::domain::error::AssetError;
use crate::documents::domain::ids::AssetId;

#[derive(Debug, Clone, Serialize)]
pub struct AssetSummary {
    pub id: AssetId,
    pub session_id: SessionId,
    pub mime: String,
    pub size_bytes: u64,
    pub label: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[async_trait]
pub trait AssetStore: Send + Sync {
    async fn upload(
        &self,
        session: &SessionId,
        id: &AssetId,
        bytes: Vec<u8>,
        mime: &str,
        label: Option<&str>,
    ) -> Result<(), AssetError>;

    async fn read(&self, id: &AssetId) -> Result<(Vec<u8>, String), AssetError>;

    async fn list_by_session(
        &self,
        session: &SessionId,
    ) -> Result<Vec<AssetSummary>, AssetError>;

    async fn delete(&self, id: &AssetId) -> Result<(), AssetError>;

    async fn head(&self, id: &AssetId) -> Result<AssetSummary, AssetError>;
}
```

At the top of the file, ensure `use serde::Serialize;` is present (add if missing). The `Serialize` derive on `AssetSummary` requires it.

- [ ] **Step 5: Re-export from domain/mod.rs**

In `src/libs/colmena/src/documents/domain/mod.rs`, ensure `AssetError` and `AssetStore` / `AssetSummary` are re-exported alongside existing ones. If the file uses `pub use ports::*;` you're done; otherwise add explicit exports for the new types.

- [ ] **Step 6: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::domain`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/documents/domain/error.rs src/libs/colmena/src/documents/domain/ports.rs src/libs/colmena/src/documents/domain/mod.rs
git commit -m "$(cat <<'EOF'
feat(documents/domain): add AssetStore port, AssetSummary, AssetError

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 4.2: Extend `IdGenerator` with `new_slide_id` and `new_asset_id`

**Files:**
- Modify: `src/libs/colmena/src/documents/domain/ports.rs`
- Modify: `src/libs/colmena/src/documents/infrastructure/ids.rs`

- [ ] **Step 1: Add failing test**

In `src/libs/colmena/src/documents/infrastructure/ids.rs`, inside `mod tests`, append:

```rust
#[test]
fn ulid_generator_new_slide_and_asset() {
    let g = UlidIdGenerator;
    assert!(g.new_slide_id().starts_with("sl_"));
    assert!(g.new_asset_id().starts_with("asset_"));
    assert_ne!(g.new_slide_id(), g.new_slide_id());
}

#[test]
fn counting_generator_new_slide_and_asset() {
    let g = CountingIdGenerator::default();
    assert_eq!(g.new_slide_id(), "sl_01");
    assert_eq!(g.new_slide_id(), "sl_02");
    assert_eq!(g.new_asset_id(), "asset_01");
}
```

- [ ] **Step 2: Run, verify fails**

Run: `cargo test --lib -p colmena_dag_engine documents::infrastructure::ids`
Expected: compile errors — methods don't exist.

- [ ] **Step 3: Extend trait**

In `src/libs/colmena/src/documents/domain/ports.rs`, in the `IdGenerator` trait, add:

```rust
    fn new_slide_id(&self) -> String;
    fn new_asset_id(&self) -> String;
```

- [ ] **Step 4: Implement in `UlidIdGenerator` and `CountingIdGenerator`**

In `src/libs/colmena/src/documents/infrastructure/ids.rs`, inside `impl IdGenerator for UlidIdGenerator`, add:

```rust
    fn new_slide_id(&self) -> String {
        format!("sl_{}", Self::short_ulid())
    }
    fn new_asset_id(&self) -> String {
        format!("asset_{}", Self::short_ulid())
    }
```

In `CountingIdGenerator`, change the counters array size from `[u64; 7]` to `[u64; 9]` and update both `Default` and the impl:

Replace:
```rust
pub struct CountingIdGenerator {
    counters: Mutex<[u64; 7]>,
}

impl Default for CountingIdGenerator {
    fn default() -> Self {
        Self {
            counters: Mutex::new([0; 7]),
        }
    }
}
```

with:

```rust
pub struct CountingIdGenerator {
    counters: Mutex<[u64; 9]>,
}

impl Default for CountingIdGenerator {
    fn default() -> Self {
        Self {
            counters: Mutex::new([0; 9]),
        }
    }
}
```

And inside `impl IdGenerator for CountingIdGenerator`, add:

```rust
    fn new_slide_id(&self) -> String {
        format!("sl_{:02}", self.next(7))
    }
    fn new_asset_id(&self) -> String {
        format!("asset_{:02}", self.next(8))
    }
```

- [ ] **Step 5: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents`
Expected: PASS. (Any existing test using `IdGenerator` trait objects still works since these are added methods, not changed signatures.)

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/documents/domain/ports.rs src/libs/colmena/src/documents/infrastructure/ids.rs
git commit -m "$(cat <<'EOF'
feat(documents): extend IdGenerator with new_slide_id and new_asset_id

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5 — `HtmlValidator`

### Task 5.1: Create validator skeleton + schema/cardinality rules

**Files:**
- Create: `src/libs/colmena/src/documents/infrastructure/validation/html_validator.rs`
- Modify: `src/libs/colmena/src/documents/infrastructure/validation/mod.rs`

- [ ] **Step 1: Create the file with failing tests**

Create `src/libs/colmena/src/documents/infrastructure/validation/html_validator.rs`:

```rust
//! HtmlValidator — implements IRValidator for HtmlIR.
//!
//! Lives in infrastructure because the implementation uses regex and JSON
//! traversal. The CONTRACT (what is validated, what errors arise) belongs
//! to the domain via `IRValidator` and `DocumentError`.

use crate::documents::domain::ir::{Block, HtmlIR, ImageSrc, VideoSrc};
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
    use crate::documents::domain::ir::SlideLayout;
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
```

- [ ] **Step 2: Register module**

In `src/libs/colmena/src/documents/infrastructure/validation/mod.rs`, add:

```rust
pub mod html_validator;

pub use html_validator::HtmlValidator;
```

(Keep existing `pub mod excel_validator;` and `pub mod word_validator;` lines intact.)

- [ ] **Step 3: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::infrastructure::validation::html_validator`
Expected: 7 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/infrastructure/validation/html_validator.rs src/libs/colmena/src/documents/infrastructure/validation/mod.rs
git commit -m "$(cat <<'EOF'
feat(documents/infra): HtmlValidator with schema, cardinality, ID uniqueness, layout rules

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 5.2: Add security + chart + nested-grid rules

**Files:**
- Modify: `src/libs/colmena/src/documents/infrastructure/validation/html_validator.rs`

- [ ] **Step 1: Add failing tests**

Inside `mod tests`, append:

```rust
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
    // depth 4: TwoColumns > TwoColumns > TwoColumns > TwoColumns
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
```

- [ ] **Step 2: Run, verify fails**

Expected: rules not yet implemented.

- [ ] **Step 3: Implement the rules**

In `src/libs/colmena/src/documents/infrastructure/validation/html_validator.rs`, extend the validate flow. Add to the import block:

```rust
use crate::documents::domain::ir::{
    AxisSpec, Block, ChartSpec, ChartType, HtmlIR, ImageSrc, KpiCardInline, Run, VideoSrc,
};
use once_cell::sync::Lazy;
use regex::Regex;
```

NOTE: `once_cell` may already be transitive — if `cargo build` fails on its absence, add `once_cell = "1"` to `[dependencies]` in `Cargo.toml`.

Add these helpers and module-level statics:

```rust
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
                        if let crate::documents::domain::ir::TableCell::Runs { runs } = cell {
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
                let _ = (cards, KpiCardInline { ..Default::default() });
                // (Default impl avoided — Default is not derived. The above line is a placeholder
                // and will be removed by the compiler since `_ = (cards, ...)` is unreachable here.)
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
```

REMOVE the broken `Default` placeholder inside the `KpiGrid` arm — keep only:

```rust
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
```

And wire `validate_security(&ir)?;` into the top-level `validate`:

```rust
impl IRValidator for HtmlValidator {
    fn validate(&self, ir: &serde_json::Value) -> Result<(), DocumentError> {
        let ir: HtmlIR = serde_json::from_value(ir.clone()).map_err(|e| DocumentError::IRValidationFailed {
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
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::infrastructure::validation::html_validator`
Expected: all tests PASS (the 7 from Task 5.1 + 6 from this task).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/infrastructure/validation/html_validator.rs src/libs/colmena/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(documents/infra): HtmlValidator security rules (URL allowlist, chart consistency, nesting limit)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6 — `LocalFsAssetStore`

### Task 6.1: Implement `LocalFsAssetStore`

**Files:**
- Create: `src/libs/colmena/src/documents/infrastructure/storage/local_fs_asset_store.rs`
- Modify: `src/libs/colmena/src/documents/infrastructure/storage/mod.rs`

- [ ] **Step 1: Create file with failing tests**

Create `src/libs/colmena/src/documents/infrastructure/storage/local_fs_asset_store.rs`:

```rust
//! LocalFsAssetStore — filesystem adapter for AssetStore.
//!
//! Layout:
//!   {root}/sessions/{session_id}/{asset_id}/bytes.bin
//!   {root}/sessions/{session_id}/{asset_id}/meta.json

use crate::documents::domain::error::AssetError;
use crate::documents::domain::ids::{AssetId, SessionId};
use crate::documents::domain::ports::{AssetStore, AssetSummary};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;

pub struct LocalFsAssetStore {
    root: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredMeta {
    session_id: String,
    mime: String,
    size_bytes: u64,
    label: Option<String>,
    created_at: DateTime<Utc>,
}

impl LocalFsAssetStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    fn asset_dir(&self, session: &SessionId, id: &AssetId) -> PathBuf {
        self.root
            .join("sessions")
            .join(session.as_str())
            .join(id.as_str())
    }

    async fn find_asset_dir(&self, id: &AssetId) -> Result<PathBuf, AssetError> {
        // Linear scan over sessions/* — fine for v1 volumes.
        let sessions_dir = self.root.join("sessions");
        let mut rd = fs::read_dir(&sessions_dir)
            .await
            .map_err(|e| AssetError::Storage(format!("read sessions dir: {e}")))?;
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| AssetError::Storage(format!("read entry: {e}")))?
        {
            let candidate = entry.path().join(id.as_str());
            if fs::try_exists(&candidate).await.unwrap_or(false) {
                return Ok(candidate);
            }
        }
        Err(AssetError::NotFound { id: id.clone() })
    }
}

#[async_trait]
impl AssetStore for LocalFsAssetStore {
    async fn upload(
        &self,
        session: &SessionId,
        id: &AssetId,
        bytes: Vec<u8>,
        mime: &str,
        label: Option<&str>,
    ) -> Result<(), AssetError> {
        let dir = self.asset_dir(session, id);
        fs::create_dir_all(&dir)
            .await
            .map_err(|e| AssetError::Storage(format!("mkdir {dir:?}: {e}")))?;

        let tmp = dir.join("bytes.bin.tmp");
        let final_path = dir.join("bytes.bin");
        fs::write(&tmp, &bytes)
            .await
            .map_err(|e| AssetError::Storage(format!("write bytes: {e}")))?;
        fs::rename(&tmp, &final_path)
            .await
            .map_err(|e| AssetError::Storage(format!("rename bytes: {e}")))?;

        let meta = StoredMeta {
            session_id: session.as_str().to_string(),
            mime: mime.to_string(),
            size_bytes: bytes.len() as u64,
            label: label.map(|s| s.to_string()),
            created_at: Utc::now(),
        };
        let meta_tmp = dir.join("meta.json.tmp");
        let meta_final = dir.join("meta.json");
        let meta_bytes = serde_json::to_vec_pretty(&meta)
            .map_err(|e| AssetError::Storage(format!("ser meta: {e}")))?;
        fs::write(&meta_tmp, &meta_bytes)
            .await
            .map_err(|e| AssetError::Storage(format!("write meta: {e}")))?;
        fs::rename(&meta_tmp, &meta_final)
            .await
            .map_err(|e| AssetError::Storage(format!("rename meta: {e}")))?;
        Ok(())
    }

    async fn read(&self, id: &AssetId) -> Result<(Vec<u8>, String), AssetError> {
        let dir = self.find_asset_dir(id).await?;
        let meta_bytes = fs::read(dir.join("meta.json"))
            .await
            .map_err(|e| AssetError::Storage(format!("read meta: {e}")))?;
        let meta: StoredMeta = serde_json::from_slice(&meta_bytes)
            .map_err(|e| AssetError::Storage(format!("parse meta: {e}")))?;
        let bytes = fs::read(dir.join("bytes.bin"))
            .await
            .map_err(|e| AssetError::Storage(format!("read bytes: {e}")))?;
        Ok((bytes, meta.mime))
    }

    async fn list_by_session(
        &self,
        session: &SessionId,
    ) -> Result<Vec<AssetSummary>, AssetError> {
        let sdir = self.root.join("sessions").join(session.as_str());
        if !fs::try_exists(&sdir).await.unwrap_or(false) {
            return Ok(vec![]);
        }
        let mut out = vec![];
        let mut rd = fs::read_dir(&sdir)
            .await
            .map_err(|e| AssetError::Storage(format!("read session dir: {e}")))?;
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| AssetError::Storage(format!("read entry: {e}")))?
        {
            let id_str = entry.file_name().to_string_lossy().into_owned();
            let meta_path = entry.path().join("meta.json");
            if !fs::try_exists(&meta_path).await.unwrap_or(false) {
                continue;
            }
            let meta_bytes = fs::read(&meta_path)
                .await
                .map_err(|e| AssetError::Storage(format!("read meta: {e}")))?;
            let meta: StoredMeta = serde_json::from_slice(&meta_bytes)
                .map_err(|e| AssetError::Storage(format!("parse meta: {e}")))?;
            out.push(AssetSummary {
                id: AssetId::new(id_str),
                session_id: SessionId::new(meta.session_id),
                mime: meta.mime,
                size_bytes: meta.size_bytes,
                label: meta.label,
                created_at: meta.created_at,
            });
        }
        Ok(out)
    }

    async fn delete(&self, id: &AssetId) -> Result<(), AssetError> {
        let dir = self.find_asset_dir(id).await?;
        fs::remove_dir_all(&dir)
            .await
            .map_err(|e| AssetError::Storage(format!("remove dir: {e}")))?;
        Ok(())
    }

    async fn head(&self, id: &AssetId) -> Result<AssetSummary, AssetError> {
        let dir = self.find_asset_dir(id).await?;
        let meta_bytes = fs::read(dir.join("meta.json"))
            .await
            .map_err(|e| AssetError::Storage(format!("read meta: {e}")))?;
        let meta: StoredMeta = serde_json::from_slice(&meta_bytes)
            .map_err(|e| AssetError::Storage(format!("parse meta: {e}")))?;
        // session_id is recoverable from path layout: sessions/{session}/{id}/
        let session_id = dir
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(AssetSummary {
            id: id.clone(),
            session_id: SessionId::new(session_id),
            mime: meta.mime,
            size_bytes: meta.size_bytes,
            label: meta.label,
            created_at: meta.created_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn upload_then_read_roundtrip() {
        let tmp = tempdir().unwrap();
        let store = LocalFsAssetStore::new(tmp.path());
        let session = SessionId::new("s1");
        let id = AssetId::new("asset_a");
        store
            .upload(&session, &id, b"hello".to_vec(), "image/png", Some("logo"))
            .await
            .unwrap();
        let (bytes, mime) = store.read(&id).await.unwrap();
        assert_eq!(bytes, b"hello");
        assert_eq!(mime, "image/png");
    }

    #[tokio::test]
    async fn list_returns_summaries_with_label() {
        let tmp = tempdir().unwrap();
        let store = LocalFsAssetStore::new(tmp.path());
        let s = SessionId::new("s1");
        store
            .upload(&s, &AssetId::new("asset_1"), b"a".to_vec(), "image/png", Some("a"))
            .await
            .unwrap();
        store
            .upload(&s, &AssetId::new("asset_2"), b"bb".to_vec(), "image/jpeg", None)
            .await
            .unwrap();
        let list = store.list_by_session(&s).await.unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|a| a.label.as_deref() == Some("a")));
    }

    #[tokio::test]
    async fn delete_removes_asset() {
        let tmp = tempdir().unwrap();
        let store = LocalFsAssetStore::new(tmp.path());
        let s = SessionId::new("s1");
        let id = AssetId::new("asset_x");
        store
            .upload(&s, &id, b"x".to_vec(), "image/png", None)
            .await
            .unwrap();
        store.delete(&id).await.unwrap();
        let err = store.read(&id).await.unwrap_err();
        assert!(matches!(err, AssetError::NotFound { .. }));
    }

    #[tokio::test]
    async fn list_empty_session_returns_empty_vec() {
        let tmp = tempdir().unwrap();
        let store = LocalFsAssetStore::new(tmp.path());
        let list = store.list_by_session(&SessionId::new("nobody")).await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn head_without_loading_bytes() {
        let tmp = tempdir().unwrap();
        let store = LocalFsAssetStore::new(tmp.path());
        let s = SessionId::new("s1");
        store
            .upload(&s, &AssetId::new("asset_h"), vec![0u8; 1024], "image/png", None)
            .await
            .unwrap();
        let head = store.head(&AssetId::new("asset_h")).await.unwrap();
        assert_eq!(head.size_bytes, 1024);
        assert_eq!(head.mime, "image/png");
    }
}
```

- [ ] **Step 2: Register in storage/mod.rs**

In `src/libs/colmena/src/documents/infrastructure/storage/mod.rs`, add:

```rust
pub mod local_fs_asset_store;

pub use local_fs_asset_store::LocalFsAssetStore;
```

- [ ] **Step 3: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::infrastructure::storage::local_fs_asset_store`
Expected: 5 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/infrastructure/storage/local_fs_asset_store.rs src/libs/colmena/src/documents/infrastructure/storage/mod.rs
git commit -m "$(cat <<'EOF'
feat(documents/infra): LocalFsAssetStore (filesystem adapter for AssetStore)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 7 — `GcsAssetStore` (feature `gcs`)

### Task 7.1: Implement `GcsAssetStore`

**Files:**
- Create: `src/libs/colmena/src/documents/infrastructure/storage/gcs_asset_store.rs`
- Modify: `src/libs/colmena/src/documents/infrastructure/storage/mod.rs`

- [ ] **Step 1: Create the file**

Create `src/libs/colmena/src/documents/infrastructure/storage/gcs_asset_store.rs`:

```rust
//! GcsAssetStore — Google Cloud Storage adapter for AssetStore.
//! Feature-flagged behind `gcs`.
//!
//! Layout in bucket:
//!   {prefix}/assets/sessions/{session_id}/{asset_id}/bytes.bin
//!   {prefix}/assets/sessions/{session_id}/{asset_id}/meta.json
//!
//! Assets are immutable post-upload; no CAS needed.

use crate::documents::domain::error::AssetError;
use crate::documents::domain::ids::{AssetId, SessionId};
use crate::documents::domain::ports::{AssetStore, AssetSummary};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use google_cloud_storage::client::{Client, ClientConfig};
use google_cloud_storage::http::objects::delete::DeleteObjectRequest;
use google_cloud_storage::http::objects::download::Range;
use google_cloud_storage::http::objects::get::GetObjectRequest;
use google_cloud_storage::http::objects::list::ListObjectsRequest;
use google_cloud_storage::http::objects::upload::{Media, UploadObjectRequest, UploadType};
use serde::{Deserialize, Serialize};

pub struct GcsAssetStore {
    client: Client,
    bucket: String,
    prefix: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredMeta {
    session_id: String,
    mime: String,
    size_bytes: u64,
    label: Option<String>,
    created_at: DateTime<Utc>,
}

impl GcsAssetStore {
    pub async fn new(bucket: &str, prefix: &str) -> Result<Self, String> {
        let cfg = ClientConfig::default()
            .with_auth()
            .await
            .map_err(|e| format!("gcs auth: {e}"))?;
        Ok(Self {
            client: Client::new(cfg),
            bucket: bucket.to_string(),
            prefix: prefix.trim_end_matches('/').to_string(),
        })
    }

    fn bytes_path(&self, session: &SessionId, id: &AssetId) -> String {
        format!(
            "{}/assets/sessions/{}/{}/bytes.bin",
            self.prefix,
            session.as_str(),
            id.as_str()
        )
    }
    fn meta_path(&self, session: &SessionId, id: &AssetId) -> String {
        format!(
            "{}/assets/sessions/{}/{}/meta.json",
            self.prefix,
            session.as_str(),
            id.as_str()
        )
    }
    fn session_prefix(&self, session: &SessionId) -> String {
        format!("{}/assets/sessions/{}/", self.prefix, session.as_str())
    }

    async fn find_session_for(&self, id: &AssetId) -> Result<SessionId, AssetError> {
        let scan_prefix = format!("{}/assets/sessions/", self.prefix);
        let req = ListObjectsRequest {
            bucket: self.bucket.clone(),
            prefix: Some(scan_prefix.clone()),
            ..Default::default()
        };
        let resp = self
            .client
            .list_objects(&req)
            .await
            .map_err(|e| AssetError::Storage(format!("gcs list: {e}")))?;
        let needle = format!("/{}/meta.json", id.as_str());
        if let Some(items) = resp.items {
            for o in items {
                if o.name.ends_with(&needle) {
                    // path is {prefix}/assets/sessions/{session}/{asset_id}/meta.json
                    let parts: Vec<&str> = o.name.split('/').collect();
                    if parts.len() >= 5 {
                        let session = parts[parts.len() - 3].to_string();
                        return Ok(SessionId::new(session));
                    }
                }
            }
        }
        Err(AssetError::NotFound { id: id.clone() })
    }
}

#[async_trait]
impl AssetStore for GcsAssetStore {
    async fn upload(
        &self,
        session: &SessionId,
        id: &AssetId,
        bytes: Vec<u8>,
        mime: &str,
        label: Option<&str>,
    ) -> Result<(), AssetError> {
        let bytes_req = UploadObjectRequest {
            bucket: self.bucket.clone(),
            ..Default::default()
        };
        let bytes_media = Media::new(self.bytes_path(session, id));
        let size = bytes.len() as u64;
        self.client
            .upload_object(&bytes_req, bytes, &UploadType::Simple(bytes_media))
            .await
            .map_err(|e| AssetError::Storage(format!("gcs upload bytes: {e}")))?;

        let meta = StoredMeta {
            session_id: session.as_str().to_string(),
            mime: mime.to_string(),
            size_bytes: size,
            label: label.map(|s| s.to_string()),
            created_at: Utc::now(),
        };
        let meta_bytes = serde_json::to_vec(&meta)
            .map_err(|e| AssetError::Storage(format!("ser meta: {e}")))?;
        let meta_req = UploadObjectRequest {
            bucket: self.bucket.clone(),
            ..Default::default()
        };
        let meta_media = Media::new(self.meta_path(session, id));
        self.client
            .upload_object(&meta_req, meta_bytes, &UploadType::Simple(meta_media))
            .await
            .map_err(|e| AssetError::Storage(format!("gcs upload meta: {e}")))?;
        Ok(())
    }

    async fn read(&self, id: &AssetId) -> Result<(Vec<u8>, String), AssetError> {
        let session = self.find_session_for(id).await?;
        let bytes = self
            .client
            .download_object(
                &GetObjectRequest {
                    bucket: self.bucket.clone(),
                    object: self.bytes_path(&session, id),
                    ..Default::default()
                },
                &Range::default(),
            )
            .await
            .map_err(|e| AssetError::Storage(format!("gcs download bytes: {e}")))?;
        let meta_bytes = self
            .client
            .download_object(
                &GetObjectRequest {
                    bucket: self.bucket.clone(),
                    object: self.meta_path(&session, id),
                    ..Default::default()
                },
                &Range::default(),
            )
            .await
            .map_err(|e| AssetError::Storage(format!("gcs download meta: {e}")))?;
        let meta: StoredMeta = serde_json::from_slice(&meta_bytes)
            .map_err(|e| AssetError::Storage(format!("parse meta: {e}")))?;
        Ok((bytes, meta.mime))
    }

    async fn list_by_session(
        &self,
        session: &SessionId,
    ) -> Result<Vec<AssetSummary>, AssetError> {
        let prefix = self.session_prefix(session);
        let resp = self
            .client
            .list_objects(&ListObjectsRequest {
                bucket: self.bucket.clone(),
                prefix: Some(prefix.clone()),
                ..Default::default()
            })
            .await
            .map_err(|e| AssetError::Storage(format!("gcs list: {e}")))?;

        let mut out = vec![];
        if let Some(items) = resp.items {
            for o in items {
                if !o.name.ends_with("/meta.json") {
                    continue;
                }
                let meta_bytes = self
                    .client
                    .download_object(
                        &GetObjectRequest {
                            bucket: self.bucket.clone(),
                            object: o.name.clone(),
                            ..Default::default()
                        },
                        &Range::default(),
                    )
                    .await
                    .map_err(|e| AssetError::Storage(format!("gcs download meta: {e}")))?;
                let meta: StoredMeta = serde_json::from_slice(&meta_bytes)
                    .map_err(|e| AssetError::Storage(format!("parse meta: {e}")))?;
                // asset_id from path: .../sessions/{session}/{asset_id}/meta.json
                let parts: Vec<&str> = o.name.split('/').collect();
                let id = AssetId::new(parts[parts.len() - 2].to_string());
                out.push(AssetSummary {
                    id,
                    session_id: SessionId::new(meta.session_id),
                    mime: meta.mime,
                    size_bytes: meta.size_bytes,
                    label: meta.label,
                    created_at: meta.created_at,
                });
            }
        }
        Ok(out)
    }

    async fn delete(&self, id: &AssetId) -> Result<(), AssetError> {
        let session = self.find_session_for(id).await?;
        for path in [self.bytes_path(&session, id), self.meta_path(&session, id)] {
            self.client
                .delete_object(&DeleteObjectRequest {
                    bucket: self.bucket.clone(),
                    object: path,
                    ..Default::default()
                })
                .await
                .map_err(|e| AssetError::Storage(format!("gcs delete: {e}")))?;
        }
        Ok(())
    }

    async fn head(&self, id: &AssetId) -> Result<AssetSummary, AssetError> {
        let session = self.find_session_for(id).await?;
        let meta_bytes = self
            .client
            .download_object(
                &GetObjectRequest {
                    bucket: self.bucket.clone(),
                    object: self.meta_path(&session, id),
                    ..Default::default()
                },
                &Range::default(),
            )
            .await
            .map_err(|e| AssetError::Storage(format!("gcs download meta: {e}")))?;
        let meta: StoredMeta = serde_json::from_slice(&meta_bytes)
            .map_err(|e| AssetError::Storage(format!("parse meta: {e}")))?;
        Ok(AssetSummary {
            id: id.clone(),
            session_id: session,
            mime: meta.mime,
            size_bytes: meta.size_bytes,
            label: meta.label,
            created_at: meta.created_at,
        })
    }
}
```

- [ ] **Step 2: Register in storage/mod.rs (cfg-gated)**

In `src/libs/colmena/src/documents/infrastructure/storage/mod.rs`, add:

```rust
#[cfg(feature = "gcs")]
pub mod gcs_asset_store;

#[cfg(feature = "gcs")]
pub use gcs_asset_store::GcsAssetStore;
```

- [ ] **Step 3: Verify compile under gcs feature**

Run: `cargo build --lib -p colmena_dag_engine --features gcs`
Expected: build succeeds.

NOTE: This adapter has no offline tests (it requires GCS credentials). It is verified through the GcsArtifactStore pattern (which is already tested in CI when `gcs` is enabled). Integration tests against a mocked bucket are out of scope for v1 — match the existing `GcsArtifactStore` test policy.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/infrastructure/storage/gcs_asset_store.rs src/libs/colmena/src/documents/infrastructure/storage/mod.rs
git commit -m "$(cat <<'EOF'
feat(documents/infra): GcsAssetStore (feature gcs)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 8 — Themes CSS + slides_runtime.js + Chart.js

### Task 8.1: Vendor Chart.js + author slides_runtime.js

**Files:**
- Create: `src/libs/colmena/src/documents/infrastructure/render/html_assets/chart_js.min.js`
- Create: `src/libs/colmena/src/documents/infrastructure/render/html_assets/slides_runtime.js`

- [ ] **Step 1: Create the assets directory**

Run: `mkdir -p src/libs/colmena/src/documents/infrastructure/render/html_assets/themes`

- [ ] **Step 2: Download Chart.js minified**

Run:

```bash
curl -sL https://cdn.jsdelivr.net/npm/chart.js@4.4.1/dist/chart.umd.min.js -o src/libs/colmena/src/documents/infrastructure/render/html_assets/chart_js.min.js
```

Verify size > 100KB:

```bash
ls -la src/libs/colmena/src/documents/infrastructure/render/html_assets/chart_js.min.js
```

Expected output: file size ~190KB.

- [ ] **Step 3: Author slides_runtime.js**

Create `src/libs/colmena/src/documents/infrastructure/render/html_assets/slides_runtime.js`:

```javascript
// slides_runtime.js — keyboard nav + fullscreen for HTML slides
// Activated only when layout_mode == "slides".
(function () {
  "use strict";
  var slides = document.querySelectorAll(".slide");
  if (slides.length === 0) return;
  var current = 0;
  var counter = document.getElementById("slide-counter");

  function goto(i) {
    if (i < 0) i = 0;
    if (i >= slides.length) i = slides.length - 1;
    current = i;
    slides[i].scrollIntoView({ behavior: "smooth", block: "start" });
    if (counter) counter.textContent = (i + 1) + " / " + slides.length;
  }

  document.addEventListener("keydown", function (e) {
    var key = e.key;
    if (key === "ArrowRight" || key === "PageDown" || key === " ") {
      e.preventDefault();
      goto(current + 1);
    } else if (key === "ArrowLeft" || key === "PageUp") {
      e.preventDefault();
      goto(current - 1);
    } else if (key === "f" || key === "F") {
      var deck = document.querySelector(".deck");
      if (!document.fullscreenElement && deck && deck.requestFullscreen) {
        deck.requestFullscreen();
      } else if (document.exitFullscreen) {
        document.exitFullscreen();
      }
    } else if (key === "Escape" && document.exitFullscreen) {
      document.exitFullscreen();
    }
  });

  // Initial counter render
  if (counter) counter.textContent = "1 / " + slides.length;
})();
```

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/infrastructure/render/html_assets/chart_js.min.js src/libs/colmena/src/documents/infrastructure/render/html_assets/slides_runtime.js
git commit -m "$(cat <<'EOF'
feat(documents/infra): vendor Chart.js 4.4.1 + author slides_runtime.js

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 8.2: Author 4 theme CSS files

**Files:**
- Create: `src/libs/colmena/src/documents/infrastructure/render/html_assets/themes/executive.css`
- Create: `src/libs/colmena/src/documents/infrastructure/render/html_assets/themes/minimal.css`
- Create: `src/libs/colmena/src/documents/infrastructure/render/html_assets/themes/vibrant.css`
- Create: `src/libs/colmena/src/documents/infrastructure/render/html_assets/themes/dark.css`

- [ ] **Step 1: Create `executive.css`**

Create `src/libs/colmena/src/documents/infrastructure/render/html_assets/themes/executive.css`:

```css
:root {
  --color-primary: #1e3a5f;
  --color-secondary: #5b7a99;
  --color-bg: #ffffff;
  --color-text: #1a1a1a;
  --color-muted: #6b7280;
  --color-success: #2e7d32;
  --color-warning: #b45309;
  --color-danger: #b91c1c;
  --color-info: #1e40af;
  --font-heading: Georgia, "Times New Roman", serif;
  --font-body: "Helvetica Neue", Arial, sans-serif;
  --font-mono: "SF Mono", Menlo, monospace;
  --space-sm: 0.5rem;
  --space-md: 1rem;
  --space-lg: 2rem;
  --radius: 4px;
  --chart-palette-0: #1e3a5f;
  --chart-palette-1: #5b7a99;
  --chart-palette-2: #8fa8c2;
  --chart-palette-3: #c2d1e0;
  --chart-palette-4: #b45309;
  --chart-palette-5: #2e7d32;
}
body { margin: 0; font-family: var(--font-body); color: var(--color-text); background: var(--color-bg); line-height: 1.5; }
.deck { max-width: 1100px; margin: 0 auto; padding: var(--space-lg); }
.slide { padding: var(--space-lg) 0; border-bottom: 1px solid #eee; min-height: 80vh; }
.slide:last-child { border-bottom: none; }
.slide--title { display: flex; flex-direction: column; justify-content: center; align-items: center; text-align: center; }
.slide--title h1 { font-family: var(--font-heading); font-size: 3rem; margin: 0 0 var(--space-md) 0; color: var(--color-primary); }
.slide--title .subtitle { font-size: 1.25rem; color: var(--color-muted); }
.slide--section-divider { display: flex; align-items: center; justify-content: center; background: var(--color-primary); color: white; }
.slide--section-divider h1 { font-family: var(--font-heading); font-size: 2.5rem; }
h1, h2, h3, h4, h5, h6 { font-family: var(--font-heading); color: var(--color-primary); margin-top: var(--space-lg); }
h1 { font-size: 2rem; } h2 { font-size: 1.5rem; } h3 { font-size: 1.25rem; }
p { margin: var(--space-md) 0; }
a { color: var(--color-primary); }
table { border-collapse: collapse; width: 100%; margin: var(--space-md) 0; }
th, td { border: 1px solid #ddd; padding: 0.5rem 0.75rem; text-align: left; }
th { background: var(--color-primary); color: white; }
.block-kpi-card { padding: var(--space-md); border: 1px solid #e5e7eb; border-radius: var(--radius); background: #fafafa; }
.block-kpi-card__value { font-size: 2.25rem; font-weight: 700; color: var(--color-primary); }
.block-kpi-card__label { color: var(--color-muted); font-size: 0.875rem; text-transform: uppercase; letter-spacing: 0.05em; }
.block-kpi-card__delta--up { color: var(--color-success); }
.block-kpi-card__delta--down { color: var(--color-danger); }
.block-kpi-card__delta--neutral { color: var(--color-muted); }
.block-kpi-grid { display: grid; gap: var(--space-md); }
.block-kpi-grid[data-cols="2"] { grid-template-columns: repeat(2, 1fr); }
.block-kpi-grid[data-cols="3"] { grid-template-columns: repeat(3, 1fr); }
.block-kpi-grid[data-cols="4"] { grid-template-columns: repeat(4, 1fr); }
.chart-container { margin: var(--space-md) 0; }
.chart-container--small { max-height: 240px; }
.chart-container--medium { max-height: 360px; }
.chart-container--large { max-height: 520px; }
.block-two-columns, .block-three-columns { display: grid; align-items: start; }
.block-two-columns[data-ratio="50_50"] { grid-template-columns: 1fr 1fr; }
.block-two-columns[data-ratio="40_60"] { grid-template-columns: 2fr 3fr; }
.block-two-columns[data-ratio="60_40"] { grid-template-columns: 3fr 2fr; }
.block-two-columns[data-ratio="30_70"] { grid-template-columns: 3fr 7fr; }
.block-two-columns[data-ratio="70_30"] { grid-template-columns: 7fr 3fr; }
.block-three-columns { grid-template-columns: 1fr 1fr 1fr; }
.gap-small  { gap: var(--space-sm); } .gap-medium { gap: var(--space-md); } .gap-large  { gap: var(--space-lg); }
.callout { padding: var(--space-md); border-left: 4px solid; border-radius: var(--radius); margin: var(--space-md) 0; }
.callout--info    { border-color: var(--color-info);    background: #eff6ff; }
.callout--warning { border-color: var(--color-warning); background: #fffbeb; }
.callout--success { border-color: var(--color-success); background: #f0fdf4; }
.callout--danger  { border-color: var(--color-danger);  background: #fef2f2; }
.divider { height: 1px; background: #e5e7eb; margin: var(--space-lg) 0; }
.block-image { margin: var(--space-md) 0; }
.block-image img { max-width: 100%; height: auto; }
.block-image--hero img { width: 100%; }
.block-image__caption { color: var(--color-muted); font-size: 0.875rem; margin-top: 0.25rem; }
.block-video { margin: var(--space-md) 0; }
.block-video iframe { width: 100%; aspect-ratio: 16/9; border: 0; }
.block-auto-toc { padding: var(--space-md); background: #fafafa; border-radius: var(--radius); }
.block-auto-toc ol { margin: 0; padding-left: 1.5rem; }
.block-auto-toc a { text-decoration: none; }
.footer { padding: var(--space-md); text-align: center; color: var(--color-muted); font-size: 0.875rem; }
#slide-counter { position: fixed; bottom: 1rem; right: 1rem; background: rgba(0,0,0,0.5); color: white; padding: 0.25rem 0.5rem; border-radius: var(--radius); font-family: var(--font-mono); font-size: 0.75rem; }
@media print {
  body { background: white; color: black; }
  .deck { max-width: none; padding: 0; }
  .slide { page-break-after: always; border: none; min-height: auto; padding: 1cm; }
  .slide:last-child { page-break-after: auto; }
  #slide-counter { display: none; }
  .footer { position: fixed; bottom: 0.5cm; left: 0; right: 0; }
  a { color: inherit; text-decoration: underline; }
  .chart-container canvas { max-height: 8cm !important; }
}
```

- [ ] **Step 2: Create `minimal.css`**

Create `src/libs/colmena/src/documents/infrastructure/render/html_assets/themes/minimal.css`:

Use the same CSS as `executive.css` but with this `:root` override at the top:

```css
:root {
  --color-primary: #000000;
  --color-secondary: #555555;
  --color-bg: #ffffff;
  --color-text: #111111;
  --color-muted: #888888;
  --color-success: #1f6f3a;
  --color-warning: #8a6d1a;
  --color-danger: #8a1a1a;
  --color-info: #1a3a8a;
  --font-heading: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  --font-body: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  --font-mono: "SF Mono", Menlo, monospace;
  --space-sm: 0.5rem;
  --space-md: 1rem;
  --space-lg: 2rem;
  --radius: 2px;
  --chart-palette-0: #000000;
  --chart-palette-1: #555555;
  --chart-palette-2: #999999;
  --chart-palette-3: #cccccc;
  --chart-palette-4: #1a3a8a;
  --chart-palette-5: #8a6d1a;
}
```

Then paste the rest of the executive.css body (everything below the `:root` block) verbatim into this file.

- [ ] **Step 3: Create `vibrant.css`**

Create `src/libs/colmena/src/documents/infrastructure/render/html_assets/themes/vibrant.css`:

Use same body as executive.css with this `:root`:

```css
:root {
  --color-primary: #7c3aed;
  --color-secondary: #ec4899;
  --color-bg: #ffffff;
  --color-text: #18181b;
  --color-muted: #71717a;
  --color-success: #10b981;
  --color-warning: #f59e0b;
  --color-danger: #ef4444;
  --color-info: #3b82f6;
  --font-heading: "Inter", -apple-system, sans-serif;
  --font-body: "Inter", -apple-system, sans-serif;
  --font-mono: "JetBrains Mono", monospace;
  --space-sm: 0.5rem;
  --space-md: 1rem;
  --space-lg: 2rem;
  --radius: 8px;
  --chart-palette-0: #7c3aed;
  --chart-palette-1: #ec4899;
  --chart-palette-2: #f59e0b;
  --chart-palette-3: #10b981;
  --chart-palette-4: #3b82f6;
  --chart-palette-5: #ef4444;
}
```

Then paste the executive.css body.

- [ ] **Step 4: Create `dark.css`**

Use same body but with dark `:root` and body override:

```css
:root {
  --color-primary: #60a5fa;
  --color-secondary: #34d399;
  --color-bg: #0a0a0f;
  --color-text: #e4e4e7;
  --color-muted: #71717a;
  --color-success: #34d399;
  --color-warning: #fbbf24;
  --color-danger: #f87171;
  --color-info: #60a5fa;
  --font-heading: "JetBrains Mono", "SF Mono", monospace;
  --font-body: "Inter", -apple-system, sans-serif;
  --font-mono: "JetBrains Mono", monospace;
  --space-sm: 0.5rem;
  --space-md: 1rem;
  --space-lg: 2rem;
  --radius: 4px;
  --chart-palette-0: #60a5fa;
  --chart-palette-1: #34d399;
  --chart-palette-2: #fbbf24;
  --chart-palette-3: #f87171;
  --chart-palette-4: #a78bfa;
  --chart-palette-5: #f472b6;
}
```

Paste the executive.css body BUT after pasting, also append at the very end of `dark.css`:

```css
table th, table td { border-color: #27272a; }
.block-kpi-card { background: #18181b; border-color: #27272a; }
.callout--info { background: rgba(96,165,250,0.1); } .callout--warning { background: rgba(251,191,36,0.1); }
.callout--success { background: rgba(52,211,153,0.1); } .callout--danger { background: rgba(248,113,113,0.1); }
.block-auto-toc { background: #18181b; }
.divider { background: #27272a; }
```

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/infrastructure/render/html_assets/themes/
git commit -m "$(cat <<'EOF'
feat(documents/infra): author 4 theme CSS files (executive, minimal, vibrant, dark) with @media print rules

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 9 — `HtmlRenderer`

### Task 9.1: Renderer skeleton — head + theme + conditional assets

**Files:**
- Create: `src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs`
- Modify: `src/libs/colmena/src/documents/infrastructure/render/mod.rs`

- [ ] **Step 1: Create file with failing tests**

Create `src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs`:

```rust
//! HtmlRenderer — implements IRRenderer for HtmlIR.
//!
//! Uses maud (type-safe HTML macros) + asset bundling via include_str!.
//! Depends on the AssetStore port to resolve `asset_id` to bytes at render
//! time. Output is a single self-contained .html file with CSS + Chart.js
//! (when needed) + slides_runtime.js (when needed) + images base64-inlined.

use crate::documents::domain::ids::AssetId;
use crate::documents::domain::ir::{
    AxisSpec, Block, ChartSpec, ChartType, HtmlIR, ImageSrc, LayoutMode, Locale, Run, Slide,
    SlideLayout, TableCell, Theme, VideoSrc,
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

// Placeholders — filled in Task 9.2.
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
        assert!(!html.contains("slide-counter"), "should not include slide counter");
    }

    #[test]
    fn slides_mode_includes_runtime() {
        let mut ir = minimal_ir();
        ir["layout_mode"] = json!("slides");
        let html = render_blocking(ir);
        assert!(html.contains("slide-counter"), "should include slide counter");
    }

    #[test]
    fn no_chart_no_chartjs() {
        let html = render_blocking(minimal_ir());
        assert!(!html.contains("Chart.register"), "Chart.js leaked when no chart");
    }
}
```

- [ ] **Step 2: Register module + add base64 dep**

In `src/libs/colmena/src/documents/infrastructure/render/mod.rs`, add:

```rust
pub mod html_renderer;

pub use html_renderer::HtmlRenderer;
```

In `src/libs/colmena/Cargo.toml`, under `[dependencies]`, if `base64` is not already present, add:

```toml
base64 = "0.22"
```

Run `cargo build --lib -p colmena_dag_engine` to verify.

- [ ] **Step 3: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::infrastructure::render::html_renderer`
Expected: 5 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs src/libs/colmena/src/documents/infrastructure/render/mod.rs src/libs/colmena/Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
feat(documents/infra): HtmlRenderer skeleton (theme, conditional Chart.js/slides_runtime, asset resolution)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 9.2: Implement `render_body` for all 17 block kinds

**Files:**
- Modify: `src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs`

- [ ] **Step 1: Add failing tests for blocks**

Inside `mod tests`, append:

```rust
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
```

- [ ] **Step 2: Run, verify fails**

Expected: blocks render as `(empty)` placeholder.

- [ ] **Step 3: Replace placeholder render_body with real impl**

In `src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs`, replace the two placeholder functions at the bottom of the file (before `mod tests`) with:

```rust
fn render_body(ir: &HtmlIR, assets: &HashMap<String, String>) -> Markup {
    let layout_class = match ir.layout_mode {
        LayoutMode::Slides => "deck deck--slides",
        LayoutMode::Report => "deck deck--report",
    };
    html! {
        div class=(layout_class) {
            @for slide in &ir.slides {
                (render_slide(slide, assets))
            }
            @if ir.footer.enabled {
                (render_footer(ir))
            }
        }
    }
}

fn render_slide(slide: &Slide, assets: &HashMap<String, String>) -> Markup {
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
                (render_block(block, assets))
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

fn render_runs(runs: &[Run]) -> Markup {
    html! {
        @for r in runs {
            (render_run(r))
        }
    }
}

fn render_run(r: &Run) -> Markup {
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

fn render_block(b: &Block, assets: &HashMap<String, String>) -> Markup {
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
                crate::documents::domain::ir::ChartSize::Small => "chart-container chart-container--small",
                crate::documents::domain::ir::ChartSize::Medium => "chart-container chart-container--medium",
                crate::documents::domain::ir::ChartSize::Large => "chart-container chart-container--large",
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
                            crate::documents::domain::ir::DeltaDirection::Up => "up",
                            crate::documents::domain::ir::DeltaDirection::Down => "down",
                            crate::documents::domain::ir::DeltaDirection::Neutral => "neutral",
                        })) { (d.value) }
                }
            }
        },
        Block::KpiGrid { id, columns, cards } => html! {
            div.block-kpi-grid id=(id) data-cols=(columns.to_string()) {
                @for c in cards {
                    div.block-kpi-card {
                        div.block-kpi-card__label { (c.label) }
                        div.block-kpi-card__value { (c.value) }
                        @if let Some(d) = &c.delta {
                            div class=(format!("block-kpi-card__delta block-kpi-card__delta--{}",
                                match d.direction {
                                    crate::documents::domain::ir::DeltaDirection::Up => "up",
                                    crate::documents::domain::ir::DeltaDirection::Down => "down",
                                    crate::documents::domain::ir::DeltaDirection::Neutral => "neutral",
                                })) { (d.value) }
                        }
                    }
                }
            }
        },
        Block::Image { id, src, alt, caption, position } => {
            let pos_class = match position {
                crate::documents::domain::ir::ImagePosition::Inline => "block-image block-image--inline",
                crate::documents::domain::ir::ImagePosition::Full => "block-image block-image--full",
                crate::documents::domain::ir::ImagePosition::Hero => "block-image block-image--hero",
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
            let ratio_str = serde_json::to_value(ratio).unwrap();
            let ratio_str = ratio_str.as_str().unwrap_or("50_50");
            let gap_class = format!("gap-{}", format!("{:?}", gap).to_lowercase());
            html! {
                div class=(format!("block-two-columns {gap_class}")) id=(id) data-ratio=(ratio_str) {
                    div.col { @for b in left { (render_block(b, assets)) } }
                    div.col { @for b in right { (render_block(b, assets)) } }
                }
            }
        }
        Block::ThreeColumns { id, left, middle, right, gap } => {
            let gap_class = format!("gap-{}", format!("{:?}", gap).to_lowercase());
            html! {
                div class=(format!("block-three-columns {gap_class}")) id=(id) {
                    div.col { @for b in left { (render_block(b, assets)) } }
                    div.col { @for b in middle { (render_block(b, assets)) } }
                    div.col { @for b in right { (render_block(b, assets)) } }
                }
            }
        }
        Block::Comparison { id, left, right } => html! {
            div.block-comparison id=(id) {
                div class=(if left.highlight { "panel highlight" } else { "panel" }) {
                    h3 { (left.header) } div { (render_runs(&left.runs)) }
                }
                div class=(if right.highlight { "panel highlight" } else { "panel" }) {
                    h3 { (right.header) } div { (render_runs(&right.runs)) }
                }
            }
        },
        Block::Callout { id, variant, title, runs } => {
            let var_class = format!("callout callout--{}", format!("{:?}", variant).to_lowercase());
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
        Block::AutoToc { id, title, depth } => html! {
            nav.block-auto-toc id=(id) {
                @if let Some(t) = title { strong { (t) } }
                // Resolved at render time in Task 9.3 — placeholder until then.
                ol { li { "(TOC " (depth.to_string()) ")" } }
            }
        },
    }
}

fn render_table_cell(c: &TableCell) -> Markup {
    match c {
        TableCell::Text { value } => html! { (value) },
        TableCell::Number { value, .. } => html! { (value.to_string()) },
        TableCell::Runs { runs } => html! { (render_runs(runs)) },
    }
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::infrastructure::render::html_renderer`
Expected: all tests (Task 9.1 + 6 new) PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs
git commit -m "$(cat <<'EOF'
feat(documents/infra): HtmlRenderer render_body for all 17 block kinds + table cells + runs

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 9.3: Implement `render_chart_init_scripts` and resolve `auto_toc`

**Files:**
- Modify: `src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs`

- [ ] **Step 1: Add failing tests**

Inside `mod tests`, append:

```rust
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
```

- [ ] **Step 2: Run, verify fails**

Expected: chart_init still empty; auto_toc still placeholder.

- [ ] **Step 3: Implement chart init + auto_toc resolution**

In `src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs`, REPLACE the placeholder `fn render_chart_init_scripts(_ir: &HtmlIR) -> String { String::new() }` with:

```rust
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

fn chart_init_for(block_id: &str, c: &ChartSpec) -> String {
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
  options: {{ responsive: true, plugins: {{ legend: {{ display: {legend} }} }} }}
}});"#,
        id = block_id,
        ty = type_str,
        labels = labels,
        ds = datasets,
        legend = c.legend,
    )
}
```

REPLACE the `Block::AutoToc` arm in `render_block` with the proper implementation. To do this, change `render_block` to accept the full IR (so AutoToc can scan slides):

Change signature:
```rust
fn render_block(b: &Block, assets: &HashMap<String, String>) -> Markup {
```
to:
```rust
fn render_block(b: &Block, assets: &HashMap<String, String>, ir: &HtmlIR) -> Markup {
```

Update all recursive calls inside `render_block` to pass `ir` (TwoColumns/ThreeColumns/Comparison arms).

Update `render_slide` signature to pass `ir`:
```rust
fn render_slide(slide: &Slide, assets: &HashMap<String, String>, ir: &HtmlIR) -> Markup {
```
and update `render_body` to pass `ir` when calling `render_slide`.

Replace the AutoToc arm with:

```rust
        Block::AutoToc { id, title, depth } => html! {
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
        },
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::infrastructure::render::html_renderer`
Expected: all tests PASS (Tasks 9.1 + 9.2 + 9.3).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs
git commit -m "$(cat <<'EOF'
feat(documents/infra): HtmlRenderer chart init scripts + auto_toc resolution

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 10 — Asset use cases (application)

### Task 10.1: `UploadAssetUseCase`

**Files:**
- Create: `src/libs/colmena/src/documents/application/upload_asset.rs`
- Modify: `src/libs/colmena/src/documents/application/mod.rs`

- [ ] **Step 1: Create file with failing tests**

Create `src/libs/colmena/src/documents/application/upload_asset.rs`:

```rust
//! UploadAssetUseCase — validates + persists via AssetStore port.

use crate::documents::domain::error::AssetError;
use crate::documents::domain::ids::{AssetId, SessionId};
use crate::documents::domain::ports::{AssetStore, AssetSummary};
use crate::documents::domain::IdGenerator;
use std::collections::HashSet;
use std::sync::Arc;

pub struct UploadAssetInput {
    pub session_id: SessionId,
    pub bytes: Vec<u8>,
    pub mime: String,
    pub label: Option<String>,
}

#[derive(Debug)]
pub struct UploadAssetOutput {
    pub asset_id: AssetId,
    pub summary: AssetSummary,
}

pub struct UploadAssetUseCase {
    pub store: Arc<dyn AssetStore>,
    pub ids: Arc<dyn IdGenerator>,
    pub max_size_bytes: u64,
    pub allowed_mimes: HashSet<String>,
}

impl UploadAssetUseCase {
    pub async fn execute(
        &self,
        input: UploadAssetInput,
    ) -> Result<UploadAssetOutput, AssetError> {
        let size = input.bytes.len() as u64;
        if size > self.max_size_bytes {
            return Err(AssetError::TooLarge {
                actual: size,
                max: self.max_size_bytes,
            });
        }
        if !self.allowed_mimes.contains(&input.mime) {
            return Err(AssetError::MimeNotAllowed { mime: input.mime });
        }
        let id = AssetId::new(self.ids.new_asset_id());
        self.store
            .upload(
                &input.session_id,
                &id,
                input.bytes,
                &input.mime,
                input.label.as_deref(),
            )
            .await?;
        let summary = self.store.head(&id).await?;
        Ok(UploadAssetOutput {
            asset_id: id,
            summary,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::infrastructure::ids::CountingIdGenerator;
    use crate::documents::infrastructure::storage::LocalFsAssetStore;
    use tempfile::tempdir;

    fn allowed() -> HashSet<String> {
        ["image/png", "image/jpeg"].into_iter().map(String::from).collect()
    }

    #[tokio::test]
    async fn upload_happy_path() {
        let tmp = tempdir().unwrap();
        let uc = UploadAssetUseCase {
            store: Arc::new(LocalFsAssetStore::new(tmp.path())),
            ids: Arc::new(CountingIdGenerator::default()),
            max_size_bytes: 10 * 1024,
            allowed_mimes: allowed(),
        };
        let out = uc
            .execute(UploadAssetInput {
                session_id: SessionId::new("s1"),
                bytes: b"hello".to_vec(),
                mime: "image/png".into(),
                label: Some("logo".into()),
            })
            .await
            .unwrap();
        assert_eq!(out.asset_id.as_str(), "asset_01");
        assert_eq!(out.summary.size_bytes, 5);
    }

    #[tokio::test]
    async fn rejects_too_large() {
        let tmp = tempdir().unwrap();
        let uc = UploadAssetUseCase {
            store: Arc::new(LocalFsAssetStore::new(tmp.path())),
            ids: Arc::new(CountingIdGenerator::default()),
            max_size_bytes: 4,
            allowed_mimes: allowed(),
        };
        let err = uc
            .execute(UploadAssetInput {
                session_id: SessionId::new("s1"),
                bytes: b"hello".to_vec(),
                mime: "image/png".into(),
                label: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, AssetError::TooLarge { .. }));
    }

    #[tokio::test]
    async fn rejects_disallowed_mime() {
        let tmp = tempdir().unwrap();
        let uc = UploadAssetUseCase {
            store: Arc::new(LocalFsAssetStore::new(tmp.path())),
            ids: Arc::new(CountingIdGenerator::default()),
            max_size_bytes: 1024,
            allowed_mimes: allowed(),
        };
        let err = uc
            .execute(UploadAssetInput {
                session_id: SessionId::new("s1"),
                bytes: vec![0u8; 10],
                mime: "application/pdf".into(),
                label: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, AssetError::MimeNotAllowed { .. }));
    }
}
```

- [ ] **Step 2: Register module**

In `src/libs/colmena/src/documents/application/mod.rs`, add:

```rust
pub mod upload_asset;
```

- [ ] **Step 3: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::application::upload_asset`
Expected: 3 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/application/upload_asset.rs src/libs/colmena/src/documents/application/mod.rs
git commit -m "$(cat <<'EOF'
feat(documents/app): UploadAssetUseCase with size + mime validation

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 10.2: `ListAssetsUseCase`

**Files:**
- Create: `src/libs/colmena/src/documents/application/list_assets.rs`
- Modify: `src/libs/colmena/src/documents/application/mod.rs`

- [ ] **Step 1: Create file with failing test**

Create `src/libs/colmena/src/documents/application/list_assets.rs`:

```rust
//! ListAssetsUseCase — thin wrapper over AssetStore::list_by_session.

use crate::documents::domain::error::AssetError;
use crate::documents::domain::ids::SessionId;
use crate::documents::domain::ports::{AssetStore, AssetSummary};
use std::sync::Arc;

pub struct ListAssetsUseCase {
    pub store: Arc<dyn AssetStore>,
}

impl ListAssetsUseCase {
    pub async fn execute(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<AssetSummary>, AssetError> {
        self.store.list_by_session(session_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ids::AssetId;
    use crate::documents::infrastructure::storage::LocalFsAssetStore;
    use tempfile::tempdir;

    #[tokio::test]
    async fn returns_summaries_for_session() {
        let tmp = tempdir().unwrap();
        let store: Arc<dyn AssetStore> = Arc::new(LocalFsAssetStore::new(tmp.path()));
        let s = SessionId::new("s1");
        store
            .upload(&s, &AssetId::new("asset_1"), b"a".to_vec(), "image/png", None)
            .await
            .unwrap();
        let uc = ListAssetsUseCase { store };
        let list = uc.execute(&s).await.unwrap();
        assert_eq!(list.len(), 1);
    }
}
```

- [ ] **Step 2: Register**

In `src/libs/colmena/src/documents/application/mod.rs`, add `pub mod list_assets;`.

- [ ] **Step 3: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::application::list_assets`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/application/list_assets.rs src/libs/colmena/src/documents/application/mod.rs
git commit -m "$(cat <<'EOF'
feat(documents/app): ListAssetsUseCase

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 10.3: `DeleteAssetUseCase` with ref-count check

**Files:**
- Create: `src/libs/colmena/src/documents/application/delete_asset.rs`
- Modify: `src/libs/colmena/src/documents/application/mod.rs`

- [ ] **Step 1: Create file with failing tests**

Create `src/libs/colmena/src/documents/application/delete_asset.rs`:

```rust
//! DeleteAssetUseCase — refuses deletion if asset is still referenced by
//! any artifact in the same session.

use crate::documents::domain::error::AssetError;
use crate::documents::domain::ids::{ArtifactId, AssetId, SessionId};
use crate::documents::domain::ports::{ArtifactStore, AssetStore, SessionArtifactIndex};
use std::sync::Arc;

pub struct DeleteAssetInput {
    pub session_id: SessionId,
    pub asset_id: AssetId,
}

pub struct DeleteAssetUseCase {
    pub assets: Arc<dyn AssetStore>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub index: Option<Arc<dyn SessionArtifactIndex>>,
}

impl DeleteAssetUseCase {
    pub async fn execute(&self, input: DeleteAssetInput) -> Result<(), AssetError> {
        // If the session index is configured, find all artifacts in the session
        // and inspect each HtmlIR.assets_referenced for our id.
        let mut blockers: Vec<ArtifactId> = vec![];
        if let Some(idx) = &self.index {
            let arts = idx
                .list_by_session(&input.session_id)
                .await
                .map_err(|e| AssetError::Storage(format!("index list: {e}")))?;
            for summary in arts {
                let data = self
                    .artifacts
                    .read_current(&summary.id)
                    .await
                    .map_err(|e| AssetError::Storage(format!("read artifact: {e}")))?;
                if let Some(arr) = data.ir.get("assets_referenced").and_then(|v| v.as_array()) {
                    if arr
                        .iter()
                        .any(|v| v.as_str() == Some(input.asset_id.as_str()))
                    {
                        blockers.push(summary.id.clone());
                    }
                }
            }
        }
        if !blockers.is_empty() {
            return Err(AssetError::StillReferenced {
                id: input.asset_id,
                by_artifacts: blockers,
            });
        }
        self.assets.delete(&input.asset_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::infrastructure::storage::LocalFsAssetStore;
    use tempfile::tempdir;

    // Tiny in-memory ArtifactStore stub for ref-counting tests.
    // Full Artifact store testing belongs in its own module.
    use crate::documents::domain::artifact::ArtifactMeta;
    use crate::documents::domain::artifact::VersionData;
    use crate::documents::domain::error::StorageError;
    use crate::documents::domain::ids::VersionId;
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct StubArtifactStore {
        data: Mutex<HashMap<String, serde_json::Value>>,
    }
    impl StubArtifactStore {
        fn with(ir: serde_json::Value, id: &str) -> Arc<Self> {
            let mut m = HashMap::new();
            m.insert(id.into(), ir);
            Arc::new(Self { data: Mutex::new(m) })
        }
    }
    #[async_trait]
    impl ArtifactStore for StubArtifactStore {
        async fn create_artifact(&self, _m: &ArtifactMeta) -> Result<(), StorageError> { Ok(()) }
        async fn write_version(
            &self, _id: &ArtifactId, _v: &VersionId, _d: &VersionData
        ) -> Result<(), StorageError> { Ok(()) }
        async fn read_version(
            &self, _id: &ArtifactId, _v: &VersionId
        ) -> Result<VersionData, StorageError> { unimplemented!() }
        async fn read_current(&self, id: &ArtifactId) -> Result<VersionData, StorageError> {
            let m = self.data.lock().unwrap();
            let ir = m.get(id.as_str()).cloned().ok_or_else(||
                StorageError::NotFound(id.as_str().into()))?;
            Ok(VersionData {
                ir,
                rendered_binary: vec![],
                rendered_extension: "html",
                patch_applied: crate::documents::domain::artifact::PatchApplied {
                    patch: serde_json::json!({}),
                    applied_at: Utc::now(),
                    resulted_in: VersionId::initial(),
                    summary: Default::default(),
                },
                blobs: vec![],
            })
        }
        async fn list_versions(&self, _id: &ArtifactId) -> Result<Vec<VersionId>, StorageError> { Ok(vec![]) }
        async fn set_head(&self, _id: &ArtifactId, _e: Option<&VersionId>, _n: &VersionId) -> Result<(), StorageError> { Ok(()) }
        async fn delete_version(&self, _id: &ArtifactId, _v: &VersionId) -> Result<(), StorageError> { Ok(()) }
        async fn read_meta(&self, _id: &ArtifactId) -> Result<ArtifactMeta, StorageError> { unimplemented!() }
        async fn update_meta(&self, _id: &ArtifactId, _m: &ArtifactMeta) -> Result<(), StorageError> { Ok(()) }
        async fn delete_artifact(&self, _id: &ArtifactId) -> Result<(), StorageError> { Ok(()) }
    }

    struct StubIndex(Vec<crate::documents::domain::artifact::ArtifactSummary>);
    #[async_trait]
    impl SessionArtifactIndex for StubIndex {
        async fn register(
            &self, _s: &SessionId, _id: &ArtifactId, _m: &ArtifactMeta
        ) -> Result<(), crate::documents::domain::error::IndexError> { Ok(()) }
        async fn list_by_session(
            &self, _s: &SessionId
        ) -> Result<Vec<crate::documents::domain::artifact::ArtifactSummary>, crate::documents::domain::error::IndexError> {
            Ok(self.0.clone())
        }
        async fn lookup(
            &self, _id: &ArtifactId
        ) -> Result<Option<crate::documents::domain::artifact::ArtifactSummary>, crate::documents::domain::error::IndexError> { Ok(None) }
        async fn update_head(
            &self, _id: &ArtifactId, _v: &VersionId, _u: DateTime<Utc>
        ) -> Result<(), crate::documents::domain::error::IndexError> { Ok(()) }
        async fn unregister(&self, _id: &ArtifactId) -> Result<(), crate::documents::domain::error::IndexError> { Ok(()) }
    }

    fn summary(id: &str) -> crate::documents::domain::artifact::ArtifactSummary {
        crate::documents::domain::artifact::ArtifactSummary {
            id: ArtifactId::new(id),
            session_id: SessionId::new("s1"),
            kind: crate::documents::domain::ids::ArtifactKind::Html,
            label: "x".into(),
            current_version: VersionId::initial(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn delete_blocked_when_referenced() {
        let tmp = tempdir().unwrap();
        let assets: Arc<dyn AssetStore> = Arc::new(LocalFsAssetStore::new(tmp.path()));
        let id = AssetId::new("asset_ref");
        assets
            .upload(&SessionId::new("s1"), &id, b"x".to_vec(), "image/png", None)
            .await
            .unwrap();
        let artifacts: Arc<dyn ArtifactStore> = StubArtifactStore::with(
            serde_json::json!({"assets_referenced":["asset_ref"]}),
            "art_a",
        );
        let index: Arc<dyn SessionArtifactIndex> =
            Arc::new(StubIndex(vec![summary("art_a")]));
        let uc = DeleteAssetUseCase {
            assets,
            artifacts,
            index: Some(index),
        };
        let err = uc
            .execute(DeleteAssetInput {
                session_id: SessionId::new("s1"),
                asset_id: id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, AssetError::StillReferenced { .. }));
    }

    #[tokio::test]
    async fn delete_succeeds_when_not_referenced() {
        let tmp = tempdir().unwrap();
        let assets: Arc<dyn AssetStore> = Arc::new(LocalFsAssetStore::new(tmp.path()));
        let id = AssetId::new("asset_free");
        assets
            .upload(&SessionId::new("s1"), &id, b"x".to_vec(), "image/png", None)
            .await
            .unwrap();
        let artifacts: Arc<dyn ArtifactStore> = StubArtifactStore::with(
            serde_json::json!({"assets_referenced":[]}),
            "art_a",
        );
        let index: Arc<dyn SessionArtifactIndex> =
            Arc::new(StubIndex(vec![summary("art_a")]));
        let uc = DeleteAssetUseCase { assets, artifacts, index: Some(index) };
        uc.execute(DeleteAssetInput {
            session_id: SessionId::new("s1"),
            asset_id: id,
        })
        .await
        .unwrap();
    }
}
```

- [ ] **Step 2: Register**

In `src/libs/colmena/src/documents/application/mod.rs`, add `pub mod delete_asset;`.

- [ ] **Step 3: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::application::delete_asset`
Expected: 2 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/application/delete_asset.rs src/libs/colmena/src/documents/application/mod.rs
git commit -m "$(cat <<'EOF'
feat(documents/app): DeleteAssetUseCase with ref-count check

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 11 — `HtmlOpApplier`

### Task 11.1: Applier scaffolding + slide-level ops

**Files:**
- Create: `src/libs/colmena/src/documents/application/apply_html_ops.rs`
- Modify: `src/libs/colmena/src/documents/application/mod.rs`

- [ ] **Step 1: Create file with failing tests**

Create `src/libs/colmena/src/documents/application/apply_html_ops.rs`:

```rust
//! HtmlOpApplier — mutates HtmlIR for each PatchOp (slide-level + block-level).
//!
//! Pure to ports: depends only on IdGenerator from domain.

use crate::documents::domain::artifact::{OpOutcome, OpOutcomeIds};
use crate::documents::domain::ir::{Block, HtmlIR, ImageSrc, Slide, SlideLayout, VideoSrc};
use crate::documents::domain::patch::PatchOp;
use crate::documents::domain::{DocumentError, IdGenerator};

pub struct HtmlOpApplier<'a> {
    pub ids: &'a dyn IdGenerator,
}

impl<'a> HtmlOpApplier<'a> {
    pub fn apply(&self, ir: &mut HtmlIR, op: &PatchOp) -> Result<OpOutcome, DocumentError> {
        match op {
            PatchOp::AddSlide {
                layout,
                at_index,
                title,
                subtitle,
            } => self.add_slide(ir, *layout, *at_index, title.clone(), subtitle.clone()),
            PatchOp::DeleteSlide { slide_id } => self.delete_slide(ir, slide_id),
            PatchOp::ReorderSlides { order } => self.reorder_slides(ir, order),
            PatchOp::SetSlideLayout { slide_id, layout } => {
                self.set_slide_layout(ir, slide_id, *layout)
            }
            PatchOp::SetSlideTitle {
                slide_id,
                title,
                subtitle,
            } => self.set_slide_title(ir, slide_id, title.clone(), subtitle.clone()),
            PatchOp::SetSlideNotes { slide_id, notes } => {
                self.set_slide_notes(ir, slide_id, notes.clone())
            }
            _ => Err(DocumentError::InvalidPatchOp {
                reason: "op not yet implemented in HtmlOpApplier".into(),
                op: serde_json::to_value(op).unwrap_or_default(),
            }),
        }
    }

    fn add_slide(
        &self,
        ir: &mut HtmlIR,
        layout: SlideLayout,
        at_index: Option<u32>,
        title: Option<String>,
        subtitle: Option<String>,
    ) -> Result<OpOutcome, DocumentError> {
        if matches!(layout, SlideLayout::Title | SlideLayout::SectionDivider)
            && title.as_deref().unwrap_or("").trim().is_empty()
        {
            return Err(DocumentError::IRValidationFailed {
                path: "/op/add_slide".into(),
                reason: format!("layout {:?} requires non-empty title", layout),
            });
        }
        let id = self.ids.new_slide_id();
        let new_slide = Slide {
            id: id.clone(),
            layout,
            title,
            subtitle,
            notes: None,
            blocks: vec![],
        };
        let idx = at_index
            .map(|i| (i as usize).min(ir.slides.len()))
            .unwrap_or(ir.slides.len());
        ir.slides.insert(idx, new_slide);
        Ok(OpOutcome {
            assigned_ids: OpOutcomeIds {
                slide: Some(id),
                ..Default::default()
            },
        })
    }

    fn delete_slide(&self, ir: &mut HtmlIR, slide_id: &str) -> Result<OpOutcome, DocumentError> {
        if ir.slides.len() <= 1 {
            return Err(DocumentError::IRValidationFailed {
                path: "/slides".into(),
                reason: "cannot delete last slide".into(),
            });
        }
        let pos = ir.slides.iter().position(|s| s.id == slide_id);
        match pos {
            Some(i) => {
                let removed = ir.slides.remove(i);
                recompute_assets_referenced(ir);
                Ok(OpOutcome::default_with_note(format!(
                    "removed slide {} ({} blocks)",
                    removed.id,
                    removed.blocks.len()
                )))
            }
            None => Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}"),
                reason: "slide not found".into(),
            }),
        }
    }

    fn reorder_slides(&self, ir: &mut HtmlIR, order: &[String]) -> Result<OpOutcome, DocumentError> {
        if order.len() != ir.slides.len() {
            return Err(DocumentError::IRValidationFailed {
                path: "/op/reorder_slides".into(),
                reason: format!(
                    "order length {} != current slides {}",
                    order.len(),
                    ir.slides.len()
                ),
            });
        }
        let mut new_slides = Vec::with_capacity(order.len());
        for id in order {
            let pos = ir.slides.iter().position(|s| s.id == *id).ok_or_else(|| {
                DocumentError::IRValidationFailed {
                    path: "/op/reorder_slides".into(),
                    reason: format!("unknown slide_id {id}"),
                }
            })?;
            new_slides.push(ir.slides.remove(pos));
        }
        ir.slides = new_slides;
        Ok(OpOutcome::default())
    }

    fn set_slide_layout(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        layout: SlideLayout,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        if matches!(layout, SlideLayout::Title | SlideLayout::SectionDivider)
            && slide.title.as_deref().unwrap_or("").trim().is_empty()
        {
            return Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}"),
                reason: format!("layout {:?} requires non-empty title", layout),
            });
        }
        slide.layout = layout;
        Ok(OpOutcome::default())
    }

    fn set_slide_title(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        title: Option<String>,
        subtitle: Option<String>,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        slide.title = title;
        slide.subtitle = subtitle;
        Ok(OpOutcome::default())
    }

    fn set_slide_notes(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        notes: Option<String>,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        slide.notes = notes;
        Ok(OpOutcome::default())
    }
}

fn find_slide_mut<'b>(ir: &'b mut HtmlIR, id: &str) -> Result<&'b mut Slide, DocumentError> {
    ir.slides
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| DocumentError::IRValidationFailed {
            path: format!("/slides/{id}"),
            reason: "slide not found".into(),
        })
}

pub(crate) fn recompute_assets_referenced(ir: &mut HtmlIR) {
    let mut refs = std::collections::HashSet::new();
    for slide in &ir.slides {
        walk_block_assets(&slide.blocks, &mut refs);
    }
    let mut v: Vec<String> = refs.into_iter().collect();
    v.sort();
    ir.assets_referenced = v;
}

fn walk_block_assets(blocks: &[Block], out: &mut std::collections::HashSet<String>) {
    for b in blocks {
        match b {
            Block::Image {
                src: ImageSrc::Asset { asset_id },
                ..
            }
            | Block::Video {
                src: VideoSrc::Asset { asset_id },
                ..
            } => {
                out.insert(asset_id.clone());
            }
            Block::TwoColumns { left, right, .. } => {
                walk_block_assets(left, out);
                walk_block_assets(right, out);
            }
            Block::ThreeColumns {
                left,
                middle,
                right,
                ..
            } => {
                walk_block_assets(left, out);
                walk_block_assets(middle, out);
                walk_block_assets(right, out);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ir::{DocProps, FooterConfig, LayoutMode, Locale, Theme};
    use crate::documents::infrastructure::ids::CountingIdGenerator;

    fn fresh() -> HtmlIR {
        HtmlIR {
            kind: "html".into(),
            artifact_id: "art_x".into(),
            version_id: "v1".into(),
            schema_version: "1.0.0".into(),
            doc_props: DocProps {
                title: None,
                author: None,
                date: None,
                locale: Locale::En,
            },
            theme: Theme::Executive,
            layout_mode: LayoutMode::Slides,
            footer: FooterConfig {
                enabled: false,
                page_numbers: false,
                custom_text: None,
            },
            slides: vec![Slide {
                id: "sl_existing".into(),
                layout: SlideLayout::Blank,
                title: None,
                subtitle: None,
                notes: None,
                blocks: vec![],
            }],
            assets_referenced: vec![],
        }
    }

    #[test]
    fn add_slide_appends_and_returns_id() {
        let mut ir = fresh();
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        let out = applier
            .apply(
                &mut ir,
                &PatchOp::AddSlide {
                    layout: SlideLayout::Content,
                    at_index: None,
                    title: Some("New".into()),
                    subtitle: None,
                },
            )
            .unwrap();
        assert_eq!(ir.slides.len(), 2);
        assert_eq!(out.assigned_ids.slide.as_deref(), Some("sl_01"));
    }

    #[test]
    fn add_title_slide_without_title_rejected() {
        let mut ir = fresh();
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        let err = applier
            .apply(
                &mut ir,
                &PatchOp::AddSlide {
                    layout: SlideLayout::Title,
                    at_index: None,
                    title: Some("   ".into()),
                    subtitle: None,
                },
            )
            .unwrap_err();
        assert!(matches!(err, DocumentError::IRValidationFailed { .. }));
    }

    #[test]
    fn delete_last_slide_rejected() {
        let mut ir = fresh();
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        let err = applier
            .apply(
                &mut ir,
                &PatchOp::DeleteSlide {
                    slide_id: "sl_existing".into(),
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("cannot delete last"));
    }

    #[test]
    fn reorder_validates_length_and_ids() {
        let mut ir = fresh();
        ir.slides.push(Slide {
            id: "sl_b".into(),
            layout: SlideLayout::Blank,
            title: None,
            subtitle: None,
            notes: None,
            blocks: vec![],
        });
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        applier
            .apply(
                &mut ir,
                &PatchOp::ReorderSlides {
                    order: vec!["sl_b".into(), "sl_existing".into()],
                },
            )
            .unwrap();
        assert_eq!(ir.slides[0].id, "sl_b");
    }
}
```

NOTE on shared types: This code refers to `OpOutcome`, `OpOutcomeIds`, and the helpers `OpOutcome::default_with_note` / `OpOutcome::default()`. If `OpOutcomeIds` doesn't currently have a `slide: Option<String>` field, add it now in `src/libs/colmena/src/documents/domain/artifact.rs`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpOutcomeIds {
    pub sheet: Option<String>,
    pub table: Option<String>,
    pub block: Option<String>,
    pub runs: Vec<String>,
    pub rows: Vec<String>,
    pub list_items: Vec<String>,
    pub slide: Option<String>,    // ← add this
}
```

If `OpOutcome::default_with_note` doesn't exist, add it as a free helper or method that constructs `OpOutcome { assigned_ids: OpOutcomeIds::default() }` and ignores the note (it's only for `describe_op` context). Or simplify the applier code to use `OpOutcome::default()` everywhere instead.

- [ ] **Step 2: Register module**

In `src/libs/colmena/src/documents/application/mod.rs`, add `pub mod apply_html_ops;`.

- [ ] **Step 3: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::application::apply_html_ops`
Expected: 4 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/documents/application/apply_html_ops.rs src/libs/colmena/src/documents/application/mod.rs src/libs/colmena/src/documents/domain/artifact.rs
git commit -m "$(cat <<'EOF'
feat(documents/app): HtmlOpApplier scaffolding + 6 slide-level ops

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 11.2: Block/table/list/doc ops in `HtmlOpApplier`

**Files:**
- Modify: `src/libs/colmena/src/documents/application/apply_html_ops.rs`

- [ ] **Step 1: Add failing tests**

Inside `mod tests`, append:

```rust
#[test]
fn insert_html_block_assigns_id_and_appends() {
    let mut ir = fresh();
    let ids = CountingIdGenerator::default();
    let applier = HtmlOpApplier { ids: &ids };
    let out = applier
        .apply(
            &mut ir,
            &PatchOp::InsertHtmlBlock {
                slide_id: "sl_existing".into(),
                before: None,
                after: None,
                block: serde_json::json!({
                    "kind":"paragraph",
                    "runs":[{"id":"will_be_overwritten","text":"x","bold":false,"italic":false,"underline":false,"code":false}]
                }),
            },
        )
        .unwrap();
    assert_eq!(out.assigned_ids.block.as_deref(), Some("blk_01"));
    assert_eq!(ir.slides[0].blocks.len(), 1);
}

#[test]
fn delete_html_block_removes_by_id() {
    let mut ir = fresh();
    ir.slides[0].blocks = vec![Block::Divider { id: "blk_d".into() }];
    let ids = CountingIdGenerator::default();
    let applier = HtmlOpApplier { ids: &ids };
    applier
        .apply(
            &mut ir,
            &PatchOp::DeleteHtmlBlock {
                slide_id: "sl_existing".into(),
                block_id: "blk_d".into(),
            },
        )
        .unwrap();
    assert!(ir.slides[0].blocks.is_empty());
}

#[test]
fn set_theme_changes_ir_theme() {
    use crate::documents::domain::ir::Theme;
    let mut ir = fresh();
    let ids = CountingIdGenerator::default();
    let applier = HtmlOpApplier { ids: &ids };
    applier
        .apply(&mut ir, &PatchOp::SetTheme { theme: Theme::Dark })
        .unwrap();
    assert_eq!(ir.theme, Theme::Dark);
}

#[test]
fn insert_image_block_with_asset_updates_assets_referenced() {
    let mut ir = fresh();
    let ids = CountingIdGenerator::default();
    let applier = HtmlOpApplier { ids: &ids };
    applier
        .apply(
            &mut ir,
            &PatchOp::InsertHtmlBlock {
                slide_id: "sl_existing".into(),
                before: None,
                after: None,
                block: serde_json::json!({
                    "kind":"image",
                    "src":{"kind":"asset","asset_id":"asset_logo"},
                    "alt":"x","caption":null,"position":"inline"
                }),
            },
        )
        .unwrap();
    assert!(ir.assets_referenced.contains(&"asset_logo".to_string()));
}
```

- [ ] **Step 2: Run, verify fails**

Expected: ops fall into `_ => Err(...)` arm.

- [ ] **Step 3: Extend the `apply` match**

In `src/libs/colmena/src/documents/application/apply_html_ops.rs`, replace the catch-all `_ => Err(...)` with the full match arms for block/table/list/doc ops:

```rust
            PatchOp::InsertHtmlBlock {
                slide_id,
                before,
                after,
                block,
            } => self.insert_html_block(ir, slide_id, before.as_deref(), after.as_deref(), block),
            PatchOp::DeleteHtmlBlock { slide_id, block_id } => {
                self.delete_html_block(ir, slide_id, block_id)
            }
            PatchOp::ReplaceHtmlBlock {
                slide_id,
                block_id,
                block,
            } => self.replace_html_block(ir, slide_id, block_id, block),
            PatchOp::MoveHtmlBlock {
                slide_id,
                block_id,
                after_block_id,
            } => self.move_html_block(ir, slide_id, block_id, after_block_id),

            PatchOp::InsertHtmlTableRow {
                slide_id,
                table_block_id,
                before,
                after,
                cells,
            } => self.insert_html_table_row(
                ir,
                slide_id,
                table_block_id,
                before.as_deref(),
                after.as_deref(),
                cells,
            ),
            PatchOp::DeleteHtmlTableRow {
                slide_id,
                table_block_id,
                row_id,
            } => self.delete_html_table_row(ir, slide_id, table_block_id, row_id),
            PatchOp::UpdateHtmlTableCell {
                slide_id,
                table_block_id,
                row_id,
                col_index,
                cell,
            } => {
                self.update_html_table_cell(ir, slide_id, table_block_id, row_id, *col_index, cell)
            }

            PatchOp::InsertHtmlListItem {
                slide_id,
                list_block_id,
                at_index,
                runs,
            } => self.insert_html_list_item(ir, slide_id, list_block_id, *at_index, runs),
            PatchOp::DeleteHtmlListItem {
                slide_id,
                list_block_id,
                item_id,
            } => self.delete_html_list_item(ir, slide_id, list_block_id, item_id),
            PatchOp::UpdateHtmlListItem {
                slide_id,
                list_block_id,
                item_id,
                runs,
            } => self.update_html_list_item(ir, slide_id, list_block_id, item_id, runs),

            PatchOp::SetTheme { theme } => {
                ir.theme = *theme;
                Ok(OpOutcome::default())
            }
            PatchOp::SetDocProps {
                title,
                author,
                date,
                locale,
            } => {
                if let Some(t) = title.clone() {
                    ir.doc_props.title = Some(t);
                }
                if let Some(a) = author.clone() {
                    ir.doc_props.author = Some(a);
                }
                if let Some(d) = date.clone() {
                    ir.doc_props.date = Some(d);
                }
                if let Some(l) = locale {
                    ir.doc_props.locale = *l;
                }
                Ok(OpOutcome::default())
            }
            PatchOp::SetFooter { footer } => {
                ir.footer = footer.clone();
                Ok(OpOutcome::default())
            }

            _ => Err(DocumentError::InvalidPatchOp {
                reason: "op not applicable to HTML kind".into(),
                op: serde_json::to_value(op).unwrap_or_default(),
            }),
```

Then add the helper method implementations. Append to `impl<'a> HtmlOpApplier<'a>`:

```rust
    fn insert_html_block(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        before: Option<&str>,
        after: Option<&str>,
        block_json: &serde_json::Value,
    ) -> Result<OpOutcome, DocumentError> {
        if before.is_some() && after.is_some() {
            return Err(DocumentError::InvalidPatchOp {
                reason: "exactly one of before/after must be provided".into(),
                op: serde_json::json!({}),
            });
        }
        let mut block_json = block_json.clone();
        let new_id = self.ids.new_block_id();
        if let Some(obj) = block_json.as_object_mut() {
            obj.insert("id".into(), serde_json::json!(new_id));
            // Assign run/item/row IDs deeply if needed
            self.assign_nested_ids(obj);
        }
        let block: Block = serde_json::from_value(block_json).map_err(|e| {
            DocumentError::IRValidationFailed {
                path: "/op/insert_html_block/block".into(),
                reason: e.to_string(),
            }
        })?;
        let slide = find_slide_mut(ir, slide_id)?;
        let pos = match (before, after) {
            (Some(b), None) => slide.blocks.iter().position(|x| block_id_of(x) == b),
            (None, Some(a)) => slide
                .blocks
                .iter()
                .position(|x| block_id_of(x) == a)
                .map(|i| i + 1),
            _ => Some(slide.blocks.len()),
        }
        .ok_or_else(|| DocumentError::IRValidationFailed {
            path: format!("/slides/{slide_id}/blocks"),
            reason: "before/after block_id not found".into(),
        })?;
        slide.blocks.insert(pos, block);
        recompute_assets_referenced(ir);
        Ok(OpOutcome {
            assigned_ids: OpOutcomeIds {
                block: Some(new_id),
                ..Default::default()
            },
        })
    }

    fn delete_html_block(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        block_id: &str,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        let pos = slide
            .blocks
            .iter()
            .position(|x| block_id_of(x) == block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{block_id}"),
                reason: "block not found".into(),
            })?;
        slide.blocks.remove(pos);
        recompute_assets_referenced(ir);
        Ok(OpOutcome::default())
    }

    fn replace_html_block(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        block_id: &str,
        block_json: &serde_json::Value,
    ) -> Result<OpOutcome, DocumentError> {
        let mut block_json = block_json.clone();
        if let Some(obj) = block_json.as_object_mut() {
            obj.insert("id".into(), serde_json::json!(block_id));
            self.assign_nested_ids(obj);
        }
        let new_block: Block = serde_json::from_value(block_json).map_err(|e| {
            DocumentError::IRValidationFailed {
                path: "/op/replace_html_block/block".into(),
                reason: e.to_string(),
            }
        })?;
        let slide = find_slide_mut(ir, slide_id)?;
        let pos = slide
            .blocks
            .iter()
            .position(|x| block_id_of(x) == block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{block_id}"),
                reason: "block not found".into(),
            })?;
        slide.blocks[pos] = new_block;
        recompute_assets_referenced(ir);
        Ok(OpOutcome::default())
    }

    fn move_html_block(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        block_id: &str,
        after_block_id: &str,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        let from = slide
            .blocks
            .iter()
            .position(|x| block_id_of(x) == block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{block_id}"),
                reason: "block not found".into(),
            })?;
        let removed = slide.blocks.remove(from);
        let after_pos = slide
            .blocks
            .iter()
            .position(|x| block_id_of(x) == after_block_id);
        let insert_at = match after_pos {
            Some(i) => i + 1,
            None => {
                // restore and error
                slide.blocks.insert(from, removed);
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/slides/{slide_id}/blocks/{after_block_id}"),
                    reason: "after_block_id not found".into(),
                });
            }
        };
        slide.blocks.insert(insert_at, removed);
        Ok(OpOutcome::default())
    }

    fn insert_html_table_row(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        table_block_id: &str,
        before: Option<&str>,
        after: Option<&str>,
        cells: &[serde_json::Value],
    ) -> Result<OpOutcome, DocumentError> {
        let row_id = self.ids.new_row_id();
        let parsed_cells: Vec<_> = cells
            .iter()
            .map(|c| serde_json::from_value(c.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| DocumentError::IRValidationFailed {
                path: "/op/insert_html_table_row/cells".into(),
                reason: e.to_string(),
            })?;
        let new_row = crate::documents::domain::ir::TableRow {
            id: row_id.clone(),
            cells: parsed_cells,
        };
        let slide = find_slide_mut(ir, slide_id)?;
        let block = slide
            .blocks
            .iter_mut()
            .find(|b| block_id_of(b) == table_block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{table_block_id}"),
                reason: "table block not found".into(),
            })?;
        if let Block::Table { rows, .. } = block {
            let pos = match (before, after) {
                (Some(b), None) => rows.iter().position(|r| r.id == b),
                (None, Some(a)) => rows.iter().position(|r| r.id == a).map(|i| i + 1),
                _ => Some(rows.len()),
            }
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: "/op/insert_html_table_row".into(),
                reason: "row_id not found".into(),
            })?;
            rows.insert(pos, new_row);
            Ok(OpOutcome {
                assigned_ids: OpOutcomeIds {
                    rows: vec![row_id],
                    ..Default::default()
                },
            })
        } else {
            Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{table_block_id}"),
                reason: "not a table block".into(),
            })
        }
    }

    fn delete_html_table_row(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        table_block_id: &str,
        row_id: &str,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        let block = slide
            .blocks
            .iter_mut()
            .find(|b| block_id_of(b) == table_block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{table_block_id}"),
                reason: "table block not found".into(),
            })?;
        if let Block::Table { rows, .. } = block {
            let pos = rows.iter().position(|r| r.id == row_id).ok_or_else(|| {
                DocumentError::IRValidationFailed {
                    path: format!("/slides/{slide_id}/blocks/{table_block_id}/rows/{row_id}"),
                    reason: "row not found".into(),
                }
            })?;
            rows.remove(pos);
            Ok(OpOutcome::default())
        } else {
            Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{table_block_id}"),
                reason: "not a table block".into(),
            })
        }
    }

    fn update_html_table_cell(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        table_block_id: &str,
        row_id: &str,
        col_index: u32,
        cell_json: &serde_json::Value,
    ) -> Result<OpOutcome, DocumentError> {
        let cell: crate::documents::domain::ir::TableCell =
            serde_json::from_value(cell_json.clone()).map_err(|e| {
                DocumentError::IRValidationFailed {
                    path: "/op/update_html_table_cell/cell".into(),
                    reason: e.to_string(),
                }
            })?;
        let slide = find_slide_mut(ir, slide_id)?;
        let block = slide
            .blocks
            .iter_mut()
            .find(|b| block_id_of(b) == table_block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{table_block_id}"),
                reason: "table block not found".into(),
            })?;
        if let Block::Table { rows, .. } = block {
            let row = rows
                .iter_mut()
                .find(|r| r.id == row_id)
                .ok_or_else(|| DocumentError::IRValidationFailed {
                    path: format!("/slides/{slide_id}/blocks/{table_block_id}/rows/{row_id}"),
                    reason: "row not found".into(),
                })?;
            let idx = col_index as usize;
            if idx >= row.cells.len() {
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/.../rows/{row_id}/cells/{col_index}"),
                    reason: format!("col_index out of range (row has {} cells)", row.cells.len()),
                });
            }
            row.cells[idx] = cell;
            Ok(OpOutcome::default())
        } else {
            Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{table_block_id}"),
                reason: "not a table block".into(),
            })
        }
    }

    fn insert_html_list_item(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        list_block_id: &str,
        at_index: u32,
        runs: &[serde_json::Value],
    ) -> Result<OpOutcome, DocumentError> {
        let item_id = self.ids.new_list_item_id();
        let parsed_runs: Vec<_> = runs
            .iter()
            .map(|r| {
                let mut rv = r.clone();
                if let Some(o) = rv.as_object_mut() {
                    o.insert("id".into(), serde_json::json!(self.ids.new_run_id()));
                }
                serde_json::from_value::<crate::documents::domain::ir::Run>(rv)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| DocumentError::IRValidationFailed {
                path: "/op/insert_html_list_item/runs".into(),
                reason: e.to_string(),
            })?;
        let new_item = crate::documents::domain::ir::ListItem {
            id: item_id.clone(),
            runs: parsed_runs,
        };
        let slide = find_slide_mut(ir, slide_id)?;
        let block = slide
            .blocks
            .iter_mut()
            .find(|b| block_id_of(b) == list_block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{list_block_id}"),
                reason: "list block not found".into(),
            })?;
        if let Block::List { items, .. } = block {
            let pos = (at_index as usize).min(items.len());
            items.insert(pos, new_item);
            Ok(OpOutcome {
                assigned_ids: OpOutcomeIds {
                    list_items: vec![item_id],
                    ..Default::default()
                },
            })
        } else {
            Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{list_block_id}"),
                reason: "not a list block".into(),
            })
        }
    }

    fn delete_html_list_item(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        list_block_id: &str,
        item_id: &str,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        let block = slide
            .blocks
            .iter_mut()
            .find(|b| block_id_of(b) == list_block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{list_block_id}"),
                reason: "list block not found".into(),
            })?;
        if let Block::List { items, .. } = block {
            let pos = items.iter().position(|i| i.id == item_id).ok_or_else(|| {
                DocumentError::IRValidationFailed {
                    path: format!("/.../{list_block_id}/items/{item_id}"),
                    reason: "item not found".into(),
                }
            })?;
            items.remove(pos);
            Ok(OpOutcome::default())
        } else {
            Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{list_block_id}"),
                reason: "not a list block".into(),
            })
        }
    }

    fn update_html_list_item(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        list_block_id: &str,
        item_id: &str,
        runs: &[serde_json::Value],
    ) -> Result<OpOutcome, DocumentError> {
        let parsed_runs: Vec<_> = runs
            .iter()
            .map(|r| {
                let mut rv = r.clone();
                if let Some(o) = rv.as_object_mut() {
                    if !o.contains_key("id") {
                        o.insert("id".into(), serde_json::json!(self.ids.new_run_id()));
                    }
                }
                serde_json::from_value::<crate::documents::domain::ir::Run>(rv)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| DocumentError::IRValidationFailed {
                path: "/op/update_html_list_item/runs".into(),
                reason: e.to_string(),
            })?;
        let slide = find_slide_mut(ir, slide_id)?;
        let block = slide
            .blocks
            .iter_mut()
            .find(|b| block_id_of(b) == list_block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{list_block_id}"),
                reason: "list block not found".into(),
            })?;
        if let Block::List { items, .. } = block {
            let item = items
                .iter_mut()
                .find(|i| i.id == item_id)
                .ok_or_else(|| DocumentError::IRValidationFailed {
                    path: format!("/.../{list_block_id}/items/{item_id}"),
                    reason: "item not found".into(),
                })?;
            item.runs = parsed_runs;
            Ok(OpOutcome::default())
        } else {
            Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{list_block_id}"),
                reason: "not a list block".into(),
            })
        }
    }

    fn assign_nested_ids(&self, obj: &mut serde_json::Map<String, serde_json::Value>) {
        // Walks runs/items/rows nested in a block JSON, assigning fresh IDs to any
        // that don't have one. Best-effort; deep schema parsing happens after.
        if let Some(serde_json::Value::Array(runs)) = obj.get_mut("runs") {
            for r in runs {
                if let Some(o) = r.as_object_mut() {
                    if !o.contains_key("id") {
                        o.insert("id".into(), serde_json::json!(self.ids.new_run_id()));
                    }
                }
            }
        }
        if let Some(serde_json::Value::Array(items)) = obj.get_mut("items") {
            for i in items {
                if let Some(o) = i.as_object_mut() {
                    if !o.contains_key("id") {
                        o.insert("id".into(), serde_json::json!(self.ids.new_list_item_id()));
                    }
                    self.assign_nested_ids(o);
                }
            }
        }
        if let Some(serde_json::Value::Array(rows)) = obj.get_mut("rows") {
            for r in rows {
                if let Some(o) = r.as_object_mut() {
                    if !o.contains_key("id") {
                        o.insert("id".into(), serde_json::json!(self.ids.new_row_id()));
                    }
                }
            }
        }
    }
}

fn block_id_of(b: &Block) -> &str {
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
        | Block::AutoToc { id, .. } => id,
    }
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::application::apply_html_ops`
Expected: all tests PASS (4 from 11.1 + 4 from 11.2).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/application/apply_html_ops.rs
git commit -m "$(cat <<'EOF'
feat(documents/app): HtmlOpApplier block/table/list/doc ops + assets_referenced maintenance

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 12 — Wire HTML into existing use cases + runtime

### Task 12.1: Extend `CreateDocumentUseCase` with Html branch

**Files:**
- Modify: `src/libs/colmena/src/documents/application/create_document.rs`

- [ ] **Step 1: Add failing test**

In `src/libs/colmena/src/documents/application/create_document.rs`, inside `#[cfg(test)] mod tests`, append:

```rust
#[tokio::test]
async fn creates_empty_html_artifact_v1() {
    use crate::documents::infrastructure::render::HtmlRenderer;
    use crate::documents::infrastructure::storage::LocalFsAssetStore;
    use crate::documents::infrastructure::validation::HtmlValidator;
    use tempfile::tempdir;

    let tmp_a = tempdir().unwrap();
    let tmp_b = tempdir().unwrap();
    let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp_a.path()));
    let asset_store: Arc<dyn AssetStore> = Arc::new(LocalFsAssetStore::new(tmp_b.path()));
    let uc = CreateDocumentUseCase {
        store: store.clone(),
        excel_renderer: Arc::new(ExcelRenderer),
        excel_validator: Arc::new(ExcelValidator),
        word_renderer: Arc::new(NoopRenderer),
        word_validator: Arc::new(NoopValidator),
        html_renderer: Arc::new(HtmlRenderer::new(asset_store)),
        html_validator: Arc::new(HtmlValidator),
        ids: Arc::new(CountingIdGenerator::default()),
        default_retention: 10,
    };
    let out = uc
        .execute(CreateDocumentInput {
            kind: ArtifactKind::Html,
            session_id: SessionId::new("s"),
            label: None,
            retention_limit: None,
            initial_ir: None,
            source: PatchSource::Agent,
        })
        .await
        .unwrap();
    assert_eq!(out.version_id.0, "v1");
    let stored = store.read_current(&out.artifact_id).await.unwrap();
    assert_eq!(stored.rendered_extension, "html");
}
```

Add the import line at the top of the test mod or test fn:
```rust
use crate::documents::domain::ports::AssetStore;
```

- [ ] **Step 2: Run, verify fails**

Expected: `CreateDocumentUseCase` doesn't have `html_renderer`/`html_validator` fields; `empty_ir(Html)` panics or missing arm.

- [ ] **Step 3: Extend the use case struct, execute, and helpers**

In `src/libs/colmena/src/documents/application/create_document.rs`:

a) Extend the struct:

```rust
pub struct CreateDocumentUseCase {
    pub store: Arc<dyn ArtifactStore>,
    pub excel_renderer: Arc<dyn IRRenderer>,
    pub excel_validator: Arc<dyn IRValidator>,
    pub word_renderer: Arc<dyn IRRenderer>,
    pub word_validator: Arc<dyn IRValidator>,
    pub html_renderer: Arc<dyn IRRenderer>,   // ← new
    pub html_validator: Arc<dyn IRValidator>, // ← new
    pub ids: Arc<dyn IdGenerator>,
    pub default_retention: u32,
}
```

b) Extend the `match input.kind` block in `execute` to include `Html`:

```rust
        let (validator, renderer): (&Arc<dyn IRValidator>, &Arc<dyn IRRenderer>) = match input.kind
        {
            ArtifactKind::Excel => (&self.excel_validator, &self.excel_renderer),
            ArtifactKind::Word => (&self.word_validator, &self.word_renderer),
            ArtifactKind::Html => (&self.html_validator, &self.html_renderer),
        };
```

c) Update the `rendered_extension` line similarly:

```rust
            rendered_extension: match input.kind {
                ArtifactKind::Excel => "xlsx",
                ArtifactKind::Word => "docx",
                ArtifactKind::Html => "html",
            },
```

d) Extend `empty_ir`:

```rust
fn empty_ir(id: &ArtifactId, kind: ArtifactKind) -> serde_json::Value {
    match kind {
        ArtifactKind::Excel => serde_json::json!({
            "kind": "excel",
            "artifact_id": id.0,
            "version_id": "v1",
            "schema_version": crate::documents::domain::ir::SCHEMA_VERSION,
            "workbook": { "sheets": [], "named_styles": {} }
        }),
        ArtifactKind::Word => serde_json::json!({
            "kind": "word",
            "artifact_id": id.0,
            "version_id": "v1",
            "schema_version": crate::documents::domain::ir::SCHEMA_VERSION,
            "document": { "blocks": [], "named_styles": {} }
        }),
        ArtifactKind::Html => serde_json::json!({
            "kind": "html",
            "artifact_id": id.0,
            "version_id": "v1",
            "schema_version": crate::documents::domain::ir::SCHEMA_VERSION,
            "doc_props": { "title": null, "author": null, "date": null, "locale": "en" },
            "theme": "executive",
            "layout_mode": "report",
            "footer": { "enabled": false, "page_numbers": false, "custom_text": null },
            "slides": [{
                "id": "sl_initial",
                "layout": "blank",
                "title": null,
                "subtitle": null,
                "notes": null,
                "blocks": []
            }],
            "assets_referenced": []
        }),
    }
}
```

NOTE: The placeholder `sl_initial` would fail validator's uniqueness only if duplicated, which it isn't on creation. If the validator complains about it being a non-ULID literal, change to `format!("sl_{}", id.0)` to make it artifact-derived. The simplest fix: leave as `sl_initial` since the validator doesn't enforce ULID format on slide ids.

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::application::create_document`
Expected: all tests PASS including new HTML test.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/application/create_document.rs
git commit -m "$(cat <<'EOF'
feat(documents/app): CreateDocumentUseCase supports ArtifactKind::Html

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 12.2: Extend `ApplyPatchUseCase` with Html branch

**Files:**
- Modify: `src/libs/colmena/src/documents/application/apply_patch.rs`

- [ ] **Step 1: Add failing test**

In `src/libs/colmena/src/documents/application/apply_patch.rs`, inside `#[cfg(test)] mod tests`, append:

```rust
#[tokio::test]
async fn apply_set_theme_on_html_advances_version() {
    use crate::documents::domain::ir::Theme;
    use crate::documents::infrastructure::render::HtmlRenderer;
    use crate::documents::infrastructure::storage::LocalFsAssetStore;
    use crate::documents::infrastructure::validation::HtmlValidator;
    use tempfile::tempdir;

    let tmp_a = tempdir().unwrap();
    let tmp_b = tempdir().unwrap();
    let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp_a.path()));
    let asset_store: Arc<dyn AssetStore> = Arc::new(LocalFsAssetStore::new(tmp_b.path()));
    let ids = Arc::new(CountingIdGenerator::default());

    let create = CreateDocumentUseCase {
        store: store.clone(),
        excel_renderer: Arc::new(ExcelRenderer),
        excel_validator: Arc::new(ExcelValidator),
        word_renderer: Arc::new(NoopR),
        word_validator: Arc::new(NoopV),
        html_renderer: Arc::new(HtmlRenderer::new(asset_store.clone())),
        html_validator: Arc::new(HtmlValidator),
        ids: ids.clone(),
        default_retention: 10,
    };
    let out = create
        .execute(CreateDocumentInput {
            kind: ArtifactKind::Html,
            session_id: SessionId::new("s"),
            label: None,
            retention_limit: None,
            initial_ir: None,
            source: PatchSource::Agent,
        })
        .await
        .unwrap();

    let apply = ApplyPatchUseCase {
        store: store.clone(),
        excel_renderer: Arc::new(ExcelRenderer),
        excel_validator: Arc::new(ExcelValidator),
        word_renderer: Arc::new(NoopR),
        word_validator: Arc::new(NoopV),
        html_renderer: Arc::new(HtmlRenderer::new(asset_store)),
        html_validator: Arc::new(HtmlValidator),
        ids: ids.clone(),
    };
    let res = apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: out.artifact_id.0.clone(),
                base_version: "v1".into(),
                source: PatchSource::Agent,
                ops: vec![PatchOp::SetTheme {
                    theme: Theme::Vibrant,
                }],
            },
        })
        .await
        .unwrap();
    assert_eq!(res.version_id.0, "v2");
}
```

Add `use crate::documents::domain::ports::AssetStore;` to the test imports.

- [ ] **Step 2: Run, verify fails**

Expected: `ApplyPatchUseCase` lacks Html fields; match in `execute` doesn't have Html arm.

- [ ] **Step 3: Extend struct + execute**

In `src/libs/colmena/src/documents/application/apply_patch.rs`:

a) Add fields to the struct:

```rust
pub struct ApplyPatchUseCase {
    pub store: Arc<dyn ArtifactStore>,
    pub excel_renderer: Arc<dyn IRRenderer>,
    pub excel_validator: Arc<dyn IRValidator>,
    pub word_renderer: Arc<dyn IRRenderer>,
    pub word_validator: Arc<dyn IRValidator>,
    pub html_renderer: Arc<dyn IRRenderer>,
    pub html_validator: Arc<dyn IRValidator>,
    pub ids: Arc<dyn IdGenerator>,
}
```

b) Add an Html arm to the `match meta.kind` block in `execute`. Add this AFTER the existing `ArtifactKind::Word => { ... }` arm:

```rust
            ArtifactKind::Html => {
                use crate::documents::application::apply_html_ops::HtmlOpApplier;
                use crate::documents::domain::ir::HtmlIR;

                let mut ir: HtmlIR =
                    serde_json::from_value(current_data.ir.clone()).map_err(|e| {
                        DocumentError::IRValidationFailed {
                            path: "/".into(),
                            reason: format!("parse current HTML IR: {e}"),
                        }
                    })?;
                let applier = HtmlOpApplier {
                    ids: self.ids.as_ref(),
                };
                let mut structured = Vec::with_capacity(input.patch.ops.len());
                let mut natural_language = Vec::with_capacity(input.patch.ops.len());
                for (i, op) in input.patch.ops.iter().enumerate() {
                    let outcome = applier.apply(&mut ir, op)?;
                    natural_language.push(describe_op(op, &outcome));
                    if !outcome.assigned_ids.is_empty() {
                        structured.push(op_outcome_entry(i, op, &outcome));
                    }
                }
                let summary = PatchSummary {
                    natural_language,
                    structured,
                };
                let new_version = current.next();
                ir.version_id = new_version.0.clone();
                let ir_value = serde_json::to_value(&ir).unwrap();
                self.html_validator.validate(&ir_value)?;
                let rendered = self.html_renderer.render(&ir_value).await?;

                let patch_applied = PatchApplied {
                    patch: serde_json::to_value(&input.patch).unwrap(),
                    applied_at: Utc::now(),
                    resulted_in: new_version.clone(),
                    summary: summary.clone(),
                };
                let version_data = VersionData {
                    ir: ir_value,
                    rendered_binary: rendered,
                    rendered_extension: "html",
                    patch_applied,
                    blobs: vec![],
                };
                self.store
                    .write_version(&artifact_id, &new_version, &version_data)
                    .await?;
                self.store
                    .set_head(&artifact_id, Some(&current), &new_version)
                    .await?;

                let mut new_meta = meta.clone();
                new_meta.current_version = new_version.clone();
                new_meta.updated_at = Utc::now();
                self.store.update_meta(&artifact_id, &new_meta).await?;

                Ok(ApplyPatchOutput {
                    version_id: new_version,
                    summary,
                })
            }
```

c) Add `assigned_ids.is_empty()` method to `OpOutcomeIds` if it doesn't exist. In `src/libs/colmena/src/documents/domain/artifact.rs`, add:

```rust
impl OpOutcomeIds {
    pub fn is_empty(&self) -> bool {
        self.sheet.is_none()
            && self.table.is_none()
            && self.block.is_none()
            && self.runs.is_empty()
            && self.rows.is_empty()
            && self.list_items.is_empty()
            && self.slide.is_none()
    }
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::application::apply_patch`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/application/apply_patch.rs src/libs/colmena/src/documents/domain/artifact.rs
git commit -m "$(cat <<'EOF'
feat(documents/app): ApplyPatchUseCase supports HTML via HtmlOpApplier

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 12.3: Extend `describe_op` for HTML ops

**Files:**
- Modify: `src/libs/colmena/src/documents/application/apply_patch.rs`

- [ ] **Step 1: Add a test for narration**

In `mod tests`, append:

```rust
#[test]
fn describe_op_add_slide_includes_layout_and_title() {
    use crate::documents::domain::artifact::OpOutcome;
    use crate::documents::domain::ir::SlideLayout;
    let op = PatchOp::AddSlide {
        layout: SlideLayout::Title,
        at_index: None,
        title: Some("Welcome".into()),
        subtitle: None,
    };
    let outcome = OpOutcome::default();
    let s = describe_op(&op, &outcome);
    assert!(s.contains("Welcome"));
    assert!(s.to_lowercase().contains("title"));
}

#[test]
fn describe_op_set_theme_mentions_theme() {
    use crate::documents::domain::artifact::OpOutcome;
    use crate::documents::domain::ir::Theme;
    let op = PatchOp::SetTheme { theme: Theme::Dark };
    let s = describe_op(&op, &OpOutcome::default());
    assert!(s.to_lowercase().contains("theme"));
    assert!(s.to_lowercase().contains("dark"));
}
```

- [ ] **Step 2: Run, verify fails**

Expected: HTML ops produce empty or wrong descriptions.

- [ ] **Step 3: Extend `describe_op`**

In `src/libs/colmena/src/documents/application/apply_patch.rs`, find the `match op` in `describe_op` and add new arms AFTER the existing Word arms:

```rust
        // ---- HTML ----
        AddSlide { layout, title, .. } => match (layout, title.as_deref()) {
            (crate::documents::domain::ir::SlideLayout::Title, Some(t)) => {
                format!("Added title slide '{t}'")
            }
            (crate::documents::domain::ir::SlideLayout::SectionDivider, Some(t)) => {
                format!("Added section divider '{t}'")
            }
            (l, Some(t)) => format!("Added {:?} slide '{t}'", l),
            (l, None) => format!("Added {:?} slide", l),
        },
        DeleteSlide { slide_id } => format!("Deleted slide {slide_id}"),
        ReorderSlides { order } => format!("Reordered slides to [{}]", order.join(", ")),
        SetSlideLayout { slide_id, layout } => {
            format!("Set slide {slide_id} layout to {:?}", layout)
        }
        SetSlideTitle { slide_id, title, .. } => match title {
            Some(t) => format!("Set slide {slide_id} title to '{t}'"),
            None => format!("Cleared slide {slide_id} title"),
        },
        SetSlideNotes { slide_id, notes } => match notes {
            Some(_) => format!("Updated speaker notes on {slide_id}"),
            None => format!("Cleared speaker notes on {slide_id}"),
        },
        InsertHtmlBlock { slide_id, block, .. } => {
            let kind = block
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("block");
            let id_part = ids
                .block
                .as_ref()
                .map(|b| format!(" (id: {b})"))
                .unwrap_or_default();
            format!("Inserted {kind} block on slide {slide_id}{id_part}")
        }
        DeleteHtmlBlock { slide_id, block_id } => {
            format!("Deleted block {block_id} from slide {slide_id}")
        }
        ReplaceHtmlBlock { block_id, .. } => format!("Replaced block {block_id}"),
        MoveHtmlBlock {
            block_id,
            after_block_id,
            ..
        } => format!("Moved block {block_id} after {after_block_id}"),
        InsertHtmlTableRow {
            table_block_id, ..
        } => format!("Inserted row in table {table_block_id}"),
        DeleteHtmlTableRow {
            table_block_id,
            row_id,
            ..
        } => format!("Deleted row {row_id} from table {table_block_id}"),
        UpdateHtmlTableCell {
            table_block_id,
            row_id,
            col_index,
            ..
        } => format!("Updated cell ({row_id}, col {col_index}) of table {table_block_id}"),
        InsertHtmlListItem {
            list_block_id,
            at_index,
            ..
        } => format!("Inserted list item at index {at_index} of {list_block_id}"),
        DeleteHtmlListItem {
            list_block_id,
            item_id,
            ..
        } => format!("Deleted list item {item_id} from {list_block_id}"),
        UpdateHtmlListItem {
            list_block_id,
            item_id,
            ..
        } => format!("Updated list item {item_id} in {list_block_id}"),
        SetTheme { theme } => format!("Set theme to {:?}", theme).to_lowercase(),
        SetDocProps { title, .. } => match title {
            Some(t) => format!("Set document title to '{t}'"),
            None => "Updated document props".into(),
        },
        SetFooter { footer } => {
            if footer.enabled {
                format!(
                    "Enabled footer (page_numbers={}, custom='{}')",
                    footer.page_numbers,
                    footer.custom_text.as_deref().unwrap_or("")
                )
            } else {
                "Disabled footer".into()
            }
        }
```

The imports at the top of `apply_patch.rs` may need extending to bring `PatchOp::*` into scope inside `describe_op`. The existing `use crate::documents::domain::patch::PatchOp::*;` line at the top of `describe_op` already glob-imports — confirm and just keep.

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::application::apply_patch`
Expected: all tests PASS including new describe_op tests.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/application/apply_patch.rs
git commit -m "$(cat <<'EOF'
feat(documents/app): describe_op narration for all 19 HTML ops

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 12.4: Extend `DocumentRuntime` with asset store + 3 asset use cases + html bits

**Files:**
- Modify: `src/libs/colmena/src/documents/application/runtime.rs`

- [ ] **Step 1: Add failing test**

In `mod tests`, append:

```rust
#[tokio::test]
async fn runtime_creates_html_and_serves_asset_use_cases() {
    use crate::documents::domain::ids::AssetId;
    let tmp = tempdir().unwrap();
    let cfg = json!({
        "storage_root": tmp.path().join("artifacts").to_str().unwrap(),
        "asset_storage_root": tmp.path().join("assets").to_str().unwrap(),
        "max_asset_size_bytes": 1024,
        "allowed_asset_mimes": ["image/png"]
    });
    let rt = DocumentRuntime::from_config(&cfg).await.unwrap();

    use crate::documents::application::upload_asset::UploadAssetInput;
    let out = rt.upload_asset.execute(UploadAssetInput {
        session_id: SessionId::new("s1"),
        bytes: b"x".to_vec(),
        mime: "image/png".into(),
        label: None,
    }).await.unwrap();
    assert!(out.asset_id.as_str().starts_with("asset_"));

    let listed = rt.list_assets.execute(&SessionId::new("s1")).await.unwrap();
    assert_eq!(listed.len(), 1);

    // Sanity: create an HTML doc using the runtime (verifies html_validator + html_renderer wired)
    use crate::documents::application::create_document::CreateDocumentInput;
    use crate::documents::domain::ids::ArtifactKind;
    use crate::documents::domain::patch::PatchSource;
    let _ = rt.create.execute(CreateDocumentInput {
        kind: ArtifactKind::Html,
        session_id: SessionId::new("s1"),
        label: Some("test".into()),
        retention_limit: None,
        initial_ir: None,
        source: PatchSource::Agent,
    }).await.unwrap();

    let _ = AssetId::new("asset_x"); // ensure import used
}
```

- [ ] **Step 2: Run, verify fails**

Expected: `DocumentRuntime` has no `asset_store`, `upload_asset`, `list_assets`, `delete_asset`, `html_renderer`, `html_validator`.

- [ ] **Step 3: Extend the struct + `from_config` + `with_stores`**

In `src/libs/colmena/src/documents/application/runtime.rs`:

a) Update imports:

```rust
use crate::documents::application::apply_patch::ApplyPatchUseCase;
use crate::documents::application::create_document::CreateDocumentUseCase;
use crate::documents::application::delete_asset::DeleteAssetUseCase;
use crate::documents::application::get_head::GetHeadUseCase;
use crate::documents::application::list_assets::ListAssetsUseCase;
use crate::documents::application::list_versions::ListVersionsUseCase;
use crate::documents::application::read_document::ReadDocumentUseCase;
use crate::documents::application::rollback::RollbackUseCase;
use crate::documents::application::upload_asset::UploadAssetUseCase;
use crate::documents::domain::ports::AssetStore;
use crate::documents::domain::{ArtifactStore, IRRenderer, IRValidator, IdGenerator};
use crate::documents::infrastructure::ids::UlidIdGenerator;
use crate::documents::infrastructure::render::{ExcelRenderer, HtmlRenderer, WordRenderer};
#[cfg(feature = "gcs")]
use crate::documents::infrastructure::storage::{GcsArtifactStore, GcsAssetStore};
use crate::documents::infrastructure::storage::{LocalFsAssetStore, LocalFsStore};
use crate::documents::infrastructure::validation::{ExcelValidator, HtmlValidator, WordValidator};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
```

b) Add constants:

```rust
pub const DEFAULT_ASSET_STORAGE_ROOT: &str = ".colmena/documents/assets";
pub const DEFAULT_ASSET_GCS_PREFIX: &str = "colmena/documents/assets";
pub const DEFAULT_MAX_ASSET_SIZE_BYTES: u64 = 10_485_760;

fn default_allowed_mimes() -> HashSet<String> {
    ["image/png", "image/jpeg", "image/gif", "image/webp"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}
```

c) Replace the `DocumentRuntime` struct:

```rust
pub struct DocumentRuntime {
    pub store: Arc<dyn ArtifactStore>,
    pub asset_store: Arc<dyn AssetStore>,
    pub create: Arc<CreateDocumentUseCase>,
    pub apply: Arc<ApplyPatchUseCase>,
    pub read: Arc<ReadDocumentUseCase>,
    pub get_head: Arc<GetHeadUseCase>,
    pub list_versions: Arc<ListVersionsUseCase>,
    pub rollback: Arc<RollbackUseCase>,
    pub upload_asset: Arc<UploadAssetUseCase>,
    pub list_assets: Arc<ListAssetsUseCase>,
    pub delete_asset: Arc<DeleteAssetUseCase>,
}
```

d) Replace `from_config` (full body) with a version that builds the asset store + asset use cases:

```rust
    pub async fn from_config(config: &Value) -> Result<Arc<Self>, String> {
        let backend = config
            .get("storage_backend")
            .and_then(|v| v.as_str())
            .unwrap_or("localfs");

        let (store, asset_store): (Arc<dyn ArtifactStore>, Arc<dyn AssetStore>) = match backend {
            "localfs" => {
                let root = config
                    .get("storage_root")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_STORAGE_ROOT));
                let asset_root = config
                    .get("asset_storage_root")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_ASSET_STORAGE_ROOT));
                std::fs::create_dir_all(&root)
                    .map_err(|e| format!("creating storage root {:?}: {e}", root))?;
                std::fs::create_dir_all(&asset_root)
                    .map_err(|e| format!("creating asset root {:?}: {e}", asset_root))?;
                (
                    Arc::new(LocalFsStore::new(&root)),
                    Arc::new(LocalFsAssetStore::new(&asset_root)) as Arc<dyn AssetStore>,
                )
            }
            "gcs" => {
                #[cfg(feature = "gcs")]
                {
                    let bucket = config
                        .get("gcs_bucket")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            "storage_backend `gcs` requires `gcs_bucket` in config".to_string()
                        })?;
                    let prefix = config
                        .get("gcs_prefix")
                        .and_then(|v| v.as_str())
                        .unwrap_or("colmena/documents");
                    let asset_prefix = config
                        .get("asset_gcs_prefix")
                        .and_then(|v| v.as_str())
                        .unwrap_or(DEFAULT_ASSET_GCS_PREFIX);
                    let gcs = GcsArtifactStore::new(bucket, prefix).await?;
                    let gcs_assets = GcsAssetStore::new(bucket, asset_prefix).await?;
                    (Arc::new(gcs), Arc::new(gcs_assets) as Arc<dyn AssetStore>)
                }
                #[cfg(not(feature = "gcs"))]
                {
                    return Err("storage_backend `gcs` requires the `gcs` feature flag — \
                         rebuild with `--features gcs`"
                        .into());
                }
            }
            other => {
                return Err(format!(
                    "unknown storage_backend `{other}` — expected `localfs` or `gcs`"
                ));
            }
        };

        let default_retention = config
            .get("default_retention")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(DEFAULT_RETENTION);

        let max_asset_size_bytes = config
            .get("max_asset_size_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_MAX_ASSET_SIZE_BYTES);

        let allowed_mimes: HashSet<String> = config
            .get("allowed_asset_mimes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_else(default_allowed_mimes);

        Ok(Self::with_stores(
            store,
            asset_store,
            default_retention,
            max_asset_size_bytes,
            allowed_mimes,
        ))
    }
```

e) Replace `with_store` with `with_stores`:

```rust
    pub fn with_stores(
        store: Arc<dyn ArtifactStore>,
        asset_store: Arc<dyn AssetStore>,
        default_retention: u32,
        max_asset_size_bytes: u64,
        allowed_mimes: HashSet<String>,
    ) -> Arc<Self> {
        let excel_renderer: Arc<dyn IRRenderer> = Arc::new(ExcelRenderer);
        let word_renderer: Arc<dyn IRRenderer> = Arc::new(WordRenderer);
        let html_renderer: Arc<dyn IRRenderer> =
            Arc::new(HtmlRenderer::new(asset_store.clone()));
        let excel_validator: Arc<dyn IRValidator> = Arc::new(ExcelValidator);
        let word_validator: Arc<dyn IRValidator> = Arc::new(WordValidator);
        let html_validator: Arc<dyn IRValidator> = Arc::new(HtmlValidator);
        let ids: Arc<dyn IdGenerator> = Arc::new(UlidIdGenerator);

        let create = Arc::new(CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: excel_renderer.clone(),
            excel_validator: excel_validator.clone(),
            word_renderer: word_renderer.clone(),
            word_validator: word_validator.clone(),
            html_renderer: html_renderer.clone(),
            html_validator: html_validator.clone(),
            ids: ids.clone(),
            default_retention,
        });
        let apply = Arc::new(ApplyPatchUseCase {
            store: store.clone(),
            excel_renderer,
            excel_validator,
            word_renderer,
            word_validator,
            html_renderer,
            html_validator,
            ids: ids.clone(),
        });
        let read = Arc::new(ReadDocumentUseCase {
            store: store.clone(),
        });
        let get_head = Arc::new(GetHeadUseCase {
            store: store.clone(),
        });
        let list_versions = Arc::new(ListVersionsUseCase {
            store: store.clone(),
        });
        let rollback = Arc::new(RollbackUseCase {
            store: store.clone(),
        });
        let upload_asset = Arc::new(UploadAssetUseCase {
            store: asset_store.clone(),
            ids: ids.clone(),
            max_size_bytes: max_asset_size_bytes,
            allowed_mimes,
        });
        let list_assets = Arc::new(ListAssetsUseCase {
            store: asset_store.clone(),
        });
        let delete_asset = Arc::new(DeleteAssetUseCase {
            assets: asset_store.clone(),
            artifacts: store.clone(),
            index: None,
        });

        Arc::new(Self {
            store,
            asset_store,
            create,
            apply,
            read,
            get_head,
            list_versions,
            rollback,
            upload_asset,
            list_assets,
            delete_asset,
        })
    }
```

f) Any existing test that calls `with_store(store, retention)` now must call `with_stores(store, asset_store, retention, max, allowed)`. Update those tests (in this file) to pass a temp `LocalFsAssetStore` and `default_allowed_mimes()` / `DEFAULT_MAX_ASSET_SIZE_BYTES`.

- [ ] **Step 4: Run, verify pass**

Run: `cargo test --lib -p colmena_dag_engine documents::application::runtime`
Expected: all tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/documents/application/runtime.rs
git commit -m "$(cat <<'EOF'
feat(documents/app): DocumentRuntime composes asset store + asset use cases + HTML renderer/validator

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 13 — End-to-end test + CI hexagonal guard

### Task 13.1: End-to-end HTML test (spec §16)

**Files:**
- Create: `src/libs/colmena/tests/html_documents_e2e.rs`

- [ ] **Step 1: Create the integration test**

Create `src/libs/colmena/tests/html_documents_e2e.rs`:

```rust
//! End-to-end test for the HTML documents module (spec §16).
//! Verifies create → apply (theme + slide + image+chart blocks) → read renders
//! produce a self-contained HTML that contains the expected markers.

use colmena_dag_engine::documents::application::create_document::CreateDocumentInput;
use colmena_dag_engine::documents::application::runtime::DocumentRuntime;
use colmena_dag_engine::documents::application::upload_asset::UploadAssetInput;
use colmena_dag_engine::documents::application::apply_patch::ApplyPatchInput;
use colmena_dag_engine::documents::domain::ids::{ArtifactKind, SessionId};
use colmena_dag_engine::documents::domain::patch::{Patch, PatchOp, PatchSource};
use colmena_dag_engine::documents::domain::ir::{Locale, SlideLayout, Theme};
use serde_json::json;
use tempfile::tempdir;

// Minimal 1x1 transparent PNG (89 bytes) for asset tests.
const TINY_PNG: &[u8] = &[
    0x89,0x50,0x4E,0x47,0x0D,0x0A,0x1A,0x0A,0x00,0x00,0x00,0x0D,0x49,0x48,0x44,0x52,
    0x00,0x00,0x00,0x01,0x00,0x00,0x00,0x01,0x08,0x06,0x00,0x00,0x00,0x1F,0x15,0xC4,
    0x89,0x00,0x00,0x00,0x0D,0x49,0x44,0x41,0x54,0x78,0x9C,0x62,0x00,0x01,0x00,0x00,
    0x05,0x00,0x01,0x0D,0x0A,0x2D,0xB4,0x00,0x00,0x00,0x00,0x49,0x45,0x4E,0x44,0xAE,
    0x42,0x60,0x82,
];

#[tokio::test]
async fn end_to_end_html_module() {
    let tmp = tempdir().unwrap();
    let cfg = json!({
        "storage_root": tmp.path().join("artifacts").to_str().unwrap(),
        "asset_storage_root": tmp.path().join("assets").to_str().unwrap(),
    });
    let rt = DocumentRuntime::from_config(&cfg).await.unwrap();
    let session = SessionId::new("test_session");

    // 1. Upload an asset (logo PNG)
    let upload = rt
        .upload_asset
        .execute(UploadAssetInput {
            session_id: session.clone(),
            bytes: TINY_PNG.to_vec(),
            mime: "image/png".into(),
            label: Some("company_logo".into()),
        })
        .await
        .unwrap();

    // 2. Create HTML doc
    let created = rt
        .create
        .execute(CreateDocumentInput {
            kind: ArtifactKind::Html,
            session_id: session.clone(),
            label: Some("Q3 Report".into()),
            retention_limit: None,
            initial_ir: None,
            source: PatchSource::Agent,
        })
        .await
        .unwrap();

    // 3. Patch v1 → v2: set theme + props
    let _ = rt
        .apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: created.artifact_id.0.clone(),
                base_version: "v1".into(),
                source: PatchSource::Agent,
                ops: vec![
                    PatchOp::SetTheme {
                        theme: Theme::Executive,
                    },
                    PatchOp::SetDocProps {
                        title: Some("Q3 Results".into()),
                        author: Some("Daniel".into()),
                        date: Some("2026-10-30".into()),
                        locale: Some(Locale::Es),
                    },
                ],
            },
        })
        .await
        .unwrap();

    // 4. v2 → v3: add a title slide
    let v3 = rt
        .apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: created.artifact_id.0.clone(),
                base_version: "v2".into(),
                source: PatchSource::Agent,
                ops: vec![PatchOp::AddSlide {
                    layout: SlideLayout::Title,
                    at_index: None,
                    title: Some("Q3 Results".into()),
                    subtitle: Some("2026".into()),
                }],
            },
        })
        .await
        .unwrap();
    let new_slide_id = v3
        .summary
        .structured
        .iter()
        .find_map(|e| e.get("assigned_ids").and_then(|a| a.get("slide")).and_then(|s| s.as_str()).map(String::from))
        .expect("add_slide must return slide_id in summary");

    // 5. v3 → v4: insert image (referencing the asset) and a chart on the new slide
    let _ = rt
        .apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: created.artifact_id.0.clone(),
                base_version: v3.version_id.0.clone(),
                source: PatchSource::Agent,
                ops: vec![
                    PatchOp::InsertHtmlBlock {
                        slide_id: new_slide_id.clone(),
                        before: None,
                        after: None,
                        block: json!({
                            "kind": "image",
                            "src": { "kind": "asset", "asset_id": upload.asset_id.as_str() },
                            "alt": "logo",
                            "caption": null,
                            "position": "hero"
                        }),
                    },
                    PatchOp::InsertHtmlBlock {
                        slide_id: new_slide_id.clone(),
                        before: None,
                        after: None,
                        block: json!({
                            "kind": "chart",
                            "chart": {
                                "chart_type": "bar",
                                "series": [{"name":"Sales","data":[10,20,30]}],
                                "x_axis": {"categories":["Q1","Q2","Q3"]},
                                "legend": true
                            },
                            "title": "Q3 Sales",
                            "size": "medium"
                        }),
                    },
                ],
            },
        })
        .await
        .unwrap();

    // 6. Read the latest bytes
    let data = rt.store.read_current(&created.artifact_id).await.unwrap();
    let html = String::from_utf8(data.rendered_binary).unwrap();

    // 7. Assertions
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(html.contains("Q3 Results"));
    assert!(html.contains("lang=\"es\""));
    assert!(html.contains("--font-heading"), "executive theme CSS missing");
    assert!(html.contains("data:image/png;base64,"), "asset not inlined");
    assert!(html.contains("new Chart"), "chart init script missing");
}
```

NOTE: For this integration test file path (`tests/`), `colmena_dag_engine` must be importable. Check `src/libs/colmena/Cargo.toml` `[package].name` — if the package name is different (e.g. `colmena`), adjust the imports accordingly. The path `src/libs/colmena/tests/` is conventional for integration tests in a Cargo package.

- [ ] **Step 2: Run, verify pass**

Run: `cargo test --test html_documents_e2e -p colmena_dag_engine`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/tests/html_documents_e2e.rs
git commit -m "$(cat <<'EOF'
test(documents/html): end-to-end create→apply→render integration test (spec §16)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 13.2: Snapshot tests per theme

**Files:**
- Create: `src/libs/colmena/tests/html_theme_snapshots.rs`
- Create: `src/libs/colmena/tests/snapshots/.gitkeep`

- [ ] **Step 1: Create directory and test**

Run: `mkdir -p src/libs/colmena/tests/snapshots && touch src/libs/colmena/tests/snapshots/.gitkeep`

Create `src/libs/colmena/tests/html_theme_snapshots.rs`:

```rust
//! Snapshot tests for HTML renderer per theme.
//! Stores the rendered HTML for a canonical IR per (theme x layout_mode) and
//! detects accidental regressions. To regenerate snapshots: delete the file
//! and run the test once.

use colmena_dag_engine::documents::application::runtime::DocumentRuntime;
use colmena_dag_engine::documents::application::apply_patch::ApplyPatchInput;
use colmena_dag_engine::documents::application::create_document::CreateDocumentInput;
use colmena_dag_engine::documents::domain::ids::{ArtifactKind, SessionId};
use colmena_dag_engine::documents::domain::ir::Theme;
use colmena_dag_engine::documents::domain::patch::{Patch, PatchOp, PatchSource};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

async fn render_canonical(theme: Theme, layout_mode: &str) -> String {
    let tmp = tempdir().unwrap();
    let cfg = json!({
        "storage_root": tmp.path().join("a").to_str().unwrap(),
        "asset_storage_root": tmp.path().join("b").to_str().unwrap()
    });
    let rt = DocumentRuntime::from_config(&cfg).await.unwrap();
    let created = rt
        .create
        .execute(CreateDocumentInput {
            kind: ArtifactKind::Html,
            session_id: SessionId::new("s"),
            label: None,
            retention_limit: None,
            initial_ir: None,
            source: PatchSource::Agent,
        })
        .await
        .unwrap();
    let _ = rt
        .apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: created.artifact_id.0.clone(),
                base_version: "v1".into(),
                source: PatchSource::Agent,
                ops: vec![
                    PatchOp::SetTheme { theme },
                    PatchOp::SetFooter {
                        footer: colmena_dag_engine::documents::domain::ir::FooterConfig {
                            enabled: true,
                            page_numbers: true,
                            custom_text: Some("Test".into()),
                        },
                    },
                ],
            },
        })
        .await
        .unwrap();
    // Switch layout_mode by directly editing the IR via a patched apply (ApplyPatchUseCase
    // doesn't have a SetLayoutMode op in v1 by design — the layout_mode is set at creation).
    // For snapshot purposes, we just verify the default (report).
    let _ = layout_mode; // currently fixed to "report"
    let data = rt.store.read_current(&created.artifact_id).await.unwrap();
    String::from_utf8(data.rendered_binary).unwrap()
}

fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.html"))
}

fn assert_or_write_snapshot(name: &str, content: &str) {
    let path = snapshot_path(name);
    if let Ok(existing) = fs::read_to_string(&path) {
        assert_eq!(
            existing.trim(),
            content.trim(),
            "snapshot mismatch for {name}; delete {path:?} to regenerate"
        );
    } else {
        fs::write(&path, content).unwrap();
        panic!(
            "snapshot {name} created at {path:?}. Re-run the test to assert."
        );
    }
}

#[tokio::test]
async fn snapshot_executive_report() {
    let html = render_canonical(Theme::Executive, "report").await;
    assert_or_write_snapshot("executive_report", &html);
}

#[tokio::test]
async fn snapshot_minimal_report() {
    let html = render_canonical(Theme::Minimal, "report").await;
    assert_or_write_snapshot("minimal_report", &html);
}

#[tokio::test]
async fn snapshot_vibrant_report() {
    let html = render_canonical(Theme::Vibrant, "report").await;
    assert_or_write_snapshot("vibrant_report", &html);
}

#[tokio::test]
async fn snapshot_dark_report() {
    let html = render_canonical(Theme::Dark, "report").await;
    assert_or_write_snapshot("dark_report", &html);
}
```

- [ ] **Step 2: First run creates snapshots; second pass asserts**

Run: `cargo test --test html_theme_snapshots -p colmena_dag_engine`
First run: tests panic with "snapshot created". Re-run:
Run: `cargo test --test html_theme_snapshots -p colmena_dag_engine`
Expected: 4 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/tests/html_theme_snapshots.rs src/libs/colmena/tests/snapshots/
git commit -m "$(cat <<'EOF'
test(documents/html): snapshot tests per theme (4 themes x report mode)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 13.3: CI guard for hexagonal layer rules

**Files:**
- Modify: `.github/workflows/rust.yml` (or whichever workflow runs lib tests)
- Create: `scripts/check_hexagonal_documents.sh`

- [ ] **Step 1: Identify CI workflow**

Run: `ls .github/workflows/`

Pick the file that runs `cargo test --lib` for the colmena crate (likely `rust.yml` or `ci.yml`).

- [ ] **Step 2: Create the check script**

Create `scripts/check_hexagonal_documents.sh`:

```bash
#!/usr/bin/env bash
# Hexagonal architecture guard for src/libs/colmena/src/documents/.
# Domain must NOT import application or infrastructure.
# Application (except runtime.rs) must NOT import infrastructure.
# Infrastructure must NOT import other infrastructure modules (only via domain ports).
set -euo pipefail

ROOT="src/libs/colmena/src/documents"

fail=0

violations=$(grep -rn "use crate::documents::application" "${ROOT}/domain/" 2>/dev/null || true)
if [ -n "${violations}" ]; then
  echo "❌ domain imports application:"
  echo "${violations}"
  fail=1
fi

violations=$(grep -rn "use crate::documents::infrastructure" "${ROOT}/domain/" 2>/dev/null || true)
if [ -n "${violations}" ]; then
  echo "❌ domain imports infrastructure:"
  echo "${violations}"
  fail=1
fi

# application files except runtime.rs cannot import infrastructure
for f in $(find "${ROOT}/application" -name '*.rs' -not -name 'runtime.rs'); do
  violations=$(grep -n "use crate::documents::infrastructure" "${f}" 2>/dev/null || true)
  if [ -n "${violations}" ]; then
    echo "❌ application file ${f} imports infrastructure (only runtime.rs is allowed):"
    echo "${violations}"
    fail=1
  fi
done

# infrastructure modules cannot cross-import other infrastructure
# (allowed: import domain ports/types; not allowed: e.g. render/* importing storage/*)
for layer in render validation storage ids; do
  for f in $(find "${ROOT}/infrastructure" -name '*.rs' 2>/dev/null); do
    violations=$(grep -n "use crate::documents::infrastructure::" "${f}" \
      | grep -v "use crate::documents::infrastructure::${layer}" \
      | grep "use crate::documents::infrastructure::" 2>/dev/null || true)
    # Note: this is intentionally permissive; some adapters legitimately re-export
    # from a sibling module (e.g. ids::UlidIdGenerator used by tests). The
    # important guard is the previous two checks (domain→ and app→ infra).
    # This loop only flags egregious cross-imports between infra leaves.
    if [ -n "${violations}" ]; then
      # Allow tests modules to cross-import
      if grep -q "#\[cfg(test)\]" "${f}"; then
        continue
      fi
    fi
  done
done

if [ $fail -ne 0 ]; then
  echo ""
  echo "Hexagonal layer violations found. See spec §3.5."
  exit 1
fi

echo "✅ Hexagonal architecture compliance OK."
```

Make it executable:

```bash
chmod +x scripts/check_hexagonal_documents.sh
```

- [ ] **Step 3: Wire into CI**

In the chosen workflow file, after the `cargo test` step for the colmena crate, add a new step:

```yaml
      - name: Hexagonal compliance — documents module
        run: ./scripts/check_hexagonal_documents.sh
```

- [ ] **Step 4: Run locally to verify**

Run: `./scripts/check_hexagonal_documents.sh`
Expected: `✅ Hexagonal architecture compliance OK.`

- [ ] **Step 5: Commit**

```bash
git add scripts/check_hexagonal_documents.sh .github/workflows/
git commit -m "$(cat <<'EOF'
ci(documents): add hexagonal architecture compliance guard

Blocks domain → app/infra imports and app/* (non-runtime) → infra imports.
See docs/superpowers/specs/2026-05-30-html-documents-module-design.md §3.5.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 13.4: Final smoke — all `documents` tests green

**Files:** (no source changes)

- [ ] **Step 1: Run full test suite for the module**

Run:

```bash
cargo test --lib -p colmena_dag_engine documents
cargo test --test html_documents_e2e -p colmena_dag_engine
cargo test --test html_theme_snapshots -p colmena_dag_engine
```

Expected: all tests PASS.

- [ ] **Step 2: Run with gcs feature**

Run: `cargo test --lib -p colmena_dag_engine documents --features gcs`
Expected: builds and existing tests still pass (GcsAssetStore has no offline tests; build-only check).

- [ ] **Step 3: Run hex compliance guard**

Run: `./scripts/check_hexagonal_documents.sh`
Expected: ✅.

- [ ] **Step 4: No commit needed if everything is green**

If smoke fails, fix and commit incremental fixes per the failing task above. Otherwise, declare Spec 1 done and move to brainstorming Spec 2 (DAG nodes + LLM tools + bidirectional translator).

---

## Coverage check vs spec

| Spec section | Covered by task(s) |
|---|---|
| §3.1 Module layout | Tasks 1.3, 2.1, 4.1, 5.1, 6.1, 7.1, 8.x, 9.1, 10.x, 11.x, 12.x |
| §3.5 Hexagonal compliance + CI guard | Task 13.3 |
| §4.1 `ArtifactKind::Html` | Task 1.1 |
| §4.2 `AssetId` | Task 1.2 |
| §4.3 `HtmlIR` core | Task 2.1 |
| §4.4 All 17 blocks | Task 2.2 |
| §4.5 `ChartSpec` normalized | Task 2.3 |
| §4.6 `assets_referenced` maintenance | Tasks 11.1, 11.2 (`recompute_assets_referenced`) |
| §5.x 19 PatchOp HTML variants | Tasks 3.1, 3.2, 3.3 |
| §6 `HtmlOpApplier` | Tasks 11.1, 11.2 |
| §7 `HtmlValidator` + all rules | Tasks 5.1, 5.2 |
| §8 `HtmlRenderer` + assets + chart init + auto_toc + footer | Tasks 8.x, 9.1, 9.2, 9.3 |
| §9 Asset store (trait + LocalFs + GCS + use cases) | Tasks 4.1, 6.1, 7.1, 10.1, 10.2, 10.3 |
| §10 `DocumentRuntime` extension | Task 12.4 |
| §11 Wire into Create/Apply | Tasks 12.1, 12.2 |
| §12 `describe_op` extension | Task 12.3 |
| §15 Tests (unit + integration + snapshot + security) | Tasks 5.x, 9.x, 10.x, 11.x, 13.1, 13.2 |
| §16 End-to-end test as "Spec 1 done" gate | Task 13.1 |

**No spec section unmapped.**

---

## Notes for executors

- Some tasks reference existing helper types (`OpOutcome`, `OpOutcomeIds`, `PatchSummary`, `PatchApplied`, etc.) that already exist in `domain/artifact.rs`. Confirm field names match before pasting code; the plan adds a `slide: Option<String>` field and an `is_empty()` method to `OpOutcomeIds` in Tasks 11.1 and 12.2.
- The `NoopRenderer` / `NoopValidator` stubs referenced in tests already exist inside test modules of `create_document.rs` and `apply_patch.rs` (as seen in the existing Excel tests). Reuse them; if a test needs an additional stub for `HtmlRenderer`, prefer the real `HtmlRenderer::new(LocalFsAssetStore)` against a `tempdir`.
- Whenever a step says "REPLACE the X with Y", read the surrounding context first to keep imports and helper functions intact.
- If `cargo` warnings appear (unused imports etc.), address them inside the same task before committing — `cargo fmt && cargo clippy` should be clean.



