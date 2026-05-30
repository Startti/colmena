# HTML Documents — Módulo Rust (Spec 1)

**Fecha:** 2026-05-30
**Autor:** Daniel Garcia (con asistencia de Claude)
**Estado:** Draft — pending user review
**Scope:** Módulo Rust del nuevo `ArtifactKind::Html` dentro de `src/libs/colmena/src/documents/`. Únicamente IR + ops + applier + validator + renderer + asset store + use cases de assets. **No incluye** DAG nodes ni LLM tools (Spec 2 separada).

**Referencia:** Extiende el módulo descrito en [2026-04-21-documents-feature-design.md](2026-04-21-documents-feature-design.md) (Excel/Word). Reutiliza ~90% del patrón hexagonal existente.

---

## 1. Overview

Agregar al módulo `documents/` un tercer kind — `Html` — para producir **reportes y presentaciones (slides) interactivas en HTML self-contained**. Igual que Excel y Word:

- **Source of truth en JSON IR** — `HtmlIR` con slides → blocks tipados.
- **Edición por patches** atómicos con 16 nuevas variantes de `PatchOp`.
- **Versionado inmutable** vía el `ArtifactStore` existente.
- **Render on-demand** a un único archivo `.html` con CSS, JS, imágenes y charts embebidos.

Suma una pieza nueva: el **`AssetStore`** — un store paralelo para binarios (logos, imágenes generadas) scoped por `SessionId`, con backends LocalFs y GCS espejando el patrón del `ArtifactStore`.

El renderer usa `maud` (macros Rust type-safe) y embebe Chart.js + un runtime mínimo de slides (keyboard nav + fullscreen) sólo cuando hace falta.

---

## 2. Goals & Non-Goals

### Goals

- Permitir crear un `ArtifactKind::Html` y aplicarle patches incrementales con la misma API que Excel/Word.
- Producir un HTML self-contained válido (un solo archivo, abrible en cualquier browser, imprimible decente vía `@media print`).
- Soportar dos modos de layout — **report** (scroll continuo) y **slides** (paginado, con navegación)— sobre la misma IR.
- Modelo de bloques composable (~15 tipos: heading, paragraph, list, table, chart, kpi_card, kpi_grid, image, two_columns, three_columns, comparison, callout, divider, video, auto_toc) que cubre 95% de los casos sin requerir presets de layout cerrados.
- Themes nombrados (`executive | minimal | vibrant | dark`) que cuidan tipografía, paleta, paddings y CSS de print — sin que el agente tenga que decidir estilos inline.
- Asset store para imágenes/logos compartibles entre artifacts de una misma sesión.
- Seguridad robusta de XSS: text escapado por construcción (`maud`), URL scheme allowlist en hyperlinks y srcs de imagen.
- Charts vía Chart.js con IR normalizado (no exponer la config raw de Chart.js) — el swap a otra librería en v2 no requiere migración de IR.

### Non-Goals (esta spec)

- **DAG nodes** nuevos (`document_export`, `document_get_url`, etc.) — van a Spec 2.
- **LLM synthetic tools** nuevos — van a Spec 2.
- **Use cases genéricos del runtime** (`export`, `get_outline`, `get_render_url`) — van a Spec 2.
- **Extensión del slicing** en `document_read` para slide_ids/block_ids — va a Spec 2.
- **Integración con `image_generation` feature** — va a Spec 2.
- **Traductor bidireccional bytes↔IR** (ingesta de `.html` existente) — va a Spec 2.
- **Export PDF server-side** (chromium headless / weasyprint) — roadmap v3+.
- **Cache de render por versión** — optimización, roadmap v2.
- **Frontend de edición + `PatchSource::User`** — requiere UI, roadmap v2.
- **Templates / data binding** (reportes recurrentes con data fresca) — requiere rediseño de IR, roadmap v2.
- **Cross-slide `MoveBlockToSlide`** — delete + insert lo cubre, roadmap v2.
- **Slide transitions / animations** — roadmap v2.

---

## 3. Architecture

### 3.1. Ubicación en el repo

El código vive dentro del crate existente:

```
src/libs/colmena/src/documents/
├── domain/
│   ├── ids.rs              # + ArtifactKind::Html, + AssetId
│   ├── ports.rs            # + AssetStore trait, + AssetError
│   ├── patch.rs            # + 16 variantes HTML de PatchOp
│   ├── error.rs            # + AssetError variants
│   └── ir/
│       ├── mod.rs          # + HtmlIR re-export
│       └── html.rs         # NUEVO — schema completo de HtmlIR
├── application/
│   ├── apply_patch.rs      # extender: match kind Html → HtmlOpApplier
│   ├── create_document.rs  # extender: empty_ir() para Html, validator/renderer
│   ├── apply_html_ops.rs   # NUEVO — HtmlOpApplier (mut HtmlIR según ops)
│   ├── upload_asset.rs     # NUEVO — UploadAssetUseCase
│   ├── list_assets.rs      # NUEVO — ListAssetsUseCase
│   ├── delete_asset.rs     # NUEVO — DeleteAssetUseCase (con ref-count check)
│   └── runtime.rs          # extender: bundle asset use cases, asset_store campo
└── infrastructure/
    ├── render/
    │   ├── mod.rs          # + HtmlRenderer re-export
    │   ├── html_renderer.rs           # NUEVO — implementa IRRenderer para HTML
    │   └── html_assets/               # NUEVO — embebidos via include_str!
    │       ├── themes/
    │       │   ├── executive.css
    │       │   ├── minimal.css
    │       │   ├── vibrant.css
    │       │   └── dark.css
    │       ├── chart_js.min.js
    │       └── slides_runtime.js
    ├── validation/
    │   ├── mod.rs          # + HtmlValidator re-export
    │   └── html_validator.rs          # NUEVO — implementa IRValidator
    └── storage/
        ├── mod.rs          # + AssetStore impls re-export
        ├── local_fs_asset_store.rs    # NUEVO
        └── gcs_asset_store.rs         # NUEVO (cfg gcs feature)
```

### 3.2. Capas (hexagonal)

| Capa | Qué vive acá (lo nuevo) |
|------|--------------------------|
| **Domain** | `HtmlIR` (entidad), `PatchOp::*Html` (16 variantes), `AssetStore` (puerto), `AssetId` (VO), `AssetError` |
| **Application** | `HtmlOpApplier` (lógica de mutación), use cases `UploadAsset`/`ListAssets`/`DeleteAsset`, `DocumentRuntime` extendido |
| **Infrastructure** | `HtmlRenderer` (maud), `HtmlValidator`, `LocalFsAssetStore`, `GcsAssetStore`, assets embebidos |

### 3.3. Flujo de creación / edición HTML

