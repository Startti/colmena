# src/libs/colmena/src/skills/infrastructure/mod.rs

**Layer:** infrastructure  **Purpose:** Centralizes the public interface for skills infrastructure by re-exporting concrete repository implementations and parsing utilities from submodules.

## Symbols

- `builtin_skill_repository` (mod, public) — Submodule defining built-in skills repository implementation
- `composite_skill_repository` (mod, public) — Submodule defining composite pattern skill repository implementation
- `filesystem_skill_repository` (mod, public) — Submodule defining filesystem-based skill repository implementation
- `frontmatter_parser` (mod, public) — Submodule defining YAML frontmatter parsing utilities
- `BuiltinSkillRepository` (re-export, public) — Repository adapter for accessing built-in skills
- `CompositeSkillRepository` (re-export, public) — Repository adapter that composes multiple skill sources
- `MAX_ACTIVE_SKILLS` (re-export, public) — Configuration constant limiting the number of active skills
- `FilesystemSkillRepository` (re-export, public) — Repository adapter for loading skills from the filesystem
- `MAX_FILE_SIZE_BYTES` (re-export, public) — Configuration constant limiting skill file size
- `parse_skill_md` (re-export, public) — Parser function that extracts YAML frontmatter and content from skill markdown files
- `ParsedSkillMd` (re-export, public) — Data structure representing the parsed result of a skill markdown file

## File-level notes

- Pure re-export aggregator: no implementations, logic, or branching in this file
- All infrastructure adapters (repositories) and parsing utilities follow hexagonal pattern
- Clean and maintainable interface surface — no organizational debt
