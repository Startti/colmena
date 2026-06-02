//! `ArtifactId` — a stable opaque identifier for a CRDT document.
//!
//! ULID-based ("art_" prefix + 26-char ULID). String-serialisable; safe to
//! send to clients and use in URL paths.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArtifactId(String);

impl ArtifactId {
    /// Generate a new id from a fresh ULID.
    pub fn new() -> Self {
        Self(format!("art_{}", ulid::Ulid::new().to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ArtifactId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ArtifactId {
    type Err = ArtifactIdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.starts_with("art_") {
            return Err(ArtifactIdError::BadPrefix);
        }
        if s.len() != 4 + 26 {
            return Err(ArtifactIdError::BadLength);
        }
        Ok(Self(s.to_string()))
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ArtifactIdError {
    #[error("artifact id must start with `art_`")]
    BadPrefix,
    #[error("artifact id must be `art_` + 26 chars")]
    BadLength,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_generates_art_prefix_and_correct_length() {
        let id = ArtifactId::new();
        assert!(id.as_str().starts_with("art_"));
        assert_eq!(id.as_str().len(), 4 + 26);
    }

    #[test]
    fn round_trip_via_from_str() {
        let original = ArtifactId::new();
        let parsed = ArtifactId::from_str(original.as_str()).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn rejects_missing_prefix() {
        assert_eq!(
            ArtifactId::from_str("01H0123456789ABCDEFGHJKMNP"),
            Err(ArtifactIdError::BadPrefix),
        );
    }

    #[test]
    fn rejects_wrong_length() {
        assert_eq!(
            ArtifactId::from_str("art_short"),
            Err(ArtifactIdError::BadLength),
        );
    }
}
