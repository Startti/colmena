# src/libs/colmena/src/skills/domain/skill.rs

**Layer:** domain  **Purpose:** Value objects and data models for the skills subsystem — structures for loaded skills, references, and their metadata with serialization support.

## Symbols

- `SkillReferenceMeta` (struct, pub) — Metadata about a reference file attached to a skill, including nested sub-references from frontmatter
- `SkillReferenceMeta::name` (field, pub) — Name of the reference
- `SkillReferenceMeta::description` (field, pub) — Description of the reference
- `SkillReferenceMeta::references` (field, pub) — Nested sub-references declared in the reference file's frontmatter, empty when this is a leaf
- `Skill` (struct, pub) — A loaded skill with name, description, markdown body, reference metadata, and source origin
- `Skill::name` (field, pub) — Name of the skill
- `Skill::description` (field, pub) — Description of the skill
- `Skill::body` (field, pub) — Markdown content without frontmatter
- `Skill::references` (field, pub) — Metadata for declared references in the skill
- `Skill::source` (field, pub) — Enum indicating where the skill came from (Builtin or Path)
- `SkillSource` (enum, pub) — Indicates skill origin (Builtin or Path); serializes to lowercase
- `SkillSource::Builtin` (variant) — Skill is built-in to Colmena
- `SkillSource::Path` (variant) — Skill is loaded from a filesystem path
- `SkillReference` (struct, pub) — The body of a loaded reference file with parent skill name and reference name
- `SkillReference::skill_name` (field, pub) — Name of the parent skill
- `SkillReference::reference_name` (field, pub) — Name of the reference
- `SkillReference::body` (field, pub) — Content of the reference file
- `tests` (mod, private) — Test module for serialization and nested reference support
- `skill_serializes_roundtrip` (test fn) — Verifies Skill round-trips through JSON serialization and deserialization
- `skill_source_serializes_lowercase` (test fn) — Verifies SkillSource enum variants serialize to lowercase strings
- `skill_reference_meta_supports_nested_references` (test fn) — Verifies SkillReferenceMeta correctly handles nested references
- `skill_reference_meta_defaults_empty_references_when_missing` (test fn) — Verifies backward compatibility when nested references field is omitted (defaults to empty)

## File-level notes

- Pure domain layer with no infrastructure dependencies; all types are serde-derived for JSON wire format.
- Four core value objects: `SkillReferenceMeta`, `Skill`, `SkillReference`, and `SkillSource` enum.
- All public fields on structs; deriving Serialize/Deserialize with serde configuration (e.g., `#[serde(default)]` on `references`, `#[serde(rename_all = "lowercase")]` on enum).
- Test suite covers serialization round-trips, enum string representation, nested reference hierarchies, and backward compatibility.
- No error types or traits defined; pure data model intended as input/output contracts for application layer.
