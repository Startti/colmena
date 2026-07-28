# src/libs/colmena/src/documents/domain/ir/word.rs

**Layer:** domain  
**Purpose:** Defines the intermediate representation (IR) structures for Word documents, including document hierarchy (blocks, runs, formatting) with full serialization/deserialization support via serde.

## Symbols

- `WordIR` (struct, pub) — Top-level container for a Word document with artifact_id, version_id, schema_version, and document content
- `WordKindTag` (enum, pub) — Single-variant tag enum identifying document kind as "Word"
- `WordDocument` (struct, pub) — Container holding document blocks and named styles map
- `Block` (enum, pub) — Tagged union representing document blocks (Heading, Paragraph, List, Table), each with optional id
- `default_list_style()` (fn, private) — Helper function returning ListStyle::Bullet as serde default for Block::List style field
- `ListStyle` (enum, pub) — List styling variants (Bullet, Numbered)
- `Run` (struct, pub) — Inline text run with text content and optional formatting (bold, italic, underline, size, color)
- `ListItem` (struct, pub) — List item container with optional id and runs
- `TableRow` (struct, pub) — Table row container with optional id and cells
- `TableCell` (struct, pub) — Table cell containing runs (no nested block structure)
- `Block::id()` (impl method, pub) — Extracts id field from any Block variant
- `WordIR::empty()` (impl method, pub) — Factory method creating empty WordIR with provided artifact_id and version_id
- `WordIR::block_mut()` (impl method, pub) — Mutable lookup of block by id; returns None if not found
- `WordIR::block_index()` (impl method, pub) — Returns the index of a block by id; returns None if not found
- `word_ir_roundtrip()` (test) — Verifies serde serialization/deserialization round-trip for Heading block with Run

## File-level notes

- All structs and enums properly derived with Debug, Clone, PartialEq, and serde traits
- Serde attributes correctly handle optional fields with `skip_serializing_if = "Option::is_none"`
- `id` fields on blocks default to empty string via `#[serde(default)]`
- `Block::List` style field uses serde default function pattern (appropriate for non-Copy struct default)
- Single test covers basic round-trip but does not exercise all block variants (List, Table)
- No infrastructure dependencies; pure domain value objects
