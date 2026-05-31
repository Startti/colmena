use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArtifactId(pub String);

impl ArtifactId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VersionId(pub String);

impl VersionId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn initial() -> Self {
        Self("v1".to_string())
    }
    pub fn next(&self) -> Self {
        let n: u64 = self.0.trim_start_matches('v').parse().unwrap_or(0);
        Self(format!("v{}", n + 1))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn number(&self) -> Option<u64> {
        self.0.trim_start_matches('v').parse().ok()
    }
}

impl fmt::Display for VersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetId(pub String);

impl AssetId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    Excel,
    Word,
    Html,
}

impl ArtifactKind {
    pub fn extension(&self) -> &'static str {
        match self {
            ArtifactKind::Excel => "xlsx",
            ArtifactKind::Word => "docx",
            ArtifactKind::Html => "html",
        }
    }
    pub fn mime(&self) -> &'static str {
        match self {
            ArtifactKind::Excel => {
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            }
            ArtifactKind::Word => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            }
            ArtifactKind::Html => "text/html; charset=utf-8",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_id_next_increments() {
        let v = VersionId::initial();
        assert_eq!(v.as_str(), "v1");
        assert_eq!(v.next().as_str(), "v2");
        assert_eq!(VersionId::new("v99").next().as_str(), "v100");
    }

    #[test]
    fn version_id_number_parses() {
        assert_eq!(VersionId::new("v7").number(), Some(7));
        assert_eq!(VersionId::new("vx").number(), None);
    }

    #[test]
    fn artifact_kind_extension() {
        assert_eq!(ArtifactKind::Excel.extension(), "xlsx");
        assert_eq!(ArtifactKind::Word.extension(), "docx");
    }

    #[test]
    fn artifact_kind_html_extension_and_mime() {
        assert_eq!(ArtifactKind::Html.extension(), "html");
        assert_eq!(ArtifactKind::Html.mime(), "text/html; charset=utf-8");
    }

    #[test]
    fn artifact_kind_html_serde_roundtrip() {
        let k = ArtifactKind::Html;
        let s = serde_json::to_string(&k).unwrap();
        assert_eq!(s, "\"html\"");
        let back: ArtifactKind = serde_json::from_str(&s).unwrap();
        assert_eq!(back, ArtifactKind::Html);
    }

    #[test]
    fn asset_id_construction_and_display() {
        let a = AssetId::new("asset_abc123");
        assert_eq!(a.as_str(), "asset_abc123");
        assert_eq!(a.to_string(), "asset_abc123");
    }

    #[test]
    fn asset_id_serde_roundtrip() {
        let a = AssetId::new("asset_xyz");
        let s = serde_json::to_string(&a).unwrap();
        let back: AssetId = serde_json::from_str(&s).unwrap();
        assert_eq!(a, back);
    }
}
