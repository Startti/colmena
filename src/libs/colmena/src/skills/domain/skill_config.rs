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
