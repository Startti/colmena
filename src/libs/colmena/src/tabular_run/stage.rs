//! Put the prepared tables where the call will read them: a directory the
//! trusted side owns (the call's `data` directory, bound read-only at `/data`).
//!
//! Streamed, never buffered: each part is copied chunk by chunk from storage to
//! a file, so the memory held is one chunk whatever the part's size. Bounded:
//! every part and the whole call have a byte limit, enforced on the declared
//! size before a part is opened AND on the bytes that actually arrive, because
//! a storage may declare one size and send another. Nothing is written outside
//! the directory it was given: paths are built from the canonical part path
//! only, every file is created new (an existing name, a link included, is
//! refused, never followed), and a refused or failed staging removes what it
//! wrote.

use super::refusal::{Budget, Invalid, RunRefusal, Unavailable};
use super::verify::{PreparedTables, DATA_MAX_BYTES};
use crate::storage::domain::OutputStorageRepository;
use crate::tabular_prepare::manifest::{part_path, MANIFEST_PATH};
use crate::tabular_prepare::writer::PART_MAX_BYTES;
use futures::StreamExt;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// Most bytes of one part file: twice the size the converter rolls at, which it
/// overshoots by at most one slice.
pub const PART_FILE_MAX_BYTES: u64 = 2 * PART_MAX_BYTES as u64;

/// What one staging may write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageLimits {
    pub total_bytes: u64,
    pub part_bytes: u64,
    /// No chunk for this long ends the stage: a storage that stalls or trickles
    /// holds nothing. Per chunk.
    pub idle: std::time::Duration,
    /// The whole staging (every part) may take at most this long.
    pub total_time: std::time::Duration,
}

impl Default for StageLimits {
    fn default() -> Self {
        Self {
            total_bytes: DATA_MAX_BYTES,
            part_bytes: PART_FILE_MAX_BYTES,
            idle: super::wire::IDLE_TIMEOUT,
            total_time: super::wire::TRANSFER_MAX,
        }
    }
}

/// What a staging wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    /// Table indexes (as in the manifest) whose parts were staged.
    pub tables: Vec<usize>,
    /// Part files written, the manifest not counted.
    pub parts: usize,
    /// Bytes written, the manifest included.
    pub bytes: u64,
}

/// An I/O failure of the directory the call will read is the executor's, not
/// the model's: it gets the generic refusal and no text from the error.
pub(super) fn local(_: std::io::Error) -> RunRefusal {
    RunRefusal::Unavailable(Unavailable::Executor)
}

pub(super) async fn make_dir(path: &Path) -> Result<(), RunRefusal> {
    use std::os::unix::fs::PermissionsExt;
    // Not `create_dir_all`: an existing entry (a link, say) is refused.
    tokio::fs::DirBuilder::new()
        .create(path)
        .await
        .map_err(local)?;
    // The umask must not decide who can read what the jail binds.
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .await
        .map_err(local)
}

pub(super) async fn create_file(path: &Path) -> Result<tokio::fs::File, RunRefusal> {
    use std::os::unix::fs::PermissionsExt;
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
        .map_err(local)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o644))
        .await
        .map_err(local)?;
    Ok(file)
}

/// A part the registry tracks that the storage DEFINITELY says it does not have is a
/// damaged copy. The adapters have no "not found" variant: they answer `InvalidInput`
/// with a message saying the key was not found for a missing object, and `InvalidInput`
/// also covers a malformed request. Only the first is "missing"; everything else
/// (transient faults, other invalid input) is the storage's moment.
pub(super) fn damaged_or_storage(e: crate::storage::domain::StorageError) -> RunRefusal {
    match e {
        crate::storage::domain::StorageError::InvalidInput(m) if m.contains("not found") => {
            RunRefusal::CopyDamaged
        }
        _ => RunRefusal::Storage,
    }
}

/// Ends a file the call will read: everything written, flushed and synced, with
/// every error reported. A file left to its drop can lose its tail silently.
async fn finish<W>(file: &mut W) -> Result<(), RunRefusal>
where
    W: tokio::io::AsyncWrite + Unpin + Syncable,
{
    file.flush().await.map_err(local)?;
    file.sync().await.map_err(local)
}

