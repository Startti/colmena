# src/libs/colmena/src/skills/domain/skill_error.rs

**Layer:** domain  **Purpose:** Defines the error type for skill loading, validation, and reference resolution operations in the skills subsystem.

## Symbols

- `SkillError` (pub enum) — Error type covering 15+ conditions: skill not found, missing/invalid SKILL.md, file size/frontmatter/name validation, path/directory checks, reference resolution (missing declarations, depth/cycle violations), and I/O errors with contextual metadata.
  - `SkillNotFound(String)` (variant) — Skill with name does not exist.
  - `ReferenceNotDeclared { skill, reference, available }` (variant) — Skill does not declare the requested reference; lists available alternatives.
  - `ReferenceFileMissing { skill, path }` (variant) — Expected reference file not found on disk.
  - `SkillMdMissing(String)` (variant) — SKILL.md file missing in directory.
  - `FileTooLarge { path, size, limit }` (variant) — File exceeds configured size limit.
  - `InvalidFrontmatter { path, reason }` (variant) — SKILL.md frontmatter parse error with reason.
  - `MissingField { field, path }` (variant) — Frontmatter missing required field.
  - `NameMismatch { name, dir, path }` (variant) — Skill name in frontmatter does not match containing directory name.
  - `PathNotAllowed(String)` (variant) — Path resolves outside allowed directories (security boundary).
  - `NotADirectory(String)` (variant) — Path is not a directory.
  - `EmptyRoot(String)` (variant) — Root has no SKILL.md and contains no skill subdirectories.
  - `TooManySkills { count, limit }` (variant) — Active skill count exceeds configured limit.
  - `SkillNameCollision { name }` (variant) — Skill name defined more than once (deduplication).
  - `Io { path, source }` (variant) — I/O error with path context and underlying `std::io::Error`.
  - `ReferenceDepthExceeded { skill, max, path }` (variant) — Reference nesting exceeds maximum depth.
  - `ReferenceCycle { skill, cycle }` (variant) — Circular reference detected; cycle describes the chain.
  - `InvalidReferencePath { path }` (variant) — Reference path is empty (validation gate).

- `tests` (mod, cfg test) — Unit tests for error message formatting.
  - `skill_not_found_display_contains_name` (fn, private) — Asserts `SkillNotFound` message contains the skill name.
  - `reference_not_declared_lists_available` (fn, private) — Asserts `ReferenceNotDeclared` message includes skill, reference, and available list.
  - `file_too_large_includes_sizes` (fn, private) — Asserts `FileTooLarge` message includes actual and limit sizes.
  - `empty_root_display_contains_path` (fn, private) — Asserts `EmptyRoot` message includes path and keywords.

## File-level notes

- **Completeness**: All 15 error variants are fully implemented with `#[error]` messages via `thiserror::Error`. No `todo!()` or stubs.
- **Correctness**: `#[source]` annotation on `Io` variant correctly propagates the underlying `std::io::Error` for error chains.
- **Test coverage**: Only 4 of 15+ variants are tested (SkillNotFound, ReferenceNotDeclared, FileTooLarge, EmptyRoot). Tests verify message content presence but not format exhaustiveness. Not a defect — typical for domain error types (testing happens at call sites).
- **Error design**: Well-structured hierarchy covering validation (path, directory, frontmatter), resource resolution (skill/reference not found), safety limits (size, depth, collision), and I/O boundaries. Messages are descriptive and include context fields for debugging.
- **Dependencies**: Only `thiserror::Error` derive; zero infrastructure coupling. Pure domain layer.
