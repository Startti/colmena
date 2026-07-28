# src/libs/colmena/src/documents/domain/ir/mod.rs

**Layer:** domain  **Purpose:** Module root for document intermediate representation (IR) system; organizes and re-exports IR types for multiple document formats (common, Excel, HTML, Word).

## Symbols

### Modules
- `common` (mod, pub) — Submodule for common document IR types shared across formats
- `excel` (mod, pub) — Submodule for Excel document IR types and structures
- `html` (mod, pub) — Submodule for HTML document IR types and structures
- `word` (mod, pub) — Submodule for Word document IR types and structures

### Re-exported from common
- `Alignment` (type, pub) — Text alignment enumeration or type
- `FontSpec` (struct, pub) — Font specification with style/size/family properties
- `NamedStyle` (struct/enum, pub) — Named style definition for document elements
- `SCHEMA_VERSION` (const, pub) — Schema version constant for IR compatibility

### Re-exported from excel
- `Cell` (struct, pub) — Represents a single Excel cell with value and type
- `CellType` (enum, pub) — Enumeration of cell data types (text, number, formula, etc.)
- `ColumnSpec` (struct, pub) — Specification for Excel column properties (width, format)
- `ExcelIR` (struct, pub) — Root intermediate representation for Excel workbooks
- `ExcelKindTag` (enum, pub) — Tag/marker enum for Excel document kind discrimination
- `NamedTable` (struct, pub) — Represents a named table range in Excel with metadata
- `Sheet` (struct, pub) — Represents a single worksheet in an Excel workbook
- `Workbook` (struct, pub) — Root structure for an Excel workbook containing sheets

### Re-exported from html
- `CalloutVariant` (enum, pub) — Enumeration of callout/highlight box variants
- `ChartSize` (enum/struct, pub) — Enumeration or sizing specification for embedded charts
- `ColumnRatio` (type, pub) — Type representing proportional column widths
- `DeltaDirection` (enum, pub) — Enumeration for direction of change (up/down/neutral)
- `FooterConfig` (struct, pub) — Configuration for HTML footer content and styling
- `Gap` (enum/type, pub) — Enumeration or type for spacing/gap sizes
- `ImagePosition` (enum, pub) — Enumeration of image positioning modes (inline, float, block)
- `LayoutMode` (enum, pub) — Enumeration of layout modes (fixed, responsive, etc.)
- `Locale` (enum/type, pub) — Locale/language identifier for internationalization
- `SlideLayout` (enum, pub) — Enumeration of slide or section layout templates
- `Theme` (enum/struct, pub) — Color scheme or design theme enumeration

### Re-exported from word
- `Block` (enum, pub) — Enumeration of block-level document elements (paragraph, table, list)
- `ListItem` (struct, pub) — Represents a single item in an ordered or unordered list
- `ListStyle` (enum/struct, pub) — List styling specification (bullet type, indent, etc.)
- `Run` (struct, pub) — Represents a contiguous run of styled text in Word
- `TableCell` (struct, pub) — Represents a cell within a Word table with content and properties
- `TableRow` (struct, pub) — Represents a row within a Word table containing cells
- `WordDocument` (struct, pub) — Root structure for a Word document IR
- `WordIR` (struct, pub) — Intermediate representation for Word documents
- `WordKindTag` (enum, pub) — Tag/marker enum for Word document kind discrimination

## File-level notes

- This is a clean module index file with no implementation logic; purely structural organization via pub use re-exports.
- All submodules (`common`, `excel`, `html`, `word`) are declared but not expanded here; their implementations are in sibling directories.
- Follows Rust idiom of centralizing re-exports in a module root for cleaner public API.
- No infrastructure dependencies observed; maintains domain-layer purity.
- No test code, examples, or type bounds visible at this level.
