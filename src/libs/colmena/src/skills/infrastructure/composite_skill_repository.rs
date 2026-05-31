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

impl std::fmt::Debug for CompositeSkillRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeSkillRepository")
            .field("builtin_names", &self.builtin_names)
            .field("filesystem_names", &self.filesystem_names)
            .finish_non_exhaustive()
    }
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
        if let Some(name) = builtin_names.intersection(&filesystem_names).next() {
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
            self.builtin
                .load_reference(skill_name, reference_name)
                .await
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

    /// Run cross-repo validations that cannot be enforced by a single child:
    /// - At most one skill per node_type (across all repos).
    pub fn validate(&self) -> Result<(), SkillError> {
        let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for entry in self.list_available() {
            if let Some(nt) = entry.node_type.as_deref() {
                if let Some(prev) = seen.get(nt) {
                    return Err(SkillError::DuplicateNodeTypeGuide {
                        node_type: nt.to_string(),
                        first: prev.clone(),
                        second: entry.name.clone(),
                    });
                }
                seen.insert(nt.to_string(), entry.name.clone());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
                    node_type: None,
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
            node_type: None,
        }
    }

    fn entry_with_node_type(name: &str, source: SkillSource, node_type: &str) -> SkillCatalogEntry {
        SkillCatalogEntry {
            name: name.to_string(),
            description: format!("desc of {}", name),
            source,
            node_type: Some(node_type.to_string()),
        }
    }

    fn make_test_repo(skill_name: &str, node_type: &str) -> FakeRepo {
        FakeRepo {
            entries: vec![entry_with_node_type(
                skill_name,
                SkillSource::Builtin,
                node_type,
            )],
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
        assert!(matches!(
            err,
            SkillError::TooManySkills {
                count: 60,
                limit: 50
            }
        ));
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

    #[test]
    fn validate_returns_error_on_duplicate_node_type() {
        let a = make_test_repo("guide-a", "sql_query");
        let b = make_test_repo("guide-b", "sql_query");
        let composite = CompositeSkillRepository::new(Arc::new(a), Arc::new(b))
            .expect("construction should succeed");
        let err = composite.validate().expect_err("should fail on duplicates");
        assert!(matches!(err, SkillError::DuplicateNodeTypeGuide { .. }));
    }
}
