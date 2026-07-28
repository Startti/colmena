# src/libs/colmena/src/documents/domain/ir/html.rs

**Layer:** domain  
**Purpose:** Pure domain value objects and enums defining the HTML Intermediate Representation (IR) schema for HTML artifact generation. Describes document structure, styling, content blocks, and metadata without any infrastructure or application-layer dependencies.

## Symbols

- `Theme` (pub enum) — Presentation theme variants (Executive, Minimal, Vibrant, Dark)
- `Locale` (pub enum) — Language locales (En, Es)
- `LayoutMode` (pub enum) — Document layout modes (Report, Slides)
- `SlideLayout` (pub enum) — Slide-specific layout types (Title, Content, SectionDivider, Blank)
- `ImagePosition` (pub enum) — Image positioning strategies (Inline, Full, Hero)
- `ChartSize` (pub enum) — Chart dimension options (Small, Medium, Large)
- `ColumnRatio` (pub enum) — Column width ratios (FiftyFifty, FortySixty, SixtyForty, ThirtySeventy, SeventyThirty)
- `Gap` (pub enum) — Spacing size options (Small, Medium, Large)
- `CalloutVariant` (pub enum) — Callout box styling variants (Info, Warning, Success, Danger)
- `DeltaDirection` (pub enum) — Direction indicator for KPI metrics (Up, Down, Neutral)
- `HtmlIR` (pub struct) — Root container holding document metadata, theme, layout mode, footer config, and slide collection; includes `assets_referenced` vector maintained by HtmlOpApplier
- `DocProps` (pub struct) — Document metadata (title, author, date, locale)
- `FooterConfig` (pub struct) — Footer rendering configuration (enabled, page_numbers, custom_text)
- `Slide` (pub struct) — Single slide/page containing id, layout type, title, subtitle, notes, and block collection
- `ChartSpec` (pub struct) — Chart specification with type, series, optional axis specs, legend flag, and palette
- `default_true()` (fn, private) — Serde default function returning true for ChartSpec.legend field
- `ChartType` (pub enum) — Chart display types (Bar, Line, Pie, Doughnut, Area, Scatter, Radar)
- `ChartSeries` (pub struct) — Named data series within a chart (name, f64 data vec)
- `AxisSpec` (pub struct) — Chart axis configuration (optional title, categories, min/max bounds)
- `Block` (pub enum, tagged union) — Polymorphic content block with 16 variants: Heading, Paragraph, List, Blockquote, Code, Table, Chart, KpiCard, KpiGrid, Image, TwoColumns, ThreeColumns, Comparison, Callout, Divider, Video, AutoToc
- `Run` (pub struct) — Text run with formatting flags (id, text, bold, italic, underline, code, optional link)
- `ListItem` (pub struct) — List item containing id and text runs
- `TableRow` (pub struct) — Table row with id and cells collection
- `TableCell` (pub enum, tagged union) — Cell content variants: Text (string), Number (f64 with optional format), Runs (vec of Run)
- `KpiDelta` (pub struct) — KPI metric change indicator (value string, direction)
- `KpiCardInline` (pub struct) — Inline KPI card (label, value, optional delta, optional icon)
- `ImageSrc` (pub enum, tagged union) — Image source variants: Asset (asset_id), DataUrl (data string), External (url)
- `ComparisonPanel` (pub struct) — Panel in comparison block (header, runs, highlight bool)
- `VideoSrc` (pub enum, tagged union) — Video source variants: Youtube (video_id), Vimeo (video_id), Asset (asset_id)

## File-level notes

- **Purity enforced:** Zero imports from application/ or infrastructure/. Only serde, schemars, and stdlib.
- **Schema coverage:** Comprehensive IR covering text, charts, tables, images, video, KPI cards, multi-column layouts, callouts, and table-of-contents blocks.
- **Serde tags:** Consistent use of `#[serde(tag = "kind")]` for Block, `#[serde(tag = "type")]` for TableCell, `#[serde(tag = "provider")]` for VideoSrc, `#[serde(tag = "kind")]` for ImageSrc. All variants properly renamed for JSON.
- **Test coverage:** 11 tests (lines 387–568) covering serialization roundtrips, invalid inputs, serde tagging, and complex nested structures. All tests pass.
- **No dead code:** All public types are referenced in domain interfaces or used by HtmlOpApplier/HtmlValidator per comments.
- **No unfinished code:** All enums, structs, and implementations are complete.
