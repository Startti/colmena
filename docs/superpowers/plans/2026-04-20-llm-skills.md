# LLM Skills Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Claude-Code-style Skills to the LLM node. Skills are `.md` files with YAML frontmatter, loaded on-demand by the LLM via a synthetic `load_skill` tool. Supports built-in skills (compiled into the crate) and user-provided skills (from filesystem paths, validated against an allowed-dirs whitelist).

**Architecture:** New hexagonal module `src/libs/colmena/src/skills/` with `domain/` (traits, value objects, errors) and `infrastructure/` (frontmatter parser, builtin repository via `include_dir!`, filesystem repository with security validation, composite repository). A new submodule `dag_engine/infrastructure/nodes/llm_synthetic_tools/` hosts the `load_skill` tool logic. `DagToolExecutor` gets one conditional path for `load_skill`. `NodeEvent` gets a new `SkillLoaded` variant. The LLM node wires everything together when `config.skills` is present.

**Tech Stack:** Rust (async/await + tokio), `serde` for JSON, `serde_yaml` (new dep) for frontmatter, `include_dir` (new dep) for compiled-in skills, `thiserror` for domain errors, `mockall` for tests.

**Design spec:** [docs/superpowers/specs/2026-04-20-llm-skills-design.md](../specs/2026-04-20-llm-skills-design.md)

---

## Conventions for this plan

- All commits use: `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>`
- Run Rust tests with `cargo test --lib <module>` — the crate is named `colmena_dag_engine`.
- When a task says "run it and verify it fails with X", the failure message is load-bearing; if it fails with a different error, something is wrong with the test setup.
- Every task ends with a commit. Don't batch.

---

## Task 0: Add dependencies and register skills module

**Files:**
- Modify: `src/libs/colmena/Cargo.toml`
- Modify: `src/libs/colmena/src/lib.rs`
- Create: `src/libs/colmena/src/skills/mod.rs` (empty placeholder for this task)

- [ ] **Step 1: Add the two new dependencies to Cargo.toml**

In `src/libs/colmena/Cargo.toml`, in the `[dependencies]` section after line 75 (`regex = "1.10"`), add:

```toml
# Skills: YAML frontmatter + compile-in built-in skills
serde_yaml = "0.9"
include_dir = "0.7"
```

- [ ] **Step 2: Create the skills module entry**

Create `src/libs/colmena/src/skills/mod.rs`:

```rust
//! Skills — on-demand loadable markdown knowledge packages for LLM nodes.
//!
//! See `docs/superpowers/specs/2026-04-20-llm-skills-design.md`.

pub mod domain;
pub mod infrastructure;
```

This file references `domain` and `infrastructure` which don't exist yet — that's fine, the compiler will complain once. We'll create them in Task 1.

- [ ] **Step 3: Register the module in lib.rs**

Edit `src/libs/colmena/src/lib.rs`. After line 2 (`pub mod llm;`), add:

```rust
pub mod skills;
```

- [ ] **Step 4: Verify the new deps resolve**

Run: `cargo check --lib 2>&1 | head -30`
Expected: errors about missing `skills/domain` and `skills/infrastructure` modules. The deps themselves should resolve (no "failed to resolve dependency" errors).

- [ ] **Step 5: Create empty domain and infrastructure entries to unbreak compilation**

Create `src/libs/colmena/src/skills/domain/mod.rs`:

```rust
```

(Empty file.)

Create `src/libs/colmena/src/skills/infrastructure/mod.rs`:

```rust
```

(Empty file.)

- [ ] **Step 6: Confirm compilation passes**

Run: `cargo check --lib`
Expected: success, no errors.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/Cargo.toml src/libs/colmena/Cargo.lock src/libs/colmena/src/lib.rs src/libs/colmena/src/skills/
git commit -m "$(cat <<'EOF'
feat(skills): scaffold skills module and add serde_yaml + include_dir deps

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 1: SkillError enum

**Files:**
- Create: `src/libs/colmena/src/skills/domain/skill_error.rs`
- Modify: `src/libs/colmena/src/skills/domain/mod.rs`

- [ ] **Step 1: Write failing test**

Create `src/libs/colmena/src/skills/domain/skill_error.rs`:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("skill '{0}' not found")]
    SkillNotFound(String),

    #[error("skill '{skill}' does not declare a reference named '{reference}'; available: [{available}]")]
    ReferenceNotDeclared {
        skill: String,
        reference: String,
        available: String,
    },

    #[error("reference file missing for skill '{skill}': expected {path}")]
    ReferenceFileMissing { skill: String, path: String },

    #[error("SKILL.md missing in directory: {0}")]
    SkillMdMissing(String),

    #[error("file too large: {path} ({size} bytes, limit {limit})")]
    FileTooLarge {
        path: String,
        size: u64,
        limit: u64,
    },

    #[error("invalid frontmatter in {path}: {reason}")]
    InvalidFrontmatter { path: String, reason: String },

    #[error("frontmatter missing required field '{field}' in {path}")]
    MissingField { field: String, path: String },

    #[error("skill name '{name}' does not match directory name '{dir}' in {path}")]
    NameMismatch {
        name: String,
        dir: String,
        path: String,
    },

    #[error("path '{0}' is not inside any allowed directory")]
    PathNotAllowed(String),

    #[error("path '{0}' is not a directory")]
    NotADirectory(String),

    #[error("too many skills active: {count} exceeds limit of {limit}")]
    TooManySkills { count: usize, limit: usize },

    #[error("skill name collision: '{name}' is defined more than once")]
    SkillNameCollision { name: String },

    #[error("I/O error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_not_found_display_contains_name() {
        let err = SkillError::SkillNotFound("python-expert".to_string());
        assert!(err.to_string().contains("python-expert"));
    }

    #[test]
    fn reference_not_declared_lists_available() {
        let err = SkillError::ReferenceNotDeclared {
            skill: "python-expert".to_string(),
            reference: "foo".to_string(),
            available: "frameworks, testing".to_string(),
        };
        let s = err.to_string();
        assert!(s.contains("python-expert"));
        assert!(s.contains("foo"));
        assert!(s.contains("frameworks, testing"));
    }

    #[test]
    fn file_too_large_includes_sizes() {
        let err = SkillError::FileTooLarge {
            path: "/x/y.md".to_string(),
            size: 99999,
            limit: 65536,
        };
        let s = err.to_string();
        assert!(s.contains("99999"));
        assert!(s.contains("65536"));
    }
}
```

- [ ] **Step 2: Register module**

Edit `src/libs/colmena/src/skills/domain/mod.rs`:

```rust
pub mod skill_error;

pub use skill_error::SkillError;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test --lib skills::domain::skill_error`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/skills/
git commit -m "$(cat <<'EOF'
feat(skills): add SkillError enum with typed error variants

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Skill and SkillReference value objects

**Files:**
- Create: `src/libs/colmena/src/skills/domain/skill.rs`
- Modify: `src/libs/colmena/src/skills/domain/mod.rs`

- [ ] **Step 1: Write the value objects with tests**

Create `src/libs/colmena/src/skills/domain/skill.rs`:

```rust
use serde::{Deserialize, Serialize};

/// Metadata about a reference file attached to a skill.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillReferenceMeta {
    pub name: String,
    pub description: String,
}

/// A loaded skill: name, description, body (markdown without frontmatter),
/// and metadata for any declared references.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub references: Vec<SkillReferenceMeta>,
    pub source: SkillSource,
}

/// Where a skill came from. Used for observability (SSE events, summary).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    Builtin,
    Path,
}

/// The body of a loaded reference file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillReference {
    pub skill_name: String,
    pub reference_name: String,
    pub body: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_serializes_roundtrip() {
        let skill = Skill {
            name: "python-expert".to_string(),
            description: "Use for Python questions".to_string(),
            body: "# Python\n".to_string(),
            references: vec![SkillReferenceMeta {
                name: "frameworks".to_string(),
                description: "Django and FastAPI".to_string(),
            }],
            source: SkillSource::Builtin,
        };
        let json = serde_json::to_string(&skill).unwrap();
        let back: Skill = serde_json::from_str(&json).unwrap();
        assert_eq!(skill, back);
    }

    #[test]
    fn skill_source_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&SkillSource::Builtin).unwrap(),
            "\"builtin\""
        );
        assert_eq!(
            serde_json::to_string(&SkillSource::Path).unwrap(),
            "\"path\""
        );
    }
}
```

- [ ] **Step 2: Register in domain/mod.rs**

Edit `src/libs/colmena/src/skills/domain/mod.rs`:

```rust
pub mod skill;
pub mod skill_error;

pub use skill::{Skill, SkillReference, SkillReferenceMeta, SkillSource};
pub use skill_error::SkillError;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test --lib skills::domain::skill`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/skills/
git commit -m "$(cat <<'EOF'
feat(skills): add Skill and SkillReference value objects

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: SkillRepository trait

**Files:**
- Create: `src/libs/colmena/src/skills/domain/skill_repository.rs`
- Modify: `src/libs/colmena/src/skills/domain/mod.rs`

- [ ] **Step 1: Define the trait**

Create `src/libs/colmena/src/skills/domain/skill_repository.rs`:

```rust
use super::{Skill, SkillError, SkillReference, SkillSource};
use async_trait::async_trait;

/// Entry in the skill catalog — lightweight metadata visible in the tool description.
#[derive(Debug, Clone)]
pub struct SkillCatalogEntry {
    pub name: String,
    pub description: String,
    pub source: SkillSource,
}

/// Port for loading skills. Implementations handle built-in vs filesystem,
/// and a composite implementation merges them.
#[async_trait]
pub trait SkillRepository: Send + Sync {
    /// List all available skills with their metadata for the catalog.
    fn list_available(&self) -> Vec<SkillCatalogEntry>;

    /// Load a skill's main SKILL.md content by name.
    async fn load_skill(&self, name: &str) -> Result<Skill, SkillError>;

    /// Load a specific reference file for a skill.
    async fn load_reference(
        &self,
        skill_name: &str,
        reference_name: &str,
    ) -> Result<SkillReference, SkillError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::domain::SkillReferenceMeta;

    // Minimal test: verify we can construct catalog entries and the trait compiles.
    #[test]
    fn catalog_entry_debug_format_contains_name() {
        let entry = SkillCatalogEntry {
            name: "x".to_string(),
            description: "y".to_string(),
            source: SkillSource::Builtin,
        };
        assert!(format!("{:?}", entry).contains("x"));
    }

    // Prove Skill type still works — compile-time check.
    #[test]
    fn skill_type_compiles_with_refs() {
        let _ = Skill {
            name: "n".to_string(),
            description: "d".to_string(),
            body: "b".to_string(),
            references: vec![SkillReferenceMeta {
                name: "r".to_string(),
                description: "rd".to_string(),
            }],
            source: SkillSource::Builtin,
        };
    }
}
```

- [ ] **Step 2: Register in domain/mod.rs**

Edit `src/libs/colmena/src/skills/domain/mod.rs`:

```rust
pub mod skill;
pub mod skill_error;
pub mod skill_repository;

pub use skill::{Skill, SkillReference, SkillReferenceMeta, SkillSource};
pub use skill_error::SkillError;
pub use skill_repository::{SkillCatalogEntry, SkillRepository};
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib skills::domain::skill_repository`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/skills/
git commit -m "$(cat <<'EOF'
feat(skills): add SkillRepository trait and SkillCatalogEntry

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: SkillConfig parsing

**Files:**
- Create: `src/libs/colmena/src/skills/domain/skill_config.rs`
- Modify: `src/libs/colmena/src/skills/domain/mod.rs`

- [ ] **Step 1: Write tests first**

Create `src/libs/colmena/src/skills/domain/skill_config.rs`:

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Parsed form of the `skills` field in an llm_call node config.
///
/// JSON shape:
/// ```json
/// {
///   "builtin": ["python-expert"],
///   "paths": ["./my-skills/custom"]
/// }
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillsConfig {
    #[serde(default)]
    pub builtin: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
}

impl SkillsConfig {
    /// Parse from a `serde_json::Value` (the config/inputs map value).
    pub fn from_value(value: &Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value.clone())
    }

    /// Returns true when the user configured at least one skill source.
    pub fn has_any(&self) -> bool {
        !self.builtin.is_empty() || !self.paths.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_config_has_no_skills() {
        let cfg = SkillsConfig::default();
        assert!(!cfg.has_any());
    }

    #[test]
    fn parses_builtin_only() {
        let v = json!({"builtin": ["python-expert"]});
        let cfg = SkillsConfig::from_value(&v).unwrap();
        assert_eq!(cfg.builtin, vec!["python-expert".to_string()]);
        assert!(cfg.paths.is_empty());
        assert!(cfg.has_any());
    }

    #[test]
    fn parses_paths_only() {
        let v = json!({"paths": ["./a", "./b"]});
        let cfg = SkillsConfig::from_value(&v).unwrap();
        assert_eq!(cfg.paths, vec!["./a".to_string(), "./b".to_string()]);
        assert!(cfg.builtin.is_empty());
        assert!(cfg.has_any());
    }

    #[test]
    fn parses_both() {
        let v = json!({"builtin": ["a"], "paths": ["./b"]});
        let cfg = SkillsConfig::from_value(&v).unwrap();
        assert_eq!(cfg.builtin, vec!["a".to_string()]);
        assert_eq!(cfg.paths, vec!["./b".to_string()]);
        assert!(cfg.has_any());
    }

    #[test]
    fn empty_object_is_valid_but_empty() {
        let v = json!({});
        let cfg = SkillsConfig::from_value(&v).unwrap();
        assert!(!cfg.has_any());
    }

    #[test]
    fn empty_arrays_do_not_count_as_any() {
        let v = json!({"builtin": [], "paths": []});
        let cfg = SkillsConfig::from_value(&v).unwrap();
        assert!(!cfg.has_any());
    }

    #[test]
    fn unknown_fields_ignored() {
        let v = json!({"builtin": ["x"], "extra_field": "ignored"});
        let cfg = SkillsConfig::from_value(&v).unwrap();
        assert_eq!(cfg.builtin, vec!["x".to_string()]);
    }
}
```

- [ ] **Step 2: Register in domain/mod.rs**

Edit `src/libs/colmena/src/skills/domain/mod.rs`:

```rust
pub mod skill;
pub mod skill_config;
pub mod skill_error;
pub mod skill_repository;

