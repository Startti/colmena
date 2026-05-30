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
    node_type: Option<String>,
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
                    node_type: parsed.node_type,
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
                node_type: entry.node_type.clone(),
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
            node_type: entry.node_type.clone(),
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
    async fn catalog_entry_carries_node_type_when_present() {
        let repo = BuiltinSkillRepository::new(&["python-expert".to_string()]).unwrap();
        let entries = repo.list_available();
        // At least one existing built-in must be node_type-less:
        assert!(entries.iter().any(|e| e.node_type.is_none()));
    }

    #[tokio::test]
    async fn sql_query_guide_is_indexed_as_node_type_guide() {
        let repo = BuiltinSkillRepository::new(&["sql_query-guide".to_string()]).unwrap();
        let entry = repo.find_by_node_type("sql_query").expect("sql_query guide present");
        assert_eq!(entry.name, "sql_query-guide");
        assert_eq!(entry.node_type.as_deref(), Some("sql_query"));
    }

    #[tokio::test]
    async fn sales_analysis_is_layer_2_domain_skill() {
        let repo = BuiltinSkillRepository::new(&["sales-analysis".to_string()]).unwrap();
        let entries = repo.list_available();
        let sales = entries
            .iter()
            .find(|e| e.name == "sales-analysis")
            .expect("sales-analysis loaded");
        assert!(
            sales.node_type.is_none(),
            "sales-analysis must be layer-2 (no node_type)"
        );
        assert!(sales.description.contains("sales data"));
    }

    #[tokio::test]
    async fn expense_analysis_is_layer_2_domain_skill() {
        let repo = BuiltinSkillRepository::new(&["expense-analysis".to_string()]).unwrap();
        let entries = repo.list_available();
        let expense = entries
            .iter()
            .find(|e| e.name == "expense-analysis")
            .expect("expense-analysis loaded");
        assert!(
            expense.node_type.is_none(),
            "expense-analysis must be layer-2 (no node_type)"
        );
        assert!(expense.description.contains("expenses"));
    }

    #[tokio::test]
    async fn both_domain_skills_can_coexist() {
        let repo = BuiltinSkillRepository::new(&[
            "sales-analysis".to_string(),
            "expense-analysis".to_string(),
        ])
        .unwrap();
        let listed: Vec<String> = repo.list_available().into_iter().map(|e| e.name).collect();
        assert!(listed.contains(&"sales-analysis".to_string()));
        assert!(listed.contains(&"expense-analysis".to_string()));
    }
}
