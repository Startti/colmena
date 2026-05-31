use serde::{Deserialize, Serialize};

/// Metadata about a reference file attached to a skill.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillReferenceMeta {
    pub name: String,
    pub description: String,
    /// Nested sub-references declared in the reference file's own frontmatter.
    /// Empty when this is a leaf or when the reference file has no frontmatter.
    #[serde(default)]
    pub references: Vec<SkillReferenceMeta>,
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
                references: vec![],
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

    #[test]
    fn skill_reference_meta_supports_nested_references() {
        let meta = SkillReferenceMeta {
            name: "frameworks".into(),
            description: "Web frameworks".into(),
            references: vec![SkillReferenceMeta {
                name: "django".into(),
                description: "Django specifics".into(),
                references: vec![],
            }],
        };
        let s = serde_json::to_string(&meta).unwrap();
        let back: SkillReferenceMeta = serde_json::from_str(&s).unwrap();
        assert_eq!(back.references.len(), 1);
        assert_eq!(back.references[0].name, "django");
    }

    #[test]
    fn skill_reference_meta_defaults_empty_references_when_missing() {
        // Backward-compat: pre-recursion SKILL.md files had references without the nested field.
        let yaml = r#"{"name":"foo","description":"bar"}"#;
        let meta: SkillReferenceMeta = serde_json::from_str(yaml).unwrap();
        assert!(meta.references.is_empty());
    }
}
