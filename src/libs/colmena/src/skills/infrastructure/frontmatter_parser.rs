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
    let after_first = content
        .strip_prefix("---\r\n")
        .unwrap_or_else(|| &content[4..]);

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