/// What [`finish`] needs besides writing: a durable sync.
#[async_trait::async_trait]
pub(super) trait Syncable {
    async fn sync(&self) -> std::io::Result<()>;
}

#[async_trait::async_trait]
impl Syncable for tokio::fs::File {
    async fn sync(&self) -> std::io::Result<()> {
        self.sync_all().await
    }
}

/// Copies one part from storage into `dest`, returning the bytes written.
/// `already` is what the call has staged so far.
#[allow(clippy::too_many_arguments)]
async fn copy_part(
    storage: &dyn OutputStorageRepository,
    key: &str,
    dest: &Path,
    already: u64,
    limits: StageLimits,
    created: &mut Vec<PathBuf>,
    deadline: tokio::time::Instant,
    single_table: bool,
) -> Result<u64, RunRefusal> {
    // Every wait on storage is bounded: by the idle limit and by the stage's deadline.
    let wait = |idle: std::time::Duration| {
        idle.min(deadline.saturating_duration_since(tokio::time::Instant::now()))
    };
    let mut stream = tokio::time::timeout(wait(limits.idle), storage.read_stream(key))
        .await
        .map_err(|_| RunRefusal::Storage)?
        .map_err(damaged_or_storage)?;
    let declared = stream.size_bytes;
    let part_over = RunRefusal::OverBudget(Budget::Part {
        limit_bytes: limits.part_bytes,
    });
    // A single chosen table that does not fit cannot be helped by naming fewer.
    let total_over = RunRefusal::OverBudget(match single_table {
        true => Budget::Table {
            limit_bytes: limits.total_bytes,
        },
        false => Budget::Data {
            limit_bytes: limits.total_bytes,
        },
    });
    // Decided from the declared size, before the file exists.
    if declared > limits.part_bytes {
        return Err(part_over);
    }
    if already.saturating_add(declared) > limits.total_bytes {
        return Err(total_over);
    }
    let mut file = create_file(dest).await?;
    created.push(dest.to_path_buf());
    let mut written = 0u64;
    loop {
        let next = tokio::time::timeout(wait(limits.idle), stream.stream.next())
            .await
            .map_err(|_| RunRefusal::Storage)?;
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|_| RunRefusal::Storage)?;
        written = written.saturating_add(chunk.len() as u64);
        // More than it said it would send: the record and the storage disagree.
        if written > declared {
            return Err(RunRefusal::Invalid(Invalid::Parts));
        }
        // And again on what arrives: the declared size may be a lie.
        if written > limits.part_bytes {
            return Err(part_over);
        }
        if already.saturating_add(written) > limits.total_bytes {
            return Err(total_over);
        }
        file.write_all(&chunk).await.map_err(local)?;
    }
    finish(&mut file).await?;
    // A stream that ended cleanly but early is a storage that cut the transfer:
    // retryable, not a mismatch with the record.
    if written != declared {
        return Err(RunRefusal::Storage);
    }
    Ok(written)
}

/// Stages the manifest and the parts of `tables` (indexes into the plan's
/// manifest) into `data_dir`, which must exist and be empty. On any refusal
/// what was written is removed.
pub async fn stage_tables(
    storage: &dyn OutputStorageRepository,
    plan: &PreparedTables,
    tables: &[usize],
    data_dir: &Path,
    limits: StageLimits,
) -> Result<Staged, RunRefusal> {
    let mut created: Vec<PathBuf> = vec![];
    let done = stage_into(storage, plan, tables, data_dir, limits, &mut created).await;
    if done.is_err() {
        // Files first, then the directories that held them, newest first.
        for path in created.iter().rev() {
            let _ = tokio::fs::remove_file(path).await;
            let _ = tokio::fs::remove_dir(path).await;
        }
    }
    done
}

