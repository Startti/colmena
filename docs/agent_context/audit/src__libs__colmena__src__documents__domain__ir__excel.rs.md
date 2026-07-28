# src/libs/colmena/src/documents/domain/ir/excel.rs

**Layer:** domain  **Purpose:** Defines the intermediate representation (IR) for Excel documents, comprising workbook structure, sheets, cells, and metadata needed for document generation and editing workflows.

## Symbols

- `ExcelIR` (struct, pub) — Root document IR containing kind tag, artifact/version IDs, schema version, and workbook structure  
- `ExcelKindTag` (enum, pub) — Single-variant enum tagging document kind as Excel  
- `Workbook` (struct, pub) — Container for sheets and named styles with Default impl  
- `Sheet` (struct, pub) — Individual sheet in workbook with ID, name, order, columns, cells, and tables  
- `ColumnSpec` (struct, pub) — Column metadata (index and width)  
- `Cell` (struct, pub) — Cell data with JSON value, optional type, format, and style reference  
- `CellType` (enum, pub) — Enumeration of cell value types: String, Number, Boolean, Date, Formula  
- `NamedTable` (struct, pub) — Named range/table with ID, name, range, header row flag, and optional style preset  
- `ExcelIR::empty` (impl fn, pub) — Constructor creating empty ExcelIR with artifact/version IDs and default workbook  
- `ExcelIR::sheet_mut` (impl fn, pub) — Mutable lookup of sheet by ID via linear search  
- `ExcelIR::sheet` (impl fn, pub) — Immutable lookup of sheet by ID via linear search  
- `tests` (mod, cfg(test)) — Serialization roundtrip tests  
- `roundtrip_empty_excel_ir` (fn, test) — Validates empty ExcelIR serialization/deserialization cycle  
- `roundtrip_ir_with_cells` (fn, test) — Validates ExcelIR with cell data roundtrips correctly  

## File-level notes

- No flags: all pub items are intentionally exposed for use by application/infrastructure layers; methods return Option appropriately; tests are adequate for roundtrip validation; no todo!(), unimplemented!(), or dead code detected.
- `Cell::value_type` uses `#[serde(rename = "type")]` due to keyword reservation; correct.
- Sheet lookup via linear `find()` is acceptable for domain layer (optimization deferred to infrastructure).
- Depends on `super::common::{NamedStyle, SCHEMA_VERSION}` for shared styling and versioning.

