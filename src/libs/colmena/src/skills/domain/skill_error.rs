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
    FileTooLarge { path: String, size: u64, limit: u64 },

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

    #[error("path '{0}' has no SKILL.md and contains no skill subdirectories")]
    EmptyRoot(String),

    #[error("too many skills active: {count} exceeds limit of {limit}")]
    TooManySkills { count: usize, limit: usize },

    #[error("skill name collision: '{name}' is defined more than once")]
    SkillNameCollision { name: String },

    #[error("duplicate node_type guide: node_type '{node_type}' is claimed by skills '{first}' and '{second}'; only one guide per node_type is allowed")]
    DuplicateNodeTypeGuide {
        node_type: String,
        first: String,
        second: String,
    },

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

    #[test]
    fn empty_root_display_contains_path() {
        let err = SkillError::EmptyRoot("/tmp/skills-cache".to_string());
        let s = err.to_string();
        assert!(s.contains("/tmp/skills-cache"));
        assert!(s.contains("no SKILL.md"));
        assert!(s.contains("subdirectories"));
    }
}