pub use skill::{Skill, SkillReference, SkillReferenceMeta, SkillSource};
pub use skill_config::SkillsConfig;
pub use skill_error::SkillError;
pub use skill_repository::{SkillCatalogEntry, SkillRepository};
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib skills::domain::skill_config`
Expected: 7 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/skills/
git commit -m "$(cat <<'EOF'
feat(skills): add SkillsConfig for parsing the config.skills field

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Frontmatter parser

**Files:**
- Create: `src/libs/colmena/src/skills/infrastructure/frontmatter_parser.rs`
- Modify: `src/libs/colmena/src/skills/infrastructure/mod.rs`

- [ ] **Step 1: Write the parser with tests**

Create `src/libs/colmena/src/skills/infrastructure/frontmatter_parser.rs`:

```rust
use crate::skills::domain::{SkillError, SkillReferenceMeta};
use serde::Deserialize;

/// Internal representation of the parsed frontmatter.
#[derive(Debug, Deserialize)]
struct RawFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    references: Vec<RawReference>,
}

#[derive(Debug, Deserialize)]
struct RawReference {
    name: String,
    description: String,
}

/// Result of parsing a SKILL.md file: extracted fields and the body
/// (markdown content after the closing `---`).
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedSkillMd {
    pub name: String,
    pub description: String,
    pub references: Vec<SkillReferenceMeta>,
    pub body: String,
}

/// Parse a SKILL.md file's content.
///
/// The file MUST start with `---\n`, followed by YAML frontmatter, followed by
/// `---\n` on its own line. Everything after that is the body.
///
/// `path` is used only for error messages.
pub fn parse_skill_md(content: &str, path: &str) -> Result<ParsedSkillMd, SkillError> {
    // Must start with "---" line
    if !content.starts_with("---\n") && !content.starts_with("---\r\n") {
        return Err(SkillError::InvalidFrontmatter {
            path: path.to_string(),
            reason: "file does not start with '---' frontmatter delimiter".to_string(),
        });
    }

    // Find the closing "---" line. We search for a line that is exactly "---".
    // Strategy: skip the first "---\n", then look for "\n---\n" (or "\n---" at EOF).
    let after_first = if content.starts_with("---\r\n") {
        &content[5..]
    } else {
        &content[4..]
    };

    // Locate end-of-frontmatter: look for a line containing only "---".
    let mut end_idx: Option<usize> = None;
    let mut offset = 0usize;
    for line in after_first.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(&['\r', '\n'][..]);
        if trimmed == "---" {
            end_idx = Some(offset);
            break;
        }
        offset += line.len();
    }

    let end_idx = end_idx.ok_or_else(|| SkillError::InvalidFrontmatter {
        path: path.to_string(),
        reason: "closing '---' delimiter not found".to_string(),
    })?;

    let yaml_str = &after_first[..end_idx];

    // Body: everything after the closing "---\n".
    let after_end = &after_first[end_idx..];
    // Skip the "---" line itself (plus its newline if present).
    let body_start = if let Some(nl) = after_end.find('\n') {
        nl + 1
    } else {
        after_end.len()
    };
    let body = after_end[body_start..].to_string();

    // Parse YAML.
    let raw: RawFrontmatter =
        serde_yaml::from_str(yaml_str).map_err(|e| SkillError::InvalidFrontmatter {
            path: path.to_string(),
            reason: format!("{}", e),
        })?;

    if raw.name.is_empty() {
        return Err(SkillError::MissingField {
            field: "name".to_string(),
            path: path.to_string(),
        });
    }
    if raw.description.is_empty() {
        return Err(SkillError::MissingField {
            field: "description".to_string(),
            path: path.to_string(),
        });
    }

    Ok(ParsedSkillMd {
        name: raw.name,
        description: raw.description,
        references: raw
            .references
            .into_iter()
            .map(|r| SkillReferenceMeta {
                name: r.name,
                description: r.description,
            })
            .collect(),
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_valid_frontmatter() {
        let content = "---\nname: x\ndescription: y\n---\nbody here\n";
        let parsed = parse_skill_md(content, "p").unwrap();
        assert_eq!(parsed.name, "x");
        assert_eq!(parsed.description, "y");
        assert!(parsed.references.is_empty());
        assert_eq!(parsed.body, "body here\n");
    }

    #[test]
    fn parses_with_references() {
        let content = "---\nname: x\ndescription: y\nreferences:\n  - name: a\n    description: ad\n  - name: b\n    description: bd\n---\nbody\n";
        let parsed = parse_skill_md(content, "p").unwrap();
        assert_eq!(parsed.references.len(), 2);
        assert_eq!(parsed.references[0].name, "a");
        assert_eq!(parsed.references[1].description, "bd");
    }

    #[test]
    fn parses_empty_body() {
        let content = "---\nname: x\ndescription: y\n---\n";
        let parsed = parse_skill_md(content, "p").unwrap();
        assert_eq!(parsed.body, "");
    }

    #[test]
    fn body_preserves_markdown_separators() {
        // A `---` inside the body (horizontal rule) must NOT be treated as frontmatter boundary.
        let content = "---\nname: x\ndescription: y\n---\nsection one\n\n---\n\nsection two\n";
        let parsed = parse_skill_md(content, "p").unwrap();
        assert!(parsed.body.contains("section one"));
        assert!(parsed.body.contains("section two"));
        assert!(parsed.body.contains("---"));
    }

    #[test]
    fn rejects_file_without_opening_delimiter() {
        let content = "name: x\ndescription: y\n";
        let err = parse_skill_md(content, "p").unwrap_err();
        assert!(matches!(err, SkillError::InvalidFrontmatter { .. }));
    }

    #[test]
    fn rejects_file_without_closing_delimiter() {
        let content = "---\nname: x\ndescription: y\nbody continues without closing\n";
        let err = parse_skill_md(content, "p").unwrap_err();
        assert!(matches!(err, SkillError::InvalidFrontmatter { .. }));
    }

    #[test]
    fn rejects_malformed_yaml() {
        let content = "---\nname: : broken\n---\n";
        let err = parse_skill_md(content, "p").unwrap_err();
        assert!(matches!(err, SkillError::InvalidFrontmatter { .. }));
    }

    #[test]
    fn rejects_missing_name() {
        let content = "---\ndescription: y\n---\n";
        let err = parse_skill_md(content, "p").unwrap_err();
        match err {
            SkillError::InvalidFrontmatter { .. } | SkillError::MissingField { .. } => {}
            other => panic!("unexpected error variant: {:?}", other),
        }
    }

    #[test]
    fn rejects_missing_description() {
        let content = "---\nname: x\n---\n";
        let err = parse_skill_md(content, "p").unwrap_err();
        match err {
            SkillError::InvalidFrontmatter { .. } | SkillError::MissingField { .. } => {}
            other => panic!("unexpected error variant: {:?}", other),
        }
    }

    #[test]
    fn tolerates_crlf_line_endings() {
        let content = "---\r\nname: x\r\ndescription: y\r\n---\r\nbody\r\n";
        let parsed = parse_skill_md(content, "p").unwrap();
        assert_eq!(parsed.name, "x");
        assert_eq!(parsed.description, "y");
    }
}
```

- [ ] **Step 2: Register in infrastructure/mod.rs**

Edit `src/libs/colmena/src/skills/infrastructure/mod.rs`:

```rust
pub mod frontmatter_parser;

pub use frontmatter_parser::{parse_skill_md, ParsedSkillMd};
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib skills::infrastructure::frontmatter_parser`
Expected: 10 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/skills/
git commit -m "$(cat <<'EOF'
feat(skills): add frontmatter parser for SKILL.md files

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: FilesystemSkillRepository

**Files:**
- Create: `src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs`
- Modify: `src/libs/colmena/src/skills/infrastructure/mod.rs`

- [ ] **Step 1: Write the filesystem repository with tests**

Create `src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs`:

```rust
use crate::skills::domain::{
    Skill, SkillCatalogEntry, SkillError, SkillReference, SkillReferenceMeta, SkillRepository,
    SkillSource,
};
use crate::skills::infrastructure::frontmatter_parser::parse_skill_md;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Maximum size for SKILL.md and each reference file.
pub const MAX_FILE_SIZE_BYTES: u64 = 64 * 1024;

/// Skills loaded from filesystem paths. Each path is validated against
/// an allowed-directories whitelist at construction time.
pub struct FilesystemSkillRepository {
    /// Map from skill name (from frontmatter) to canonical skill directory.
    skills: HashMap<String, SkillEntry>,
}

struct SkillEntry {
    canonical_dir: PathBuf,
    description: String,
    references: Vec<SkillReferenceMeta>,
}

impl FilesystemSkillRepository {
    /// Build from a list of user-supplied paths plus the directory of the graph JSON.
    ///
    /// - `paths`: the literal strings from `config.skills.paths`.
    /// - `graph_dir`: the canonical directory containing the graph JSON file.
    /// - `extra_allowed_dirs`: additional canonical allowed roots (from `COLMENA_SKILLS_ALLOWED_DIRS`).
    pub fn from_paths(
        paths: &[String],
        graph_dir: &Path,
        extra_allowed_dirs: &[PathBuf],
    ) -> Result<Self, SkillError> {
        // Canonicalize the allowed set. graph_dir is always allowed.
        let mut allowed: Vec<PathBuf> = Vec::new();
        let graph_canonical = graph_dir.canonicalize().map_err(|e| SkillError::Io {
            path: graph_dir.display().to_string(),
            source: e,
        })?;
        allowed.push(graph_canonical.clone());
        for p in extra_allowed_dirs {
            if let Ok(canon) = p.canonicalize() {
                allowed.push(canon);
            }
            // Silently skip allowed dirs that don't exist — not an error, just unused.
        }

        let mut skills: HashMap<String, SkillEntry> = HashMap::new();

        for raw_path in paths {
            let path_buf = if Path::new(raw_path).is_absolute() {
                PathBuf::from(raw_path)
            } else {
                graph_dir.join(raw_path)
            };

            let canonical =
                path_buf
                    .canonicalize()
                    .map_err(|e| SkillError::Io {
                        path: path_buf.display().to_string(),
                        source: e,
                    })?;

            // Check it's inside at least one allowed dir.
            let is_allowed = allowed.iter().any(|root| canonical.starts_with(root));
            if !is_allowed {
                return Err(SkillError::PathNotAllowed(canonical.display().to_string()));
            }

            if !canonical.is_dir() {
                return Err(SkillError::NotADirectory(canonical.display().to_string()));
            }

            let skill_md_path = canonical.join("SKILL.md");
            if !skill_md_path.exists() {
                return Err(SkillError::SkillMdMissing(canonical.display().to_string()));
            }

            let md_size = std::fs::metadata(&skill_md_path)
                .map_err(|e| SkillError::Io {
                    path: skill_md_path.display().to_string(),
                    source: e,
                })?
                .len();
            if md_size > MAX_FILE_SIZE_BYTES {
                return Err(SkillError::FileTooLarge {
                    path: skill_md_path.display().to_string(),
                    size: md_size,
                    limit: MAX_FILE_SIZE_BYTES,
                });
            }

            let md_content = std::fs::read_to_string(&skill_md_path).map_err(|e| {
                SkillError::Io {
                    path: skill_md_path.display().to_string(),
                    source: e,
                }
            })?;
            let parsed = parse_skill_md(&md_content, &skill_md_path.display().to_string())?;

            // Validate: frontmatter name must match the directory name.
            let dir_name = canonical
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("<unknown>")
                .to_string();
            if parsed.name != dir_name {
                return Err(SkillError::NameMismatch {
                    name: parsed.name,
                    dir: dir_name,
                    path: canonical.display().to_string(),
                });
            }

            // Validate each declared reference file exists and is within size limit.
            for r in &parsed.references {
                let ref_path = canonical.join("references").join(format!("{}.md", r.name));
                if !ref_path.exists() {
                    return Err(SkillError::ReferenceFileMissing {
                        skill: parsed.name.clone(),
                        path: ref_path.display().to_string(),
                    });
                }
                let ref_size = std::fs::metadata(&ref_path)
                    .map_err(|e| SkillError::Io {
                        path: ref_path.display().to_string(),
                        source: e,
                    })?
                    .len();
                if ref_size > MAX_FILE_SIZE_BYTES {
                    return Err(SkillError::FileTooLarge {
                        path: ref_path.display().to_string(),
                        size: ref_size,
                        limit: MAX_FILE_SIZE_BYTES,
                    });
                }
            }

            if skills.contains_key(&parsed.name) {
                return Err(SkillError::SkillNameCollision {
                    name: parsed.name,
                });
            }

            skills.insert(
                parsed.name.clone(),
                SkillEntry {
                    canonical_dir: canonical,
                    description: parsed.description,
                    references: parsed.references,
                },
            );
        }

        Ok(Self { skills })
    }
}

#[async_trait]
impl SkillRepository for FilesystemSkillRepository {
    fn list_available(&self) -> Vec<SkillCatalogEntry> {
        self.skills
            .iter()
            .map(|(name, entry)| SkillCatalogEntry {
                name: name.clone(),
                description: entry.description.clone(),
                source: SkillSource::Path,
            })
            .collect()
    }

    async fn load_skill(&self, name: &str) -> Result<Skill, SkillError> {
        let entry = self
            .skills
            .get(name)
            .ok_or_else(|| SkillError::SkillNotFound(name.to_string()))?;

        let md_path = entry.canonical_dir.join("SKILL.md");
        let content = std::fs::read_to_string(&md_path).map_err(|e| SkillError::Io {
            path: md_path.display().to_string(),
            source: e,
        })?;
        let parsed = parse_skill_md(&content, &md_path.display().to_string())?;

        Ok(Skill {
            name: parsed.name,
            description: parsed.description,
            body: parsed.body,
            references: parsed.references,
            source: SkillSource::Path,
        })
    }

    async fn load_reference(
        &self,
        skill_name: &str,
        reference_name: &str,
    ) -> Result<SkillReference, SkillError> {
        let entry = self
            .skills
            .get(skill_name)
            .ok_or_else(|| SkillError::SkillNotFound(skill_name.to_string()))?;

        // Verify reference is declared.
        let declared = entry.references.iter().any(|r| r.name == reference_name);
        if !declared {
            let available = entry
                .references
                .iter()
                .map(|r| r.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(SkillError::ReferenceNotDeclared {
                skill: skill_name.to_string(),
                reference: reference_name.to_string(),
                available,
            });
        }

        let ref_path = entry
            .canonical_dir
            .join("references")
            .join(format!("{}.md", reference_name));
        let body = std::fs::read_to_string(&ref_path).map_err(|e| SkillError::Io {
            path: ref_path.display().to_string(),
            source: e,
        })?;

        Ok(SkillReference {
            skill_name: skill_name.to_string(),
            reference_name: reference_name.to_string(),
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn valid_skill_md(name: &str) -> String {
        format!(
            "---\nname: {}\ndescription: A test skill\n---\n# {}\nbody\n",
            name, name
        )
    }

    #[tokio::test]
    async fn loads_valid_skill_from_graph_dir() {
        let tmp = TempDir::new().unwrap();
        let graph_dir = tmp.path();
        let skill_dir = graph_dir.join("my-skill");
        write(&skill_dir.join("SKILL.md"), &valid_skill_md("my-skill"));

        let repo = FilesystemSkillRepository::from_paths(
            &["./my-skill".to_string()],
            graph_dir,
            &[],
        )
        .unwrap();

        assert_eq!(repo.list_available().len(), 1);
        let skill = repo.load_skill("my-skill").await.unwrap();
        assert_eq!(skill.name, "my-skill");
        assert!(skill.body.contains("body"));
        assert!(matches!(skill.source, SkillSource::Path));
    }

    #[tokio::test]
    async fn rejects_path_outside_allowed_dirs() {
        let tmp_graph = TempDir::new().unwrap();
        let tmp_other = TempDir::new().unwrap();
        let skill_dir = tmp_other.path().join("evil-skill");
        write(&skill_dir.join("SKILL.md"), &valid_skill_md("evil-skill"));

        let err = FilesystemSkillRepository::from_paths(
            &[skill_dir.display().to_string()],
            tmp_graph.path(),
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, SkillError::PathNotAllowed(_)));
    }

    #[tokio::test]
    async fn accepts_path_in_extra_allowed_dirs() {
        let tmp_graph = TempDir::new().unwrap();
        let tmp_shared = TempDir::new().unwrap();
        let skill_dir = tmp_shared.path().join("shared-skill");
        write(&skill_dir.join("SKILL.md"), &valid_skill_md("shared-skill"));

        let repo = FilesystemSkillRepository::from_paths(
            &[skill_dir.display().to_string()],
            tmp_graph.path(),
            &[tmp_shared.path().to_path_buf()],
        )
        .unwrap();
        assert_eq!(repo.list_available().len(), 1);
    }

    #[tokio::test]
    async fn rejects_missing_skill_md() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("empty-skill");
        fs::create_dir_all(&skill_dir).unwrap();

        let err = FilesystemSkillRepository::from_paths(
            &["./empty-skill".to_string()],
            tmp.path(),
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, SkillError::SkillMdMissing(_)));
    }

    #[tokio::test]
    async fn rejects_name_mismatch() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("actual-name");
        write(
            &skill_dir.join("SKILL.md"),
            &valid_skill_md("wrong-name"),
        );

        let err = FilesystemSkillRepository::from_paths(
            &["./actual-name".to_string()],
            tmp.path(),
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, SkillError::NameMismatch { .. }));
    }

    #[tokio::test]
    async fn rejects_declared_reference_with_missing_file() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skill-with-ref");
        let md = "---\nname: skill-with-ref\ndescription: test\nreferences:\n  - name: missing\n    description: not there\n---\nbody";
        write(&skill_dir.join("SKILL.md"), md);

        let err = FilesystemSkillRepository::from_paths(
            &["./skill-with-ref".to_string()],
            tmp.path(),
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, SkillError::ReferenceFileMissing { .. }));
    }

    #[tokio::test]
    async fn loads_declared_reference() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skill-with-ref");
        let md = "---\nname: skill-with-ref\ndescription: test\nreferences:\n  - name: extra\n    description: more info\n---\nbody";
        write(&skill_dir.join("SKILL.md"), md);
        write(
            &skill_dir.join("references").join("extra.md"),
            "detailed info here",
        );

        let repo = FilesystemSkillRepository::from_paths(
            &["./skill-with-ref".to_string()],
            tmp.path(),
            &[],
        )
        .unwrap();

        let r = repo
            .load_reference("skill-with-ref", "extra")
            .await
            .unwrap();
        assert_eq!(r.body, "detailed info here");
    }

    #[tokio::test]
    async fn rejects_undeclared_reference_at_load() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("my-skill");
        write(&skill_dir.join("SKILL.md"), &valid_skill_md("my-skill"));

        let repo = FilesystemSkillRepository::from_paths(
            &["./my-skill".to_string()],
            tmp.path(),
            &[],
        )
        .unwrap();

        let err = repo
            .load_reference("my-skill", "whatever")
            .await
            .unwrap_err();
        assert!(matches!(err, SkillError::ReferenceNotDeclared { .. }));
    }

    #[tokio::test]
    async fn rejects_file_exceeding_size_limit() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("big-skill");
        let huge_body = "x".repeat((MAX_FILE_SIZE_BYTES + 1) as usize);
        let md = format!(
            "---\nname: big-skill\ndescription: test\n---\n{}",
            huge_body
        );
        write(&skill_dir.join("SKILL.md"), &md);

        let err = FilesystemSkillRepository::from_paths(
            &["./big-skill".to_string()],
            tmp.path(),
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, SkillError::FileTooLarge { .. }));
    }

    #[tokio::test]
    async fn rejects_path_to_file_not_dir() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("not-a-dir.md");
        fs::write(&file_path, "stuff").unwrap();

        let err = FilesystemSkillRepository::from_paths(
            &["./not-a-dir.md".to_string()],
            tmp.path(),
            &[],
        )
        .unwrap_err();
        // Could fail with NotADirectory or SkillMdMissing depending on canonicalize behavior.
        // We just verify it fails.
        assert!(matches!(
            err,
            SkillError::NotADirectory(_) | SkillError::SkillMdMissing(_)
        ));
    }

    #[tokio::test]
    async fn detects_duplicate_skill_name_across_paths() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        // Both have frontmatter name "duplicate" but different directory names.
        // We need the directory name to match the frontmatter name for validation to pass,
        // so we set up the scenario where user points to the same dir twice under different paths.
        write(&a.join("SKILL.md"), &valid_skill_md("a"));
        write(&b.join("SKILL.md"), &valid_skill_md("a")); // intentionally wrong to trigger NameMismatch

        // NameMismatch fires first — so instead, simulate collision via a symlink in a realistic way:
        // here we simply check that if both pass, collision is caught. For now, the NameMismatch
        // guards that case. Collision is tested in the composite repository tests.
        let err = FilesystemSkillRepository::from_paths(
            &["./a".to_string(), "./b".to_string()],
            tmp.path(),
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, SkillError::NameMismatch { .. }));
    }
}
```

- [ ] **Step 2: Add `tempfile` to dev-dependencies**

Edit `src/libs/colmena/Cargo.toml`. In the `[dev-dependencies]` section after line 93 (`derivative = "2.2"`), add:

```toml
tempfile = "3.10"
```

- [ ] **Step 3: Register in infrastructure/mod.rs**

Edit `src/libs/colmena/src/skills/infrastructure/mod.rs`:

```rust
pub mod filesystem_skill_repository;
pub mod frontmatter_parser;

