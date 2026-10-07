//! Getting an xlsx source into a local file, and what can stop reading one.
//!
//! The storage port only streams a source from its start, and a zip has its
//! directory at the end, so the workbook is written to a temporary file and read
//! with random access from there. That file is at most [`MAX_XLSX_BYTES`]
//! (400 MiB). On Unix it is unlinked as soon as it is created, so it cannot
//! outlive the process; elsewhere a guard removes it when dropped. A job's
//! temporary directory can be memory-backed (Cloud Run's is): the cap counts
//! against the job's memory there.

use crate::storage::domain::StorageError;
use crate::tabular_prepare::convert::ByteStream;
use crate::tabular_prepare::precheck::ArchiveError;
use futures::StreamExt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// Largest workbook accepted, checked before the download and while it runs.
pub const MAX_XLSX_BYTES: u64 = 400 * 1024 * 1024;
/// Which limit a workbook went over.
#[derive(Debug, Error, PartialEq, Eq, Clone, Copy)]
pub enum Cap {
    #[error("the workbook is larger than the 400 MiB limit")]
    Bytes,
}

/// Why a part is not a workbook part, in fixed words.
#[derive(Debug, Error, PartialEq, Eq, Clone, Copy)]
pub enum Invalid {
    #[error("a part of the workbook is not valid XML or is corrupt")]
    Xml,
    #[error("an XML element is longer than the limit")]
    TokenTooLong,
}

