use super::ids::{ArtifactId, SessionId, VersionId};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error, Serialize)]
pub enum StorageError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("precondition failed (generation mismatch): {0}")]
    PreconditionFailed(String),

    #[error("transient error: {0}")]
    Transient(String),

    #[error("backend error: {0}")]
    Backend(String),
}

#[derive(Debug, Error, Serialize)]
pub enum IndexError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("backend error: {0}")]
    Backend(String),
}

#[derive(Debug, Error, Serialize)]
pub enum RenderError {
    #[error("render failed: {0}")]
    Failed(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct ConflictDetail {
    pub incoming_op: serde_json::Value,
    pub conflicting_with: serde_json::Value,
    pub in_version: VersionId,
    pub reason: String,
}

#[derive(Debug, Error, Serialize)]
pub enum DocumentError {
    #[error("artifact not found: {0}")]
    ArtifactNotFound(ArtifactId),

    #[error("version not found: {artifact}/{version}")]
    VersionNotFound {
        artifact: ArtifactId,
        version: VersionId,
    },

    #[error("version conflict: base {base}, current {current}")]
    VersionConflict {
        artifact: ArtifactId,
        base: VersionId,
        current: VersionId,
        conflicts: Vec<ConflictDetail>,
    },

    #[error("IR validation failed at {path}: {reason}")]
    IRValidationFailed { path: String, reason: String },

    #[error("invalid patch op: {reason}")]
    InvalidPatchOp {
        reason: String,
        op: serde_json::Value,
    },

    #[error("render failed: {0}")]
    RenderFailed(String),

    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error(transparent)]
    Index(#[from] IndexError),

    #[error("session isolation violation: artifact {0} not in session {1}")]
    SessionIsolationViolation(ArtifactId, SessionId),
}

impl From<RenderError> for DocumentError {
    fn from(e: RenderError) -> Self {
        DocumentError::RenderFailed(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display() {
        let e = DocumentError::ArtifactNotFound(ArtifactId::new("art_x"));
        assert_eq!(e.to_string(), "artifact not found: art_x");
    }

    #[test]
    fn storage_into_document_error() {
        let s = StorageError::NotFound("x".into());
        let d: DocumentError = s.into();
        assert!(matches!(d, DocumentError::Storage(_)));
    }
}
