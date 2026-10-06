//! The storage side of preparing a CSV: where the converter's parts and
//! manifest go, on the host's storage port.
//!
//! Layout. Everything a source produces lives under the root the adapter
//! reports with `derived_root` and nowhere else: `<root>/manifest.json` and
//! `<root>/t<n>/part-NNNNN.parquet`. The key of an object is built here, before
//! the put, as `<root>/<relative path>`; the adapter must answer with exactly
//! that key. A different key is refused: the cleanup pass can only delete keys
//! it can contain in the root, so an object stored anywhere else could never be
//! removed. An adapter that reports no root cannot be used at all.
//!
//! Errors never echo a storage key, a URL or a cell: the text of a storage
//! error is dropped and replaced by a fixed sentence.

use crate::storage::domain::{OutputStorageRepository, StorePlacement, StoreStreamRequest};
use crate::tabular_prepare::manifest::{parse_part_path, MANIFEST_PATH};
use crate::tabular_prepare::part_sink::{PartSink, SinkError};
use async_trait::async_trait;
use bytes::Bytes;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Why a preparation could not even start.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PrepareStartError {
    #[error(
        "the storage adapter reports no derived root for this source, so the prepared \
         objects could not be contained or cleaned up"
    )]
    NoDerivedRoot,
}

/// Writes parts and the manifest of one source through
/// `OutputStorageRepository::store_stream`, under the derived root.
pub struct StoragePartSink {
    storage: Arc<dyn OutputStorageRepository>,
    source_key: String,
    root: String,
    /// Bytes of the last write of each path (a restart overwrites a path).
    sizes: Mutex<BTreeMap<String, u64>>,
}

impl StoragePartSink {
    pub fn new(
        storage: Arc<dyn OutputStorageRepository>,
        source_key: &str,
    ) -> Result<Self, PrepareStartError> {
        let root = storage
            .derived_root(source_key)
            .map(|r| r.trim_end_matches('/').to_string())
            .filter(|r| !r.is_empty())
            .ok_or(PrepareStartError::NoDerivedRoot)?;
        Ok(Self {
            storage,
            source_key: source_key.to_string(),
            root,
            sizes: Mutex::new(BTreeMap::new()),
        })
    }

    /// The storage key of a path relative to the prepared root.
    pub fn key_of(&self, relative: &str) -> String {
        format!("{}/{}", self.root, relative)
    }

    /// Bytes held by the given relative paths, as last written.
    pub fn bytes_of<'a>(&self, paths: impl IntoIterator<Item = &'a String>) -> u64 {
        let sizes = self.sizes.lock().unwrap_or_else(|p| p.into_inner());
        paths
            .into_iter()
            .map(|p| sizes.get(p).copied().unwrap_or(0))
            .sum()
    }
}

#[async_trait]
impl PartSink for StoragePartSink {
    async fn put(&self, path: &str, data: Bytes) -> Result<(), SinkError> {
        let mime = if path == MANIFEST_PATH {
            "application/json"
        } else if parse_part_path(path).is_some() {
            "application/vnd.apache.parquet"
        } else {
            return Err(SinkError(
                "refused a path outside the prepared layout".into(),
            ));
        };
        let expected = self.key_of(path);
        let len = data.len() as u64;
        let request = StoreStreamRequest {
            stream: Box::pin(futures::stream::once(async move { Ok(data) })),
            size_hint: Some(len),
            mime_type: mime.to_string(),
            filename: path.rsplit('/').next().unwrap_or(path).to_string(),
            session_id: None,
            agent_session_id: None,
            placement: StorePlacement::DerivedFrom {
                source_storage_key: self.source_key.clone(),
                relative_path: path.to_string(),
            },
        };
        let stored = self
            .storage
            .store_stream(request)
            .await
            .map_err(|_| SinkError("the storage refused to store a prepared object".into()))?;
        if stored.storage_key != expected {
            return Err(SinkError(
                "the storage adapter placed a prepared object outside the prepared layout".into(),
            ));
        }
        self.sizes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(path.to_string(), len);
        Ok(())
    }
}