/// What can stop reading an xlsx. The text is fixed: no name, key, cell or
/// library message is echoed.
#[derive(Debug, Error, PartialEq, Eq, Clone, Copy)]
pub enum XlsxError {
    #[error(transparent)]
    Archive(#[from] ArchiveError),
    #[error(transparent)]
    TooLarge(Cap),
    #[error(transparent)]
    Invalid(Invalid),
    #[error("the source file does not exist")]
    SourceMissing,
    #[error("the source file could not be read from storage")]
    SourceUnavailable,
    #[error("the preparation was cancelled")]
    Cancelled,
    #[error("the workbook could not be handled in local storage")]
    Local,
}

/// The source, written to a local file. Reads and seeks go to the file.
pub struct Spooled {
    file: File,
    /// Set where the file could not be unlinked at creation.
    path: Option<PathBuf>,
    len: u64,
}

impl Spooled {
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Drop for Spooled {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Read for Spooled {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

impl Seek for Spooled {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

fn temp_file(dir: &Path) -> io::Result<(File, Option<PathBuf>)> {
    let path = dir.join(format!("colmena-xlsx-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path)?;
    #[cfg(unix)]
    {
        std::fs::remove_file(&path)?;
        Ok((file, None))
    }
    #[cfg(not(unix))]
    Ok((file, Some(path)))
}

/// Writes `stream` to a temporary file in `dir`, at most `cap` bytes. `declared`
/// is the size the storage reports: over the cap it is refused before a byte is
/// read. `read` is the bytes written so far, for progress. A source that fails is
/// told apart from one that does not exist.
pub async fn spool_stream(
    dir: &Path,
    mut stream: ByteStream,
    declared: Option<u64>,
    cap: u64,
    cancel: &CancellationToken,
    read: &AtomicU64,
) -> Result<Spooled, XlsxError> {
    if declared.is_some_and(|d| d > cap) {
        return Err(XlsxError::TooLarge(Cap::Bytes));
    }
    let (file, path) = temp_file(dir).map_err(|_| XlsxError::Local)?;
    let mut guard = Spooled {
        file: file.try_clone().map_err(|_| XlsxError::Local)?,
        path,
        len: 0,
    };
    let mut out = tokio::fs::File::from_std(file);
    read.store(0, Ordering::Relaxed);
    loop {
        let next = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(XlsxError::Cancelled),
            item = stream.next() => item,
        };
        let chunk = match next {
            None => break,
            Some(Ok(chunk)) => chunk,
            Some(Err(StorageError::InvalidInput(_))) => return Err(XlsxError::SourceMissing),
            Some(Err(_)) => return Err(XlsxError::SourceUnavailable),
        };
        guard.len += chunk.len() as u64;
        if guard.len > cap {
            return Err(XlsxError::TooLarge(Cap::Bytes));
        }
        out.write_all(&chunk).await.map_err(|_| XlsxError::Local)?;
        read.store(guard.len, Ordering::Relaxed);
    }
    out.flush().await.map_err(|_| XlsxError::Local)?;
    drop(out);
    // The handles share one offset: leave it at the start for the reader.
    guard
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|_| XlsxError::Local)?;
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn stream_of(chunks: Vec<Result<Bytes, StorageError>>) -> ByteStream {
        Box::pin(futures::stream::iter(chunks))
    }

    fn ok(data: &[u8]) -> Result<Bytes, StorageError> {
        Ok(Bytes::copy_from_slice(data))
    }

    async fn spool(
        dir: &Path,
        chunks: Vec<Result<Bytes, StorageError>>,
        declared: Option<u64>,
        cap: u64,
    ) -> Result<Spooled, XlsxError> {
        let read = AtomicU64::new(0);
        spool_stream(
            dir,
            stream_of(chunks),
            declared,
            cap,
            &CancellationToken::new(),
            &read,
        )
        .await
    }

    #[tokio::test]
    async fn a_source_is_spooled_whole_and_leaves_no_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let read = AtomicU64::new(0);
        let chunks = vec![ok(b"hello "), ok(b"world")];
        let mut spooled = spool_stream(
            dir.path(),
            stream_of(chunks),
            Some(11),
            1024,
            &CancellationToken::new(),
            &read,
        )
        .await
        .unwrap();
        let mut text = String::new();
        spooled.read_to_string(&mut text).unwrap();
        assert_eq!(
            (text.as_str(), spooled.len(), read.load(Ordering::Relaxed)),
            ("hello world", 11, 11)
        );
        drop(spooled);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn over_the_cap_is_refused_before_or_while_reading_and_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // The storage's own size is over the cap: no byte is read.
        let never = Box::pin(futures::stream::poll_fn(
            |_| -> std::task::Poll<Option<_>> { panic!("the stream must not be polled") },
        ));
        let read = AtomicU64::new(0);
        let r = spool_stream(
            dir.path(),
            never,
            Some(2000),
            1000,
            &CancellationToken::new(),
            &read,
        )
        .await;
        assert!(matches!(r, Err(XlsxError::TooLarge(Cap::Bytes))));
        // The storage's size lies: the bytes are counted as they arrive.
        let r = spool(
            dir.path(),
            vec![ok(&[0; 600]), ok(&[0; 600])],
            Some(10),
            1000,
        )
        .await;
        assert!(matches!(r, Err(XlsxError::TooLarge(Cap::Bytes))));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn a_missing_source_a_failing_one_and_a_cancel_are_told_apart() {
        let dir = tempfile::tempdir().unwrap();
        let missing = vec![ok(b"x"), Err(StorageError::InvalidInput("gone".into()))];
        let r = spool(dir.path(), missing, None, 1000).await;
        assert!(matches!(r, Err(XlsxError::SourceMissing)));
        let down = vec![Err(StorageError::BackendUnavailable("secret".into()))];
        let r = spool(dir.path(), down, None, 1000).await;
        assert!(matches!(r, Err(XlsxError::SourceUnavailable)));
        // A stream that never yields ends when the token is cancelled.
        let cancel = CancellationToken::new();
        cancel.cancel();
        let read = AtomicU64::new(0);
        let stalled: ByteStream = Box::pin(futures::stream::pending());
        let r = spool_stream(dir.path(), stalled, None, 1000, &cancel, &read).await;
        assert!(matches!(r, Err(XlsxError::Cancelled)));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
