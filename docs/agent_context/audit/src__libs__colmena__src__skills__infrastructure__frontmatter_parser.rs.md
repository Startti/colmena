# src/libs/colmena/src/skills/infrastructure/frontmatter_parser.rs

**Layer:** infrastructure  **Purpose:** Parses YAML frontmatter from SKILL.md files (mandatory) and optional reference file metadata, extracting skill name, description, references, and markdown body.

## Symbols

- `RawFrontmatter` (struct, private) — internal representation of parsed SKILL.md frontmatter with name, description, and optional references list
- `RawReference` (struct, private) — reference declaration supporting both full `{name, description}` and bare string forms
- `impl Deserialize for RawReference` (impl, private) — custom deserializer using `#[serde(untagged)]` to accept bare strings or full objects, enabling compatibility with existing SKILL.md files that use terse reference syntax
- `RawReferenceFrontmatter` (struct, private) — optional frontmatter structure for reference files containing only a references list
- `parse_reference_file_refs()` (fn, pub) — parses optional YAML frontmatter from a reference file and returns sub-reference declarations; silently returns empty list if no frontmatter present
- `RawReferenceMeta` (struct, pub) — flat reference metadata from reference file frontmatter (name, description)
- `ParsedSkillMd` (struct, pub) — complete result of parsing a SKILL.md file: name, description, references, and body
- `parse_skill_md()` (fn, pub) — parses mandatory SKILL.md frontmatter and markdown body; validates required fields; rejects deprecated `node_type` frontmatter with migration message
- `tests::parses_minimal_valid_frontmatter()` (test, private) — verifies basic frontmatter parsing with name, description, and body
- `tests::parses_with_references()` (test, private) — verifies multi-reference parsing
- `tests::parses_empty_body()` (test, private) — verifies body can be empty
- `tests::body_preserves_markdown_separators()` (test, private) — verifies markdown horizontal rules (`---` in body) are not mistaken for frontmatter boundaries
- `tests::rejects_file_without_opening_delimiter()` (test, private) — verifies error when opening `---` missing
- `tests::rejects_file_without_closing_delimiter()` (test, private) — verifies error when closing `---` missing
- `tests::rejects_malformed_yaml()` (test, private) — verifies YAML parse errors are caught and wrapped
- `tests::rejects_missing_name()` (test, private) — verifies validation of required `name` field
- `tests::rejects_missing_description()` (test, private) — verifies validation of required `description` field
- `tests::tolerates_crlf_line_endings()` (test, private) — verifies support for both `\n` and `\r\n` line endings
- `tests::legacy_node_type_frontmatter_is_rejected_with_migration_error()` (test, private) — verifies deprecated `node_type` field triggers clear migration error

## File-level notes

- **Duplication (lines 72–85 vs. 144–158):** Both `parse_reference_file_refs()` and `parse_skill_md()` contain nearly identical logic to scan for the closing `---` delimiter by iterating lines, accumulating byte offsets, and checking for a trimmed match to `"---"`. This ~10-line pattern repeats verbatim; could be extracted to a shared helper (e.g., `find_delimiter_end()`) to reduce duplication and improve maintainability.
- **Migration safeguard:** `parse_skill_md()` explicitly rejects `node_type:` frontmatter field (lines 171–183) from a reverted layered-tool-context experiment, with a clear actionable error message. Prevents silent data loss and guides users to update their files.
- **Custom deserializer well-motivated:** `RawReference` uses untagged Serde pattern (lines 32–47) to accept both bare strings and full `{name, description}` maps. Design is necessary because existing built-in skills (`gsheets-table-exploration`, `crdt-doc-table-exploration`) use terse form and would fail at runtime otherwise; comment (lines 13–18) explains this trade-off.
- **Intentional asymmetry in strictness:** `parse_skill_md()` requires both opening and closing delimiters (strict), while `parse_reference_file_refs()` treats frontmatter as optional (lenient). Both behaviors are deliberate and documented; allows flexibility in reference files while enforcing structure in main skill files.
- **Comprehensive test coverage:** 11 unit tests cover minimal valid input, multi-reference parsing, empty/preserved body, CRLF tolerance, missing delimiters, malformed YAML, missing required fields, and migration error. All major paths and edge cases exercised.
- **Robust error handling:** All `serde_yaml` deserialization errors and validation failures are wrapped in `SkillError` variants with path context for actionable diagnostics.
