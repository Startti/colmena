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