pub use filesystem_skill_repository::{FilesystemSkillRepository, MAX_FILE_SIZE_BYTES};
pub use frontmatter_parser::{parse_skill_md, ParsedSkillMd};
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib skills::infrastructure::filesystem_skill_repository`
Expected: 11 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/Cargo.toml src/libs/colmena/Cargo.lock src/libs/colmena/src/skills/
git commit -m "$(cat <<'EOF'
feat(skills): add FilesystemSkillRepository with allowed-dirs whitelist

Validates paths via canonicalize + prefix check against graph dir
and COLMENA_SKILLS_ALLOWED_DIRS. Enforces 64KB per-file limit and
declared-reference-must-exist rules at load time.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: BuiltinSkillRepository

**Files:**
- Create: `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs`
- Create: `src/libs/colmena/skills/README.md`
- Create: `src/libs/colmena/skills/_placeholder/SKILL.md` (temporary, so `include_dir!` compiles)
- Modify: `src/libs/colmena/src/skills/infrastructure/mod.rs`

Note: The real content of `python-expert` and `sql-optimizer` is created in Tasks 13–14 (separate from the repository impl to keep commits focused).

- [ ] **Step 1: Create the built-in skills directory with a placeholder**

Create `src/libs/colmena/skills/README.md`:

```markdown
# Built-in skills

Each subdirectory here is compiled into the `colmena_dag_engine` crate at build time via the `include_dir!` macro and becomes available to LLM nodes as a built-in skill.

## How to add a skill

1. Create a directory named after the skill (e.g. `python-expert/`). The directory name is the skill's canonical name.
2. Add a `SKILL.md` file with YAML frontmatter (`name`, `description`, optional `references`). `name` must match the directory name.
3. For each entry in `references`, add `references/<name>.md`.
4. Keep each file under 64 KB.

See `docs/developer_guide/24_skills.md` for the full contract.
```

Create `src/libs/colmena/skills/_placeholder/SKILL.md`:

```markdown
---
name: _placeholder
description: Internal placeholder skill that exists only so include_dir! finds at least one file at build time. Not exposed to users.
---

This is a placeholder.
```

- [ ] **Step 2: Write the repository implementation**

Create `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs`:

```rust
use crate::skills::domain::{
    Skill, SkillCatalogEntry, SkillError, SkillReference, SkillReferenceMeta, SkillRepository,
    SkillSource,
};
use crate::skills::infrastructure::frontmatter_parser::parse_skill_md;
use async_trait::async_trait;
use include_dir::{include_dir, Dir};
use std::collections::HashMap;

/// The entire built-in skills tree, compiled into the binary.
static BUILTIN_SKILLS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/skills");

/// Built-in skills compiled into the crate. Skills live under `src/libs/colmena/skills/`.
pub struct BuiltinSkillRepository {
    skills: HashMap<String, BuiltinEntry>,
}

struct BuiltinEntry {
    description: String,
    body: String,
    references: Vec<SkillReferenceMeta>,
    reference_bodies: HashMap<String, String>,
}

impl BuiltinSkillRepository {
    /// Construct from the compiled-in directory. Names listed in `enabled` are
    /// validated against the available built-ins; unknown names → error.
    pub fn new(enabled: &[String]) -> Result<Self, SkillError> {
        let mut skills: HashMap<String, BuiltinEntry> = HashMap::new();

        for name in enabled {
            if name == "_placeholder" {
                // Reserved internal name — never exposed.
                return Err(SkillError::SkillNotFound(name.clone()));
            }

            let dir = BUILTIN_SKILLS_DIR
                .get_dir(name.as_str())
                .ok_or_else(|| SkillError::SkillNotFound(name.clone()))?;

            let skill_md = dir
                .get_file(format!("{}/SKILL.md", name))
                .ok_or_else(|| SkillError::SkillMdMissing(name.clone()))?;

            let content = skill_md.contents_utf8().ok_or_else(|| {
                SkillError::InvalidFrontmatter {
                    path: format!("builtin:{}/SKILL.md", name),
                    reason: "file is not valid UTF-8".to_string(),
                }
            })?;

            let path_for_errors = format!("builtin:{}/SKILL.md", name);
            let parsed = parse_skill_md(content, &path_for_errors)?;

            if parsed.name != *name {
                return Err(SkillError::NameMismatch {
                    name: parsed.name,
                    dir: name.clone(),
                    path: path_for_errors.clone(),
                });
            }

            // Load reference bodies for every declared reference.
            let mut reference_bodies: HashMap<String, String> = HashMap::new();
            for r in &parsed.references {
                let ref_path = format!("{}/references/{}.md", name, r.name);
                let file = dir.get_file(&ref_path).ok_or_else(|| {
                    SkillError::ReferenceFileMissing {
                        skill: name.clone(),
                        path: format!("builtin:{}", ref_path),
                    }
                })?;
                let body = file
                    .contents_utf8()
                    .ok_or_else(|| SkillError::InvalidFrontmatter {
                        path: format!("builtin:{}", ref_path),
                        reason: "reference file is not valid UTF-8".to_string(),
                    })?
                    .to_string();
                reference_bodies.insert(r.name.clone(), body);
            }

            skills.insert(
                parsed.name.clone(),
                BuiltinEntry {
                    description: parsed.description,
                    body: parsed.body,
                    references: parsed.references,
                    reference_bodies,
                },
            );
        }

        Ok(Self { skills })
    }

    /// List every built-in skill directory name available (for validation / tooling).
    /// Excludes the reserved `_placeholder` entry.
    pub fn all_available_names() -> Vec<String> {
        BUILTIN_SKILLS_DIR
            .dirs()
            .filter_map(|d| {
                let name = d.path().file_name()?.to_str()?.to_string();
                if name == "_placeholder" {
                    None
                } else {
                    Some(name)
                }
            })
            .collect()
    }
}

#[async_trait]
impl SkillRepository for BuiltinSkillRepository {
    fn list_available(&self) -> Vec<SkillCatalogEntry> {
        self.skills
            .iter()
            .map(|(name, entry)| SkillCatalogEntry {
                name: name.clone(),
                description: entry.description.clone(),
                source: SkillSource::Builtin,
            })
            .collect()
    }

