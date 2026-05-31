use super::ids::{ArtifactId, ArtifactKind, SessionId, VersionId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactMeta {
    pub artifact_id: ArtifactId,
    pub kind: ArtifactKind,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub current_version: VersionId,
    pub retention_limit: u32,
    pub pin_initial: bool,
    pub schema_version: String,
    pub session_id: SessionId,
    pub label: String,
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
}

impl ArtifactMeta {
    pub fn initial(
        artifact_id: ArtifactId,
        kind: ArtifactKind,
        session_id: SessionId,
        label: String,
        retention_limit: u32,
    ) -> Self {
        let now = Utc::now();
        Self {
            artifact_id,
            kind,
            created_at: now,
            updated_at: now,
            current_version: VersionId::initial(),
            retention_limit,
            pin_initial: true,
            schema_version: super::ir::SCHEMA_VERSION.to_string(),
            session_id,
            label,
            tags: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchApplied {
    pub patch: serde_json::Value,
    pub applied_at: DateTime<Utc>,
    pub resulted_in: VersionId,
    pub summary: PatchSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PatchSummary {
    #[serde(default)]
    pub natural_language: Vec<String>,
    #[serde(default)]
    pub structured: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpOutcome {
    #[serde(default, skip_serializing_if = "AssignedIds::is_empty")]
    pub assigned_ids: AssignedIds,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssignedIds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub list_items: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slide: Option<String>,
}

impl AssignedIds {
    pub fn is_empty(&self) -> bool {
        self.block.is_none()
            && self.runs.is_empty()
            && self.list_items.is_empty()
            && self.rows.is_empty()
            && self.table.is_none()
            && self.sheet.is_none()
            && self.slide.is_none()
    }
}

pub struct VersionData {
    pub ir: serde_json::Value,
    pub rendered_binary: Vec<u8>,
    pub rendered_extension: &'static str,
    pub patch_applied: PatchApplied,
    pub blobs: Vec<(String, Vec<u8>)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactSummary {
    pub artifact_id: ArtifactId,
    pub session_id: SessionId,
    pub kind: ArtifactKind,
    pub label: Option<String>,
    pub current_version: VersionId,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_meta_sets_v1() {
        let m = ArtifactMeta::initial(
            ArtifactId::new("art_x"),
            ArtifactKind::Excel,
            SessionId::new("sess_1"),
            "Test".into(),
            20,
        );
        assert_eq!(m.current_version, VersionId::initial());
        assert!(m.pin_initial);
        assert_eq!(m.retention_limit, 20);
    }

    #[test]
    fn meta_roundtrip_json() {
        let m = ArtifactMeta::initial(
            ArtifactId::new("art_x"),
            ArtifactKind::Word,
            SessionId::new("sess_1"),
            "R".into(),
            5,
        );
        let j = serde_json::to_string(&m).unwrap();
        let back: ArtifactMeta = serde_json::from_str(&j).unwrap();
        assert_eq!(back.kind, ArtifactKind::Word);
    }
}