```
caller (Rust)
   │
   ▼
DocumentRuntime
   │
   ├── create.execute(CreateDocumentInput { kind: Html, ... })
   │     │
   │     ├── empty_ir() → HtmlIR vacío con 1 slide-contenedor
   │     ├── HtmlValidator.validate(ir)
   │     ├── HtmlRenderer.render(ir) → bytes (.html self-contained)
   │     ├── store.create_artifact(meta)
   │     ├── store.write_version(v1, { ir, bytes, ... })
   │     └── store.set_head(None, v1)
   │
   └── apply.execute(ApplyPatchInput { patch })
         │
         ├── store.read_version(current) → VersionData
         ├── deserialize ir → HtmlIR
         ├── HtmlOpApplier.apply(&mut ir, op) for each op
         │     └── retorna OpOutcome con assigned_ids (slide_id, block_id, ...)
         ├── HtmlValidator.validate(ir)   ← post-mutación
         ├── HtmlRenderer.render(ir) → bytes nuevos
         └── store.write_version(vN+1) + set_head(current, vN+1, CAS)
```

### 3.4. Flujo de assets (paralelo, independiente del versionado de artifact)

```
caller (Rust)
   │
   ▼
DocumentRuntime
   │
   ├── upload_asset.execute({ session_id, bytes, mime, label? })
   │     ├── valida mime contra allowlist (image/*, video/* opcional)
   │     ├── valida tamaño contra max (config, default 10 MB)
   │     ├── genera AssetId (ULID, prefijo "asset_")
   │     └── asset_store.upload(session, id, bytes, mime, label)
   │
   ├── list_assets.execute({ session_id })
   │     └── asset_store.list_by_session(session) → Vec<AssetSummary>
   │
   └── delete_asset.execute({ asset_id })
         ├── busca todos los artifacts de la sesión del asset
         ├── chequea ningún HtmlIR.assets_referenced[] lo contenga
         └── si OK → asset_store.delete(id); si no → AssetError::StillReferenced
```

Los assets se referencian desde la IR vía `src: { kind: "asset", asset_id: "asset_xxx" }` en los bloques `image` (y `video` cuando es archivo subido, no embed). En render time el `HtmlRenderer` resuelve el `asset_id` cargando los bytes y embebiéndolos como `data:` URL — manteniendo el output self-contained.

---

## 4. Domain — IR y tipos

### 4.1. `ArtifactKind` extendido

```rust
// domain/ids.rs
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    Excel,
    Word,
    Html,    // ← nuevo
}

impl ArtifactKind {
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Excel => "xlsx",
            Self::Word => "docx",
            Self::Html => "html",
        }
    }
    pub fn mime(&self) -> &'static str {
        match self {
            Self::Excel => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Self::Word => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Html => "text/html; charset=utf-8",
        }
    }
}
```

### 4.2. `AssetId`

```rust
// domain/ids.rs
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct AssetId(pub String);  // formato: "asset_<ulid>"

impl AssetId {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
}
```

### 4.3. `HtmlIR` — schema completo

```rust
// domain/ir/html.rs
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HtmlIR {
    /// Discriminador del kind. Siempre "html".
    pub kind: String,
    pub artifact_id: String,
    pub version_id: String,
    pub schema_version: String,    // "1.0.0"

    pub doc_props: DocProps,
    pub theme: String,             // "executive" | "minimal" | "vibrant" | "dark"
    pub layout_mode: LayoutMode,
    pub footer: FooterConfig,
    pub slides: Vec<Slide>,

    /// IDs de assets referenciados desde cualquier block de la IR.
    /// Mantenido por HtmlOpApplier al insertar/remover image/video blocks.
    /// Usado por DeleteAssetUseCase para impedir borrado de assets vivos.
    pub assets_referenced: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DocProps {
    pub title: Option<String>,
    pub author: Option<String>,
    pub date: Option<String>,
    pub locale: Locale,            // "en" | "es"
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Locale { En, Es }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode { Report, Slides }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FooterConfig {
    pub enabled: bool,
    pub page_numbers: bool,
    pub custom_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Slide {
    pub id: String,                // "sl_<ulid>"
    pub layout: SlideLayout,
    pub title: Option<String>,     // slot del layout (no es un block)
    pub subtitle: Option<String>,  // solo aplica a layout=title
    pub notes: Option<String>,     // speaker notes — no visibles en slide normal
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlideLayout {
    Title,            // título grande centrado + subtítulo
    Content,          // título arriba + blocks libres abajo (default)
    SectionDivider,   // separador de capítulo
    Blank,            // sin slot de título, solo blocks
}
```

### 4.4. Bloques — schema

Todos los bloques llevan `id: "bk_<ulid>"` estable y un discriminador `kind`. Definidos como un enum tagged:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum Block {
    #[serde(rename = "heading")]
    Heading {
        id: String,
        level: u8,                 // 1..=6
        runs: Vec<Run>,
    },
    #[serde(rename = "paragraph")]
    Paragraph {
        id: String,
        runs: Vec<Run>,
    },
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
        chart: ChartSpec,          // ver §4.5
        title: Option<String>,
        size: ChartSize,           // small | medium | large
    },
    #[serde(rename = "kpi_card")]
    KpiCard {
        id: String,
        label: String,
        value: String,
        delta: Option<KpiDelta>,
        icon: Option<String>,      // nombre de icono del theme
    },
    #[serde(rename = "kpi_grid")]
    KpiGrid {
        id: String,
        columns: u8,               // 2 | 3 | 4
        cards: Vec<KpiCardInline>, // versión inline sin id propio
    },
    #[serde(rename = "image")]
    Image {
        id: String,
        src: ImageSrc,
        alt: String,
        caption: Option<String>,
        position: ImagePosition,   // inline | full | hero
    },
    #[serde(rename = "two_columns")]
    TwoColumns {
        id: String,
        left: Vec<Block>,
        right: Vec<Block>,
        ratio: ColumnRatio,        // "50_50" | "40_60" | "60_40" | "30_70" | "70_30"
        gap: Gap,                  // small | medium | large
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
        variant: CalloutVariant,   // info | warning | success | danger
        title: Option<String>,
        runs: Vec<Run>,
    },
    #[serde(rename = "divider")]
    Divider { id: String },
    #[serde(rename = "video")]
    Video {
        id: String,
        src: VideoSrc,             // youtube { id } | vimeo { id } | asset { asset_id }
        caption: Option<String>,
    },
    #[serde(rename = "auto_toc")]
    AutoToc {
        id: String,
        title: Option<String>,
        depth: u8,                 // 1 = solo section_dividers; 2 = + slide titles
    },
}