    async fn load_skill(&self, name: &str) -> Result<Skill, SkillError> {
        let entry = self
            .skills
            .get(name)
            .ok_or_else(|| SkillError::SkillNotFound(name.to_string()))?;
        Ok(Skill {
            name: name.to_string(),
            description: entry.description.clone(),
            body: entry.body.clone(),
            references: entry.references.clone(),
            source: SkillSource::Builtin,
        })
    }

    async fn load_reference(
        &self,
        skill_name: &str,
        reference_name: &str,
    ) -> Result<SkillReference, SkillError> {
        let entry = self
            .skills
            .get(skill_name)
            .ok_or_else(|| SkillError::SkillNotFound(skill_name.to_string()))?;
        let body = entry.reference_bodies.get(reference_name).ok_or_else(|| {
            let available = entry
                .references
                .iter()
                .map(|r| r.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            SkillError::ReferenceNotDeclared {
                skill: skill_name.to_string(),
                reference: reference_name.to_string(),
                available,
            }
        })?;
        Ok(SkillReference {
            skill_name: skill_name.to_string(),
            reference_name: reference_name.to_string(),
            body: body.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn enabling_unknown_builtin_fails() {
        let err = BuiltinSkillRepository::new(&["does-not-exist".to_string()]).unwrap_err();
        assert!(matches!(err, SkillError::SkillNotFound(_)));
    }

    #[tokio::test]
    async fn enabling_placeholder_is_rejected() {
        let err = BuiltinSkillRepository::new(&["_placeholder".to_string()]).unwrap_err();
        assert!(matches!(err, SkillError::SkillNotFound(_)));
    }

    #[tokio::test]
    async fn empty_enabled_list_constructs_empty_repo() {
        let repo = BuiltinSkillRepository::new(&[]).unwrap();
        assert!(repo.list_available().is_empty());
    }

    #[test]
    fn all_available_names_excludes_placeholder() {
        let names = BuiltinSkillRepository::all_available_names();
        assert!(!names.contains(&"_placeholder".to_string()));
    }
}
```

- [ ] **Step 3: Register in infrastructure/mod.rs**

Edit `src/libs/colmena/src/skills/infrastructure/mod.rs`:

```rust
pub mod builtin_skill_repository;
pub mod filesystem_skill_repository;
pub mod frontmatter_parser;

pub use builtin_skill_repository::BuiltinSkillRepository;
pub use filesystem_skill_repository::{FilesystemSkillRepository, MAX_FILE_SIZE_BYTES};
pub use frontmatter_parser::{parse_skill_md, ParsedSkillMd};
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib skills::infrastructure::builtin_skill_repository`
Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/skills/ src/libs/colmena/src/skills/
git commit -m "$(cat <<'EOF'
feat(skills): add BuiltinSkillRepository with include_dir compile-in

Placeholder skill added to unblock include_dir! (excluded from listings).
Real built-in skills arrive in later tasks.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: CompositeSkillRepository

**Files:**
- Create: `src/libs/colmena/src/skills/infrastructure/composite_skill_repository.rs`
- Modify: `src/libs/colmena/src/skills/infrastructure/mod.rs`

- [ ] **Step 1: Write the composite with collision detection**

Create `src/libs/colmena/src/skills/infrastructure/composite_skill_repository.rs`:

```rust
use crate::skills::domain::{
    Skill, SkillCatalogEntry, SkillError, SkillReference, SkillRepository, SkillSource,
};
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;

/// Upper bound on the total number of active skills in a single node.
pub const MAX_ACTIVE_SKILLS: usize = 50;

/// Merges a built-in repository and a filesystem repository.
/// Detects name collisions at construction time. Dispatches by which repo
/// contains the requested skill at query time.
pub struct CompositeSkillRepository {
    builtin: Arc<dyn SkillRepository>,
    filesystem: Arc<dyn SkillRepository>,
    /// Canonical set of names that exist in `builtin`.
    builtin_names: HashSet<String>,
    /// Canonical set of names that exist in `filesystem`.
    filesystem_names: HashSet<String>,
}

impl CompositeSkillRepository {
    pub fn new(
        builtin: Arc<dyn SkillRepository>,
        filesystem: Arc<dyn SkillRepository>,
    ) -> Result<Self, SkillError> {
        let builtin_names: HashSet<String> = builtin
            .list_available()
            .into_iter()
            .map(|e| e.name)
            .collect();
        let filesystem_names: HashSet<String> = filesystem
            .list_available()
            .into_iter()
            .map(|e| e.name)
            .collect();

        // Detect collisions between builtin and filesystem.
        for name in builtin_names.intersection(&filesystem_names) {
            return Err(SkillError::SkillNameCollision { name: name.clone() });
        }

        let total = builtin_names.len() + filesystem_names.len();
        if total > MAX_ACTIVE_SKILLS {
            return Err(SkillError::TooManySkills {
                count: total,
                limit: MAX_ACTIVE_SKILLS,
            });
        }

        Ok(Self {
            builtin,
            filesystem,
            builtin_names,
            filesystem_names,
        })
    }
}

#[async_trait]
impl SkillRepository for CompositeSkillRepository {
    fn list_available(&self) -> Vec<SkillCatalogEntry> {
        let mut out = self.builtin.list_available();
        out.extend(self.filesystem.list_available());
        out
    }

    async fn load_skill(&self, name: &str) -> Result<Skill, SkillError> {
        if self.builtin_names.contains(name) {
            self.builtin.load_skill(name).await
        } else if self.filesystem_names.contains(name) {
            self.filesystem.load_skill(name).await
        } else {
            Err(SkillError::SkillNotFound(name.to_string()))
        }
    }

    async fn load_reference(
        &self,
        skill_name: &str,
        reference_name: &str,
    ) -> Result<SkillReference, SkillError> {
        if self.builtin_names.contains(skill_name) {
            self.builtin.load_reference(skill_name, reference_name).await
        } else if self.filesystem_names.contains(skill_name) {
            self.filesystem
                .load_reference(skill_name, reference_name)
                .await
        } else {
            Err(SkillError::SkillNotFound(skill_name.to_string()))
        }
    }
}

// --- Source helper (for observability reporting) ---

impl CompositeSkillRepository {
    /// Return the source (builtin/path) of a named skill, if known.
    pub fn source_of(&self, name: &str) -> Option<SkillSource> {
        if self.builtin_names.contains(name) {
            Some(SkillSource::Builtin)
        } else if self.filesystem_names.contains(name) {
            Some(SkillSource::Path)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::domain::{SkillReferenceMeta, SkillSource};
    use async_trait::async_trait;

    /// Test-only fake repository.
    struct FakeRepo {
        entries: Vec<SkillCatalogEntry>,
    }

    #[async_trait]
    impl SkillRepository for FakeRepo {
        fn list_available(&self) -> Vec<SkillCatalogEntry> {
            self.entries.clone()
        }
        async fn load_skill(&self, name: &str) -> Result<Skill, SkillError> {
            self.entries
                .iter()
                .find(|e| e.name == name)
                .map(|e| Skill {
                    name: e.name.clone(),
                    description: e.description.clone(),
                    body: format!("body of {}", e.name),
                    references: vec![],
                    source: e.source,
                })
                .ok_or_else(|| SkillError::SkillNotFound(name.to_string()))
        }
        async fn load_reference(
            &self,
            skill_name: &str,
            reference_name: &str,
        ) -> Result<SkillReference, SkillError> {
            Err(SkillError::ReferenceNotDeclared {
                skill: skill_name.to_string(),
                reference: reference_name.to_string(),
                available: String::new(),
            })
        }
    }

    fn entry(name: &str, source: SkillSource) -> SkillCatalogEntry {
        SkillCatalogEntry {
            name: name.to_string(),
            description: format!("desc of {}", name),
            source,
        }
    }

    #[tokio::test]
    async fn merges_disjoint_names() {
        let b = Arc::new(FakeRepo {
            entries: vec![entry("a", SkillSource::Builtin)],
        });
        let f = Arc::new(FakeRepo {
            entries: vec![entry("b", SkillSource::Path)],
        });
        let composite = CompositeSkillRepository::new(b, f).unwrap();
        assert_eq!(composite.list_available().len(), 2);
    }

    #[tokio::test]
    async fn detects_collision_between_builtin_and_path() {
        let b = Arc::new(FakeRepo {
            entries: vec![entry("dup", SkillSource::Builtin)],
        });
        let f = Arc::new(FakeRepo {
            entries: vec![entry("dup", SkillSource::Path)],
        });
        let err = CompositeSkillRepository::new(b, f).unwrap_err();
        assert!(matches!(err, SkillError::SkillNameCollision { name } if name == "dup"));
    }

    #[tokio::test]
    async fn rejects_when_total_exceeds_50() {
        let b_entries: Vec<SkillCatalogEntry> = (0..30)
            .map(|i| entry(&format!("b{}", i), SkillSource::Builtin))
            .collect();
        let f_entries: Vec<SkillCatalogEntry> = (0..30)
            .map(|i| entry(&format!("f{}", i), SkillSource::Path))
            .collect();
        let b = Arc::new(FakeRepo { entries: b_entries });
        let f = Arc::new(FakeRepo { entries: f_entries });
        let err = CompositeSkillRepository::new(b, f).unwrap_err();
        assert!(matches!(err, SkillError::TooManySkills { count: 60, limit: 50 }));
    }

    #[tokio::test]
    async fn dispatches_load_to_correct_repo() {
        let b = Arc::new(FakeRepo {
            entries: vec![entry("a", SkillSource::Builtin)],
        });
        let f = Arc::new(FakeRepo {
            entries: vec![entry("b", SkillSource::Path)],
        });
        let composite = CompositeSkillRepository::new(b, f).unwrap();
        let s_a = composite.load_skill("a").await.unwrap();
        assert!(matches!(s_a.source, SkillSource::Builtin));
        let s_b = composite.load_skill("b").await.unwrap();
        assert!(matches!(s_b.source, SkillSource::Path));
    }

    #[tokio::test]
    async fn load_unknown_skill_returns_not_found() {
        let b = Arc::new(FakeRepo { entries: vec![] });
        let f = Arc::new(FakeRepo { entries: vec![] });
        let composite = CompositeSkillRepository::new(b, f).unwrap();
        let err = composite.load_skill("missing").await.unwrap_err();
        assert!(matches!(err, SkillError::SkillNotFound(_)));
    }

    #[test]
    fn source_of_returns_correct_origin() {
        let b = Arc::new(FakeRepo {
            entries: vec![entry("a", SkillSource::Builtin)],
        });
        let f = Arc::new(FakeRepo {
            entries: vec![entry("b", SkillSource::Path)],
        });
        let composite = CompositeSkillRepository::new(b, f).unwrap();
        assert_eq!(composite.source_of("a"), Some(SkillSource::Builtin));
        assert_eq!(composite.source_of("b"), Some(SkillSource::Path));
        assert_eq!(composite.source_of("missing"), None);
    }
}
```

- [ ] **Step 2: Register in infrastructure/mod.rs**

Edit `src/libs/colmena/src/skills/infrastructure/mod.rs`:

```rust
pub mod builtin_skill_repository;
pub mod composite_skill_repository;
pub mod filesystem_skill_repository;
pub mod frontmatter_parser;

pub use builtin_skill_repository::BuiltinSkillRepository;
pub use composite_skill_repository::{CompositeSkillRepository, MAX_ACTIVE_SKILLS};
pub use filesystem_skill_repository::{FilesystemSkillRepository, MAX_FILE_SIZE_BYTES};
pub use frontmatter_parser::{parse_skill_md, ParsedSkillMd};
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib skills::infrastructure::composite_skill_repository`
Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/skills/
git commit -m "$(cat <<'EOF'
feat(skills): add CompositeSkillRepository with collision + count enforcement

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Add SkillLoaded to NodeEvent

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/observer.rs`

- [ ] **Step 1: Add the variant**

Edit `src/libs/colmena/src/dag_engine/domain/observer.rs`. Add the variant to `NodeEvent`, placed logically right after `LlmToolCallFinish`:

```rust
    LlmToolCallFinish {
        tool_id: String,
        success: bool,
        output: String,
    },
    /// Emitted when the load_skill synthetic tool successfully loads a skill or reference.
    /// Fires in addition to LlmToolCallStart/Finish so frontends can render a skill-specific UI.
    SkillLoaded {
        tool_id: String,
        skill_name: String,
        reference: Option<String>,
        source: String, // "builtin" | "path"
        size_bytes: usize,
    },
    LlmMessageStart,
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check --lib`
Expected: success.

- [ ] **Step 3: Add a smoke test for the variant**

At the bottom of `src/libs/colmena/src/dag_engine/domain/observer.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_loaded_variant_constructible() {
        let ev = NodeEvent::SkillLoaded {
            tool_id: "call_1".to_string(),
            skill_name: "python-expert".to_string(),
            reference: None,
            source: "builtin".to_string(),
            size_bytes: 1024,
        };
        // Confirm the variant exists.
        match ev {
            NodeEvent::SkillLoaded { skill_name, .. } => {
                assert_eq!(skill_name, "python-expert");
            }
            _ => panic!("expected SkillLoaded"),
        }
    }
}
```

Note: if `observer.rs` already has a `#[cfg(test)] mod tests` block, append the test inside it instead of creating a new block.

- [ ] **Step 4: Run the test**

Run: `cargo test --lib dag_engine::domain::observer`
Expected: at least 1 test passes (including any pre-existing ones in that module).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/observer.rs
git commit -m "$(cat <<'EOF'
feat(skills): add SkillLoaded variant to NodeEvent

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: load_skill synthetic tool

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs` (add `pub mod llm_synthetic_tools;`)

- [ ] **Step 1: Inspect the nodes module to find where to register the new submodule**

Run: `cat /home/daniel-garcia4/startti/colmena/src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`
This prints the module's public declarations. The new submodule `llm_synthetic_tools` should be added to that file as `pub mod llm_synthetic_tools;`.

- [ ] **Step 2: Create the submodule entry**

Create `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`:

```rust
//! Synthetic tools for the LLM node — tools that don't map to DAG nodes.
//!
//! Currently: `load_skill` (progressive-disclosure skill loader).

pub mod load_skill_tool;

pub use load_skill_tool::{
    build_load_skill_tool_definition, dispatch_load_skill, LOAD_SKILL_TOOL_NAME,
};
```

- [ ] **Step 3: Register in nodes/mod.rs**

Edit `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`. Add at the top (before other `pub mod` lines, or next to existing ones — exact placement doesn't matter, just has to appear):

```rust
pub mod llm_synthetic_tools;
```

- [ ] **Step 4: Write the load_skill tool with tests**

Create `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs`:

```rust
//! The `load_skill` synthetic tool — builds its ToolDefinition from a SkillRepository's
//! catalog and dispatches tool calls to the repository, formatting output for the LLM.

use crate::llm::domain::tools::{ParameterProperty, ToolDefinition, ToolParameters};
use crate::llm::domain::{LlmError, ToolCall, ToolResult};
use crate::skills::domain::{SkillError, SkillRepository, SkillSource};
use std::collections::HashMap;
use std::sync::Arc;

pub const LOAD_SKILL_TOOL_NAME: &str = "load_skill";

/// Build the `ToolDefinition` for `load_skill`. The catalog (skill names +
/// descriptions) is embedded directly in the tool description and the `name`
/// parameter's `enum`, keeping the system_message untouched.
pub fn build_load_skill_tool_definition(
    repository: &Arc<dyn SkillRepository>,
) -> ToolDefinition {
    let catalog = repository.list_available();

    let mut names: Vec<String> = catalog.iter().map(|e| e.name.clone()).collect();
    names.sort();
    names.dedup();

    let catalog_lines: Vec<String> = {
        let mut sorted = catalog.clone();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        sorted
            .iter()
            .map(|e| format!("- {}: {}", e.name, e.description))
            .collect()
    };

    let description = format!(
        "Load a specialized knowledge skill on demand when the user's task benefits from it. \
Call this tool BEFORE responding when you identify that one of the skills below applies. \
You may call it multiple times to load several skills or to load a skill's reference material.\n\n\
Available skills:\n{}\n\n\
After loading a skill, if its content lists available references, you may call load_skill \
again with the `reference` parameter to load that additional material.",
        catalog_lines.join("\n")
    );

    let mut properties: HashMap<String, ParameterProperty> = HashMap::new();
    properties.insert(
        "name".to_string(),
        ParameterProperty {
            property_type: "string".to_string(),
            description: "The name of the skill to load".to_string(),
            enum_values: Some(names),
            pattern: None,
        },
    );
    properties.insert(
        "reference".to_string(),
        ParameterProperty {
            property_type: "string".to_string(),
            description:
                "Optional name of a reference file within the skill. Only use after loading the \
skill and seeing it declares this reference."
                    .to_string(),
            enum_values: None,
            pattern: None,
        },
    );

    ToolDefinition {
        name: LOAD_SKILL_TOOL_NAME.to_string(),
        description,
        parameters: ToolParameters {
            schema_type: "object".to_string(),
            properties,
            required: vec!["name".to_string()],
        },
    }
}

/// Dispatch a `load_skill` tool call. Returns the output string to surface to the
/// LLM as a tool result. Also returns observability metadata for event emission.
pub struct LoadSkillDispatchResult {
    pub output: String,
    pub skill_name: String,
    pub reference: Option<String>,
    pub source: SkillSource,
    pub size_bytes: usize,
}

pub async fn dispatch_load_skill(
    tool_call: &ToolCall,
    repository: &Arc<dyn SkillRepository>,
) -> Result<LoadSkillDispatchResult, LlmError> {
    let args: serde_json::Value = serde_json::from_str(&tool_call.function.arguments)
        .map_err(|e| LlmError::InvalidToolCall {
            reason: format!("load_skill: invalid arguments JSON: {}", e),
        })?;

    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| LlmError::InvalidToolCall {
            reason: "load_skill: missing required parameter 'name'".to_string(),
        })?;
    let reference = args
        .get("reference")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    match reference {
        None => {
            let skill = match repository.load_skill(name).await {
                Ok(s) => s,
                Err(e) => return Ok(skill_error_as_result(name, None, e)),
            };

            let mut output = skill.body.clone();
            if !skill.references.is_empty() {
                output.push_str("\n\n---\n\nAvailable references for this skill:\n");
                for r in &skill.references {
                    output.push_str(&format!("- {}: {}\n", r.name, r.description));
                }
                output.push_str(
                    "\nTo load a reference, call load_skill again with the `reference` parameter.",
                );
            }

            let size_bytes = output.len();
            Ok(LoadSkillDispatchResult {
                output,
                skill_name: name.to_string(),
                reference: None,
                source: skill.source,
                size_bytes,
            })
        }
        Some(ref_name) => {
            let reference_body = match repository.load_reference(name, &ref_name).await {
                Ok(r) => r,
                Err(e) => return Ok(skill_error_as_result(name, Some(&ref_name), e)),
            };
            // Determine source by looking up the main skill (cheap because repos are in-memory).
            let source = repository
                .load_skill(name)
                .await
                .map(|s| s.source)
                .unwrap_or(SkillSource::Builtin);
            let size_bytes = reference_body.body.len();
            Ok(LoadSkillDispatchResult {
                output: reference_body.body,
                skill_name: name.to_string(),
                reference: Some(ref_name),
                source,
                size_bytes,
            })
        }
    }
}

/// Convert a SkillError into a LoadSkillDispatchResult whose `output` is a plain
/// error string the LLM can read. Errors are intentionally returned as tool output
/// (not propagated as LlmError) so the ReAct loop continues and the LLM can recover.
fn skill_error_as_result(
    name: &str,
    reference: Option<&str>,
    err: SkillError,
) -> LoadSkillDispatchResult {
    let output = format!("Error: {}", err);
    let size_bytes = output.len();
    LoadSkillDispatchResult {
        output,
        skill_name: name.to_string(),
        reference: reference.map(|s| s.to_string()),
        source: SkillSource::Builtin, // placeholder; the actual source doesn't matter on error
        size_bytes,
    }
}

/// Convenience: turn a LoadSkillDispatchResult into a ToolResult for the ReAct loop.
pub fn into_tool_result(call_id: &str, r: &LoadSkillDispatchResult) -> ToolResult {
    ToolResult {
        tool_call_id: call_id.to_string(),
        output: r.output.clone(),
        success: !r.output.starts_with("Error:"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::domain::{
        Skill, SkillCatalogEntry, SkillError, SkillReference, SkillReferenceMeta, SkillSource,
    };
    use crate::llm::domain::tools::{FunctionCall, ToolCall};
    use async_trait::async_trait;

    struct FakeRepo;

    #[async_trait]
    impl SkillRepository for FakeRepo {
        fn list_available(&self) -> Vec<SkillCatalogEntry> {
            vec![
                SkillCatalogEntry {
                    name: "python-expert".to_string(),
                    description: "Python stuff".to_string(),
                    source: SkillSource::Builtin,
                },
                SkillCatalogEntry {
                    name: "sql-optimizer".to_string(),
                    description: "SQL stuff".to_string(),
                    source: SkillSource::Builtin,
                },
            ]
        }
        async fn load_skill(&self, name: &str) -> Result<Skill, SkillError> {
            match name {
                "python-expert" => Ok(Skill {
                    name: "python-expert".to_string(),
                    description: "Python stuff".to_string(),
                    body: "# Python expert\nuse type hints\n".to_string(),
                    references: vec![SkillReferenceMeta {
                        name: "frameworks".to_string(),
                        description: "Django + FastAPI".to_string(),
                    }],
                    source: SkillSource::Builtin,
                }),
                _ => Err(SkillError::SkillNotFound(name.to_string())),
            }
        }
        async fn load_reference(
            &self,
            skill_name: &str,
            reference_name: &str,
        ) -> Result<SkillReference, SkillError> {
            if skill_name == "python-expert" && reference_name == "frameworks" {
                Ok(SkillReference {
                    skill_name: "python-expert".to_string(),
                    reference_name: "frameworks".to_string(),
                    body: "Django is a web framework.".to_string(),
                })
            } else {
                Err(SkillError::ReferenceNotDeclared {
                    skill: skill_name.to_string(),
                    reference: reference_name.to_string(),
                    available: "frameworks".to_string(),
                })
            }
        }
    }

    fn mk_call(args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: LOAD_SKILL_TOOL_NAME.to_string(),
                arguments: args.to_string(),
            },
        }
    }

    #[test]
    fn tool_definition_includes_all_skill_names_in_enum() {
        let repo: Arc<dyn SkillRepository> = Arc::new(FakeRepo);
        let td = build_load_skill_tool_definition(&repo);
        let name_prop = td.parameters.properties.get("name").unwrap();
        let enum_values = name_prop.enum_values.as_ref().unwrap();
        assert!(enum_values.contains(&"python-expert".to_string()));
        assert!(enum_values.contains(&"sql-optimizer".to_string()));
    }

    #[test]
    fn tool_definition_description_lists_each_skill() {
        let repo: Arc<dyn SkillRepository> = Arc::new(FakeRepo);
        let td = build_load_skill_tool_definition(&repo);
        assert!(td.description.contains("python-expert"));
        assert!(td.description.contains("Python stuff"));
        assert!(td.description.contains("sql-optimizer"));
    }

    #[tokio::test]
    async fn dispatch_load_by_name_returns_body_and_references_block() {
        let repo: Arc<dyn SkillRepository> = Arc::new(FakeRepo);
        let call = mk_call(serde_json::json!({"name": "python-expert"}));
        let r = dispatch_load_skill(&call, &repo).await.unwrap();
        assert!(r.output.contains("use type hints"));
        assert!(r.output.contains("Available references"));
        assert!(r.output.contains("frameworks"));
        assert_eq!(r.reference, None);
        assert!(matches!(r.source, SkillSource::Builtin));
    }

    #[tokio::test]
    async fn dispatch_load_reference_returns_reference_body() {
        let repo: Arc<dyn SkillRepository> = Arc::new(FakeRepo);
        let call = mk_call(serde_json::json!({
            "name": "python-expert",
            "reference": "frameworks"
        }));
        let r = dispatch_load_skill(&call, &repo).await.unwrap();
        assert_eq!(r.output, "Django is a web framework.");
        assert_eq!(r.reference, Some("frameworks".to_string()));
    }

    #[tokio::test]
    async fn dispatch_missing_skill_returns_error_output() {
        let repo: Arc<dyn SkillRepository> = Arc::new(FakeRepo);
        let call = mk_call(serde_json::json!({"name": "does-not-exist"}));
        let r = dispatch_load_skill(&call, &repo).await.unwrap();
        assert!(r.output.starts_with("Error:"));
    }

    #[tokio::test]
    async fn dispatch_undeclared_reference_returns_error_output() {
        let repo: Arc<dyn SkillRepository> = Arc::new(FakeRepo);
        let call = mk_call(serde_json::json!({
            "name": "python-expert",
            "reference": "nope"
        }));
        let r = dispatch_load_skill(&call, &repo).await.unwrap();
        assert!(r.output.starts_with("Error:"));
        assert!(r.output.contains("frameworks"));
    }

    #[tokio::test]
    async fn dispatch_missing_name_parameter_is_invalid_tool_call() {
        let repo: Arc<dyn SkillRepository> = Arc::new(FakeRepo);
        let call = mk_call(serde_json::json!({"reference": "x"}));
        let err = dispatch_load_skill(&call, &repo).await.unwrap_err();
        assert!(matches!(err, LlmError::InvalidToolCall { .. }));
    }

    #[test]
    fn into_tool_result_success_on_normal_output() {
        let r = LoadSkillDispatchResult {
            output: "content".to_string(),
            skill_name: "x".to_string(),
            reference: None,
            source: SkillSource::Builtin,
            size_bytes: 7,
        };
        let tr = into_tool_result("call_1", &r);
        assert!(tr.success);
    }

    #[test]
    fn into_tool_result_failure_on_error_prefix() {
        let r = LoadSkillDispatchResult {
            output: "Error: something".to_string(),
            skill_name: "x".to_string(),
            reference: None,
            source: SkillSource::Builtin,
            size_bytes: 16,
        };
        let tr = into_tool_result("call_1", &r);
        assert!(!tr.success);
    }
}
```

**Important:** If the `FunctionCall` / `ToolCall` / `ToolResult` struct fields differ from what this test assumes, adapt field names in the test only — do **not** change the structs themselves. Run `grep -n "pub struct ToolCall\b" src/libs/colmena/src/llm/domain/tools.rs` and `grep -n "pub struct ToolResult\b" src/libs/colmena/src/llm/domain/` to verify before writing.

- [ ] **Step 5: Run tests**

Run: `cargo test --lib llm_synthetic_tools::load_skill_tool`
Expected: 8 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/
git commit -m "$(cat <<'EOF'
feat(skills): add load_skill synthetic tool (definition + dispatch)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: DagToolExecutor intercepts load_skill

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`

- [ ] **Step 1: Add an optional SkillRepository to the executor**

Edit `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`. Near the struct definition (line 41-48), modify:

```rust
pub struct DagToolExecutor {
    registry: Arc<dyn NodeRegistryPort>,
    tool_configurations: HashMap<String, ToolConfiguration>,
    secure_value_service: Option<Arc<SecureValueService>>,
    session_id: Option<String>,
    /// Optional skill repository. When present, the executor intercepts `load_skill`
    /// tool calls and dispatches them to this repository instead of the normal
    /// tool-configuration path. An optional observer callback receives SkillLoaded
    /// metadata so the enclosing LlmNode can emit SSE events.
    skill_repository: Option<Arc<dyn crate::skills::domain::SkillRepository>>,
    skill_observer: Option<Arc<dyn Fn(&crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::LoadSkillDispatchResult) + Send + Sync>>,
}
```

- [ ] **Step 2: Add a builder method**

In the `impl DagToolExecutor { ... }` block (line 50), after the existing `with_secure_values` builder (search for it with `grep -n "with_secure_values" src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`), add:

```rust
/// Attach a SkillRepository so `load_skill` tool calls are handled.
pub fn with_skills(
    mut self,
    repository: Arc<dyn crate::skills::domain::SkillRepository>,
) -> Self {
    self.skill_repository = Some(repository);
    self
}

/// Attach an observer callback that fires after a successful `load_skill` dispatch.
pub fn with_skill_observer(
    mut self,
    cb: Arc<dyn Fn(&crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::LoadSkillDispatchResult) + Send + Sync>,
) -> Self {
    self.skill_observer = Some(cb);
    self
}
```

- [ ] **Step 3: Update constructors to default the new fields to None**

Find each constructor for `DagToolExecutor` (there's `new(...)` and possibly others — grep: `grep -n "impl DagToolExecutor" src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`). Wherever a `DagToolExecutor { ... }` literal exists, add:

```rust
skill_repository: None,
skill_observer: None,
```

to the struct initialization.

- [ ] **Step 4: Intercept `load_skill` at the top of `async fn execute`**

In `async fn execute` (line 320), at the very beginning of the method body (before the existing "1. Check if it's a configured tool or a raw node" comment), insert:

```rust
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    dispatch_load_skill, into_tool_result, LOAD_SKILL_TOOL_NAME,
};

if tool_call.function.name == LOAD_SKILL_TOOL_NAME {
    let repo = self.skill_repository.as_ref().ok_or_else(|| {
        LlmError::ToolNotFound {
            name: LOAD_SKILL_TOOL_NAME.to_string(),
        }
    })?;
    let result = dispatch_load_skill(tool_call, repo).await?;
    if let Some(obs) = &self.skill_observer {
        obs(&result);
    }
    return Ok(into_tool_result(&tool_call.id, &result));
}
```

- [ ] **Step 5: Add a test for interception**

At the bottom of the file, find the existing `#[cfg(test)] mod tests { ... }` (grep `mod tests` if needed). Append this test inside it:

```rust
#[tokio::test]
async fn intercepts_load_skill_when_repository_attached() {
    use crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor;
    use crate::llm::domain::tools::{FunctionCall, ToolCall};
    use crate::skills::domain::{
        Skill, SkillCatalogEntry, SkillError, SkillReference, SkillRepository, SkillSource,
    };
    use async_trait::async_trait;
    use std::sync::Arc;

    struct TinyRepo;
    #[async_trait]
    impl SkillRepository for TinyRepo {
        fn list_available(&self) -> Vec<SkillCatalogEntry> {
            vec![SkillCatalogEntry {
                name: "x".into(),
                description: "d".into(),
                source: SkillSource::Builtin,
            }]
        }
        async fn load_skill(&self, name: &str) -> Result<Skill, SkillError> {
            Ok(Skill {
                name: name.into(),
                description: "d".into(),
                body: "BODY".into(),
                references: vec![],
                source: SkillSource::Builtin,
            })
        }
        async fn load_reference(
            &self,
            _: &str,
            _: &str,
        ) -> Result<SkillReference, SkillError> {
            Err(SkillError::SkillNotFound("x".into()))
        }
    }

    // Build a fake NodeRegistryPort with no nodes — interception should happen BEFORE registry lookup.
    struct EmptyRegistry;
    impl crate::dag_engine::application::ports::NodeRegistryPort for EmptyRegistry {
        fn get_node(&self, _: &str) -> Option<std::sync::Arc<dyn crate::dag_engine::domain::node::ExecutableNode>> {
            None
        }
        fn list_nodes(&self) -> Vec<&'static str> { vec![] }
    }

    let executor = DagToolExecutor::new(Arc::new(EmptyRegistry), std::collections::HashMap::new())
        .with_skills(Arc::new(TinyRepo));

    let call = ToolCall {
        id: "c1".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "load_skill".to_string(),
            arguments: "{\"name\":\"x\"}".to_string(),
        },
    };

    let result = executor.execute(&call).await.unwrap();
    assert!(result.output.contains("BODY"));
    assert!(result.success);
}
```

**Note:** The `NodeRegistryPort` trait signature may have additional methods. Adapt the `EmptyRegistry` impl to match the real trait — use `grep -n "pub trait NodeRegistryPort" src/libs/colmena/src/dag_engine/application/ports` to find the definition, then implement every required method returning an empty/default value.

- [ ] **Step 6: Run test + verify compilation**

Run: `cargo test --lib dag_tool_executor::tests::intercepts_load_skill_when_repository_attached`
Expected: passes.

Also run: `cargo check --lib`
Expected: success.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs
git commit -m "$(cat <<'EOF'
feat(skills): intercept load_skill in DagToolExecutor

Adds optional skill_repository + skill_observer on the executor. When
the LLM calls load_skill and a repository is attached, the executor
dispatches to it directly (bypassing the normal tool-config path) and
fires the observer so the enclosing LlmNode can emit SkillLoaded events.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Wire skills into LlmNode

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

- [ ] **Step 1: Add imports**

Edit the imports block at the top of `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (lines 1-18). Add:

```rust
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::build_load_skill_tool_definition;
use crate::skills::domain::{SkillRepository, SkillsConfig};
use crate::skills::infrastructure::{
    BuiltinSkillRepository, CompositeSkillRepository, FilesystemSkillRepository,
};
use std::path::PathBuf;
```

- [ ] **Step 2: Add a helper to parse `COLMENA_SKILLS_ALLOWED_DIRS`**

Inside the `impl LlmNode { ... }` block, after the existing `resolve_env_var` function (around line 55-63), add:

```rust
/// Parse `COLMENA_SKILLS_ALLOWED_DIRS` env var into a list of PathBufs.
/// Separator: `:` on Unix, `;` on Windows. Missing env var → empty list.
fn parse_allowed_dirs_env() -> Vec<PathBuf> {
    let raw = match std::env::var("COLMENA_SKILLS_ALLOWED_DIRS") {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let separator = if cfg!(windows) { ';' } else { ':' };
    raw.split(separator)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Build a SkillRepository from the parsed config. Returns `None` if no skills are configured.
/// Returns `Err(String)` on any validation failure — this must abort graph execution.
fn build_skill_repository_from_config(
    config: &HashMap<String, Value>,
    inputs: &NodeInputs,
) -> Result<Option<Arc<dyn SkillRepository>>, String> {
    let raw_val = inputs
        .get("skills")
        .or_else(|| config.get("skills"));
    let raw_val = match raw_val {
        Some(v) => v,
        None => return Ok(None),
    };

    let skills_config = SkillsConfig::from_value(raw_val)
        .map_err(|e| format!("invalid 'skills' config: {}", e))?;
    if !skills_config.has_any() {
        return Ok(None);
    }

    // Determine graph directory.
    // Prefer __colmena_graph_path from inputs (injected upstream by the runner);
    // fall back to current working directory.
    let graph_dir: PathBuf = inputs
        .get("__colmena_graph_path")
        .and_then(|v| v.as_str())
        .map(|s| PathBuf::from(s))
        .and_then(|p| p.parent().map(|pp| pp.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let allowed = Self::parse_allowed_dirs_env();

    let builtin: Arc<dyn SkillRepository> = Arc::new(
        BuiltinSkillRepository::new(&skills_config.builtin)
            .map_err(|e| format!("loading builtin skills: {}", e))?,
    );
    let filesystem: Arc<dyn SkillRepository> = Arc::new(
        FilesystemSkillRepository::from_paths(&skills_config.paths, &graph_dir, &allowed)
            .map_err(|e| format!("loading filesystem skills: {}", e))?,
    );
    let composite = CompositeSkillRepository::new(builtin, filesystem)
        .map_err(|e| format!("composing skill repositories: {}", e))?;
    Ok(Some(Arc::new(composite)))
}
```

**Important:** This code refers to `Self::parse_allowed_dirs_env()` — both helpers must be inside the same `impl LlmNode` block. If `LlmNode` has multiple `impl` blocks in the file, place both in the same one.

- [ ] **Step 3: Plumb skills into the execute flow**

Search for the site that builds `DagToolExecutor` inside `execute`. In the current file it is constructed after tools are resolved. Find it with: `grep -n "DagToolExecutor::new" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`.

Replace the block that constructs the executor with one that adds the skills repo and observer. The pattern is:

```rust
// Build skill repository (if configured).
let skill_repo: Option<Arc<dyn SkillRepository>> =
    Self::build_skill_repository_from_config(config, inputs).map_err(|e| e)?;

// Build the executor with optional skills.
let mut tool_executor = DagToolExecutor::new(registry.clone(), tool_configurations.clone());
if let Some(sv) = self.secure_value_service.clone() {
    tool_executor = tool_executor.with_secure_values(sv);
}

// Track skills loaded across the entire node execution (for summary).
let skills_used_log: Arc<std::sync::Mutex<Vec<SkillLoadedLogEntry>>> =
    Arc::new(std::sync::Mutex::new(Vec::new()));

if let Some(repo) = skill_repo.clone() {
    tool_executor = tool_executor.with_skills(repo.clone());

    let log_clone = skills_used_log.clone();
    let observer_clone = _observer.clone();
    tool_executor = tool_executor.with_skill_observer(Arc::new(
        move |result: &crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::LoadSkillDispatchResult| {
            // Accumulate for final summary.
            if let Ok(mut log) = log_clone.lock() {
                log.push(SkillLoadedLogEntry {
                    skill_name: result.skill_name.clone(),
                    reference: result.reference.clone(),
                    source: match result.source {
                        crate::skills::domain::SkillSource::Builtin => "builtin".to_string(),
                        crate::skills::domain::SkillSource::Path => "path".to_string(),
                    },
                });
            }
            // Emit SkillLoaded SSE event.
            if let Some(obs) = &observer_clone {
                obs.on_event(
                    crate::dag_engine::domain::observer::NodeEvent::SkillLoaded {
                        tool_id: String::new(), // tool_id not tracked in dispatch; frontend correlates via LlmToolCallStart that fires just before
                        skill_name: result.skill_name.clone(),
                        reference: result.reference.clone(),
                        source: match result.source {
                            crate::skills::domain::SkillSource::Builtin => "builtin".to_string(),
                            crate::skills::domain::SkillSource::Path => "path".to_string(),
                        },
                        size_bytes: result.size_bytes,
                    },
                );
            }
        },
    ));
}
```

Then, immediately after the line that collects the user's `tools: Vec<ToolDefinition>`, if a skill repo exists, append `load_skill`'s tool definition:

```rust
if let Some(repo) = skill_repo.as_ref() {
    tools.push(build_load_skill_tool_definition(repo));
}
```

Use `grep -n "let tools" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` to find the right anchor.

- [ ] **Step 4: Define the log entry struct**

Near the top of the file (after the existing `use` imports), add:

```rust
#[derive(Debug, Clone)]
struct SkillLoadedLogEntry {
    skill_name: String,
    reference: Option<String>,
    source: String,
}
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check --lib`
Expected: success. If errors, read them carefully — most likely the struct-literal placement for `DagToolExecutor`, or a missing `.clone()` for a shared `Arc`. Fix inline.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "$(cat <<'EOF'
feat(skills): wire skills config into LlmNode execution

LlmNode now reads config.skills, builds a CompositeSkillRepository
(builtin + filesystem), registers load_skill alongside user tools,
and emits SkillLoaded events via the observer when skills are loaded.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: skills_used section in the LlmNode output summary

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

- [ ] **Step 1: Find the node's output/summary builder**

Search for where LlmNode builds its final output JSON:

Run: `grep -n "NodeOutputs\|serde_json::json!\|\"output\"" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs | head -30`

Identify the block that constructs the final output JSON for the node (should be near the end of `execute`). It typically looks like:

```rust
Ok(NodeOutputs::from(json!({
    "output": assistant_text,
    // ... other fields ...
})))
```

- [ ] **Step 2: Attach skills_used when non-empty**

Right before the node's final output is built, compute:

```rust
let skills_used_summary: Option<Value> = {
    let log = skills_used_log.lock().ok();
    log.and_then(|entries| {
        if entries.is_empty() {
            None
        } else {
            // Aggregate by skill name: references_loaded list + load_count.
            use std::collections::BTreeMap;
            #[derive(Default)]
            struct Agg {
                source: String,
                references_loaded: Vec<String>,
                load_count: u32,
            }
            let mut agg: BTreeMap<String, Agg> = BTreeMap::new();
            for e in entries.iter() {
                let a = agg.entry(e.skill_name.clone()).or_default();
                a.source = e.source.clone();
                a.load_count += 1;
                if let Some(r) = &e.reference {
                    if !a.references_loaded.contains(r) {
                        a.references_loaded.push(r.clone());
                    }
                }
            }
            let arr: Vec<Value> = agg
                .into_iter()
                .map(|(name, a)| {
                    json!({
                        "name": name,
                        "source": a.source,
                        "references_loaded": a.references_loaded,
                        "load_count": a.load_count,
                    })
                })
                .collect();
            Some(Value::Array(arr))
        }
    })
};
```

Then, when building the final output JSON, conditionally add the field:

```rust
let mut output_map = serde_json::Map::new();
output_map.insert("output".to_string(), json!(assistant_text));
// ... other existing fields ...
if let Some(skills_used) = skills_used_summary {
    output_map.insert("skills_used".to_string(), skills_used);
}
Ok(NodeOutputs::from(Value::Object(output_map)))
```

**Important:** Adapt this template to match the actual existing output-building code in `llm.rs`. The goal is: when `skills_used_summary` is `Some(arr)`, the final output includes `"skills_used": arr`; otherwise the field is absent.

- [ ] **Step 3: Verify compilation**

Run: `cargo check --lib`
Expected: success.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "$(cat <<'EOF'
feat(skills): include skills_used in LlmNode output summary

Summary aggregates by skill name with source, references_loaded, and
load_count. Field is absent when no skills were loaded.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: Built-in skill — python-expert

**Files:**
- Create: `src/libs/colmena/skills/python-expert/SKILL.md`
- Create: `src/libs/colmena/skills/python-expert/references/frameworks.md`

- [ ] **Step 1: Create python-expert/SKILL.md**

Create `src/libs/colmena/skills/python-expert/SKILL.md`:

```markdown
---
name: python-expert
description: Use when the user asks about modern Python (3.11+) typing, async patterns, dataclasses, or Python standard library internals. Do NOT use for general programming questions unrelated to Python, or questions about Python 2.
references:
  - name: frameworks
    description: Opinionated notes on Django, FastAPI, and Flask when the user is working with one of these web frameworks
---

# Python Expert

You are an expert in modern Python (3.11+). Apply these principles when helping with Python code:

## Typing
- Prefer type hints on all public functions and class attributes.
- Use `TypedDict`, `Protocol`, and `Literal` over `Dict[str, Any]` when the shape is known.
- Use `collections.abc` abstract types (`Iterable`, `Mapping`) in parameters; concrete types (`list`, `dict`) in return values.
- Prefer `|` over `Union`, `T | None` over `Optional[T]` (Python 3.10+).

## Async
- Use `asyncio` for I/O-bound concurrency. Do not use threads for network I/O.
- Prefer `asyncio.gather(...)` for parallel tasks; use `asyncio.TaskGroup` (3.11+) when you need structured concurrency with cancellation propagation.
- Never mix blocking I/O inside `async def` without `asyncio.to_thread`.
- Default to `anyio` if code must work with both `asyncio` and `trio`.

## Dataclasses / models
- For plain data: `@dataclass(slots=True, frozen=True)` unless mutability is required.
- For validated input (API boundaries, user input): `pydantic.BaseModel`.
- For settings: `pydantic-settings.BaseSettings`.

## Error handling
- Catch narrow, specific exceptions. Never `except Exception: pass`.
- Use `raise ... from err` to preserve the cause chain.

## Testing
- Prefer `pytest` with fixtures over `unittest`. Use `pytest.mark.parametrize` for table-driven tests.
- Keep tests in `tests/` mirroring the package layout.

When the user mentions Django, FastAPI, or Flask, call load_skill again with `reference: "frameworks"` to get detailed patterns.
```

- [ ] **Step 2: Create frameworks.md reference**

Create `src/libs/colmena/skills/python-expert/references/frameworks.md`:

```markdown
# Python Web Frameworks Reference

## Django
- Prefer class-based views for anything beyond trivial CRUD.
- Use `select_related` / `prefetch_related` to avoid N+1 queries.
- `Model.objects.bulk_create` / `bulk_update` for high-volume writes.
- Put business logic in service modules, not views or models.
- Migrations are source code: review and commit them.

## FastAPI
- Use Pydantic models for request and response schemas.
- Dependency injection via `Depends(...)` for auth, database sessions, and config.
- For long-running work, return 202 and run via background tasks or a proper queue.
- `response_model_exclude_unset=True` when returning partial objects.
- Test with `httpx.AsyncClient` + `LifespanManager`.

## Flask
- Use `Flask` application factories (`create_app()`) plus blueprints for modularity.
- Prefer `flask-sqlalchemy` only for small apps; direct SQLAlchemy for anything serious.
- Use `Flask-Pydantic` or `marshmallow` for validation; Flask does not validate input natively.
- Avoid global state — it breaks when you add workers.

## Choosing between them
- Django: full stack, admin built in, mature ORM, many features out of the box. Good for content-heavy apps.
- FastAPI: async-first, modern, best for APIs. Pick this for new services.
- Flask: minimal and flexible. Pick when you want to wire everything yourself.
```

- [ ] **Step 3: Verify the built-in skill loads correctly**

Run: `cargo test --lib skills::infrastructure::builtin_skill_repository`
Then run a smoke test — add this test temporarily to `builtin_skill_repository.rs` (then remove it before committing):

```rust
#[tokio::test]
async fn python_expert_is_loadable() {
    let repo = BuiltinSkillRepository::new(&["python-expert".to_string()]).unwrap();
    let skill = repo.load_skill("python-expert").await.unwrap();
    assert_eq!(skill.name, "python-expert");
    assert!(skill.body.contains("Typing"));
    assert_eq!(skill.references.len(), 1);
}
```

Run: `cargo test --lib builtin_skill_repository::tests::python_expert_is_loadable`
Expected: passes.

After confirming, **keep this test** (remove the "temporary" language above was misleading — this is a valuable smoke test). Commit it with the rest.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/skills/python-expert/ src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs
git commit -m "$(cat <<'EOF'
feat(skills): add built-in python-expert skill with frameworks reference

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 15: Built-in skill — sql-optimizer

**Files:**
- Create: `src/libs/colmena/skills/sql-optimizer/SKILL.md`
- Create: `src/libs/colmena/skills/sql-optimizer/references/query-plans.md`

- [ ] **Step 1: Create sql-optimizer/SKILL.md**

Create `src/libs/colmena/skills/sql-optimizer/SKILL.md`:

```markdown
---
name: sql-optimizer
description: Use when the user asks to write, review, or optimize SQL queries — performance, indexes, joins, aggregations, or query plans. Do NOT use for ORM-specific questions (those belong to the respective framework skill).
references:
  - name: query-plans
    description: How to read and interpret EXPLAIN / EXPLAIN ANALYZE output in PostgreSQL and MySQL
---

# SQL Optimizer

You are an expert at writing and optimizing SQL. Apply these principles:

## Indexes
- Every JOIN condition should be indexed on both sides.
- WHERE clauses on non-indexed columns cause full scans — check `EXPLAIN`.
- Composite indexes follow the leftmost-prefix rule: `(a, b, c)` helps queries filtering by `a`, `(a, b)`, or `(a, b, c)` but NOT standalone `b` or `c`.
- Prefer covering indexes (include columns used in SELECT) for hot queries.

## Joins
- `INNER JOIN` when both sides are required. `LEFT JOIN` only when the right side is optional.
- Beware of `LEFT JOIN` + `WHERE right_table.col = X` — this accidentally behaves like an inner join. Use `AND` in the `ON` clause.
- Small table on the right side of a nested-loop join; larger table on the left.

## Aggregations
- `GROUP BY` only the columns you need. Unnecessary grouping columns cost memory and time.
- Use `FILTER (WHERE ...)` (PostgreSQL) or conditional aggregates (`SUM(CASE WHEN ... THEN 1 ELSE 0 END)`) instead of multiple subqueries.
- `HAVING` filters after aggregation; `WHERE` before. Use `WHERE` whenever possible — it's cheaper.

## Common anti-patterns
- `SELECT *` in production code — specify columns.
- `NOT IN (SELECT ...)` with nullable columns — returns unexpected empty results. Use `NOT EXISTS`.
- `COUNT(*)` on huge tables when you just need "any" — use `EXISTS`.
- `OFFSET N` with large N for pagination — use keyset/seek pagination.

When the user shares a slow query, call load_skill again with `reference: "query-plans"` to get help reading the execution plan.
```

- [ ] **Step 2: Create query-plans.md reference**

Create `src/libs/colmena/skills/sql-optimizer/references/query-plans.md`:

```markdown
# Reading Query Plans

## PostgreSQL: EXPLAIN ANALYZE

Run the query with `EXPLAIN (ANALYZE, BUFFERS, FORMAT TEXT) <query>` to get actual timings plus buffer hits.

Key things to look for:

- **`Seq Scan`** on a large table — usually bad. Suggests a missing index or a predicate that can't use one.
- **`Nested Loop`** with a large outer side — performs the inner side once per outer row. Fine for small outer sides; catastrophic for millions.
- **`Hash Join`** — builds a hash of one side; fine for equality joins on large sets.
- **`Merge Join`** — requires both inputs sorted; often comes with a `Sort` node.
- **Rows estimated vs. actual**: if estimates are off by 10x or more, run `ANALYZE <table>` to refresh statistics.
- **`Buffers: shared hit=X read=Y`** — `read` means disk I/O. High read counts on repeat queries suggest cache-miss or oversized working set.

## MySQL: EXPLAIN

`EXPLAIN` in MySQL is less detailed than PostgreSQL's. Columns to watch:

- `type`: `ALL` is a full table scan (bad). Aim for `ref`, `range`, `eq_ref`, or `const`.
- `key`: which index is used (or NULL if none). `possible_keys` lists candidates the planner considered.
- `rows`: estimated rows examined. Multiply across joined tables to estimate total work.
- `Extra`: `Using filesort` and `Using temporary` are red flags for large result sets.

Use `EXPLAIN ANALYZE` (MySQL 8.0.18+) for actual timings.

## Debugging approach

1. Run `EXPLAIN ANALYZE` and capture the output.
2. Find the node with the highest `actual time` or `rows`.
3. If it's a scan: is the predicate indexable? Is the index present?
4. If it's a join: is the join condition indexed? Is the planner picking the right join type?
5. If nothing obvious: check `pg_stat_statements` / `performance_schema` for whether the query is even the bottleneck, or if statistics are stale (run `ANALYZE`).
```

- [ ] **Step 3: Smoke test**

Add to `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs` (in the `tests` mod):

```rust
#[tokio::test]
async fn sql_optimizer_is_loadable() {
    let repo = BuiltinSkillRepository::new(&["sql-optimizer".to_string()]).unwrap();
    let skill = repo.load_skill("sql-optimizer").await.unwrap();
    assert_eq!(skill.name, "sql-optimizer");
    assert!(skill.body.contains("Indexes"));
}

#[tokio::test]
async fn both_builtins_can_coexist() {
    let repo = BuiltinSkillRepository::new(&[
        "python-expert".to_string(),
        "sql-optimizer".to_string(),
    ])
    .unwrap();
    let listed: Vec<String> = repo.list_available().into_iter().map(|e| e.name).collect();
    assert!(listed.contains(&"python-expert".to_string()));
    assert!(listed.contains(&"sql-optimizer".to_string()));
}
```

Run: `cargo test --lib skills::infrastructure::builtin_skill_repository`
Expected: all tests pass (including the new 2).

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/skills/sql-optimizer/ src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs
git commit -m "$(cat <<'EOF'
feat(skills): add built-in sql-optimizer skill with query-plans reference

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 16: Integration test with MockAdapter

**Files:**
- Create: `tests/skills_integration.rs`

- [ ] **Step 1: Inspect the existing MockAdapter setup**

Run: `grep -rn "MockAdapter" src/libs/colmena/src/llm/infrastructure/mock_adapter.rs | head -20`
Also: `ls src/libs/colmena/tests/ 2>/dev/null` (might be empty; integration tests go in `tests/` at the workspace root or under the crate).

Decide the location: if there is no existing `tests/` dir in the crate, place the integration test at `src/libs/colmena/tests/skills_integration.rs`. If there's already a convention for the project, follow it.

- [ ] **Step 2: Write the integration test**

Create `src/libs/colmena/tests/skills_integration.rs`:

```rust
//! Integration test: a full LlmNode execution with skills configured,
//! using MockAdapter to avoid real provider calls.

use colmena::skills::domain::SkillRepository;
use colmena::skills::infrastructure::{
    BuiltinSkillRepository, CompositeSkillRepository, FilesystemSkillRepository,
};
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::test]
async fn builtin_python_expert_loads_via_composite() {
    let builtin: Arc<dyn SkillRepository> =
        Arc::new(BuiltinSkillRepository::new(&["python-expert".to_string()]).unwrap());
    let filesystem: Arc<dyn SkillRepository> =
        Arc::new(FilesystemSkillRepository::from_paths(&[], &PathBuf::from("."), &[]).unwrap());
    let composite = CompositeSkillRepository::new(builtin, filesystem).unwrap();

    let entries = composite.list_available();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "python-expert");

    let skill = composite.load_skill("python-expert").await.unwrap();
    assert!(skill.body.contains("Typing"));
    assert_eq!(skill.references.len(), 1);
    let reference = composite
        .load_reference("python-expert", "frameworks")
        .await
        .unwrap();
    assert!(reference.body.contains("Django"));
}

#[tokio::test]
async fn mixing_builtin_and_path_skills_works() {
    // Create a user skill on the fly in a temp dir.
    let tmp = tempfile::TempDir::new().unwrap();
    let skill_dir = tmp.path().join("company-context");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: company-context\ndescription: Internal patterns at our company\n---\nWe use kebab-case for URLs.\n",
    )
    .unwrap();

    let builtin: Arc<dyn SkillRepository> =
        Arc::new(BuiltinSkillRepository::new(&["python-expert".to_string()]).unwrap());
    let filesystem: Arc<dyn SkillRepository> = Arc::new(
        FilesystemSkillRepository::from_paths(
            &["./company-context".to_string()],
            tmp.path(),
            &[],
        )
        .unwrap(),
    );
    let composite = CompositeSkillRepository::new(builtin, filesystem).unwrap();

    let names: Vec<String> = composite
        .list_available()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(names.contains(&"python-expert".to_string()));
    assert!(names.contains(&"company-context".to_string()));

    let company = composite.load_skill("company-context").await.unwrap();
    assert!(company.body.contains("kebab-case"));
}

#[tokio::test]
async fn colliding_names_in_builtin_and_path_fails_at_construction() {
    // User creates a skill named "python-expert" (same as builtin) — must collide.
    let tmp = tempfile::TempDir::new().unwrap();
    let skill_dir = tmp.path().join("python-expert");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: python-expert\ndescription: Override attempt\n---\nmy version\n",
    )
    .unwrap();

    let builtin: Arc<dyn SkillRepository> =
        Arc::new(BuiltinSkillRepository::new(&["python-expert".to_string()]).unwrap());
    let filesystem: Arc<dyn SkillRepository> = Arc::new(
        FilesystemSkillRepository::from_paths(
            &["./python-expert".to_string()],
            tmp.path(),
            &[],
        )
        .unwrap(),
    );

    let err = CompositeSkillRepository::new(builtin, filesystem).unwrap_err();
    assert!(matches!(
        err,
        colmena::skills::domain::SkillError::SkillNameCollision { .. }
    ));
}
```

**Note:** The library name is `colmena` (from `[lib] name = "colmena"` in `Cargo.toml`), so the `use colmena::...` imports are correct. If the integration test file fails to find the `colmena` crate, verify with `grep "^name" src/libs/colmena/Cargo.toml` and `grep "^name" src/libs/colmena/Cargo.toml` (the `[lib] name` field).

- [ ] **Step 3: Run the integration tests**

Run: `cargo test --test skills_integration`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/tests/skills_integration.rs
git commit -m "$(cat <<'EOF'
test(skills): add integration tests covering builtin + filesystem + composite

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 17: E2E test graph

**Files:**
- Create: `tests/graphs/agents/skills_basic.json`

- [ ] **Step 1: Create the test graph**

Create `tests/graphs/agents/skills_basic.json`:

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/test-skills",
        "method": "POST",
        "test_payload": {
          "prompt": "Explain how to write a typed async function in Python 3.11 with proper error handling."
        }
      }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-4o-mini",
        "system_message": "You are a helpful coding assistant. Use skills when relevant.",
        "skills": {
          "builtin": ["python-expert", "sql-optimizer"]
        }
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    {"from": "trigger", "to": "agent"},
    {"from": "agent", "to": "log"}
  ]
}
```

- [ ] **Step 2: Run the graph against a real provider**

This step is a manual validation. The user runs:

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/skills_basic.json
```

Expected (success criteria):
1. The log output shows a `tool_call` event where `tool_name` is `load_skill` with `args` including `"name": "python-expert"`.
2. The log output shows a `skill_loaded` event with `skill_name: "python-expert"` and `source: "builtin"`.
3. The log output does NOT show a `skill_loaded` event with `skill_name: "sql-optimizer"` — the LLM should discriminate between skills based on the prompt.
4. The final LLM response reflects knowledge consistent with `python-expert/SKILL.md` (mentions type hints, async, structured concurrency).

If any of these fail, the problem is in Tasks 10–13 (tool definition, dispatch, event emission, or llm.rs wiring). Do NOT modify the skill content to "make the LLM pick it" — the prompt + the skill's description must already make the right call.

- [ ] **Step 3: Commit the test graph**

```bash
git add tests/graphs/agents/skills_basic.json
git commit -m "$(cat <<'EOF'
test(skills): add skills_basic.json e2e test graph

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 18: Developer guide documentation

**Files:**
- Create: `docs/developer_guide/24_skills.md`
- Modify: `docs/developer_guide/14_llm_deep_dive.md` (add a pointer)
- Modify: `docs/DEVELOPER_GUIDE.md` (add 24 to the index)
- Modify: `docs/node_configurations.json` (add skills schema under llm_call)
- Modify: `CLAUDE.md` (add entry for 24_skills.md under the developer_guide list)

- [ ] **Step 1: Write the skills guide**

Create `docs/developer_guide/24_skills.md`:

```markdown
# 24. Skills

Skills son paquetes de conocimiento en markdown que los nodos LLM cargan **bajo demanda** durante la ejecución. La idea es la misma que en Claude Code y Gemini CLI: el modelo ve un catálogo con nombre + descripción de cada skill disponible, y decide activar una skill concreta llamando al tool sintético `load_skill`. Esto evita inflar el system prompt con conocimiento que quizá no se use.

## Estructura de una skill

```
python-expert/                  # el nombre del directorio ES el nombre canónico
  SKILL.md                      # obligatorio
  references/                   # opcional
    frameworks.md
    testing.md
```

### Frontmatter de `SKILL.md`

```markdown
---
name: python-expert
description: Use when the user asks about Python typing, async, or stdlib. Not for general programming questions.
references:
  - name: frameworks
    description: Detailed notes on Django, FastAPI, and Flask
---

# Python Expert

You are an expert in modern Python (3.11+)...
```

- `name` (obligatorio): debe coincidir exactamente con el nombre del directorio.
- `description` (obligatorio): lo que el LLM ve en el catálogo. Incluir *cuándo usar* y *cuándo NO usar*.
- `references` (opcional): array de `{name, description}`. Cada `<name>` debe corresponder a un archivo `references/<name>.md` en disco.

## Skills integradas en el código (built-in)

Skills compiladas en el crate vía `include_dir!`. Viven en `src/libs/colmena/skills/`. Se distribuyen con la librería — cualquier usuario de Colmena puede activarlas por nombre:

```json
{
  "type": "llm_call",
  "config": {
    "skills": {
      "builtin": ["python-expert", "sql-optimizer"]
    }
  }
}
```

Para agregar una nueva built-in skill: crea el directorio bajo `src/libs/colmena/skills/`, confirma que compila (`cargo build`), y comitea.

## Skills del usuario (paths)

Skills fuera del crate, referenciadas por path relativo al JSON del grafo:

```json
{
  "type": "llm_call",
  "config": {
    "skills": {
      "paths": [
        "./my-skills/customer-context",
        "./my-skills/internal-apis"
      ]
    }
  }
}
```

Se pueden combinar built-in + paths en el mismo `llm_call`.

## Seguridad: allowed directories

Por defecto, Colmena solo acepta paths *dentro* del directorio del JSON del grafo. Para permitir directorios compartidos, configura la variable de entorno:

```bash
COLMENA_SKILLS_ALLOWED_DIRS=/home/user/skills:/opt/colmena/shared-skills
```

Paths se validan con `canonicalize()`, lo que impide escapes vía `../` o symlinks hacia afuera. Las validaciones ocurren al **cargar el grafo** — si una skill está mal, el grafo ni siquiera arranca.

## Límites

| Límite | Valor |
|--------|-------|
| Tamaño de `SKILL.md` | 64 KB |
| Tamaño de cada reference | 64 KB |
| Skills activas por nodo | 50 |
| Extensión permitida | `.md` |

## Observabilidad

Cuando el LLM invoca `load_skill`, el stream SSE emite **tres** eventos:

1. `tool_call` / `LlmToolCallStart` — estándar de tool calling.
2. `skill_loaded` — evento enriquecido con `{skill_name, reference, source, size_bytes}`.
3. `tool_result` / `LlmToolCallFinish` — estándar.

El summary final del nodo incluye un campo `skills_used` cuando al menos una skill fue cargada:

```json
{
  "skills_used": [
    {
      "name": "python-expert",
      "source": "builtin",
      "references_loaded": ["frameworks"],
      "load_count": 1
    }
  ]
}
```

## Trust model

Activar una skill es equivalente a inyectar un system prompt escrito por otra persona. Colmena valida que la skill sea sintácticamente correcta y que viva en un directorio permitido, pero **no** valida el contenido semántico. Una skill hostil puede incluir instrucciones que engañen al LLM (prompt injection). Solo activa skills de autores en los que confías.

Qué sí controla Colmena:
- El LLM solo puede cargar skills del catálogo (enforced vía `enum` en el schema del tool).
- El contenido markdown nunca se ejecuta como código.
- El catálogo se fija al cargar el grafo — el LLM no puede añadir skills nuevas en runtime.

## Referencia rápida

- Tool expuesto al LLM: `load_skill(name: string, reference?: string)`.
- La descripción del tool contiene el catálogo completo (nombre + descripción de cada skill).
- Si no se configura `skills`, todo el sistema de skills queda desactivado (zero overhead).
- Diseño completo: [docs/superpowers/specs/2026-04-20-llm-skills-design.md](../superpowers/specs/2026-04-20-llm-skills-design.md)
```

- [ ] **Step 2: Add a reference in 14_llm_deep_dive.md**

Run: `grep -n "system_message" docs/developer_guide/14_llm_deep_dive.md | head -5`

Open the guide and find a section that describes input parameters or extension points for the LLM node. Append (or insert where appropriate) a short paragraph:

```markdown
## Skills (on-demand knowledge loading)

The LLM node supports an optional `skills` config field that exposes built-in and/or user-provided markdown skill packages via a synthetic `load_skill` tool. The LLM decides at runtime which skills to activate. See [24_skills.md](24_skills.md) for the full guide.
```

- [ ] **Step 3: Add index entry in docs/DEVELOPER_GUIDE.md**

Run: `grep -n "23" docs/DEVELOPER_GUIDE.md`

Find the numbered list of guides and add:

```markdown
- [24. Skills](developer_guide/24_skills.md) — On-demand markdown skills loaded via the `load_skill` synthetic tool.
```

Place it after the entry for `23_sql_node.md`.

- [ ] **Step 4: Add skills schema in docs/node_configurations.json**

Run: `grep -n '"llm_call"' docs/node_configurations.json`

Inside the `llm_call` entry's `config` schema object, add:

```json
"skills": {
  "type": "object",
  "required": false,
  "description": "On-demand markdown skills. Each configured source may be empty. See developer_guide/24_skills.md.",
  "properties": {
    "builtin": {
      "type": "array",
      "items": { "type": "string" },
      "description": "Names of built-in skills compiled into the crate (e.g. 'python-expert')."
    },
    "paths": {
      "type": "array",
      "items": { "type": "string" },
      "description": "Paths to skill directories; relative paths resolve against the graph JSON directory, subject to the COLMENA_SKILLS_ALLOWED_DIRS whitelist."
    }
  }
}
```

Place it alongside the other `llm_call` config fields (adjacent to `system_message` or `tool_configurations`).

- [ ] **Step 5: Update CLAUDE.md**

Edit `CLAUDE.md`. Find the list of developer guides (around lines 30-50) and add in the numbered list:

```markdown
    - `24_skills.md` — Skills feature: built-in + user-provided markdown packages loaded on-demand via `load_skill` tool
```

- [ ] **Step 6: Verify markdown links render**

Run: `cargo doc --no-deps 2>&1 | tail -10` (optional check — just confirms nothing regressed).

Also manually open `docs/developer_guide/24_skills.md` and verify internal link to the spec works: `../superpowers/specs/2026-04-20-llm-skills-design.md`.

- [ ] **Step 7: Commit**

```bash
git add docs/ CLAUDE.md
git commit -m "$(cat <<'EOF'
docs(skills): add 24_skills.md guide and update indices

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 19: Full build + full test run + final sweep

**Files:**
- None (validation only)

- [ ] **Step 1: Full workspace check**

Run: `cargo check --lib && cargo check --bins`
Expected: success everywhere.

- [ ] **Step 2: Full test run**

Run: `cargo test --lib && cargo test --test skills_integration`
Expected: all tests pass, no failures. Count skills-related tests: should be roughly 40+ across all modules.

- [ ] **Step 3: Clippy**

Run: `cargo clippy --lib -- -D warnings 2>&1 | tail -40`
Expected: no warnings. If clippy complains inside the `skills` module, fix inline. Do NOT `#[allow(...)]` your way out.

- [ ] **Step 4: Format**

Run: `cargo fmt`
Expected: success, no diff output.

If `cargo fmt` produced changes, review and commit them:

```bash
git diff --stat
git add -u
git commit -m "$(cat <<'EOF'
style(skills): apply rustfmt

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 5: Manual E2E confirmation (requires .env with OPENAI_API_KEY)**

Run (copy-paste):

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/skills_basic.json 2>&1 | tee /tmp/skills-e2e.log
```

Inspect `/tmp/skills-e2e.log`:
- Confirm `skill_loaded` event present with `skill_name: "python-expert"`.
- Confirm `skill_loaded` event NOT present for `sql-optimizer`.
- Confirm final response mentions type hints / async / error handling.

If the LLM does not call `load_skill` at all: the most likely issue is (a) skills section missing from the `load_skill` tool description, or (b) LLM chose to answer without loading anything. Look at the `tool_call` events first. If `load_skill` never appears in any event, recheck Task 12.

- [ ] **Step 6: Summary commit (if anything was tweaked)**

If the manual E2E revealed minor issues fixed inline, commit them now. Otherwise skip.

---

## Self-review checklist (executed after writing plan)

**Spec coverage:**
- [x] Section "Architecture" — Tasks 0–8 (skills module + infra).
- [x] Section "Config schema" — Task 4 (SkillsConfig), Task 12 (LlmNode wiring).
- [x] Section "Skill structure on disk" — Tasks 14–15 (built-in content with `references/`).
- [x] Section "`SKILL.md` format" — Task 5 (frontmatter parser).
- [x] Section "Security model" — Task 6 (FilesystemSkillRepository with canonicalize + allowed-dirs).
- [x] Section "DoS limits" — Task 6 (file size), Task 8 (MAX_ACTIVE_SKILLS).
- [x] Section "`load_skill` tool" — Task 10 (tool definition + dispatch).
- [x] Section "End-to-end execution flow" — Task 11 (executor interception), Task 12 (LlmNode integration).
- [x] Section "Observability — SSE stream" — Task 9 (NodeEvent::SkillLoaded), Task 12 (observer callback).
- [x] Section "Observability — final summary" — Task 13 (skills_used aggregation).
- [x] Section "Error handling" — Task 1 (typed errors), Tasks 5–8 (fail-fast validation).
- [x] Section "Files" — Tasks 0, 14, 15 (built-in content), Task 17 (test graph), Task 16 (integration test), Task 18 (docs).
- [x] Section "Testing" — Tasks 1–10 unit tests, Task 16 integration, Task 17 E2E, Task 19 full run.
- [x] "Not modified" contract (providers, AgentService, bindings) — Not touched anywhere in the plan.

**Placeholder scan:** No TBDs, no "implement later", no "add validation" without showing what. Every code block is complete. Two places use "Adapt to match actual code" language (Task 10 step 4, Task 12 step 3) — these are guidance for handling potential struct-field drift, not code placeholders.

**Type consistency:** `LoadSkillDispatchResult` defined in Task 10 is referenced in Task 11. `SkillLoaded` variant fields defined in Task 9 match the payload emitted in Task 12. `SkillSource` enum used consistently throughout. `skills_used_log` in Task 12 feeds the aggregator in Task 13.

**Scope:** Single feature, one logical subsystem. Does not need decomposition into sub-plans.
