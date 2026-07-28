# src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs

**Layer:** infrastructure  **Purpose:** Filesystem-backed implementation of the SkillRepository trait that loads SKILL.md definitions from filesystem paths with validation, cycle detection, and nested reference resolution.

## Symbols

- `MAX_FILE_SIZE_BYTES` (const, pub) — Maximum permitted size for SKILL.md and reference files; set to 64 KB.
- `FilesystemSkillRepository` (struct, pub) — Repository struct storing a HashMap of skill name to SkillEntry; implements SkillRepository trait.
- `FilesystemSkillRepository::from_paths` (impl fn, pub) — Creates repository from user-supplied paths with allowed-directory whitelist validation; auto-detects single-skill directories vs. roots-of-skills; enforces symlink/canonicalization checks.
- `FilesystemSkillRepository::load_skill_dir` (impl fn, private) — Loads and validates a single skill directory; checks SKILL.md and reference file sizes, validates frontmatter name matches directory name, builds nested reference tree.
- `SkillEntry` (struct, private) — Internal struct storing canonical directory path, description string, and nested SkillReferenceMeta tree.
- `MAX_REFERENCE_DEPTH` (const, private) — Maximum nesting depth for reference declarations; set to 5 hops from skill root.
- `build_nested_references` (fn, private) — Transforms flat reference list from SKILL.md into recursive SkillReferenceMeta tree by triggering recursive walks.
- `build_reference_recursive` (fn, private) — Recursively reads and parses reference files, detects cycles, enforces MAX_REFERENCE_DEPTH limit, builds nested meta tree.
- `list_available` (trait method, pub) — Returns Vec of SkillCatalogEntry for all loaded skills.
- `load_skill` (trait method, pub async) — Retrieves full Skill struct by name; reads and re-parses SKILL.md; returns already-built nested reference tree.
- `load_reference` (trait method, pub async) — Loads nested reference file by slash-separated path; validates all segments exist in reference tree; reads final file body; returns SkillReference.
- `tests::write` (fn, cfg(test)) — Test helper; creates parent directories and writes string content to path.
- `tests::valid_skill_md` (fn, cfg(test)) — Test helper; generates valid SKILL.md frontmatter + body template.
- `tests::loads_valid_skill_from_graph_dir` (test, async) — Verifies skill loads from graph-relative path.
- `tests::rejects_path_outside_allowed_dirs` (test, async) — Verifies path whitelist enforcement.
- `tests::accepts_path_in_extra_allowed_dirs` (test, async) — Verifies extra_allowed_dirs parameter works.
- `tests::rejects_missing_skill_md` (test, async) — Verifies EmptyRoot error on directory without SKILL.md.
- `tests::rejects_name_mismatch` (test, async) — Verifies NameMismatch error when SKILL.md name != directory name.
- `tests::rejects_declared_reference_with_missing_file` (test, async) — Verifies ReferenceFileMissing error.
- `tests::loads_declared_reference` (test, async) — Verifies reference file loads and returns correct body.
- `tests::rejects_undeclared_reference_at_load` (test, async) — Verifies ReferenceNotDeclared error on unknown reference.
- `tests::rejects_file_exceeding_size_limit` (test, async) — Verifies FileTooLarge error.
- `tests::rejects_path_to_file_not_dir` (test, async) — Verifies NotADirectory error.
- `tests::detects_duplicate_skill_name_across_paths` (test, async) — Verifies SkillNameCollision detection.
- `tests::from_paths_scans_root_directory_with_multiple_skills` (test) — Verifies root scan loads all child skills, ignores non-skill children.
- `tests::from_paths_empty_root_returns_empty_root_error` (test) — Verifies EmptyRoot error on root with no skills.
- `tests::from_paths_single_skill_still_works` (test) — Verifies single skill directory load.
- `tests::from_paths_root_and_single_in_same_array` (test) — Verifies mixed path array (root + single skill).
- `tests::from_paths_root_scan_skips_symlinked_child_outside_allowed` (test, unix-only) — Verifies symlinks resolving outside allowed dirs are skipped.
- `tests::from_paths_collision_across_root_and_single_errors` (test) — Verifies SkillNameCollision across root expansion + single path.
- `tests::reference_file_can_declare_nested_references` (test, async) — Verifies nested reference tree building.
- `tests::reference_cycle_is_rejected` (test, async) — Verifies ReferenceCycle error detection.
- `tests::reference_depth_over_5_is_rejected` (test, async) — Verifies ReferenceDepthExceeded error (depth > 5).
- `tests::load_reference_navigates_nested_path` (test, async) — Verifies load_reference works with nested path "fw/django".
- `tests::load_reference_rejects_undeclared_path` (test, async) — Verifies reference path not declared in reference tree is rejected.
- `tests::reference_depth_exactly_5_is_allowed` (test, async) — Verifies depth = 5 is the boundary (allowed).

## File-level notes

- **Design:** Implements hexagonal architecture (SkillRepository trait port in domain layer; filesystem adapter here in infrastructure). All I/O and file parsing delegated to `parse_skill_md` and `parse_reference_file_refs` from sibling frontmatter_parser module.
- **Error handling:** Comprehensive; all I/O failures wrapped in `SkillError::Io`; structural validation errors (NameMismatch, ReferenceFileMissing, FileTooLarge, etc.) return specific enum variants.
- **Security:** Path whitelist enforcement (allowed-dirs + canonicalization) prevents directory escape attacks; symlink canonicalization follows and validates final target is still in allowed set.
- **Cycle & depth protection:** Reference trees validated at load time (build_reference_recursive enforces MAX_REFERENCE_DEPTH = 5 and detects cycles).
- **Test coverage:** 25+ tests covering happy path, all error paths, edge cases (symlinks, empty roots, collisions, depth boundaries), and nested reference scenarios.
- **No flagged items:** Code is complete, no dead code, no unfinished patterns (todo!/unimplemented!/FIXME). All functions exercised by tests or called by trait impl.