/// Texto con estilo + hyperlink opcional.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Run {
    pub id: String,                // "rn_<ulid>"
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub code: bool,
    pub link: Option<String>,      // URL — validada por HtmlValidator
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListItem {
    pub id: String,                // "li_<ulid>"
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TableRow {
    pub id: String,                // "tr_<ulid>"
    pub cells: Vec<TableCell>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum TableCell {
    Text { value: String },
    Number { value: f64, format: Option<String> },
    Runs { runs: Vec<Run> },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KpiDelta {
    pub value: String,             // "+12.3%" o "-50"
    pub direction: DeltaDirection,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DeltaDirection { Up, Down, Neutral }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KpiCardInline {
    pub label: String,
    pub value: String,
    pub delta: Option<KpiDelta>,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum ImageSrc {
    #[serde(rename = "asset")]
    Asset { asset_id: String },
    #[serde(rename = "data_url")]
    DataUrl { data: String },      // "data:image/png;base64,..."
    #[serde(rename = "external")]
    External { url: String },      // http/https only — validado
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ImagePosition { Inline, Full, Hero }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChartSize { Small, Medium, Large }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum ColumnRatio {
    #[serde(rename = "50_50")] FiftyFifty,
    #[serde(rename = "40_60")] FortySixty,
    #[serde(rename = "60_40")] SixtyForty,
    #[serde(rename = "30_70")] ThirtySeventy,
    #[serde(rename = "70_30")] SeventyThirty,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Gap { Small, Medium, Large }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ComparisonPanel {
    pub header: String,
    pub runs: Vec<Run>,
    pub highlight: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CalloutVariant { Info, Warning, Success, Danger }

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

### 4.5. `ChartSpec` — IR normalizado (independiente de lib)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChartSpec {
    pub chart_type: ChartType,
    pub series: Vec<ChartSeries>,
    pub x_axis: Option<AxisSpec>,  // categorías o numérico
    pub y_axis: Option<AxisSpec>,
    pub legend: bool,              // mostrar leyenda
    pub palette: Option<String>,   // override del theme, opcional
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChartType { Bar, Line, Pie, Doughnut, Area, Scatter, Radar }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChartSeries {
    pub name: String,
    pub data: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AxisSpec {
    pub title: Option<String>,
    pub categories: Option<Vec<String>>,   // si presente, eje categórico
    pub min: Option<f64>,
    pub max: Option<f64>,
}
```

**Por qué no exponer Chart.js raw config:** swapeable a ApexCharts/ECharts en v2 sin migración de IR; reduce la superficie que el agente tiene que conocer; impide options que rompen visualmente el theme.

### 4.6. `assets_referenced` — mantenido por el applier

El campo `HtmlIR.assets_referenced` es la lista de `asset_id`s alcanzables desde algún `Block::Image { src: Asset { asset_id } }` o `Block::Video { src: Asset { asset_id } }` en la IR. Se recalcula incrementalmente por `HtmlOpApplier` en cada `insert_block` / `delete_block` / `replace_block` que toque image/video blocks.

Es **descriptivo, no autoritativo**: la fuente de verdad sigue siendo escanear los blocks. Pero mantenerlo cacheado en la IR habilita lookups O(1) durante `DeleteAssetUseCase`.

---

## 5. Patch ops — 16 nuevas variantes HTML

Se agregan a `PatchOp` en `domain/patch.rs`. El discriminador `op` ya existe y soporta serde tag — solo se suman variantes.

### 5.1. Slide-level (6)

```rust
#[serde(rename = "add_slide")]
AddSlide {
    layout: SlideLayout,
    at_index: Option<u32>,
    title: Option<String>,
    subtitle: Option<String>,
},
// → assigned_ids.slide en OpOutcome

#[serde(rename = "delete_slide")]
DeleteSlide { slide_id: String },

#[serde(rename = "reorder_slides")]
ReorderSlides { order: Vec<String> },

#[serde(rename = "set_slide_layout")]
SetSlideLayout { slide_id: String, layout: SlideLayout },

#[serde(rename = "set_slide_title")]
SetSlideTitle {
    slide_id: String,
    title: Option<String>,
    subtitle: Option<String>,
},

#[serde(rename = "set_slide_notes")]
SetSlideNotes { slide_id: String, notes: Option<String> },
```

### 5.2. Block-level (4)

```rust
#[serde(rename = "insert_html_block")]
InsertHtmlBlock {
    slide_id: String,
    before: Option<String>,        // block_id
    after: Option<String>,
    block: serde_json::Value,      // Block tagged JSON, id asignado server-side
},
// → assigned_ids.block

#[serde(rename = "delete_html_block")]
DeleteHtmlBlock { slide_id: String, block_id: String },

#[serde(rename = "replace_html_block")]
ReplaceHtmlBlock {
    slide_id: String,
    block_id: String,
    block: serde_json::Value,      // preserva block_id; valida shape
},

#[serde(rename = "move_html_block")]
MoveHtmlBlock {
    slide_id: String,
    block_id: String,
    after_block_id: String,
},
```

**Nota de naming:** sufijo `_html_block` para evitar colisión con `insert_block` / `delete_block` de Word. Aunque `serde_json` resolvería por shape, mantener tags únicos hace la validación más temprana y los errores más claros.

### 5.3. Table-specific (3)

```rust
#[serde(rename = "insert_html_table_row")]
InsertHtmlTableRow {
    slide_id: String,
    table_block_id: String,
    before: Option<String>,        // row_id (tr_xxx)
    after: Option<String>,
    cells: Vec<serde_json::Value>, // TableCell tagged JSON
},
// → assigned_ids.rows[0]

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
    col_index: u32,                // 0-indexed
    cell: serde_json::Value,       // TableCell tagged JSON
},
```

### 5.4. List-specific (3)

```rust
#[serde(rename = "insert_html_list_item")]
InsertHtmlListItem {
    slide_id: String,
    list_block_id: String,
    at_index: u32,
    runs: Vec<serde_json::Value>,
},
// → assigned_ids.list_items[0]

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

### 5.5. Document-level (3 — no son 16 si los contamos, pero conceptualmente cierran el set)

```rust
#[serde(rename = "set_theme")]
SetTheme { theme: String },

#[serde(rename = "set_doc_props")]
SetDocProps {
    title: Option<String>,
    author: Option<String>,
    date: Option<String>,
    locale: Option<Locale>,
},

#[serde(rename = "set_footer")]
SetFooter { footer: FooterConfig },
```

**Total: 19 ops nuevas.** (El conteo "16" del scope inicial era aproximado; al detallar quedaron 19 sin perder cohesión. Ninguna es opcional dada la decisión de mantener `set_slide_notes` y `set_footer` separados de `set_doc_props`.)

### 5.6. Regla de IDs heredada

Igual que Excel/Word:
- Los IDs **existentes** se pasan en la op.
- Los IDs **nuevos** los genera el server (ULID con prefijo) y se devuelven en `ApplyPatchOutput.summary.structured[*].assigned_ids`.
- **Nunca usar un ID recién generado en el mismo patch.** Splitear en patches sucesivos.

---

## 6. HtmlOpApplier — application/apply_html_ops.rs

Espejo de `ExcelOpApplier` y `WordOpApplier`. Recibe `&mut HtmlIR` y un `PatchOp` HTML, muta in-place y devuelve un `OpOutcome` con los IDs asignados.

```rust
pub struct HtmlOpApplier<'a> {
    pub ids: &'a dyn IdGenerator,
}

impl<'a> HtmlOpApplier<'a> {
    pub fn apply(
        &self,
        ir: &mut HtmlIR,
        op: &PatchOp,
    ) -> Result<OpOutcome, DocumentError> {
        match op {
            PatchOp::AddSlide { layout, at_index, title, subtitle } => { ... }
            PatchOp::DeleteSlide { slide_id } => { ... }
            // ... cada variante HTML
            _ => Err(DocumentError::OpNotApplicable {
                op: op_tag(op),
                kind: "html".into(),
            }),
        }
    }
}
```

### 6.1. Reglas críticas implementadas por el applier

- **Atomicidad:** el applier muta la IR en memoria; si una op falla, la IR no se commitea (el use case retorna error sin escribir). Esto ya lo cubre `ApplyPatchUseCase` aplicando todas las ops a una IR local antes de persistir.
- **Mantenimiento de `assets_referenced`:** al insertar/borrar/reemplazar bloques `Image`/`Video` con `src::Asset`, el applier recalcula el set.
- **Generación de IDs:** usa `self.ids` (inyectado) para producir nuevos `sl_*`, `bk_*`, `tr_*`, `li_*`, `rn_*`.
- **Validación de referencias internas:** `DeleteSlide` falla si hay menos de 1 slide tras la op (la IR debe tener siempre ≥1 slide). En `layout_mode: "report"`, `AddSlide`/`DeleteSlide` están permitidos pero el renderer ignora todos los slides excepto el primero — el validator no rechaza, solo el render comporta.
- **`SetSlideLayout`**: cambiar a `Title` o `SectionDivider` requiere `slide.title` no-vacío (validation en applier).
- **Identidad de blocks en `ReplaceHtmlBlock`:** preserva el `id` original (lo sobrescribe en el nuevo JSON antes de deserializar a `Block`).

---

## 7. HtmlValidator — infrastructure/validation/html_validator.rs

Implementa `IRValidator`. Corre antes del renderer en `CreateDocumentUseCase` y después del applier en `ApplyPatchUseCase`.

### 7.1. Reglas

1. **Schema parse** — la IR JSON deserializa a `HtmlIR` sin error.
2. **Unicidad de IDs:**
   - `slides[*].id` únicos.
   - `blocks[*].id` únicos a través de TODOS los slides (no solo dentro de un slide).
   - `runs[*].id`, `items[*].id`, `rows[*].id` únicos dentro de su contenedor.
3. **Cardinalidad mínima:** `slides.len() >= 1`.
4. **Layouts válidos:**
   - `SlideLayout::Title` requiere `slide.title` no-vacío.
   - `SlideLayout::SectionDivider` requiere `slide.title` no-vacío.
5. **Theme válido:** valor en `{"executive", "minimal", "vibrant", "dark"}`. Otros → error.
6. **Heading level:** `1..=6`.
7. **Chart consistency:**
   - Para `ChartType::Bar | Line | Area | Radar`: si `x_axis.categories` está presente, `series[i].data.len() == categories.len()` para todo `i`.
   - Para `ChartType::Pie | Doughnut`: una sola serie permitida.
   - `series.len() >= 1` siempre.
8. **URL scheme allowlist (CRÍTICO — seguridad XSS):**
   - `Run.link` (cualquier run en cualquier block): scheme en `{"http", "https", "mailto", "tel"}`. Resto → error.
   - `ImageSrc::External.url`: scheme en `{"http", "https"}`.
   - `ImageSrc::DataUrl.data`: debe matchear regex `^data:image/(png|jpeg|jpg|gif|webp|svg\+xml);base64,`. SVG con `<script>` adentro es responsabilidad del validador externo de SVG (out of scope v1 — no aceptar SVG en data_url, solo asset).
   - `VideoSrc::Youtube/Vimeo.video_id`: regex alfanumérico simple (`^[A-Za-z0-9_-]+$`).
9. **Asset refs existen:** todos los `asset_id` referenciados desde Image/Video deben aparecer en `HtmlIR.assets_referenced` (consistencia mantenida por applier; el validator chequea cierre).
10. **KpiGrid columns:** `columns ∈ {2, 3, 4}` y `cards.len() == columns` (idealmente, o ≤ columns con celdas vacías permitidas — decisión: requerir `==` para simplicidad).
11. **TwoColumns/ThreeColumns no anidados infinito:** profundidad máxima de bloques anidados = 3 niveles (rechaza grids dentro de grids dentro de grids para evitar DOM explosivo).

### 7.2. Tests obligatorios del validator

| Test | Expectativa |
|---|---|
| `valid_minimal_ir` | IR de 1 slide vacío con theme válido → OK |
| `duplicate_block_ids_across_slides` | Error |
| `chart_mismatch_categories_vs_data` | Error con path al chart |
| `link_with_javascript_scheme` | Error con mensaje "URL scheme not allowed" |
| `image_data_url_with_script_payload` | Error (regex rechaza no-image) |
| `unknown_theme_value` | Error |
| `heading_level_7` | Error |
| `pie_chart_two_series` | Error |
| `nested_grids_4_deep` | Error |
| `title_slide_without_title` | Error |

---

## 8. HtmlRenderer — infrastructure/render/html_renderer.rs

Implementa `IRRenderer`. Recibe el IR JSON, devuelve `Vec<u8>` (el `.html` final como UTF-8).

### 8.1. Estrategia

Construcción con `maud::html!` — type-safe, auto-escape, sintaxis declarativa Rust.

```rust
use maud::{html, DOCTYPE, Markup, PreEscaped};

pub struct HtmlRenderer {
    asset_store: Arc<dyn AssetStore>,  // para resolver asset_id → bytes
}

#[async_trait]
impl IRRenderer for HtmlRenderer {
    async fn render(&self, ir: &serde_json::Value) -> Result<Vec<u8>, RenderError> {
        let ir: HtmlIR = serde_json::from_value(ir.clone())
            .map_err(|e| RenderError::Schema(e.to_string()))?;

        let asset_bytes = self.resolve_assets(&ir).await?;  // HashMap<AssetId, (Vec<u8>, mime)>
        let theme_css = load_theme(&ir.theme);
        let needs_chart = ir.slides.iter().any(|s| s.has_chart_block());
        let needs_slides_runtime = matches!(ir.layout_mode, LayoutMode::Slides);

        let head = render_head(&ir, theme_css, needs_chart, needs_slides_runtime);
        let body = render_body(&ir, &asset_bytes);
        let chart_init_js = render_chart_init_scripts(&ir);

        let doc = html! {
            (DOCTYPE)
            html lang=(locale_to_html_lang(&ir.doc_props.locale)) {
                (head)
                body {
                    (body)
                    @if !chart_init_js.is_empty() {
                        script { (PreEscaped(chart_init_js)) }
                    }
                }
            }
        };

        Ok(doc.into_string().into_bytes())
    }

    fn target_extension(&self) -> &'static str { "html" }
    fn target_mime(&self) -> &'static str { "text/html; charset=utf-8" }
}
```

### 8.2. Assets embebidos vía `include_str!`

```rust
const THEME_EXECUTIVE: &str = include_str!("html_assets/themes/executive.css");
const THEME_MINIMAL:   &str = include_str!("html_assets/themes/minimal.css");
const THEME_VIBRANT:   &str = include_str!("html_assets/themes/vibrant.css");
const THEME_DARK:      &str = include_str!("html_assets/themes/dark.css");
const CHART_JS:        &str = include_str!("html_assets/chart_js.min.js");
const SLIDES_RUNTIME:  &str = include_str!("html_assets/slides_runtime.js");
```

Carga condicional: Chart.js (~190KB) solo se inlinea si la IR contiene al menos un `Block::Chart`. `slides_runtime.js` (~3KB) solo si `layout_mode == "slides"`.

### 8.3. Themes — contrato de CSS

Cada `theme.css` debe exponer:

| Selector / variable CSS | Propósito |
|---|---|
| `:root { --color-primary, --color-secondary, --color-bg, --color-text, --color-muted, ... }` | Paleta del theme |
| `:root { --font-heading, --font-body, --font-mono }` | Tipografía |
| `:root { --radius, --space-sm, --space-md, --space-lg }` | Spacing y bordes |
| `.slide`, `.slide--title`, `.slide--content`, `.slide--section-divider`, `.slide--blank` | Layouts de slide |
| `.block-heading`, `.block-paragraph`, ..., `.block-auto-toc` | Cada bloque |
| `.kpi-card`, `.kpi-card__value`, `.kpi-card__delta--up/down/neutral` | KPI cards |
| `.chart-container`, `.chart-container--small/medium/large` | Charts |
| `.two-columns`, `.two-columns--50-50`, ..., `.gap-small/medium/large` | Layouts de columnas |
| `.callout--info/warning/success/danger` | Callouts |
| `@media print { ... }` | Reglas de impresión: page-breaks entre slides, hide footer-nav, white background |
| Chart.js color palette JSON via CSS-vars consumido por `render_chart_init_scripts` | Palette de charts coherente con theme |

Cada CSS pesa entre 5-10 KB.

### 8.4. `slides_runtime.js` — qué hace

Cuando `layout_mode == "slides"`:

- Captura `ArrowRight` / `ArrowLeft` / `Space` / `PageUp` / `PageDown` → scroll a próximo/anterior slide.
- `F` → toggle `Element.requestFullscreen()` en `.deck`.
- Indicador "3 / 12" fijo en esquina (oculto en print).
- `Esc` → salir de fullscreen.
- No dependencias externas. ~80 líneas de vanilla JS.

### 8.5. Resolución de assets

`resolve_assets(&ir)` recolecta todos los `asset_id` referenciados en Image/Video blocks, los carga del `AssetStore` y arma un `HashMap`. El renderer entonces sustituye `ImageSrc::Asset { asset_id }` → `<img src="data:{mime};base64,{base64(bytes)}">`.

Si un asset no se encuentra → `RenderError::AssetNotFound { asset_id }` (fallo temprano, no render parcial).

### 8.6. Charts — inicialización

Cada `Block::Chart` se renderiza como:

```html
<div class="chart-container chart-container--large">
  <canvas id="chart_bk_xyz"></canvas>
</div>
```

Y al final del body, en un único `<script>`:

```javascript
new Chart(document.getElementById("chart_bk_xyz"), { type: "bar", data: {...}, options: {...} });
new Chart(document.getElementById("chart_bk_abc"), ...);
```

`render_chart_init_scripts` traduce cada `ChartSpec` → la config de Chart.js correspondiente. La función es pura, `serde_json::Value` → `String`, fácilmente testeable.

### 8.7. AutoToc

`Block::AutoToc { depth }` se resuelve al render iterando los slides:
- `depth == 1`: lista de `SectionDivider` slides.
- `depth == 2`: lista de todos los slides (con título).

Genera links `<a href="#sl_xxx">` que el browser resuelve por scroll-into-view nativo (o por el `slides_runtime.js` que captura el click y hace transición).

### 8.8. Footer global

Si `FooterConfig.enabled`:
- En `layout_mode: "report"`: footer único al final del scroll, fijo o estático según theme.
- En `layout_mode: "slides"`: footer en cada slide.
- Con `page_numbers: true`: render "{i} / {n}" usando `FooterConfig.custom_text` como prefijo (o vacío).
- Strings localizados por `doc_props.locale` (default "en"): `Page X of Y` / `Página X de Y`.

---

## 9. Asset store

### 9.1. Trait nuevo en `domain/ports.rs`

```rust
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

    async fn read(&self, id: &AssetId) -> Result<(Vec<u8>, String /* mime */), AssetError>;

    async fn list_by_session(
        &self,
        session: &SessionId,
    ) -> Result<Vec<AssetSummary>, AssetError>;

    async fn delete(&self, id: &AssetId) -> Result<(), AssetError>;

    /// Cheap metadata lookup sin cargar bytes.
    async fn head(&self, id: &AssetId) -> Result<AssetSummary, AssetError>;
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetSummary {
    pub id: AssetId,
    pub session_id: SessionId,
    pub mime: String,
    pub size_bytes: u64,
    pub label: Option<String>,
    pub created_at: DateTime<Utc>,
}
```

### 9.2. Errores nuevos en `domain/error.rs`

```rust
pub enum AssetError {
    NotFound { id: AssetId },
    MimeNotAllowed { mime: String },
    TooLarge { actual: u64, max: u64 },
    StillReferenced { id: AssetId, by_artifacts: Vec<ArtifactId> },
    Storage(String),
}
```

### 9.3. `LocalFsAssetStore` — layout en disco

```
{asset_root}/sessions/{session_id}/
    {asset_id}/
        bytes.bin
        meta.json     # { mime, size_bytes, label, created_at }
```

Escrituras atómicas vía temp + `rename`. Config:

| Campo | Default |
|---|---|
| `asset_storage_root` | `./.colmena/documents/assets` |
| `max_asset_size_bytes` | `10_485_760` (10 MB) |
| `allowed_mimes` | `["image/png", "image/jpeg", "image/gif", "image/webp"]` |

(Videos como asset quedan v2; en v1 los Video blocks solo aceptan YouTube/Vimeo via `VideoSrc::Youtube/Vimeo`.)

### 9.4. `GcsAssetStore` — feature flag `gcs`

Espejo del `GcsArtifactStore`:
- Bucket configurable, prefix `{prefix}/assets/sessions/{session_id}/{asset_id}/...`.
- Bytes y meta en blobs separados.
- Sin CAS — assets son inmutables tras upload.
- Lifecycle policy opcional para auto-GC (out of scope: documentar y dejar a operación).

### 9.5. Use cases (`application/upload_asset.rs` etc.)

```rust
pub struct UploadAssetInput {
    pub session_id: SessionId,
    pub bytes: Vec<u8>,
    pub mime: String,
    pub label: Option<String>,
}

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
    pub async fn execute(&self, input: UploadAssetInput) -> Result<UploadAssetOutput, AssetError> {
        if input.bytes.len() as u64 > self.max_size_bytes { return Err(AssetError::TooLarge { ... }); }
        if !self.allowed_mimes.contains(&input.mime) { return Err(AssetError::MimeNotAllowed { ... }); }
        let id = AssetId::new(format!("asset_{}", self.ids.new_asset_id()));
        self.store.upload(&input.session_id, &id, input.bytes, &input.mime, input.label.as_deref()).await?;
        let summary = self.store.head(&id).await?;
        Ok(UploadAssetOutput { asset_id: id, summary })
    }
}
```

`ListAssetsUseCase` es trivial wrapper sobre `store.list_by_session`.

`DeleteAssetUseCase`:
- Lista artifacts de la sesión (vía `SessionArtifactIndex`).
- Lee el meta de cada uno; carga la IR HEAD; chequea si `assets_referenced` contiene el `asset_id`.
- Si alguno lo referencia → `AssetError::StillReferenced { by_artifacts }`.
- Si no → `store.delete`.

`IdGenerator` se extiende con `new_asset_id()` (ULID).

---

## 10. DocumentRuntime extendido

```rust
pub struct DocumentRuntime {
    pub store: Arc<dyn ArtifactStore>,
    pub asset_store: Arc<dyn AssetStore>,         // nuevo
    pub create: Arc<CreateDocumentUseCase>,
    pub apply: Arc<ApplyPatchUseCase>,
    pub read: Arc<ReadDocumentUseCase>,
    pub get_head: Arc<GetHeadUseCase>,
    pub list_versions: Arc<ListVersionsUseCase>,
    pub rollback: Arc<RollbackUseCase>,
    pub upload_asset: Arc<UploadAssetUseCase>,    // nuevo
    pub list_assets: Arc<ListAssetsUseCase>,      // nuevo
    pub delete_asset: Arc<DeleteAssetUseCase>,    // nuevo
}
```

`from_config` extendido para aceptar:

| Campo | Tipo | Default | Aplica a |
|-------|------|---------|----------|
| `storage_backend` | string | `"localfs"` | siempre (existente) |
| `storage_root` | string | `./.colmena/documents` | localfs (existente) |
| `gcs_bucket` | string | — | gcs (existente) |
| `gcs_prefix` | string | `colmena/documents` | gcs (existente) |
| `default_retention` | u32 | `20` | siempre (existente) |
| **`asset_storage_root`** | string | `./.colmena/documents/assets` | localfs |
| **`asset_gcs_prefix`** | string | `colmena/documents/assets` | gcs |
| **`max_asset_size_bytes`** | u64 | `10_485_760` | siempre |
| **`allowed_asset_mimes`** | array<string> | `["image/png","image/jpeg","image/gif","image/webp"]` | siempre |

El asset store usa el mismo `storage_backend` que el artifact store (no se mezclan localfs+gcs).

`with_store` se reemplaza por `with_stores(artifact_store, asset_store, default_retention, asset_limits)` para los tests.

---

## 11. Extensión de `CreateDocumentUseCase` y `ApplyPatchUseCase` para HTML

Ambos use cases ya hacen `match meta.kind { Excel => ..., Word => ... }`. Se agrega rama `Html`:

```rust
// CreateDocumentUseCase::execute
let (validator, renderer): (&Arc<dyn IRValidator>, &Arc<dyn IRRenderer>) = match input.kind {
    ArtifactKind::Excel => (&self.excel_validator, &self.excel_renderer),
    ArtifactKind::Word  => (&self.word_validator,  &self.word_renderer),
    ArtifactKind::Html  => (&self.html_validator,  &self.html_renderer),  // ← nuevo
};
```

El `CreateDocumentUseCase` se enriquece con `html_renderer: Arc<dyn IRRenderer>` y `html_validator: Arc<dyn IRValidator>` (paralelos a los existentes excel_/word_).

`ApplyPatchUseCase` análogo, con un tercer brazo del match sobre kind que usa `HtmlOpApplier`.

`empty_ir()` se extiende:

```rust
fn empty_ir(id: &ArtifactId, kind: ArtifactKind) -> serde_json::Value {
    match kind {
        ArtifactKind::Excel => json!({ ... }),  // existente
        ArtifactKind::Word  => json!({ ... }),  // existente
        ArtifactKind::Html  => json!({
            "kind": "html",
            "artifact_id": id.0,
            "version_id": "v1",
            "schema_version": "1.0.0",
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

(El `sl_initial` se reemplaza con un ULID real al instanciar; usar string fijo aquí solo es ejemplo.)

---

## 12. describe_op extendido

`describe_op` en `apply_patch.rs` genera narración legible para `PatchSummary.natural_language`. Se extiende con un brazo por op HTML:

```rust
AddSlide { layout, title, .. } => match (layout, title.as_deref()) {
    (SlideLayout::Title, Some(t)) => format!("Added title slide '{t}'"),
    (SlideLayout::SectionDivider, Some(t)) => format!("Added section '{t}'"),
    (l, _) => format!("Added {l:?} slide"),
},
DeleteSlide { slide_id } => format!("Deleted slide {slide_id}"),
SetSlideNotes { slide_id, notes } => match notes {
    Some(_) => format!("Updated speaker notes on {slide_id}"),
    None    => format!("Cleared speaker notes on {slide_id}"),
},
InsertHtmlBlock { slide_id, block, .. } => {
    let kind = block.get("kind").and_then(|v| v.as_str()).unwrap_or("block");
    format!("Inserted {kind} block on slide {slide_id}")
},
// ... etc
```

Esto alimenta el flujo de narración usuario→agente cuando aplique (Spec 2 expone esto vía LLM tool).

---

## 13. Estructura de directorios final

```
src/libs/colmena/src/documents/
├── domain/
│   ├── ids.rs              # MOD: + Html, + AssetId
│   ├── ports.rs            # MOD: + AssetStore, + AssetError
│   ├── patch.rs            # MOD: + 19 variantes HTML
│   ├── error.rs            # MOD: + AssetError
│   └── ir/
│       ├── mod.rs          # MOD: + html
│       └── html.rs         # NEW
├── application/
│   ├── apply_patch.rs      # MOD: + brazo Html
│   ├── create_document.rs  # MOD: + brazo Html + empty_ir Html
│   ├── apply_html_ops.rs   # NEW
│   ├── upload_asset.rs     # NEW
│   ├── list_assets.rs      # NEW
│   ├── delete_asset.rs     # NEW
│   └── runtime.rs          # MOD: + asset_store + 3 asset use cases
└── infrastructure/
    ├── ids.rs              # MOD: + new_asset_id
    ├── render/
    │   ├── mod.rs          # MOD: + html_renderer
    │   ├── html_renderer.rs           # NEW
    │   └── html_assets/               # NEW
    │       ├── themes/{executive,minimal,vibrant,dark}.css
    │       ├── chart_js.min.js
    │       └── slides_runtime.js
    ├── validation/
    │   ├── mod.rs          # MOD: + html_validator
    │   └── html_validator.rs          # NEW
    └── storage/
        ├── mod.rs          # MOD: + asset stores
        ├── local_fs_asset_store.rs    # NEW
        └── gcs_asset_store.rs         # NEW (#[cfg(feature = "gcs")])
```

---

## 14. Dependencias nuevas (`Cargo.toml`)

```toml
[dependencies]
# Existentes (rust_xlsxwriter, docx-rs, ulid, serde, etc.) ya están.

maud = "0.26"                # macros HTML type-safe
```

Eso es **todo**. Chart.js y los CSS son archivos estáticos, no crates. No agregamos nada para charts (solo embebemos el JS). No agregamos parser HTML (eso va a Spec 2 con la ingesta bidireccional). No agregamos chromium (PDF queda v3+).

---

## 15. Tests

### 15.1. Unit (por módulo)

| Archivo | Tests |
|---|---|
| `domain/ir/html.rs` | serde roundtrip de cada bloque; schema generation con schemars |
| `domain/patch.rs` | serde roundtrip de las 19 ops HTML; tags correctos |
| `application/apply_html_ops.rs` | una test por op: estado pre/post, IDs asignados, errores |
| `infrastructure/validation/html_validator.rs` | ver §7.2 |
| `infrastructure/render/html_renderer.rs` | render de IR mínima, theme aplicado, Chart.js solo si hay chart, slides_runtime solo si slides mode, escape de `<script>` en texto |
| `infrastructure/storage/local_fs_asset_store.rs` | upload→read→list→delete; head sin cargar bytes |
| `infrastructure/storage/gcs_asset_store.rs` | (feature gcs) idem con bucket mock |
| `application/upload_asset.rs` | size limit, mime allowlist, id generation |
| `application/delete_asset.rs` | rechazo si still referenced; OK si no |

### 15.2. Integration (cross-module)

| Test | Flujo |
|---|---|
| `create_html_doc_default` | `CreateDocumentInput { kind: Html, ... }` → version v1 escrita, bytes válidos, IR consistente |
| `apply_add_slide_then_insert_block` | 2 patches sucesivos; primer patch retorna slide_id en summary; segundo lo usa |
| `apply_with_invalid_chart_returns_error` | patch con chart series/categories mismatch → DocumentError::IRValidationFailed, no se persiste versión |
| `apply_with_javascript_link_rejected` | run con `link: "javascript:..."` → error |
| `roundtrip_create_read` | crear, leer HEAD, deserializar → IR === IR original |
| `version_conflict_on_stale_base` | dos applies concurrentes simulados; segundo retorna VersionConflict |
| `upload_asset_then_reference_in_image` | upload → insert image block con asset_id → render resuelve el asset a base64 en HTML |
| `delete_asset_blocked_by_reference` | crear artifact con image referenciando asset; intentar borrar asset → StillReferenced con lista de artifacts |
| `theme_executive_renders_with_serif_var` | render con theme `executive` contiene el CSS de executive.css |
| `slides_mode_includes_runtime_js` | render con `layout_mode: slides` incluye `slides_runtime.js` inline |
| `report_mode_excludes_runtime_js` | render con `layout_mode: report` NO incluye runtime |
| `no_chart_no_chartjs` | render sin chart blocks no incluye Chart.js (~190KB ahorrados) |
| `auto_toc_resolves_slides` | block `auto_toc` con depth=1 lista correctamente los section_divider slides |
| `footer_locale_es_renders_pagina` | con `locale: "es"` y `footer.page_numbers`, contiene "Página 1 de 1" |

### 15.3. Snapshot (HTML output)

Para cada theme (4) + cada layout_mode (2) = 8 snapshots de un IR canónico, fijos en `tests/snapshots/html_*.html`. Detecta regresiones visuales y de embedding accidental.

### 15.4. Security tests (CRÍTICOS)

| Test | Vector |
|---|---|
| `text_with_script_tag_escapes` | paragraph run con `<script>alert(1)</script>` → HTML output contiene `&lt;script&gt;` |
| `link_javascript_rejected` | validator |
| `link_data_uri_rejected` | validator (excepto `data:image/*`) |
| `image_external_http_only` | `External { url: "ftp://..." }` rechazado |
| `image_data_url_non_image_rejected` | `data:text/html;base64,...` rechazado |
| `svg_in_data_url_rejected_v1` | `data:image/svg+xml;base64,...` rechazado (v1 no acepta SVG inline) |

### 15.5. Comando para correr todo

```bash
cargo test --lib documents::                       # toda la familia documents
cargo test --lib documents::infrastructure::render::html_renderer
cargo test --lib documents::infrastructure::storage --features gcs
```

---

## 16. Criterio de "Spec 1 listo"

El módulo está completo cuando este programa Rust corre verde sin tocar DAG ni LLM tools:

```rust
#[tokio::test]
async fn end_to_end_html_module() {
    let tmp = tempdir().unwrap();
    let rt = DocumentRuntime::from_config(&json!({
        "storage_root": tmp.path().join("artifacts").to_str().unwrap(),
        "asset_storage_root": tmp.path().join("assets").to_str().unwrap(),
    })).await.unwrap();

    let session = SessionId::new("test_session");

    // 1. Subir un asset (logo)
    let upload = rt.upload_asset.execute(UploadAssetInput {
        session_id: session.clone(),
        bytes: include_bytes!("fixtures/logo.png").to_vec(),
        mime: "image/png".into(),
        label: Some("company_logo".into()),
    }).await.unwrap();

    // 2. Crear doc HTML
    let created = rt.create.execute(CreateDocumentInput {
        kind: ArtifactKind::Html,
        session_id: session.clone(),
        label: Some("Q3 Report".into()),
        retention_limit: None,
        initial_ir: None,
        source: PatchSource::Agent,
    }).await.unwrap();

    // 3. Configurar theme y layout slides
    let _ = rt.apply.execute(ApplyPatchInput { patch: Patch {
        artifact_id: created.artifact_id.0.clone(),
        base_version: "v1".into(),
        source: PatchSource::Agent,
        ops: vec![
            PatchOp::SetTheme { theme: "executive".into() },
            PatchOp::SetDocProps {
                title: Some("Q3 Results".into()),
                author: Some("Daniel".into()),
                date: Some("2026-10-30".into()),
                locale: Some(Locale::Es),
            },
        ],
    }}).await.unwrap();

    // 4. Agregar slides
    let v3 = rt.apply.execute(ApplyPatchInput { patch: Patch {
        artifact_id: created.artifact_id.0.clone(),
        base_version: "v2".into(),
        source: PatchSource::Agent,
        ops: vec![
            PatchOp::AddSlide {
                layout: SlideLayout::Title,
                at_index: None,
                title: Some("Q3 Results".into()),
                subtitle: Some("2026".into()),
            },
        ],
    }}).await.unwrap();

    // 5. Leer bytes renderizados
    let store = rt.store.clone();
    let v3_data = store.read_current(&created.artifact_id).await.unwrap();
    let html = String::from_utf8(v3_data.rendered_binary).unwrap();

    // 6. Assertions sobre el HTML
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(html.contains("Q3 Results"));
    assert!(html.contains("lang=\"es\""));
    assert!(html.contains("--font-heading"));  // CSS del theme inline
    assert!(!html.contains("chart.js"));        // sin charts, sin Chart.js
}
```

Si ese test pasa, Spec 1 está completo y se puede empezar Spec 2 (integraciones DAG + LLM tools + bidirectional translator).

---

## 17. Roadmap explícito post-Spec 1

| Item | Spec | Notas |
|---|---|---|
| DAG nodes `document_create`/`edit`/`read` extendidos para Html | Spec 2 | Polimórficos por kind, ya cubren HTML al ser kind-agnósticos. Solo verificar. |
| Use case `export` (genérico para los 3 kinds) | Spec 2 | Devuelve bytes para downstream nodes |
| Use case `get_outline` (HTML-specific) | Spec 2 | Estructura del deck para que el agente "se vea" |
| Use case `get_render_url` (signed URL si GCS) | Spec 2 | Resuelve el gap de delivery |
| DAG nodes `document_export`, `document_get_url` | Spec 2 | Wrappers de los use cases |
| LLM tools nuevos: `document_get_outline`, `document_upload_asset`, `document_list_assets`, `document_attach_generated_image` | Spec 2 | + extensión `DOCUMENTS_SYSTEM_PRELUDE` |
| Integración con `image_generation` feature existente | Spec 2 | `document_attach_generated_image` wrappea image_generation + upload_asset |
| Extender slice en `document_read` para HTML (slide_ids, block_ids) | Spec 2 | Aplica a los 3 kinds en general |
| **Traductor bidireccional** bytes→IR para Excel/Word/HTML | Spec 2 | Destraba la limitación v1 de "sin ingesta de binarios" |
| Export PDF server-side (chromium o weasyprint) | Spec 3+ | Dependencia pesada |
| Cache de render por (version_id, theme) | v2 | Optimización |
| Frontend de edición + `PatchSource::User` para HTML | v2 | Requiere UI (Tiptap o custom) |
| Templates / data binding | v2 | Requiere rediseño de IR |
| Cross-slide `MoveBlockToSlide` | v2 | delete + insert cubre |
| Slide transitions / animations | v2 | Estética |
| SVG inline en images | v2 | Requiere sanitizer de SVG |
| ApexCharts swap | v2 | Sin migración de IR gracias a ChartSpec normalizado |
| i18n profundo (más locales, RTL) | v3 | |
| Comments / collaboration | v3 | Requiere CRDT/OT |

---

## 18. Riesgos identificados

| Riesgo | Mitigación |
|---|---|
| `maud` macro syntax tiene curva si nunca se usó | Documentar patrones en code review; templates ejemplo en doc comments del renderer |
| HTML rendered crece mucho con base64 de muchas imágenes | Mensaje en logs si HTML > 5 MB; asset store mitigará en v2 cuando sirvamos por URL |
| Chart.js bundleado infla todos los HTMLs con chart | Carga condicional ya prevista (solo si hay chart block) |
| CSS de themes divergen visualmente sin notar | Snapshot tests por theme atrapan regresiones |
| `serde_json::Value` en ops (`block`, `cells`, `runs`) pierde type-safety hasta el applier | El applier deserializa a structs concretos inmediatamente y devuelve errores específicos por shape mismatch |
| Validator y applier pueden diverger en reglas de unicidad de IDs | Tests cross-module con la misma IR fixture corriendo ambos |
| `assets_referenced` mantenido manualmente por applier puede quedar inconsistente | Re-scan defensivo en validator (chequea que el set declarado === set real) |

---

## 19. Decisiones registradas

- **Single kind `html`** con `layout_mode: 'report' | 'slides'` (no dos kinds separados).
- **Slides como contenedor de primer nivel** en la IR (no lista plana con separadores).
- **4 layouts cerrados + ~15 bloques composables** (no presets ricos).
- **Themes nombrados predefinidos** (no inline styling, no Tailwind classes).
- **Chart.js** como librería para v1 con **ChartSpec normalizado** (independiente de la lib).
- **Renderer con `maud`** (no askama, no hand-rolled).
- **Asset store separado del artifact store**, scoped por SessionId.
- **Assets immutable post-upload**; cambios = nuevo asset.
- **Asset bytes embebidos como base64** en el HTML final (self-contained); URLs servidas vienen en Spec 2.
- **Chart data validado server-side** (consistency).
- **URL scheme allowlist** en hyperlinks e image srcs (seguridad XSS no negociable).
- **SVG inline NO permitido en v1** (data:image/svg+xml rechazado; permitido solo via asset_id, con asunción de que el operador confía en lo que sube).
- **MoveBlockToSlide queda fuera de v1.**
- **Sin frontend de edición**: `PatchSource::User` no aplica para HTML hasta Spec 2 o posterior.

---

## 20. Próximo paso

Tras aprobación de este spec → escribir **plan de implementación** vía `superpowers:writing-plans`, ordenando los componentes en bloques pequeños con verificación entre cada uno (estilo TDD: tests primero, código después).
