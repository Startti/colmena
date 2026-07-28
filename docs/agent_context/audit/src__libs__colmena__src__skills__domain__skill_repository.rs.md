# src/libs/colmena/src/skills/domain/skill_repository.rs

**Layer:** domain  
**Purpose:** Defines the SkillRepository port trait and SkillCatalogEntry value object for loading and discovering skills. Abstractions permit multiple implementations (builtin, filesystem, composite).

## Symbols

- `SkillCatalogEntry` (struct, pub) — Lightweight metadata struct for catalog listings; contains name, description, and source.
- `SkillCatalogEntry::name` (field, pub) — The skill's display name.
- `SkillCatalogEntry::description` (field, pub) — Brief description for catalog preview.
- `SkillCatalogEntry::source` (field, pub) — Enum marking whether the skill is builtin or filesystem-sourced.
- `SkillRepository` (trait, pub) — Port trait defining async contract for skill loading and discovery; supports multiple implementations (builtin, filesystem, composite).
- `SkillRepository::list_available` (fn, pub) — Returns vector of all available SkillCatalogEntry metadata; synchronous, lightweight.
- `SkillRepository::load_skill` (async fn, pub) — Loads a skill's main SKILL.md content by name; returns Skill or SkillError.
- `SkillRepository::load_reference` (async fn, pub) — Loads a specific reference file for a skill; returns SkillReference or SkillError.
- `tests::catalog_entry_debug_format_contains_name` (test fn, private) — Compile-time check that SkillCatalogEntry Debug format contains the name field.
- `tests::skill_type_compiles_with_refs` (test fn, private) — Compile-time check that Skill type can be constructed with reference metadata.

## File-level notes

- **Test coverage is compile-time only**: `catalog_entry_debug_format_contains_name` and `skill_type_compiles_with_refs` do not test trait behavior or implementations; they ensure types compile together. No functional verification of SkillRepository contract.
- **Async/sync asymmetry**: `list_available` is synchronous while `load_skill` and `load_reference` are async. This is probably intentional (metadata assumed lightweight/preloaded), but worth documenting the design intent.
- **No error documentation**: Error conditions for `load_skill` and `load_reference` are not documented (when might they fail? missing file? parse error? permission denied?).
- **Imports all dependencies from sibling domain module** (Skill, SkillError, SkillReference, SkillSource, SkillReferenceMeta); zero infrastructure coupling — pure port definition.
