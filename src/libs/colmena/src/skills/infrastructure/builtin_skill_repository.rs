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
#[derive(Debug)]
pub struct BuiltinSkillRepository {
    skills: HashMap<String, BuiltinEntry>,
}

#[derive(Debug)]
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

            let content =
                skill_md
                    .contents_utf8()
                    .ok_or_else(|| SkillError::InvalidFrontmatter {
                        path: format!("builtin:{}/SKILL.md", name),
                        reason: "file is not valid UTF-8".to_string(),
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
                let file =
                    dir.get_file(&ref_path)
                        .ok_or_else(|| SkillError::ReferenceFileMissing {
                            skill: name.clone(),
                            path: format!("builtin:{}", ref_path),
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

    /// Names of every SKILL.md compiled into the binary via `include_dir!`.
    /// Used by callers that want to know which names are available before
    /// constructing a repo — e.g. to auto-derive the load list from
    /// `tool_configurations.*.skills`.
    ///
    /// Excludes the reserved `_placeholder` entry.
    pub fn available_builtin_names() -> Vec<String> {
        Self::all_available_names()
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

        // Split the path into segments (e.g. "fw/django" → ["fw", "django"]).
        let segments: Vec<&str> = reference_name
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        if segments.is_empty() {
            return Err(SkillError::InvalidReferencePath {
                path: reference_name.to_string(),
            });
        }

        // Walk the declared reference tree to validate every segment.
        // Built-in skills store flat references (no recursive nesting at load time),
        // so nested paths will correctly fail validation here.
        let mut current_level: &[SkillReferenceMeta] = &entry.references;
        for seg in &segments {
            match current_level.iter().find(|r| r.name == *seg) {
                Some(meta) => {
                    current_level = &meta.references;
                }
                None => {
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
            }
        }

        // All segments declared. Look up the LEAF body (final segment) by its name.
        let leaf = segments.last().unwrap();
        let body = entry.reference_bodies.get(*leaf).ok_or_else(|| {
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

    #[test]
    fn available_builtin_names_includes_authored_skills() {
        let names = BuiltinSkillRepository::available_builtin_names();
        assert!(
            names.contains(&"sales-analysis".to_string()),
            "expected sales-analysis in: {:?}",
            names
        );
        assert!(
            names.contains(&"expense-analysis".to_string()),
            "expected expense-analysis in: {:?}",
            names
        );
        assert!(
            names.contains(&"python-expert".to_string()),
            "expected python-expert in: {:?}",
            names
        );
        assert!(
            names.contains(&"crdt-doc-formulas".to_string()),
            "expected crdt-doc-formulas in: {:?}",
            names
        );
        assert!(
            !names.contains(&"_placeholder".to_string()),
            "_placeholder must not appear in available_builtin_names"
        );
    }

    #[tokio::test]
    async fn crdt_doc_formulas_is_loadable() {
        let repo = BuiltinSkillRepository::new(&["crdt-doc-formulas".to_string()]).unwrap();
        let skill = repo.load_skill("crdt-doc-formulas").await.unwrap();
        assert_eq!(skill.name, "crdt-doc-formulas");
        assert_eq!(skill.references.len(), 3);
        let ref_names: Vec<&str> = skill.references.iter().map(|r| r.name.as_str()).collect();
        assert!(ref_names.contains(&"write-formula"));
        assert!(ref_names.contains(&"read-with-formulas"));
        assert!(ref_names.contains(&"needs-browser-fallback"));

        // Each reference must be loadable
        for r in &[
            "write-formula",
            "read-with-formulas",
            "needs-browser-fallback",
        ] {
            let reference = repo.load_reference("crdt-doc-formulas", r).await.unwrap();
            assert!(!reference.body.is_empty(), "{} body must not be empty", r);
        }
    }

    #[tokio::test]
    async fn python_expert_is_loadable() {
        let repo = BuiltinSkillRepository::new(&["python-expert".to_string()]).unwrap();
        let skill = repo.load_skill("python-expert").await.unwrap();
        assert_eq!(skill.name, "python-expert");
        assert!(skill.body.contains("Typing"));
        assert_eq!(skill.references.len(), 1);
    }

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

    #[tokio::test]
    async fn sql_query_best_practices_is_loadable() {
        let repo = BuiltinSkillRepository::new(&["sql-query-best-practices".to_string()]).unwrap();
        let skill = repo.load_skill("sql-query-best-practices").await.unwrap();
        assert_eq!(skill.name, "sql-query-best-practices");
        assert!(
            skill.body.contains("Quick rules"),
            "body should contain the quick-rules section"
        );
        // Six references: multi_statement, bulk_insert, select_after_mutation,
        // anti_patterns, schema_discovery, error_recovery.
        assert_eq!(
            skill.references.len(),
            6,
            "expected 6 references, got {}",
            skill.references.len()
        );
        let names: Vec<String> = skill.references.iter().map(|r| r.name.clone()).collect();
        for expected in &[
            "multi_statement",
            "bulk_insert",
            "select_after_mutation",
            "anti_patterns",
            "schema_discovery",
            "error_recovery",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "missing reference '{}', got {:?}",
                expected,
                names
            );
        }
    }

    /// Verifies the `gsheets-presentable-output` skill compiles into the
    /// binary, parses its frontmatter, and exposes the five expected
    /// references (recipe, palettes, number_formats, multi_op_template,
    /// layout). Catches regressions where a filename/frontmatter name
    /// mismatch or a dropped file would break the skill at runtime.
    #[tokio::test]
    async fn gsheets_presentable_output_is_loadable() {
        let repo =
            BuiltinSkillRepository::new(&["gsheets-presentable-output".to_string()]).unwrap();
        let skill = repo.load_skill("gsheets-presentable-output").await.unwrap();
        assert_eq!(skill.name, "gsheets-presentable-output");
        assert!(
            skill.body.contains("Quick rules"),
            "body should contain the quick-rules section"
        );
        assert_eq!(
            skill.references.len(),
            5,
            "expected 5 references, got {}",
            skill.references.len()
        );
        let names: Vec<String> = skill.references.iter().map(|r| r.name.clone()).collect();
        for expected in &[
            "recipe",
            "palettes",
            "number_formats",
            "multi_op_template",
            "layout",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "missing reference '{}', got {:?}",
                expected,
                names
            );
        }
        for r in &skill.references {
            let reference = repo
                .load_reference("gsheets-presentable-output", &r.name)
                .await
                .unwrap();
            assert!(
                !reference.body.trim().is_empty(),
                "reference {} body is empty",
                r.name
            );
        }
    }

    /// Verifies the `gdocs-surgical-edits` skill (shipped 2026-06-10
    /// alongside the apply_edits ConfirmManyMatches guard) compiles
    /// into the binary via `include_dir!`, parses its frontmatter, and
    /// exposes the five expected references. Catches regressions where
    /// a future YAML edit breaks the frontmatter or someone drops a
    /// reference file.
    #[tokio::test]
    async fn gdocs_surgical_edits_is_loadable() {
        let repo = BuiltinSkillRepository::new(&["gdocs-surgical-edits".to_string()]).unwrap();
        let skill = repo.load_skill("gdocs-surgical-edits").await.unwrap();
        assert_eq!(skill.name, "gdocs-surgical-edits");
        assert!(
            skill.body.contains("Quick rules"),
            "body should contain the quick-rules section"
        );
        assert_eq!(
            skill.references.len(),
            5,
            "expected 5 references, got {}",
            skill.references.len()
        );
        let names: Vec<String> = skill.references.iter().map(|r| r.name.clone()).collect();
        for expected in &[
            "replace_text_scoping",
            "apply_edits_patterns",
            "error_recovery",
            "style_changes_pattern",
            "before_after_examples",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "missing reference '{}', got {:?}",
                expected,
                names
            );
        }
    }

    /// EVERY built-in skill must parse + load. Enumerates the whole compiled-in
    /// tree and loads each one — a malformed frontmatter (e.g. an unescaped
    /// `': '` in a description) or a missing reference file makes this fail at
    /// CI, instead of only at runtime when an agent auto-enrolls the skill and
    /// the node errors with `loading builtin skills: invalid frontmatter`.
    #[tokio::test]
    async fn all_builtin_skills_parse_and_load() {
        let names: Vec<String> = BUILTIN_SKILLS_DIR
            .dirs()
            .filter_map(|d| {
                d.path()
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(String::from)
            })
            .filter(|n| n != "_placeholder")
            .collect();
        assert!(
            !names.is_empty(),
            "expected built-in skills to be compiled in"
        );
        let repo =
            BuiltinSkillRepository::new(&names).expect("every built-in skill must parse cleanly");
        for n in &names {
            repo.load_skill(n)
                .await
                .unwrap_or_else(|e| panic!("built-in skill '{n}' failed to load: {e:?}"));
        }
    }
}
