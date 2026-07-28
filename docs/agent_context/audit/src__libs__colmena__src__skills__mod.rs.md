# src/libs/colmena/src/skills/mod.rs

**Layer:** infrastructure  **Purpose:** Root module for the Skills subsystem — organizes domain (entities, ports) and infrastructure (implementations) for on-demand loadable markdown knowledge packages.

## Symbols

- `domain` (mod, pub) — Public module containing domain-layer traits and entities (SkillRepository port, Skill value object, SkillConfig, SkillError).
- `infrastructure` (mod, pub) — Public module containing infrastructure-layer implementations (builtin, filesystem, and composite skill repositories; frontmatter parser).

## File-level notes

- Clean minimal root module; delegates all logic to submodules.
- Submodules correctly split domain (ports/entities) from infrastructure (concrete adapters).
- References design spec in doc comment: `docs/superpowers/specs/2026-04-20-llm-skills-design.md`.
- No flagged items.
