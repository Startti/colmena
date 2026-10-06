//! Where finished Parquet parts go. The writer hands each part over as one
//! buffer with a known length; the host decides what storage that is. Dark
//! behind `COLMENA_LARGE_TABULAR`.

use crate::tabular_prepare::manifest::{parse_part_path, MANIFEST_PATH};
use async_trait::async_trait;
use bytes::Bytes;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
#[error("part sink: {0}")]
pub struct SinkError(pub String);

/// Receives finished parts and the manifest, by path relative to the prepared
/// root of the source (`t<table>/part-NNNNN.parquet`, `manifest.json`).
#[async_trait]
pub trait PartSink: Send + Sync {
    /// Stores `data` at `path`. Writing the same path again replaces it, which
    /// is what lets a restarted conversion overwrite its earlier parts.
    async fn put(&self, path: &str, data: Bytes) -> Result<(), SinkError>;
}

/// Writes parts under a local directory. Used by tests and the manual bench;
/// the host's storage adapter replaces it in production.
///
/// Only the two path shapes the converter produces are accepted, and a part is
/// written to a temporary file and renamed, so a reader never sees a partial
/// part under a final name.
pub struct DirSink {
    root: PathBuf,
}

impl DirSink {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

#[async_trait]
impl PartSink for DirSink {
    async fn put(&self, path: &str, data: Bytes) -> Result<(), SinkError> {
        if parse_part_path(path).is_none() && path != MANIFEST_PATH {
            return Err(SinkError(format!("refused path {path:?}")));
        }
        let target = self.root.join(path);
        let tmp = target.with_extension("tmp");
        let io = |e: std::io::Error| SinkError(e.to_string());
        if let Some(dir) = target.parent() {
            tokio::fs::create_dir_all(dir).await.map_err(io)?;
        }
        let result = async {
            tokio::fs::write(&tmp, &data).await?;
            tokio::fs::rename(&tmp, &target).await
        }
        .await;
        if let Err(e) = result {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(io(e));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dir_sink_writes_a_part_and_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let sink = DirSink::new(dir.path());
        sink.put("t0/part-00000.parquet", Bytes::from_static(b"one"))
            .await
            .unwrap();
        sink.put("t0/part-00000.parquet", Bytes::from_static(b"two!"))
            .await
            .unwrap();
        sink.put(MANIFEST_PATH, Bytes::from_static(b"{}"))
            .await
            .unwrap();
        let read = |p: &str| std::fs::read(dir.path().join(p)).unwrap();
        assert_eq!(read("t0/part-00000.parquet"), b"two!");
        assert_eq!(read("manifest.json"), b"{}");
        assert!(!dir.path().join("t0/part-00000.tmp").exists());
    }

    #[tokio::test]
    async fn dir_sink_refuses_paths_the_converter_never_produces() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let sink = DirSink::new(&root);
        for bad in [
            "../evil.parquet",
            "t0/../../evil",
            "/etc/passwd",
            "t0/part-0.parquet",
            "other.json",
            "",
        ] {
            assert!(
                sink.put(bad, Bytes::from_static(b"x")).await.is_err(),
                "{bad}"
            );
        }
        // Nothing was created, inside the root or beside it.
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn dir_sink_failure_leaves_no_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        // A directory sits where the part would be renamed to.
        std::fs::create_dir_all(dir.path().join("t0/part-00000.parquet")).unwrap();
        let sink = DirSink::new(dir.path());
        let err = sink
            .put("t0/part-00000.parquet", Bytes::from_static(b"data"))
            .await;
        assert!(err.is_err());
        let names: Vec<_> = std::fs::read_dir(dir.path().join("t0"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["part-00000.parquet"],
            "temporary file left behind"
        );
    }
}
