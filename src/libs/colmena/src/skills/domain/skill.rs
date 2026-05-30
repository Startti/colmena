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
    /// When set, this skill is a layer-1 node-type guide auto-folded into the
    /// tool context block for any tool whose node_type matches. Layer-1 guides
    /// are excluded from the `load_skill` catalog.
    #[serde(default)]
    pub node_type: Option<String>,
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
            node_type: None,
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