async fn stage_into(
    storage: &dyn OutputStorageRepository,
    plan: &PreparedTables,
    tables: &[usize],
    data_dir: &Path,
    limits: StageLimits,
    created: &mut Vec<PathBuf>,
) -> Result<Staged, RunRefusal> {
    // The manifest staged is the verified one, serialised again: no byte of the
    // stored file reaches the call unparsed.
    let manifest = plan
        .manifest()
        .to_json()
        .map_err(|_| RunRefusal::Invalid(Invalid::Manifest))?;
    let mut bytes = manifest.len() as u64;
    if bytes > limits.total_bytes {
        return Err(RunRefusal::OverBudget(Budget::Data {
            limit_bytes: limits.total_bytes,
        }));
    }
    let manifest_path = data_dir.join(MANIFEST_PATH);
    let mut manifest_file = create_file(&manifest_path).await?;
    created.push(manifest_path);
    manifest_file
        .write_all(manifest.as_bytes())
        .await
        .map_err(local)?;
    finish(&mut manifest_file).await?;

    let deadline = tokio::time::Instant::now() + limits.total_time;
    let mut parts = 0usize;
    for &t in tables {
        let table = plan
            .tables()
            .get(t)
            .ok_or(RunRefusal::Invalid(Invalid::Manifest))?;
        let dir = data_dir.join(format!("t{t}"));
        make_dir(&dir).await?;
        created.push(dir);
        for p in 0..table.parts as usize {
            let rel = part_path(t, p).map_err(|_| RunRefusal::Invalid(Invalid::Manifest))?;
            let dest = data_dir.join(&rel);
            let key = plan.part_key(t, p)?;
            bytes += copy_part(
                storage,
                &key,
                &dest,
                bytes,
                limits,
                created,
                deadline,
                tables.len() <= 1,
            )
            .await?;
            parts += 1;
        }
    }
    Ok(Staged {
        tables: tables.to_vec(),
        parts,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::super::testkit::*;
    use super::super::verify::verify_prepared;
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::Ordering::SeqCst;
    use std::sync::Arc;

    const MIB: usize = 1024 * 1024;

    async fn plan_of(p: &Prepared) -> PreparedTables {
        verify_prepared(&*p.registry, &*p.storage, SOURCE)
            .await
            .unwrap()
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut out = vec![];
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let e = e.unwrap();
                let rel = e.path().strip_prefix(dir).unwrap().display().to_string();
                if e.file_type().unwrap().is_dir() {
                    stack.push(e.path());
                }
                out.push(rel);
            }
        }
        out.sort();
        out
    }

    #[tokio::test]
    async fn the_manifest_and_every_part_land_at_their_canonical_paths() {
        let p = prepared(&[("sales", 2), ("stores", 1)], 10).await;
        let plan = plan_of(&p).await;
        let dir = tempfile::tempdir().unwrap();
        let staged = stage_tables(&*p.storage, &plan, &[0, 1], dir.path(), Default::default())
            .await
            .unwrap();
        assert_eq!(staged.parts, 3);
        assert_eq!(staged.tables, vec![0, 1]);
        assert_eq!(
            entries(dir.path()),
            [
                "manifest.json",
                "t0",
                "t0/part-00000.parquet",
                "t0/part-00001.parquet",
                "t1",
                "t1/part-00000.parquet"
            ]
        );
        assert_eq!(
            std::fs::read(dir.path().join("t0/part-00001.parquet")).unwrap(),
            vec![b'a'; 10]
        );
        assert_eq!(
            std::fs::read(dir.path().join("t1/part-00000.parquet")).unwrap(),
            vec![b'b'; 10]
        );
        let manifest = std::fs::read(dir.path().join("manifest.json")).unwrap();
        let again = crate::tabular_prepare::manifest::Manifest::from_json(&manifest).unwrap();
        assert_eq!(again, p.manifest);
        assert_eq!(staged.bytes, 30 + manifest.len() as u64);
    }

    /// The jail binds `data` read-only for a user that is not the owner: what is
    /// staged must be readable by it whatever the umask of the host process.
    #[tokio::test]
    async fn staged_files_and_directories_are_world_readable_whatever_the_umask() {
        let p = prepared(&[("sales", 1)], 4).await;
        let plan = plan_of(&p).await;
        let dir = tempfile::tempdir().unwrap();
        let old = unsafe { libc::umask(0o077) };
        let staged = stage_tables(&*p.storage, &plan, &[0], dir.path(), Default::default()).await;
        unsafe { libc::umask(old) };
        staged.unwrap();
        let mode = |rel: &str| {
            std::fs::metadata(dir.path().join(rel))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode("t0"), 0o755);
        assert_eq!(mode("t0/part-00000.parquet"), 0o644);
        assert_eq!(mode("manifest.json"), 0o644);
    }

    #[tokio::test]
    async fn only_the_selected_tables_are_staged() {
        let p = prepared(&[("sales", 1), ("stores", 1)], 4).await;
        let plan = plan_of(&p).await;
        let dir = tempfile::tempdir().unwrap();
        let chosen = plan.select(&["stores".to_string()]).unwrap();
        stage_tables(&*p.storage, &plan, &chosen, dir.path(), Default::default())
            .await
            .unwrap();
        assert_eq!(
            entries(dir.path()),
            ["manifest.json", "t1", "t1/part-00000.parquet"]
        );
        // Only that part was read from storage (the manifest at verification).
        let reads = p.storage.reads.lock().unwrap().clone();
        assert!(reads.iter().all(|k| !k.contains("/t0/")), "{reads:?}");
    }

    /// The total is checked on what the parts DECLARE before a part is opened,
    /// and a refusal leaves nothing behind.
    #[tokio::test]
    async fn a_call_over_the_total_is_refused_and_leaves_nothing() {
        let p = prepared(&[("sales", 3)], 100).await;
        let plan = plan_of(&p).await;
        let dir = tempfile::tempdir().unwrap();
        let manifest_len = p.manifest.to_json().unwrap().len() as u64;
        let limits = StageLimits {
            total_bytes: manifest_len + 250,
            part_bytes: 100,
            ..StageLimits::default()
        };
        let err = stage_tables(&*p.storage, &plan, &[0], dir.path(), limits)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            // One table asked for: naming fewer cannot help, and the text says so.
            RunRefusal::OverBudget(Budget::Table {
                limit_bytes: limits.total_bytes
            })
        );
        assert!(entries(dir.path()).is_empty(), "{:?}", entries(dir.path()));
        // Exactly at the limit is accepted.
        let at = StageLimits {
            total_bytes: manifest_len + 300,
            part_bytes: 100,
            ..StageLimits::default()
        };
        stage_tables(&*p.storage, &plan, &[0], dir.path(), at)
            .await
            .unwrap();
    }

    /// A part that declares more than the part limit is refused before its
    /// file exists and before one byte of it is read.
    #[tokio::test]
    async fn a_part_declaring_over_the_part_limit_is_refused_unread() {
        let p = prepared(&[("sales", 1)], 4).await;
        let plan = plan_of(&p).await;
        let live = Arc::new(Live::default());
        let key = plan.part_key(0, 0).unwrap();
        let l = live.clone();
        p.storage.serve(&key, move || {
            generated(10 * MIB as u64, MIB, 10 * MIB as u64, l.clone(), None)
        });
        let dir = tempfile::tempdir().unwrap();
        let limits = StageLimits {
            total_bytes: 100 * MIB as u64,
            part_bytes: 5 * MIB as u64,
            ..StageLimits::default()
        };
        let err = stage_tables(&*p.storage, &plan, &[0], dir.path(), limits)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            RunRefusal::OverBudget(Budget::Part {
                limit_bytes: limits.part_bytes
            })
        );
        assert_eq!(live.produced.load(SeqCst), 0, "no chunk was read");
        assert!(entries(dir.path()).is_empty());
    }

    /// The declared size is not trusted: a stream that declares little and sends
    /// without end is cut off at the limit, one chunk past it at most.
    #[tokio::test]
    async fn a_stream_that_outgrows_its_declared_size_is_cut_off_at_the_limit() {
        let p = prepared(&[("sales", 1)], 4).await;
        let plan = plan_of(&p).await;
        let live = Arc::new(Live::default());
        let key = plan.part_key(0, 0).unwrap();
        let l = live.clone();
        p.storage.serve(&key, move || {
            generated(u64::MAX, MIB, 1024, l.clone(), None)
        });
        let dir = tempfile::tempdir().unwrap();
        let limits = StageLimits {
            total_bytes: 100 * MIB as u64,
            part_bytes: 4 * MIB as u64,
            ..StageLimits::default()
        };
        let err = stage_tables(&*p.storage, &plan, &[0], dir.path(), limits)
            .await
            .unwrap_err();
        // Past what it declared (1 KiB) is a disagreement with the record, found
        // at the first chunk that crosses it; nothing like the limit is read.
        assert_eq!(err, RunRefusal::Invalid(Invalid::Parts));
        let produced = live.produced.load(SeqCst);
        assert!(
            produced <= 2 * MIB,
            "read {produced} bytes of an endless stream"
        );
        assert!(entries(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn the_total_is_enforced_on_arriving_bytes_too() {
        let p = prepared(&[("sales", 1)], 4).await;
        let plan = plan_of(&p).await;
        let live = Arc::new(Live::default());
        let key = plan.part_key(0, 0).unwrap();
        let l = live.clone();
        p.storage.serve(&key, move || {
            generated(u64::MAX, MIB, 1024, l.clone(), None)
        });
        let dir = tempfile::tempdir().unwrap();
        let manifest_len = p.manifest.to_json().unwrap().len() as u64;
        let limits = StageLimits {
            total_bytes: manifest_len + 3 * MIB as u64,
            part_bytes: 100 * MIB as u64,
            ..StageLimits::default()
        };
        let err = stage_tables(&*p.storage, &plan, &[0], dir.path(), limits)
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Invalid(Invalid::Parts), "{err:?}");
        assert!(live.produced.load(SeqCst) <= 2 * MIB);
        assert!(entries(dir.path()).is_empty());
    }

    /// Bytes that differ from the declared size mean the part is not what the
    /// storage said it was: refused, nothing kept.
    #[tokio::test]
    async fn bytes_that_differ_from_the_declared_size_are_refused() {
        // Less than declared: a storage that cut the transfer (retryable). More:
        // the storage and the record disagree.
        for (total, declared, expected) in [
            (1000u64, 2000u64, RunRefusal::Storage),
            (2000, 1000, RunRefusal::Invalid(Invalid::Parts)),
        ] {
            let p = prepared(&[("sales", 1)], 4).await;
            let plan = plan_of(&p).await;
            let live = Arc::new(Live::default());
            let key = plan.part_key(0, 0).unwrap();
            let l = live.clone();
            p.storage.serve(&key, move || {
                generated(total, 100, declared, l.clone(), None)
            });
            let dir = tempfile::tempdir().unwrap();
            let err = stage_tables(&*p.storage, &plan, &[0], dir.path(), Default::default())
                .await
                .unwrap_err();
            assert_eq!(err, expected, "{total}/{declared}");
            assert!(entries(dir.path()).is_empty());
        }
    }

    #[tokio::test]
    async fn a_storage_failure_mid_part_hides_the_adapters_text_and_leaves_nothing() {
        let p = prepared(&[("sales", 2)], 4).await;
        let plan = plan_of(&p).await;
        let live = Arc::new(Live::default());
        let key = plan.part_key(0, 1).unwrap();
        let l = live.clone();
        p.storage
            .serve(&key, move || generated(1000, 100, 1000, l.clone(), Some(3)));
        let dir = tempfile::tempdir().unwrap();
        let err = stage_tables(&*p.storage, &plan, &[0], dir.path(), Default::default())
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Storage);
        assert!(!err.message().contains("secret"));
        assert!(entries(dir.path()).is_empty());
    }

    /// Memory held while staging is one chunk, however large the part: 64 MiB in
    /// 1 MiB chunks never has more than two chunks alive.
    #[tokio::test]
    async fn memory_is_one_chunk_whatever_the_size_of_the_part() {
        let p = prepared(&[("sales", 1)], 4).await;
        let plan = plan_of(&p).await;
        let live = Arc::new(Live::default());
        let key = plan.part_key(0, 0).unwrap();
        let l = live.clone();
        let total = 64 * MIB as u64;
        p.storage
            .serve(&key, move || generated(total, MIB, total, l.clone(), None));
        let dir = tempfile::tempdir().unwrap();
        let limits = StageLimits {
            total_bytes: 200 * MIB as u64,
            part_bytes: 100 * MIB as u64,
            ..StageLimits::default()
        };
        stage_tables(&*p.storage, &plan, &[0], dir.path(), limits)
            .await
            .unwrap();
        assert_eq!(live.produced.load(SeqCst) as u64, total);
        let peak = live.peak.load(SeqCst);
        assert!(
            peak <= 2 * MIB,
            "peak of {peak} bytes alive for 1 MiB chunks"
        );
        assert_eq!(
            std::fs::metadata(dir.path().join("t0/part-00000.parquet"))
                .unwrap()
                .len(),
            total
        );
        assert_eq!(live.now.load(SeqCst), 0, "every chunk was dropped");
    }

    /// A name already taken in the directory (a link the call could not have
    /// planted, but a bug or a reused directory could) is refused, never followed.
    #[tokio::test]
    async fn an_existing_name_or_link_is_refused_and_never_followed() {
        let p = prepared(&[("sales", 1)], 4).await;
        let plan = plan_of(&p).await;
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), dir.path().join("t0")).unwrap();
        let err = stage_tables(&*p.storage, &plan, &[0], dir.path(), Default::default())
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Unavailable(Unavailable::Executor));
        assert!(
            entries(elsewhere.path()).is_empty(),
            "nothing went through the link"
        );
        // The same for a link in place of the manifest file.
        let dir = tempfile::tempdir().unwrap();
        let target = elsewhere.path().join("victim");
        std::fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("manifest.json")).unwrap();
        let err = stage_tables(&*p.storage, &plan, &[0], dir.path(), Default::default())
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Unavailable(Unavailable::Executor));
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    }

    #[tokio::test]
    async fn a_directory_that_cannot_be_written_is_the_executors_problem() {
        let p = prepared(&[("sales", 1)], 4).await;
        let plan = plan_of(&p).await;
        let err = stage_tables(
            &*p.storage,
            &plan,
            &[0],
            Path::new("/nonexistent-colmena-stage-dir"),
            Default::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err, RunRefusal::Unavailable(Unavailable::Executor));
        assert!(!err.message().contains("nonexistent"));
    }

    #[test]
    fn the_part_limit_is_twice_what_the_converter_rolls_at() {
        assert_eq!(PART_FILE_MAX_BYTES, 128 * 1024 * 1024);
        assert_eq!(StageLimits::default().total_bytes, DATA_MAX_BYTES);
    }

    /// With several tables chosen the refusal is the one that says to name fewer.
    #[tokio::test]
    async fn several_tables_over_the_total_are_told_to_name_fewer() {
        let p = prepared(&[("sales", 1), ("stores", 1)], 100).await;
        let plan = plan_of(&p).await;
        let dir = tempfile::tempdir().unwrap();
        let manifest_len = p.manifest.to_json().unwrap().len() as u64;
        let limits = StageLimits {
            total_bytes: manifest_len + 150,
            part_bytes: 100,
            ..StageLimits::default()
        };
        let err = stage_tables(&*p.storage, &plan, &[0, 1], dir.path(), limits)
            .await
            .unwrap_err();
        assert!(
            matches!(err, RunRefusal::OverBudget(Budget::Data { .. })),
            "{err:?}"
        );
        assert!(err.message().contains("name fewer tables"));
        assert!(Budget::Table {
            limit_bytes: 1 << 30
        }
        .eq(&Budget::Table {
            limit_bytes: 1 << 30
        }));
        let one = RunRefusal::OverBudget(Budget::Table {
            limit_bytes: 1 << 30,
        });
        assert!(one.message().contains("alone") && !one.message().contains("name fewer"));
    }

    /// A writer that accepts everything but fails when it is flushed: a file left
    /// to its drop would lose its tail without a word. The manifest must be
    /// flushed and synced, and the failure must be the call's.
    #[tokio::test]
    async fn a_write_that_fails_at_the_flush_is_a_failure_not_a_truncated_file() {
        struct FailsOnFlush;
        impl tokio::io::AsyncWrite for FailsOnFlush {
            fn poll_write(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                b: &[u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                std::task::Poll::Ready(Ok(b.len()))
            }
            fn poll_flush(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Err(std::io::Error::other("ENOSPC at the flush")))
            }
            fn poll_shutdown(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
        }
        #[async_trait::async_trait]
        impl Syncable for FailsOnFlush {
            async fn sync(&self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut w = FailsOnFlush;
        let err = finish(&mut w).await.unwrap_err();
        assert_eq!(err, RunRefusal::Unavailable(Unavailable::Executor));
        assert!(!err.message().contains("ENOSPC"));
    }

    /// A storage that stalls (or trickles) holds nothing for longer than the idle
    /// limit, and the stage as a whole has a deadline.
    #[tokio::test]
    async fn a_storage_that_stalls_or_trickles_is_cut_off() {
        use crate::storage::domain::StoredStream;
        use futures::StreamExt;
        let p = prepared(&[("sales", 1)], 4).await;
        let plan = plan_of(&p).await;
        let key = plan.part_key(0, 0).unwrap();
        // Stalls: three bytes, then nothing, ever.
        p.storage.serve(&key, || StoredStream {
            stream: Box::pin(
                futures::stream::iter(vec![Ok(bytes::Bytes::from_static(b"abc"))])
                    .chain(futures::stream::pending()),
            ),
            size_bytes: 1000,
            mime_type: "x".into(),
            filename: "p".into(),
        });
        let dir = tempfile::tempdir().unwrap();
        let limits = StageLimits {
            idle: std::time::Duration::from_millis(100),
            ..StageLimits::default()
        };
        let started = std::time::Instant::now();
        let err = stage_tables(&*p.storage, &plan, &[0], dir.path(), limits)
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Storage);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(entries(dir.path()).is_empty());
        // Trickles: a byte every 40 ms never trips the idle limit; the deadline does.
        p.storage.serve(&key, || StoredStream {
            stream: Box::pin(futures::stream::unfold(0u64, |n| async move {
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                Some((Ok(bytes::Bytes::from_static(b"x")), n + 1))
            })),
            size_bytes: 100_000,
            mime_type: "x".into(),
            filename: "p".into(),
        });
        let limits = StageLimits {
            idle: std::time::Duration::from_millis(500),
            total_time: std::time::Duration::from_millis(300),
            ..StageLimits::default()
        };
        let started = std::time::Instant::now();
        let err = stage_tables(&*p.storage, &plan, &[0], dir.path(), limits)
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Storage);
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        assert!(entries(dir.path()).is_empty());
    }

    /// Only a definite "not found" says an object is missing; any other invalid input,
    /// and every transient fault, is the storage's moment.
    #[test]
    fn only_a_definite_not_found_is_a_damaged_copy() {
        use crate::storage::domain::StorageError as E;
        assert_eq!(
            damaged_or_storage(E::InvalidInput(
                "storage_key 'k' not found in LocalCache".into()
            )),
            RunRefusal::CopyDamaged
        );
        for other in [
            E::InvalidInput("empty bytes".into()),
            E::BackendUnavailable("dns".into()),
            E::UploadFailed("503".into()),
            E::CallbackFailed {
                status: 500,
                body: String::new(),
            },
        ] {
            assert_eq!(damaged_or_storage(other), RunRefusal::Storage);
        }
    }

    /// A part the registry tracks that the storage no longer has is a damaged copy
    /// (permanent until it is prepared again); any other storage failure is a moment.
    #[tokio::test]
    async fn a_missing_part_is_a_damaged_copy_and_other_failures_are_storage() {
        let p = prepared(&[("sales", 2)], 4).await;
        let plan = plan_of(&p).await;
        p.storage
            .objects
            .lock()
            .unwrap()
            .remove(&plan.part_key(0, 1).unwrap());
        let dir = tempfile::tempdir().unwrap();
        let err = stage_tables(&*p.storage, &plan, &[0], dir.path(), Default::default())
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::CopyDamaged);
        assert!(entries(dir.path()).is_empty());
        assert!(!err.retryable() && err.message().contains("prepared again"));
        let p = prepared(&[("sales", 1)], 4).await;
        let plan = plan_of(&p).await;
        p.storage
            .broken
            .lock()
            .unwrap()
            .push(plan.part_key(0, 0).unwrap());
        let err = stage_tables(&*p.storage, &plan, &[0], dir.path(), Default::default())
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Storage);
    }
}
