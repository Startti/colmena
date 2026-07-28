# src/libs/colmena/src/documents/domain/ir/common.rs

**Layer:** domain  
**Purpose:** Defines core IR (intermediate representation) data structures for styling configuration across document formats. Provides serializable value objects for named styles, fonts, and text alignment that are re-exported by the parent ir module.

## Symbols

- `SCHEMA_VERSION` (const, pub) — Schema version identifier constant for IR versioning
- `NamedStyle` (struct, pub) — Aggregates optional styling attributes: font spec, fill color, alignment, and number format
- `FontSpec` (struct, pub) — Encapsulates font styling properties: weight, italic, underline, size, color, and font name
- `Alignment` (enum, pub) — Four-variant enum for text alignment: Left, Center, Right, Justify

## File-level notes

- This is a leaf module in the IR domain with zero internal dependencies — only external serde serialization
- All structures use `Option<T>` fields with conditional serialization (`skip_serializing_if = "Option::is_none"`)
- Symbols are re-exported by parent `ir/mod.rs` and used transitively by 9 downstream modules (apply_patch, renderers, validators)
- No dead code — the module serves as the canonical home for style IR definitions even though it has no direct importers
- Clean, maintainable design with no unfinished work or obvious improvements
