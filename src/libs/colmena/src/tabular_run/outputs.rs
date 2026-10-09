//! Storing the outputs a call kept: each checked file is streamed from its open
//! descriptor to the host's storage, one chunk at a time, so no output is ever
//! held whole in memory. The size the reader checked is the size that must
//! arrive: a file that turns out shorter (truncated after the check) is refused
//! and stored nowhere.

use super::collect::OutFile;
use super::mounted::OutputSink;
use super::refusal::RunRefusal;
use crate::storage::domain::{
    OutputStorageRepository, StorageError, StorePlacement, StoreStreamRequest,
};
use async_trait::async_trait;
use bytes::Bytes;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncReadExt;

const CHUNK: usize = 1024 * 1024;

/// An output that reached storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emitted {
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    /// The storage handle, the way every generated attachment is named.
    pub storage_key: String,
}

/// A sink that stores each output with `store_stream` and remembers where.
pub struct StoreSink {
    storage: Arc<dyn OutputStorageRepository>,
    session_id: Option<String>,
    agent_session_id: Option<String>,
    stored: Mutex<Vec<Emitted>>,
}

impl StoreSink {
    pub fn new(
        storage: Arc<dyn OutputStorageRepository>,
        session_id: Option<String>,
        agent_session_id: Option<String>,
    ) -> Self {
        Self {
            storage,
            session_id,
            agent_session_id,
            stored: Mutex::new(vec![]),
        }
    }

    /// What was stored, in the order it was accepted.
    pub fn take(&self) -> Vec<Emitted> {
        std::mem::take(&mut *self.stored.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// `size` bytes from `file`, a chunk at a time; an early end is an error.
fn chunks(
    file: std::fs::File,
    size: u64,
) -> impl futures::Stream<Item = Result<Bytes, StorageError>> + Send {
    futures::stream::unfold(
        (tokio::fs::File::from_std(file), size),
        |(mut file, left)| async move {
            if left == 0 {
                return None;
            }
            let mut buf = vec![0u8; (left.min(CHUNK as u64)) as usize];
            let got = match file.read(&mut buf).await {
                Ok(0) | Err(_) => {
                    let e = StorageError::InvalidInput("the output ended early".into());
                    return Some((Err(e), (file, 0)));
                }
                Ok(n) => n,
            };
            buf.truncate(got);
            Some((Ok(Bytes::from(buf)), (file, left - got as u64)))
        },
    )
}

#[async_trait]
impl OutputSink for StoreSink {
    async fn accept(&self, file: OutFile) -> Result<(), RunRefusal> {
        let (name, size, mime) = (file.name.clone(), file.size, file.format.mime());
        let stored = self
            .storage
            .store_stream(StoreStreamRequest {
                stream: Box::pin(chunks(file.into_file(), size)),
                size_hint: Some(size),
                mime_type: mime.to_string(),
                filename: name.clone(),
                session_id: self.session_id.clone(),
                agent_session_id: self.agent_session_id.clone(),
                placement: StorePlacement::Generated,
            })
            .await
            .map_err(|_| RunRefusal::Storage)?;
        self.stored
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Emitted {
                name,
                mime_type: mime.to_string(),
                size_bytes: size,
                storage_key: stored.storage_key,
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::collect::{collect_out, CollectLimits};
    use super::*;
    use crate::storage::domain::{StoreRequest, StoredBytes, StoredOutput, StoredStream};
    use futures::StreamExt;

    /// Storage that keeps what `store_stream` is given and the biggest chunk.
    #[derive(Default)]
    struct Capturing {
        got: Mutex<Vec<(String, Vec<u8>)>>,
        biggest: Mutex<usize>,
    }

    #[async_trait]
    impl OutputStorageRepository for Capturing {
        async fn store(&self, _: StoreRequest) -> Result<StoredOutput, StorageError> {
            unreachable!("outputs are streamed")
        }
        async fn read(&self, _: &str) -> Result<StoredBytes, StorageError> {
            unreachable!()
        }
        async fn read_stream(&self, _: &str) -> Result<StoredStream, StorageError> {
            unreachable!()
        }
        async fn delete(&self, _: &str) -> Result<(), StorageError> {
            unreachable!()
        }
        async fn store_stream(
            &self,
            mut req: StoreStreamRequest,
        ) -> Result<StoredOutput, StorageError> {
            assert_eq!(req.placement, StorePlacement::Generated);
            let mut all = vec![];
            while let Some(chunk) = req.stream.next().await {
                let chunk = chunk?;
                let mut big = self.biggest.lock().unwrap();
                *big = (*big).max(chunk.len());
                all.extend_from_slice(&chunk);
            }
            let key = format!("generated/{}", req.filename);
            let size = all.len() as u64;
            self.got.lock().unwrap().push((req.filename.clone(), all));
            Ok(StoredOutput {
                storage_key: key,
                mime_type: req.mime_type,
                filename: req.filename,
                size_bytes: size,
                read_url: String::new(),
            })
        }
    }

    fn one(dir: &std::path::Path, name: &str, bytes: &[u8]) -> OutFile {
        std::fs::write(dir.join(name), bytes).unwrap();
        collect_out(dir, CollectLimits::default())
            .unwrap()
            .files
            .into_iter()
            .find(|f| f.name == name)
            .unwrap()
    }

    #[tokio::test]
    async fn an_output_is_streamed_to_storage_with_its_name_type_and_size() {
        let d = tempfile::tempdir().unwrap();
        let storage = Arc::new(Capturing::default());
        let sink = StoreSink::new(storage.clone(), Some("s".into()), Some("a".into()));
        sink.accept(one(d.path(), "out.csv", b"a,b\n1,2\n"))
            .await
            .unwrap();
        assert_eq!(
            sink.take(),
            [Emitted {
                name: "out.csv".into(),
                mime_type: "text/csv".into(),
                size_bytes: 8,
                storage_key: "generated/out.csv".into()
            }]
        );
        assert_eq!(storage.got.lock().unwrap()[0].1, b"a,b\n1,2\n");
        assert!(sink.take().is_empty(), "taken once");
    }

    /// Memory is one chunk: 5 MiB reach storage in chunks of at most 1 MiB.
    #[tokio::test]
    async fn a_large_output_is_never_held_whole() {
        let d = tempfile::tempdir().unwrap();
        let storage = Arc::new(Capturing::default());
        let sink = StoreSink::new(storage.clone(), None, None);
        sink.accept(one(d.path(), "big.parquet", &vec![7u8; 5 * CHUNK + 3]))
            .await
            .unwrap();
        assert_eq!(*storage.biggest.lock().unwrap(), CHUNK);
        assert_eq!(storage.got.lock().unwrap()[0].1.len(), 5 * CHUNK + 3);
    }

    /// A file truncated after the check is refused: the size that was checked
    /// is the size that must arrive.
    #[tokio::test]
    async fn an_output_that_ends_early_is_refused_and_stored_nowhere() {
        let d = tempfile::tempdir().unwrap();
        let file = one(d.path(), "cut.csv", b"0123456789");
        std::fs::OpenOptions::new()
            .write(true)
            .open(d.path().join("cut.csv"))
            .unwrap()
            .set_len(4)
            .unwrap();
        let storage = Arc::new(Capturing::default());
        let sink = StoreSink::new(storage.clone(), None, None);
        let err = sink.accept(file).await.unwrap_err();
        assert_eq!(err, RunRefusal::Storage);
        assert!(sink.take().is_empty());
        assert!(storage.got.lock().unwrap().is_empty());
    }
}
