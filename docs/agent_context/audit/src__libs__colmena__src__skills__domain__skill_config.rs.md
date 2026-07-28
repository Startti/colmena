# src/libs/colmena/src/skills/domain/skill_config.rs

**Layer:** domain  **Purpose:** Defines SkillsConfig value object for parsing and validating the `skills` field in llm_call node config (builtin skill names and custom filesystem paths).

## Symbols

- `SkillsConfig` (struct, pub) — Config container holding builtin skill names (Vec<String>) and custom skill paths (Vec<String>), with Serde derives for JSON parsing.
- `SkillsConfig::builtin` (field, pub) — List of builtin skill identifiers to load.
- `SkillsConfig::paths` (field, pub) — List of filesystem paths to custom skill directories.
- `SkillsConfig::from_value` (fn, pub) — Parses SkillsConfig from a serde_json::Value, returning Result with serde_json::Error on parse failure.
- `SkillsConfig::has_any` (fn, pub) — Boolean predicate; returns true if at least one skill source (builtin or paths) is configured.
- `tests::empty_config_has_no_skills` (test, private) — Verifies default SkillsConfig reports false for has_any().
- `tests::parses_builtin_only` (test, private) — Verifies parsing of JSON with builtin array and empty paths.
- `tests::parses_paths_only` (test, private) — Verifies parsing of JSON with paths array and empty builtin.
- `tests::parses_both` (test, private) — Verifies parsing of JSON with both builtin and paths populated.
- `tests::empty_object_is_valid_but_empty` (test, private) — Verifies empty JSON object parses to empty config.
- `tests::empty_arrays_do_not_count_as_any` (test, private) — Verifies explicitly empty arrays report false for has_any().
- `tests::unknown_fields_ignored` (test, private) — Verifies serde ignores extra fields in JSON (standard serde default behavior).

## File-level notes

- Clean, minimal domain value object with no infrastructure dependencies.
- Comprehensive test coverage (6 test cases) of all parsing and validation paths.
- No dead code, unfinished implementations, or TODOs.
- The `from_value` implementation clones the input Value to satisfy serde_json::from_value ownership requirement; this is necessary and not avoidable given the function signature.
- Documentation comment clearly explains the JSON shape expected.
- All derives are standard and appropriate (Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq).
