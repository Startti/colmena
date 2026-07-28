# src/libs/colmena/src/skills/domain/mod.rs

**Layer:** domain  **Purpose:** Domain-layer module root for skills subsystem. Consolidates and re-exports type definitions (Skill, SkillError, SkillsConfig, SkillRepository) from four submodules: skill, skill_config, skill_error, and skill_repository.

## Symbols

- `mod skill` (module declaration) — skill type and reference definitions
- `mod skill_config` (module declaration) — SkillsConfig type for skill configuration
- `mod skill_error` (module declaration) — SkillError enum for skill domain errors
- `mod skill_repository` (module declaration) — SkillRepository trait and catalog entry
- `pub use skill::{Skill, SkillReference, SkillReferenceMeta, SkillSource}` (re-export) — core skill types: value object (Skill), reference wrapper (SkillReference), metadata (SkillReferenceMeta), and source enumeration (SkillSource)
- `pub use skill_config::SkillsConfig` (re-export) — configuration schema for skills
- `pub use skill_error::SkillError` (re-export) — domain error type for skill operations
- `pub use skill_repository::{SkillCatalogEntry, SkillRepository}` (re-export) — repository trait and catalog entry type for skill persistence/lookup

## File-level notes

- Pure re-export aggregator file; no executable code or logic
- All four submodules are declared public (`pub mod`), and their exports are selectively re-exported via `pub use` to form the domain's public API
- Comprehensive domain API surface: exposes error, config, value types, and repository port
- No flags: clean domain-layer module aggregator with no dead code, unfinished stubs, or obvious improvements
