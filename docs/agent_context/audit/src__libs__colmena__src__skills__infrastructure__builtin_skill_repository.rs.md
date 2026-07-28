# src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs

**Layer:** infrastructure  
**Purpose:** Built-in skills repository that loads compiled-in skill markdown files via `include_dir!` macro and implements the `SkillRepository` trait for querying, loading, and resolving reference bodies.

## Symbols

- `BUILTIN_SKILLS_DIR` (static, pub) — compiled-in directory tree of built-in skills from `$CARGO_MANIFEST_DIR/skills`
- `BuiltinSkillRepository` (struct, pub) — in-memory repository holding parsed built-in skills indexed by name
- `BuiltinEntry` (struct, private) — holds parsed skill metadata: description, body, references, and reference bodies keyed by name
- `BuiltinSkillRepository::new()` (fn, pub) — validates enabled skill names against compiled-in tree, parses SKILL.md frontmatter, loads reference bodies, returns error if any skill/reference missing or name mismatches directory
- `BuiltinSkillRepository::all_available_names()` (fn, pub) — lists all built-in skill directory names, excluding reserved `_placeholder`
- `BuiltinSkillRepository::available_builtin_names()` (fn, pub) — wrapper around `all_available_names()` with identical implementation
- `SkillRepository::list_available()` (async trait method) — returns catalog entries for all loaded skills with names and descriptions
- `SkillRepository::load_skill()` (async trait method) — loads a full skill by name, returning error if not found
- `SkillRepository::load_reference()` (async trait method) — validates nested reference path segments against declared hierarchy, looks up final segment body in flat HashMap, returns error if path invalid or body missing
- Tests: `enabling_unknown_builtin_fails()`, `enabling_placeholder_is_rejected()`, `empty_enabled_list_constructs_empty_repo()` — constructor validation
- Tests: `all_available_names_excludes_placeholder()`, `available_builtin_names_includes_authored_skills()` — enumeration
- Tests: `crdt_doc_formulas_is_loadable()`, `python_expert_is_loadable()`, `sql_optimizer_is_loadable()`, `both_builtins_can_coexist()`, `sql_query_best_practices_is_loadable()`, `data_run_python_recipes_is_loadable()`, `gsheets_presentable_output_is_loadable()`, `gdocs_surgical_edits_is_loadable()` — individual skill load verification
- Tests: `all_builtin_skills_parse_and_load()` — exhaustive smoke test ensuring all compiled-in skills parse cleanly

## File-level notes

- **Improvement (duplication)**: `available_builtin_names()` (line 122–124) is a zero-value wrapper that simply calls `all_available_names()` with no additional logic. The method adds no semantic value and duplicates the implementation; consider removing it in favor of direct calls to `all_available_names()`.

- **Improvement (reference lookup design limitation)**: Reference path resolution assumes final segment names are unique across the declared hierarchy. Line 202 looks up the reference body by the final segment name only in a flat `HashMap<String, String>`. If a skill declares nested references with duplicate final names (e.g., "database/config" and "api/config"), both paths would resolve to the same body in `reference_bodies["config"]` with no validation or warning. The design comment (line 176–177) notes "Built-in skills store flat references (no recursive nesting at load time)", which explains this limitation, but the code does not enforce or document that leaf names must be globally unique within a skill. Consider either: (a) validating leaf-name uniqueness at parse time, (b) keying bodies by full path instead of final segment, or (c) adding a prominent comment explaining the flat-body constraint.

- **Hardened error handling**: UTF-8 validation and frontmatter parsing errors are clearly reported with file paths; name/directory mismatches detected at construction time.

- **Test coverage**: Comprehensive smoke tests for each built-in skill, plus an exhaustive test (`all_builtin_skills_parse_and_load()`) that catches frontmatter and reference-file regressions at CI time.