/// A storage that honours the placement, for tests: the keys under the derived
/// root are `<parent of the source>/prepared/<relative path>`.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use crate::storage::domain::{
        StorageError, StoreRequest, StoredBytes, StoredOutput, StoredStream,
    };
    use futures::StreamExt;

    #[derive(Default)]
    pub struct PlacedStorage {
        pub objects: Mutex<BTreeMap<String, Bytes>>,
        pub deleted: Mutex<Vec<String>>,
        pub no_root: Mutex<bool>,
        /// The adapter reports an empty root.
        pub empty_root: Mutex<bool>,
        /// Stores answer this suffix appended to the right key (a misplacing adapter).
        pub misplace: Mutex<bool>,
        /// The nth store (0-based) and every later one fail.
        pub fail_stores_from: Mutex<Option<usize>>,
        pub stores: Mutex<usize>,
    }

    impl PlacedStorage {
        pub fn with_source(key: &str, bytes: Vec<u8>) -> Arc<Self> {
            let s = Self::default();
            s.objects
                .lock()
                .unwrap()
                .insert(key.to_string(), Bytes::from(bytes));
            Arc::new(s)
        }

        pub fn keys(&self) -> Vec<String> {
            self.objects.lock().unwrap().keys().cloned().collect()
        }
    }

    pub fn root_of(source: &str) -> String {
        let (parent, _) = source.rsplit_once('/').unwrap();
        format!("{parent}/prepared")
    }

    #[async_trait]
    impl OutputStorageRepository for PlacedStorage {
        async fn store(&self, _req: StoreRequest) -> Result<StoredOutput, StorageError> {
            Err(StorageError::InvalidInput("not used".into()))
        }
        async fn read(&self, key: &str) -> Result<StoredBytes, StorageError> {
            let bytes = self
                .objects
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .ok_or_else(|| StorageError::InvalidInput("missing".into()))?;
            Ok(StoredBytes {
                bytes: bytes.to_vec(),
                mime_type: String::new(),
                filename: String::new(),
            })
        }
        async fn read_stream(&self, key: &str) -> Result<StoredStream, StorageError> {
            let bytes = self
                .objects
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .ok_or_else(|| StorageError::InvalidInput("missing".into()))?;
            let size = bytes.len() as u64;
            // Chunks of 1 KiB, so a read crosses chunk boundaries.
            let chunks: Vec<Result<Bytes, StorageError>> = bytes
                .chunks(1024)
                .map(|c| Ok(Bytes::copy_from_slice(c)))
                .collect();
            Ok(StoredStream {
                stream: Box::pin(futures::stream::iter(chunks)),
                size_bytes: size,
                mime_type: String::new(),
                filename: String::new(),
            })
        }
        async fn store_stream(
            &self,
            mut req: StoreStreamRequest,
        ) -> Result<StoredOutput, StorageError> {
            let n = {
                let mut s = self.stores.lock().unwrap();
                *s += 1;
                *s - 1
            };
            if self
                .fail_stores_from
                .lock()
                .unwrap()
                .is_some_and(|f| n >= f)
            {
                return Err(StorageError::UploadFailed(
                    "secret-key-and-url-in-the-adapter-text".into(),
                ));
            }
            let mut all = Vec::new();
            while let Some(chunk) = req.stream.next().await {
                all.extend_from_slice(&chunk?);
            }
            let StorePlacement::DerivedFrom {
                source_storage_key,
                relative_path,
            } = &req.placement
            else {
                return Err(StorageError::InvalidInput("generated placement".into()));
            };
            let mut key = format!("{}/{}", root_of(source_storage_key), relative_path);
            if *self.misplace.lock().unwrap() {
                key = format!("elsewhere/{relative_path}");
            }
            let size = all.len() as u64;
            self.objects
                .lock()
                .unwrap()
                .insert(key.clone(), Bytes::from(all));
            Ok(StoredOutput {
                storage_key: key,
                read_url: String::new(),
                mime_type: req.mime_type,
                filename: req.filename,
                size_bytes: size,
            })
        }
        fn derived_root(&self, source: &str) -> Option<String> {
            if *self.no_root.lock().unwrap() {
                None
            } else if *self.empty_root.lock().unwrap() {
                Some("/".to_string())
            } else {
                Some(root_of(source))
            }
        }
        async fn delete(&self, key: &str) -> Result<(), StorageError> {
            self.objects.lock().unwrap().remove(key);
            self.deleted.lock().unwrap().push(key.to_string());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{root_of, PlacedStorage};
    use super::*;
    use crate::storage::domain::StorageError;
    use crate::storage::infrastructure::LocalCacheStorageAdapter;
    use crate::tabular_prepare::manifest::{part_path, MAX_PARTS, MAX_TABLES};

    const SOURCE: &str = "chat-attachments/u/s/sales.csv";

    #[tokio::test]
    async fn parts_and_manifest_land_under_the_derived_root_at_the_expected_keys() {
        let storage = PlacedStorage::with_source(SOURCE, b"a\n1\n".to_vec());
        let sink = StoragePartSink::new(storage.clone(), SOURCE).unwrap();
        sink.put("t0/part-00000.parquet", Bytes::from_static(b"PAR1"))
            .await
            .unwrap();
        sink.put("manifest.json", Bytes::from_static(b"{}"))
            .await
            .unwrap();
        let root = root_of(SOURCE);
        let mut want = vec![
            SOURCE.to_string(),
            format!("{root}/manifest.json"),
            format!("{root}/t0/part-00000.parquet"),
        ];
        want.sort();
        assert_eq!(storage.keys(), want);
        assert_eq!(
            sink.key_of("manifest.json"),
            format!("{root}/manifest.json")
        );
        // Sizes are those of the last write of each path.
        sink.put("t0/part-00000.parquet", Bytes::from_static(b"PAR1PAR1"))
            .await
            .unwrap();
        let part = "t0/part-00000.parquet".to_string();
        assert_eq!(sink.bytes_of([&part]), 8);
    }

    #[tokio::test]
    async fn every_path_the_converter_can_produce_has_the_shape_the_signing_route_accepts() {
        // The route takes exactly `manifest.json` and `t<n>/part-NNNNN.parquet`
        // (n up to four digits, five digits for the part).
        let ok = |p: &str| {
            let rest = p.strip_prefix('t').and_then(|r| r.split_once('/'));
            match rest {
                Some((n, file)) => {
                    (1..=4).contains(&n.len())
                        && n.bytes().all(|b| b.is_ascii_digit())
                        && file.len() == "part-00000.parquet".len()
                        && file.starts_with("part-")
                        && file.ends_with(".parquet")
                        && file[5..10].bytes().all(|b| b.is_ascii_digit())
                }
                None => false,
            }
        };
        for t in [0, 1, 9, 10, 99, 100, MAX_TABLES - 1] {
            for p in [0, 1, 99_999, MAX_PARTS - 1] {
                let path = part_path(t, p).unwrap();
                assert!(ok(&path), "{path}");
                assert!(parse_part_path(&path).is_some(), "{path}");
            }
        }
        assert!(part_path(MAX_TABLES, 0).is_err() && part_path(0, MAX_PARTS).is_err());
        assert_eq!(MANIFEST_PATH, "manifest.json");
    }

    #[tokio::test]
    async fn a_path_outside_the_layout_is_refused_before_any_store() {
        let storage = PlacedStorage::with_source(SOURCE, b"a\n".to_vec());
        let sink = StoragePartSink::new(storage.clone(), SOURCE).unwrap();
        for bad in [
            "../x",
            "t0/../manifest.json",
            "/manifest.json",
            "t00/part-00000.parquet",
            "notes.txt",
        ] {
            assert!(
                sink.put(bad, Bytes::from_static(b"x")).await.is_err(),
                "{bad}"
            );
        }
        assert_eq!(*storage.stores.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn an_adapter_with_no_derived_root_cannot_be_used() {
        let storage = PlacedStorage::with_source(SOURCE, b"a\n".to_vec());
        *storage.no_root.lock().unwrap() = true;
        assert_eq!(
            StoragePartSink::new(storage, SOURCE).err(),
            Some(PrepareStartError::NoDerivedRoot)
        );
        // A root that is empty is no root: the keys would be at the top.
        let storage = PlacedStorage::with_source(SOURCE, b"a\n".to_vec());
        *storage.empty_root.lock().unwrap() = true;
        assert!(StoragePartSink::new(storage, SOURCE).is_err());
        // The default adapter of the crate reports none either.
        assert!(StoragePartSink::new(Arc::new(LocalCacheStorageAdapter::new()), SOURCE).is_err());
    }

    #[tokio::test]
    async fn a_key_the_adapter_chose_itself_is_refused() {
        let storage = PlacedStorage::with_source(SOURCE, b"a\n".to_vec());
        *storage.misplace.lock().unwrap() = true;
        let sink = StoragePartSink::new(storage.clone(), SOURCE).unwrap();
        let err = sink
            .put("t0/part-00000.parquet", Bytes::from_static(b"x"))
            .await
            .unwrap_err();
        assert!(err.0.contains("outside the prepared layout"), "{err}");
        assert_eq!(sink.bytes_of([&"t0/part-00000.parquet".to_string()]), 0);
    }

    /// The crate's own adapter ignores the placement and invents a key; given a
    /// root, it must be refused rather than trusted.
    struct RootedLocal(LocalCacheStorageAdapter);

    #[async_trait]
    impl OutputStorageRepository for RootedLocal {
        async fn store(
            &self,
            r: crate::storage::domain::StoreRequest,
        ) -> Result<crate::storage::domain::StoredOutput, StorageError> {
            self.0.store(r).await
        }
        async fn read(&self, k: &str) -> Result<crate::storage::domain::StoredBytes, StorageError> {
            self.0.read(k).await
        }
        async fn read_stream(
            &self,
            k: &str,
        ) -> Result<crate::storage::domain::StoredStream, StorageError> {
            self.0.read_stream(k).await
        }
        async fn delete(&self, k: &str) -> Result<(), StorageError> {
            self.0.delete(k).await
        }
        fn derived_root(&self, s: &str) -> Option<String> {
            Some(root_of(s))
        }
    }

    #[tokio::test]
    async fn the_default_adapter_is_refused_for_ignoring_the_placement() {
        let local = Arc::new(RootedLocal(LocalCacheStorageAdapter::new()));
        let sink = StoragePartSink::new(local, SOURCE).unwrap();
        let err = sink
            .put("t0/part-00000.parquet", Bytes::from_static(b"x"))
            .await
            .unwrap_err();
        assert!(err.0.contains("outside the prepared layout"), "{err}");
    }

    #[tokio::test]
    async fn a_storage_failure_never_echoes_the_adapter_text_or_a_key() {
        let storage = PlacedStorage::with_source(SOURCE, b"a\n".to_vec());
        *storage.fail_stores_from.lock().unwrap() = Some(0);
        let sink = StoragePartSink::new(storage, SOURCE).unwrap();
        let err = sink
            .put("t0/part-00000.parquet", Bytes::from_static(b"x"))
            .await
            .unwrap_err();
        let text = err.to_string();
        assert!(
            !text.contains("secret") && !text.contains("chat-attachments"),
            "{text}"
        );
    }
}
