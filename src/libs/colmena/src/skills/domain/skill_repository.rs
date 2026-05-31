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
                references: vec![],
            }],
            source: SkillSource::Builtin,
        };
    }
}
