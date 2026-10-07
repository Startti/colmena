//! The driver that prepares one large CSV: claim the registry row, convert the
//! source into Parquet parts on the host's storage, write the manifest LAST and
//! complete the row with it. Dark behind `COLMENA_LARGE_TABULAR`.
//!
//! Order matters. The manifest is the only thing that names the parts, so it is
//! put after every part is stored and the row is completed only after the
//! manifest is stored: a reader that finds a ready row finds every part it
//! lists.
//!
//! What holds about the objects. Before an object is put, its key is listed in the
//! registry row by an owner-guarded write (`track_blobs`): the part keys are
//! deterministic, so one write lists the next sixteen parts and the manifest, not
//! one write per part. So every object a preparation may have written is in the
//! row whatever happens next (a crash, a dropped future, a registry error at the
//! terminal write); a key listed and never written is harmless, deleting is
//! idempotent. A failure is recorded with `fail_with_blobs` and the objects are
//! then deleted. If the registry cannot say whether `complete` was applied nothing
//! is deleted (the row may be ready), the objects stay listed. Nothing but the
//! failure reason is user-visible, and it never carries a cell or a storage key.

use crate::storage::domain::{OutputStorageRepository, StorageError};
use crate::tabular_prepare::convert::{
    convert_csv_table_with, ConvertControl, ConvertError, ConvertedTable, TableError,
};
use crate::tabular_prepare::csv::{CsvError, Encoding};
use crate::tabular_prepare::manifest::{
    parse_part_path, part_path, unique_table_names, ConversionReport, Manifest, ManifestError,
    TableInfo, MANIFEST_PATH, MAX_PARTS, MAX_REPORTED_DEMOTED,
};
use crate::tabular_prepare::part_sink::{PartSink, SinkError};
use crate::tabular_prepare::ports::{
    NoopProgress, PrepareConfig, PrepareProgress, PrepareProgressInfo, PrepareRequest,
    PrepareRunner, ProgressState,
};
use crate::tabular_prepare::prepare::{PrepareStartError, StorageCsvSource, StoragePartSink};
use crate::tabular_prepare::registry::{
    lease_for, ClaimRequest, PreparationRegistry, ReadyInfo, RegistryError, TerminalOutcome,
    FORMAT_VERSION, LEASE_GRACE,
};
use crate::tabular_prepare::writer::{WriterConfig, WriterError};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// How often a running preparation reports its progress.
pub const PROGRESS_INTERVAL: Duration = Duration::from_secs(2);

/// The bound of one registry or storage step that comes after the budget (a
/// terminal write, a delete, an ownership read) and of the claim. A step that
/// does not answer in this time is given up on, never waited for forever.
pub const TERMINAL_STEP: Duration = Duration::from_secs(10);

/// The most such steps a job takes outside its budget: the first progress report
/// before it starts, and after it ended a failure write, a delete, an ownership
/// read, a second delete or the release.
pub const MAX_TERMINAL_STEPS: u32 = 5;

// The lease is the budget plus `LEASE_GRACE` (see `registry::lease_for`), and the
// budget bounds everything up to the manifest, so the job outlasts its lease only
// if the steps after the budget could together take the whole grace.
const _: () = assert!(
    (TERMINAL_STEP.as_secs() * MAX_TERMINAL_STEPS as u64) < LEASE_GRACE.num_seconds() as u64
);

/// Time a preparation may take before it stops itself.
pub const PREP_TIMEOUT: Duration = Duration::from_secs(300);

/// The reasons a preparation can be recorded as failed. Stable, lowercase
/// snake_case; the host maps them to what the user sees. Each is written
/// together with a fixed detail sentence that carries no cell and no key.
pub mod reason {
    /// The time budget ([`super::PREP_TIMEOUT`]) ran out.
    pub const TIME: &str = "time";
    /// The storage could not be read or written (source read, a part or the
    /// manifest put), or answered with a key outside the prepared layout.
    pub const STORAGE: &str = "storage";
    /// The file cannot be read as a CSV: empty, UTF-16 or binary, an unclosed
    /// quote, a record over the limit, a row longer than the header, or more
    /// columns than the reader accepts.
    pub const UNREADABLE_FILE: &str = "unreadable_file";
    /// The table list does not fit the registry row (more than 64 KiB).
    pub const TABLE_TOO_LARGE: &str = "table_too_large";
    /// Anything else: a defect, never the file's fault.
    pub const INTERNAL: &str = "internal";
}

/// Source of "now". Read once per registry write.
pub type Clock = dyn Fn() -> DateTime<Utc> + Send + Sync;

/// How the driver waits out a duration: `tokio::time::sleep` in production, a
/// test's own trigger in tests, so no test depends on how long anything takes.
pub type Sleeper = dyn Fn(Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync;

/// What the driver needs.
pub struct PrepareEnv {
    pub registry: Arc<dyn PreparationRegistry>,
    pub storage: Arc<dyn OutputStorageRepository>,
    pub clock: Arc<Clock>,
    pub sleeper: Arc<Sleeper>,
    pub progress: Arc<dyn PrepareProgress>,
    pub budget: Duration,
    pub writer: WriterConfig,
}

impl PrepareEnv {
    pub fn new(
        registry: Arc<dyn PreparationRegistry>,
        storage: Arc<dyn OutputStorageRepository>,
    ) -> Self {
        Self {
            registry,
            storage,
            clock: Arc::new(Utc::now),
            sleeper: Arc::new(|d| Box::pin(tokio::time::sleep(d))),
            progress: Arc::new(NoopProgress),
            budget: PREP_TIMEOUT,
            writer: WriterConfig::default(),
        }
    }

    pub fn with_clock(mut self, clock: Arc<Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_sleeper(mut self, sleeper: Arc<Sleeper>) -> Self {
        self.sleeper = sleeper;
        self
    }

    pub fn with_progress(mut self, progress: Arc<dyn PrepareProgress>) -> Self {
        self.progress = progress;
        self
    }

    pub fn with_budget(mut self, budget: Duration) -> Self {
        self.budget = budget;
        self
    }

    pub fn with_writer(mut self, writer: WriterConfig) -> Self {
        self.writer = writer;
        self
    }
}

/// A table that is ready: what the registry row and the manifest record, and
/// what the conversion reported.
#[derive(Debug)]
pub struct PreparedTable {
    pub manifest: Manifest,
    /// Full storage key of the manifest.
    pub manifest_key: String,
    /// Every object any attempt may have written, parts and manifest, as full
    /// storage keys.
    pub blob_keys: Vec<String>,
    /// The part of `blob_keys` the manifest does not reference (left by an
    /// aborted run): safe to delete.
    pub stale_keys: Vec<String>,
    pub prepared_bytes: u64,
    /// Everything the conversion reported (rows, parts, encoding, replacements,
    /// the UTF-8 counts, blank and padded rows, demoted columns, restarts).
    pub converted: ConvertedTable,
}

/// A failure that was recorded in the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareFailure {
    pub code: &'static str,
    pub detail: String,
}

#[derive(Debug)]
pub enum PrepareOutcome {
    Ready(Box<PreparedTable>),
    /// The storage adapter reports no derived root: nothing was written, not
    /// even a registry row.
    Refused(PrepareStartError),
    /// Someone else holds the source, it is final, or it is ready already.
    NotClaimed,
    /// The row is gone or another job owns it: nothing was recorded.
    Cancelled,
    /// The source no longer exists: what was written is removed and the row
    /// released, with no failure kept.
    SourceGone,
    Failed(PrepareFailure),
}

fn detail_of_csv(e: &CsvError) -> String {
    match e {
        CsvError::Empty => "the file is empty".into(),
        CsvError::UnsupportedEncoding(what) => format!("unsupported encoding: {what}"),
        CsvError::RecordTooLong { limit, .. } => format!("a record is longer than {limit} bytes"),
        CsvError::TooManyColumns { limit } => format!("the file has more than {limit} columns"),
        _ => "the file is not valid CSV".into(),
    }
}

/// The reason and the fixed detail of a failed conversion.
fn classify(e: &TableError) -> (&'static str, String) {
    use reason::*;
    match e {
        TableError::Convert(ConvertError::Csv(c)) => match c {
            CsvError::Io(_) => (STORAGE, "the source could not be read from storage".into()),
            CsvError::Cancelled | CsvError::InvalidLimits(_) => {
                (INTERNAL, "the conversion stopped unexpectedly".into())
            }
            other => (UNREADABLE_FILE, detail_of_csv(other)),
        },
        TableError::Convert(ConvertError::SourceUnavailable) => {
            (STORAGE, "the source could not be read from storage".into())
        }
        TableError::Convert(ConvertError::Manifest(ManifestError::ManifestTooLarge { .. }))
        | TableError::Writer(WriterError::Manifest(ManifestError::ManifestTooLarge { .. })) => (
            TABLE_TOO_LARGE,
            "the table list does not fit the registry row; export fewer columns".into(),
        ),
        TableError::Writer(WriterError::Sink(_)) => {
            (STORAGE, "a prepared object could not be stored".into())
        }
        _ => (INTERNAL, "the conversion failed".into()),
    }
}

pub fn encoding_name(e: Encoding) -> &'static str {
    match e {
        Encoding::Utf8 => "utf-8",
        Encoding::Windows1252 => "windows-1252",
    }
}

pub fn table_name(filename: &str) -> String {
    let stem = std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    unique_table_names(&[stem]).remove(0)
}

pub fn report_of(table: &str, t: &ConvertedTable) -> ConversionReport {
    ConversionReport {
        table: table.to_string(),
        encoding: encoding_name(t.encoding).to_string(),
        replacements: t.replacements,
        utf8_valid_multibyte: t.utf8_valid_multibyte,
        utf8_invalid: t.utf8_invalid,
        blank_rows: t.blank_rows,
        blank_dropped: t.blank_dropped,
        padded_rows: t.padded_rows,
        restarts: t.restarts as u32,
        all_strings: t.all_strings,
        demoted_count: t.demoted.len() as u32,
        demoted: t
            .demoted
            .iter()
            .take(MAX_REPORTED_DEMOTED)
            .cloned()
            .collect(),
    }
}

/// The in-process runner behind `InlineTrigger`: prepares the CSV a request names.
/// With the engine switch off it runs nothing (no registry read, no storage
/// call); a source that is not a CSV is left for the Excel unit and is logged.
pub struct CsvPrepareRunner {
    env: Arc<PrepareEnv>,
    enabled: bool,
}

impl CsvPrepareRunner {
    /// `config.large_tabular` is the engine switch, read once.
    pub fn new(env: Arc<PrepareEnv>, config: &PrepareConfig) -> Self {
        Self {
            env,
            enabled: config.large_tabular,
        }
    }
}

/// A short, stable identifier of a source for logs, derived from its key with
/// SHA-256: it lets one preparation be followed through the logs without the key
/// (which names a user and a file) being written, and it cannot be turned back.
pub(crate) fn opaque_id(source_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(source_key.as_bytes());
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

fn is_csv(mime: &str) -> bool {
    mime.split(';')
        .next()
        .is_some_and(|m| m.trim().eq_ignore_ascii_case("text/csv"))
}

#[async_trait]
impl PrepareRunner for CsvPrepareRunner {
    async fn run(&self, req: PrepareRequest) {
        if !self.enabled {
            return;
        }
        let source = opaque_id(&req.source_key);
        if !is_csv(&req.mime_type) {
            tracing::warn!(
                target: "colmena::tabular_prepare",
                source = %source,
                "only CSV sources are prepared; the request was dropped"
            );
            return;
        }
        match prepare_csv(&self.env, &req).await {
            Ok(PrepareOutcome::Failed(f)) => tracing::warn!(
                target: "colmena::tabular_prepare",
                source = %source,
                reason = f.code,
                "the preparation failed"
            ),
            Ok(PrepareOutcome::Refused(_)) => tracing::error!(
                target: "colmena::tabular_prepare",
                source = %source,
                kind = "refused",
                "the preparation was refused"
            ),
            Ok(_) => {}
            Err(_) => tracing::error!(
                target: "colmena::tabular_prepare",
                source = %source,
                kind = "registry",
                "the preparation could not record its outcome"
            ),
        }
    }
}

/// The sink of a run: before every object it checks, with a read, that the
/// preparation still owns its row, and refuses to write when it does not. The
/// objects are written under deterministic keys, so a job that lost its row must
/// not write over those of whoever owns it now.
struct OwnedSink {
    inner: Arc<StoragePartSink>,
    registry: Arc<dyn PreparationRegistry>,
    clock: Arc<Clock>,
    source_key: String,
    owner: String,
    lost: AtomicBool,
    /// Parts `0..tracked_parts` of each table are listed in the row already.
    tracked_parts: AtomicUsize,
    manifest_tracked: AtomicBool,
}

/// How many part keys are listed in the row ahead of the part being written. The
/// part keys are deterministic, so they are known long before the parts exist: one
/// registry write lists the next sixteen (up to a gigabyte of output) and the
/// manifest, instead of one write per part. A key listed and never written is
/// harmless, deleting is idempotent; an object written and never listed is what
/// this prevents.
const TRACK_AHEAD: usize = 16;

impl OwnedSink {
    /// Lists in the row, owner-guarded, the keys that writing `path` needs listed,
    /// before it is written. `Ok(false)` means the row is no longer ours.
    async fn track_for(&self, path: &str) -> Result<bool, RegistryError> {
        let mut keys: Vec<String> = Vec::new();
        let mut upto = None;
        if let Some((table, part)) = parse_part_path(path) {
            if part >= self.tracked_parts.load(Ordering::SeqCst) {
                let end = (part + TRACK_AHEAD).min(MAX_PARTS);
                keys.extend((part..end).filter_map(|p| part_path(table, p).ok()));
                upto = Some(end);
            }
        }
        let with_manifest = !self.manifest_tracked.load(Ordering::SeqCst);
        if with_manifest {
            keys.push(MANIFEST_PATH.to_string());
        }
        if keys.is_empty() {
            return self.registry_owns().await;
        }
        let full: Vec<String> = keys.iter().map(|k| self.inner.key_of(k)).collect();
        let outcome = self
            .registry
            .track_blobs(&self.source_key, &self.owner, &full, (self.clock)())
            .await?;
        if outcome == TerminalOutcome::Cancelled {
            return Ok(false);
        }
        if let Some(end) = upto {
            self.tracked_parts.store(end, Ordering::SeqCst);
        }
        self.manifest_tracked.store(true, Ordering::SeqCst);
        Ok(true)
    }

    async fn registry_owns(&self) -> Result<bool, RegistryError> {
        self.registry
            .still_owned(&self.source_key, &self.owner)
            .await
    }
}

#[async_trait]
impl PartSink for OwnedSink {
    async fn put(&self, path: &str, data: Bytes) -> Result<(), SinkError> {
        match self.track_for(path).await {
            Ok(true) => self.inner.put(path, data).await,
            Ok(false) => {
                self.lost.store(true, Ordering::SeqCst);
                Err(SinkError("the preparation no longer owns its row".into()))
            }
            Err(_) => Err(SinkError(
                "the registry could not confirm the preparation's row".into(),
            )),
        }
    }
}

/// Runs one step under [`TERMINAL_STEP`]; `None` when it did not answer in time.
async fn bounded<T>(env: &PrepareEnv, step: impl Future<Output = T>) -> Option<T> {
    tokio::select! {
        out = step => Some(out),
        () = (env.sleeper)(TERMINAL_STEP) => None,
    }
}

/// What a registry step that did not answer in time reports. Fixed text: it
/// carries no key and nothing from the backend.
fn no_answer() -> RegistryError {
    RegistryError::Backend("the registry did not answer in time".into())
}

/// A registry step under its bound.
async fn within<T>(
    env: &PrepareEnv,
    step: impl Future<Output = Result<T, RegistryError>>,
) -> Result<T, RegistryError> {
    bounded(env, step).await.unwrap_or_else(|| Err(no_answer()))
}

/// Prepares `req.source_key` as table 0 of its source. Registry errors are
/// returned as they are: if the registry cannot be written nothing can be
/// recorded.
pub async fn prepare_csv(
    env: &PrepareEnv,
    req: &PrepareRequest,
) -> Result<PrepareOutcome, RegistryError> {
    let outcome = run_prepare(env, req).await;
    let (state, done, total) = match &outcome {
        Ok(PrepareOutcome::Ready(_)) => {
            (ProgressState::Ready, req.size_bytes, Some(req.size_bytes))
        }
        Ok(PrepareOutcome::Failed(_)) => (ProgressState::Failed, 0, None),
        Ok(PrepareOutcome::Cancelled | PrepareOutcome::SourceGone) => {
            (ProgressState::Cancelled, 0, None)
        }
        // Nothing was started, or the registry could not be written.
        _ => return outcome,
    };
    bounded(
        env,
        env.progress
            .report(&req.source_key, PrepareProgressInfo { state, done, total }),
    )
    .await;
    outcome
}

async fn run_prepare(
    env: &PrepareEnv,
    req: &PrepareRequest,
) -> Result<PrepareOutcome, RegistryError> {
    let stored = match StoragePartSink::new(env.storage.clone(), &req.source_key) {
        Ok(sink) => Arc::new(sink),
        Err(e) => return Ok(PrepareOutcome::Refused(e)),
    };
    let owner = format!("prep-{}", uuid::Uuid::new_v4());
    let lease = chrono::Duration::from_std(env.budget).unwrap_or(chrono::Duration::days(1));
    let claim = within(
        env,
        env.registry.claim(ClaimRequest {
            source_key: req.source_key.clone(),
            source_bytes: i64::try_from(req.size_bytes).unwrap_or(i64::MAX),
            format_version: FORMAT_VERSION,
            owner: owner.clone(),
            lease: lease_for(lease),
            now: (env.clock)(),
        }),
    )
    .await?;
    if claim.is_none() {
        return Ok(PrepareOutcome::NotClaimed);
    }
    let sink = Arc::new(OwnedSink {
        inner: stored.clone(),
        registry: env.registry.clone(),
        clock: env.clock.clone(),
        source_key: req.source_key.clone(),
        owner: owner.clone(),
        lost: AtomicBool::new(false),
        tracked_parts: AtomicUsize::new(0),
        manifest_tracked: AtomicBool::new(false),
    });
    let control = ConvertControl::new();
    let source = StorageCsvSource::new(env.storage.clone(), &req.source_key);
    let read = source.bytes_read();
    // Under the total until the table is ready: the file is read before its output
    // is written, and a bar must not show complete while that goes on.
    let running = |done: u64| PrepareProgressInfo {
        state: ProgressState::Running,
        done: done.min(req.size_bytes.saturating_sub(1)),
        total: Some(req.size_bytes),
    };
    bounded(env, env.progress.report(&req.source_key, running(0))).await;
    // Progress is reported every [`PROGRESS_INTERVAL`] while the run goes on, to
    // the host's progress port and never to the registry.
    let ticker = async {
        loop {
            (env.sleeper)(PROGRESS_INTERVAL).await;
            let done = read.load(Ordering::Relaxed);
            env.progress.report(&req.source_key, running(done)).await;
        }
    };
    // Every key any run may have written, as full storage keys.
    let keys_of =
        |paths: Vec<String>| -> Vec<String> { paths.iter().map(|p| stored.key_of(p)).collect() };
    // One budget for everything up to the manifest: the conversion and the
    // manifest put. When it ends first the run is dropped, which cancels its
    // reader; the keys were listed in the row before each put.
    let budget = (env.sleeper)(env.budget);
    tokio::pin!(budget);
    let converted = tokio::select! {
        done = convert_csv_table_with(
            &source,
            sink.clone() as Arc<dyn PartSink>,
            0,
            env.writer,
            &control,
        ) => done,
        never = ticker => {
            let _: std::convert::Infallible = never;
            unreachable!("the progress ticker never ends")
        }
        () = &mut budget => {
            let detail = "the preparation did not finish within its time budget".to_string();
            return fail(env, req, &owner, reason::TIME, detail, keys_of(control.paths_of(0))).await;
        }
    };
    match converted {
        Ok(table) => {
            let name = table_name(&req.filename);
            let manifest = Manifest::new(vec![TableInfo {
                name: name.clone(),
                rows: table.written.rows,
                parts: table.written.parts,
                columns: table.written.columns.clone(),
            }])
            .with_conversion(vec![report_of(&name, &table)]);
            let mut blob_keys = keys_of(table.blob_paths.clone());
            let manifest_key = stored.key_of(MANIFEST_PATH);
            blob_keys.push(manifest_key.clone());
            let manifest_put = async {
                let tables_json = manifest.tables_json().map_err(|_| {
                    (
                        reason::TABLE_TOO_LARGE,
                        "the table list does not fit the registry row; export fewer columns"
                            .to_string(),
                    )
                })?;
                let json = manifest.to_json().map_err(|_| {
                    (
                        reason::INTERNAL,
                        "the manifest could not be written".to_string(),
                    )
                })?;
                sink.put(MANIFEST_PATH, Bytes::from(json))
                    .await
                    .map_err(|_| {
                        (
                            reason::STORAGE,
                            "the manifest could not be stored".to_string(),
                        )
                    })?;
                Ok::<_, (&'static str, String)>(tables_json)
            };
            let manifest_put = tokio::select! {
                done = manifest_put => done,
                () = &mut budget => Err((
                    reason::TIME,
                    "the preparation did not finish within its time budget".to_string(),
                )),
            };
            let tables_json = match manifest_put {
                Ok(t) => t,
                Err((code, detail)) => {
                    if sink.lost.load(Ordering::SeqCst) {
                        return settle_lost(env, req, &blob_keys).await;
                    }
                    return fail(env, req, &owner, code, detail, blob_keys).await;
                }
            };
            let mut live: Vec<String> = table.live_paths(0);
            live.push(MANIFEST_PATH.to_string());
            let prepared_bytes = stored.bytes_of(live.iter());
            let outcome = within(
                env,
                env.registry.complete(
                    &req.source_key,
                    &owner,
                    ReadyInfo {
                        manifest_key: manifest_key.clone(),
                        blob_keys: blob_keys.clone(),
                        tables_json,
                        prepared_bytes: i64::try_from(prepared_bytes).unwrap_or(i64::MAX),
                    },
                    (env.clock)(),
                ),
            )
            .await?;
            match outcome {
                TerminalOutcome::Cancelled => settle_lost(env, req, &blob_keys).await,
                TerminalOutcome::Written => {
                    let stale_keys = keys_of(table.stale_paths.clone());
                    Ok(PrepareOutcome::Ready(Box::new(PreparedTable {
                        manifest,
                        manifest_key,
                        blob_keys,
                        stale_keys,
                        prepared_bytes,
                        converted: table,
                    })))
                }
            }
        }
        Err(failure) => {
            let keys = keys_of(failure.blob_paths.clone());
            if sink.lost.load(Ordering::SeqCst) {
                return settle_lost(env, req, &keys).await;
            }
            if matches!(
                failure.error,
                TableError::Convert(ConvertError::SourceMissing)
            ) {
                return source_gone(env, req, &owner, keys).await;
            }
            let (code, detail) = classify(&failure.error);
            fail(env, req, &owner, code, detail, keys).await
        }
    }
}

/// The preparation lost its row: it records nothing. If the row is gone (the
/// source was deleted) what it wrote belongs to nobody and is removed; if
/// another job owns the row, the keys are the same deterministic ones and belong
/// to that job now, so they are left alone.
async fn settle_lost(
    env: &PrepareEnv,
    req: &PrepareRequest,
    keys: &[String],
) -> Result<PrepareOutcome, RegistryError> {
    if within(env, env.registry.get(&req.source_key))
        .await?
        .is_none()
    {
        delete_best_effort(env, req, keys).await;
    }
    Ok(PrepareOutcome::Cancelled)
}

/// The source does not exist (any more): nothing is worth keeping and no
/// failure is recorded. The objects are deleted first; if that fails the row
/// stays, failed, with the keys, so the cleanup pass can still reach them.
async fn source_gone(
    env: &PrepareEnv,
    req: &PrepareRequest,
    owner: &str,
    keys: Vec<String>,
) -> Result<PrepareOutcome, RegistryError> {
    // Ownership first: after a lease takeover the deterministic keys are the new
    // owner's, and a job that is not the owner deletes and releases nothing.
    if !within(env, env.registry.still_owned(&req.source_key, owner)).await? {
        return settle_lost(env, req, &keys).await;
    }
    if !matches!(
        bounded(env, env.storage.delete_derived(&req.source_key, &keys)).await,
        Some(Ok(()))
    ) {
        let detail = "the objects of a deleted source could not be removed".to_string();
        return fail(env, req, owner, reason::STORAGE, detail, keys).await;
    }
    Ok(
        if within(env, env.registry.release(&req.source_key, owner)).await? {
            PrepareOutcome::SourceGone
        } else {
            PrepareOutcome::Cancelled
        },
    )
}

/// Deletes what a preparation wrote. The keys are listed in the row, so what
/// this cannot delete the cleanup pass will; the failure is logged as a fixed
/// sentence and a kind, never with the adapter's text or a key.
async fn delete_best_effort(env: &PrepareEnv, req: &PrepareRequest, keys: &[String]) {
    match bounded(env, env.storage.delete_derived(&req.source_key, keys)).await {
        Some(Ok(())) => {}
        Some(Err(e)) => tracing::warn!(
            target: "colmena::tabular_prepare",
            kind = storage_kind(&e),
            "could not delete prepared objects; the cleanup pass will"
        ),
        None => tracing::warn!(
            target: "colmena::tabular_prepare",
            kind = "no_answer",
            "could not delete prepared objects; the cleanup pass will"
        ),
    }
}

fn storage_kind(e: &StorageError) -> &'static str {
    match e {
        StorageError::BackendUnavailable(_) => "backend_unavailable",
        StorageError::InvalidInput(_) => "invalid_input",
        StorageError::UploadFailed(_) => "upload_failed",
        StorageError::CallbackFailed { .. } => "callback_failed",
    }
}

/// Records the failure with every key that may exist, then deletes them.
async fn fail(
    env: &PrepareEnv,
    req: &PrepareRequest,
    owner: &str,
    code: &'static str,
    detail: String,
    keys: Vec<String>,
) -> Result<PrepareOutcome, RegistryError> {
    let outcome = match within(
        env,
        env.registry
            .fail_with_blobs(&req.source_key, owner, code, &detail, &keys, (env.clock)()),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(e) => {
            // Whether the failure was recorded is unknown, and deleting the objects
            // of a failed preparation is right either way: they are listed in the
            // row (they were tracked before they were written).
            delete_best_effort(env, req, &keys).await;
            return Err(e);
        }
    };
    if outcome == TerminalOutcome::Cancelled {
        // The row is not ours any more: the same decision as everywhere else, row
        // gone (delete what this job wrote) or row taken (delete nothing).
        return settle_lost(env, req, &keys).await;
    }
    // Best effort: the keys are tracked, so the cleanup pass removes what this
    // could not.
    delete_best_effort(env, req, &keys).await;
    Ok(PrepareOutcome::Failed(PrepareFailure { code, detail }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabular_prepare::manifest::{ColumnInfo, ColumnType, MAX_NAME_CHARS};
    use crate::tabular_prepare::writer::TableWritten;

    #[test]
    fn the_table_is_named_after_the_file_without_its_extension() {
        assert_eq!(table_name("sales.csv"), "sales");
        assert_eq!(table_name("Q3 report.final.CSV"), "Q3 report.final");
        assert_eq!(table_name("data"), "data");
        // A name that is empty or only whitespace gets the generic one.
        assert_eq!(table_name(""), "sheet1");
        assert_eq!(table_name("  .csv"), "sheet1");
        assert_eq!(table_name(".csv"), ".csv");
        // Control characters are replaced and the length is bounded.
        assert_eq!(table_name("a\u{7}b.csv"), "a_b");
        assert_eq!(
            table_name(&format!("{}.csv", "x".repeat(200)))
                .chars()
                .count(),
            MAX_NAME_CHARS
        );
        // What comes out always passes the manifest's own check.
        for raw in ["a\u{0}b.csv", "\u{202e}x.csv", "../../etc/passwd.csv"] {
            let name = table_name(raw);
            assert!(
                !name.is_empty() && !name.chars().any(char::is_control),
                "{raw}"
            );
        }
    }

    fn converted(demoted: usize) -> ConvertedTable {
        ConvertedTable {
            written: TableWritten {
                rows: 10,
                parts: 1,
                columns: vec![ColumnInfo {
                    name: "a".into(),
                    column_type: ColumnType::String,
                    uncompressed_bytes: 1,
                    in_memory_bytes: 40,
                }],
            },
            restarts: 2,
            demoted: (0..demoted).map(|i| format!("c{i}")).collect(),
            all_strings: demoted > 0,
            encoding: Encoding::Windows1252,
            replacements: 0,
            utf8_valid_multibyte: 3,
            utf8_invalid: 90,
            blank_rows: 1,
            blank_dropped: 2,
            padded_rows: 4,
            blob_paths: Vec::new(),
            stale_paths: Vec::new(),
        }
    }

    #[test]
    fn the_report_carries_every_count_the_conversion_gave() {
        let r = report_of("sales", &converted(2));
        assert_eq!(r.table, "sales");
        assert_eq!(r.encoding, "windows-1252");
        assert_eq!(
            (r.replacements, r.utf8_valid_multibyte, r.utf8_invalid),
            (0, 3, 90)
        );
        assert_eq!((r.blank_rows, r.blank_dropped, r.padded_rows), (1, 2, 4));
        assert_eq!((r.restarts, r.all_strings), (2, true));
        assert_eq!((r.demoted_count, r.demoted.len()), (2, 2));
        let mut utf8 = converted(0);
        utf8.encoding = Encoding::Utf8;
        assert_eq!(report_of("t", &utf8).encoding, "utf-8");
    }

    #[test]
    fn a_wide_table_reports_the_exact_count_and_only_the_first_names() {
        let r = report_of("wide", &converted(5000));
        assert_eq!(r.demoted_count, 5000);
        assert_eq!(r.demoted.len(), MAX_REPORTED_DEMOTED);
        assert_eq!(r.demoted[0], "c0");
        assert_eq!(r.demoted[MAX_REPORTED_DEMOTED - 1], "c31");
        // The manifest accepts what the report produces.
        let manifest = Manifest::new(vec![crate::tabular_prepare::manifest::TableInfo {
            name: "wide".into(),
            rows: 10,
            parts: 1,
            columns: converted(0).written.columns,
        }])
        .with_conversion(vec![r]);
        assert!(manifest.to_json().is_ok());
    }

    #[test]
    fn every_failure_has_a_reason_and_a_fixed_text_without_content() {
        use crate::tabular_prepare::part_sink::SinkError;
        let csv = |c: CsvError| TableError::Convert(ConvertError::Csv(c));
        let cases: Vec<(TableError, &str, &str)> = vec![
            (
                csv(CsvError::Empty),
                reason::UNREADABLE_FILE,
                "the file is empty",
            ),
            (
                csv(CsvError::UnsupportedEncoding("UTF-16".into())),
                reason::UNREADABLE_FILE,
                "unsupported encoding: UTF-16",
            ),
            (
                csv(CsvError::RecordTooLong {
                    record: 7,
                    limit: 1048576,
                }),
                reason::UNREADABLE_FILE,
                "a record is longer than 1048576 bytes",
            ),
            (
                csv(CsvError::TooManyColumns { limit: 16384 }),
                reason::UNREADABLE_FILE,
                "the file has more than 16384 columns",
            ),
            (
                csv(CsvError::Parse("row 3: secret cell".into())),
                reason::UNREADABLE_FILE,
                "the file is not valid CSV",
            ),
            (
                csv(CsvError::Io("https://x/secret-key".into())),
                reason::STORAGE,
                "the source could not be read from storage",
            ),
            (
                TableError::Convert(ConvertError::SourceUnavailable),
                reason::STORAGE,
                "the source could not be read from storage",
            ),
            (
                TableError::Writer(WriterError::Sink(SinkError("secret-key".into()))),
                reason::STORAGE,
                "a prepared object could not be stored",
            ),
            (
                TableError::Writer(WriterError::Manifest(ManifestError::ManifestTooLarge {
                    bytes: 70_000,
                    cap: 65_536,
                })),
                reason::TABLE_TOO_LARGE,
                "the table list does not fit the registry row; export fewer columns",
            ),
            (
                TableError::Convert(ConvertError::Manifest(ManifestError::ManifestTooLarge {
                    bytes: 70_000,
                    cap: 65_536,
                })),
                reason::TABLE_TOO_LARGE,
                "the table list does not fit the registry row; export fewer columns",
            ),
            (
                TableError::Convert(ConvertError::ReaderPanicked),
                reason::INTERNAL,
                "the conversion failed",
            ),
            (
                TableError::Convert(ConvertError::Cast("secret".into())),
                reason::INTERNAL,
                "the conversion failed",
            ),
            (
                csv(CsvError::Cancelled),
                reason::INTERNAL,
                "the conversion stopped unexpectedly",
            ),
            (
                csv(CsvError::InvalidLimits("secret".into())),
                reason::INTERNAL,
                "the conversion stopped unexpectedly",
            ),
        ];
        for (err, code, detail) in cases {
            let (c, d) = classify(&err);
            assert_eq!((c, d.as_str()), (code, detail), "{err:?}");
            assert!(!d.contains("secret"), "{err:?}");
        }
    }

    #[test]
    fn the_time_budget_is_the_design_value_and_the_reasons_are_stable_snake_case() {
        assert_eq!(PREP_TIMEOUT, Duration::from_secs(300));
        let all = [
            reason::TIME,
            reason::STORAGE,
            reason::UNREADABLE_FILE,
            reason::TABLE_TOO_LARGE,
            reason::INTERNAL,
        ];
        for r in all {
            assert!(
                r.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{r}"
            );
        }
        let mut sorted = all.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len());
        assert_eq!(reason::TIME, "time");
    }
}

/// The driver's cases, written once over any registry: SQLite here, Postgres in
/// the ignored tests of `postgres_registry`.
#[cfg(test)]
pub(crate) mod cases {
    use super::*;
    use crate::tabular_prepare::prepare::fake::{root_of, PlacedStorage};
    use crate::tabular_prepare::registry::PrepareStatus;
    use chrono::TimeZone;

    /// A key no other run of the same database has used.
    pub(crate) fn fresh_source() -> String {
        format!("chat-attachments/u/s/{}.csv", uuid::Uuid::new_v4())
    }

    pub(crate) fn request(source: &str, size: u64) -> PrepareRequest {
        PrepareRequest {
            source_key: source.to_string(),
            mime_type: "text/csv".into(),
            filename: "sales.csv".into(),
            size_bytes: size,
        }
    }

    pub(crate) fn env(
        registry: Arc<dyn PreparationRegistry>,
        storage: Arc<PlacedStorage>,
    ) -> PrepareEnv {
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        PrepareEnv::new(registry, storage)
            .with_clock(Arc::new(move || now))
            .with_writer(WriterConfig {
                max_rows: 2,
                max_bytes: usize::MAX,
            })
    }

    fn five_rows() -> Vec<u8> {
        b"id,name\n1,ann\n2,bob\n3,cy\n4,di\n5,ed\n".to_vec()
    }

    fn part_keys(source: &str, n: usize) -> Vec<String> {
        let root = root_of(source);
        (0..n)
            .map(|i| format!("{root}/t0/part-{i:05}.parquet"))
            .collect()
    }

    fn objects_but_source(storage: &PlacedStorage, source: &str) -> Vec<String> {
        storage.keys().into_iter().filter(|k| k != source).collect()
    }

    async fn failed_row(
        registry: &Arc<dyn PreparationRegistry>,
        source: &str,
        outcome: PrepareOutcome,
    ) -> (
        PrepareFailure,
        crate::tabular_prepare::registry::PreparedRow,
    ) {
        let PrepareOutcome::Failed(f) = outcome else {
            panic!("expected a recorded failure, got {outcome:?}");
        };
        let row = registry.get(source).await.unwrap().unwrap();
        assert_eq!(row.status, PrepareStatus::Failed);
        assert_eq!(row.error_code.as_deref(), Some(f.code));
        assert_eq!(row.error_detail.as_deref(), Some(f.detail.as_str()));
        (f, row)
    }

    pub(crate) async fn a_prepared_table_is_ready_with_the_manifest_stored_last(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let env = env(registry.clone(), storage.clone());
        let PrepareOutcome::Ready(table) = prepare_csv(&env, &request(source, 40)).await.unwrap()
        else {
            panic!("expected a ready table");
        };
        let root = root_of(source);
        let manifest_key = format!("{root}/manifest.json");
        // Layout: exactly the manifest and three parts of two rows (the last of one).
        let mut want = part_keys(source, 3);
        want.push(manifest_key.clone());
        let mut got = objects_but_source(&storage, source);
        got.sort();
        want.sort();
        assert_eq!(got, want);
        // The manifest is the last object stored.
        assert_eq!(storage.order.lock().unwrap().last(), Some(&manifest_key));
        // The row says what the storage holds.
        let row = registry.get(source).await.unwrap().unwrap();
        assert_eq!(row.status, PrepareStatus::Ready);
        assert_eq!(row.format_version, FORMAT_VERSION);
        assert_eq!(row.manifest_key.as_deref(), Some(manifest_key.as_str()));
        // Every object is listed; so are the part keys listed ahead of the parts
        // (sixteen) that were never needed, which cleanup deletes as no-ops.
        for k in &want {
            assert!(row.blob_keys.contains(k), "{k} not tracked");
        }
        assert_eq!(row.blob_keys.len(), TRACK_AHEAD + 1);
        assert_eq!(table.manifest.tables[0].rows, 5);
        assert_eq!(table.manifest.tables[0].parts, 3);
        assert_eq!(
            row.tables_json.as_deref(),
            Some(table.manifest.tables_json().unwrap().as_str())
        );
        let stored_manifest = storage.objects.lock().unwrap()[&manifest_key].clone();
        assert_eq!(
            Manifest::from_json(&stored_manifest).unwrap(),
            table.manifest
        );
        let held: usize = objects_but_source(&storage, source)
            .iter()
            .map(|k| storage.objects.lock().unwrap()[k].len())
            .sum();
        assert_eq!(row.prepared_bytes, Some(held as i64));
        assert_eq!(table.prepared_bytes, held as u64);
        assert!(table.stale_keys.is_empty());
    }

    pub(crate) async fn without_a_derived_root_nothing_is_written_not_even_a_row(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        *storage.no_root.lock().unwrap() = true;
        let env = env(registry.clone(), storage.clone());
        let out = prepare_csv(&env, &request(source, 40)).await.unwrap();
        assert!(matches!(
            out,
            PrepareOutcome::Refused(PrepareStartError::NoDerivedRoot)
        ));
        assert!(registry.get(source).await.unwrap().is_none());
        assert_eq!(*storage.stores.lock().unwrap(), 0);
        assert_eq!(*storage.opens.lock().unwrap(), 0);
    }

    pub(crate) async fn the_result_carries_what_the_conversion_reports(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        // Windows-1252, a short row and a blank line.
        let body = b"name,v\ncaf\xE9,1\nshort\n\nzo\xEB,2\n".to_vec();
        let storage = PlacedStorage::with_source(source, body);
        let env = env(registry, storage.clone());
        let PrepareOutcome::Ready(table) = prepare_csv(&env, &request(source, 30)).await.unwrap()
        else {
            panic!("expected a ready table");
        };
        let c = &table.converted;
        assert_eq!(c.encoding, Encoding::Windows1252);
        assert_eq!((c.replacements, c.padded_rows, c.blank_dropped), (0, 1, 1));
        assert!(c.utf8_invalid >= 2 && c.utf8_valid_multibyte == 0);
        let r = &table.manifest.conversion[0];
        assert_eq!(r.table, "sales");
        assert_eq!(r.encoding, "windows-1252");
        assert_eq!(
            (
                r.replacements,
                r.padded_rows,
                r.blank_dropped,
                r.utf8_invalid
            ),
            (0, 1, 1, c.utf8_invalid)
        );
        // The stored manifest carries the same section.
        let key = table.manifest_key.clone();
        let json = storage.objects.lock().unwrap()[&key].clone();
        assert_eq!(
            Manifest::from_json(&json).unwrap().conversion,
            table.manifest.conversion
        );
    }

    pub(crate) async fn a_second_preparation_is_not_claimed_and_stores_nothing(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let env = env(registry, storage.clone());
        assert!(matches!(
            prepare_csv(&env, &request(source, 40)).await.unwrap(),
            PrepareOutcome::Ready(_)
        ));
        let stores = *storage.stores.lock().unwrap();
        assert!(matches!(
            prepare_csv(&env, &request(source, 40)).await.unwrap(),
            PrepareOutcome::NotClaimed
        ));
        assert_eq!(*storage.stores.lock().unwrap(), stores);
    }

    pub(crate) async fn a_file_that_is_not_a_csv_fails_with_a_fixed_text_and_no_cell(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        for (body, detail) in [
            (&b""[..], "the file is empty"),
            (&b"\xFF\xFEa\0,\0b\0"[..], "unsupported encoding: UTF-16"),
            (
                &b"a,b\n1,a secret cell,3\n"[..],
                "the file is not valid CSV",
            ),
        ] {
            let storage = Arc::new(PlacedStorage::default());
            let env = env(registry.clone(), storage.clone());
            // A fresh key per case keeps the rows apart.
            let source = format!("chat-attachments/u/s/{}.csv", uuid::Uuid::new_v4());
            storage
                .objects
                .lock()
                .unwrap()
                .insert(source.clone(), Bytes::from(body.to_vec()));
            let out = prepare_csv(&env, &request(&source, 10)).await.unwrap();
            let PrepareOutcome::Failed(f) = out else {
                panic!("expected a failure");
            };
            assert_eq!(
                (f.code, f.detail.as_str()),
                (reason::UNREADABLE_FILE, detail)
            );
            let row = registry.get(&source).await.unwrap().unwrap();
            assert_eq!(row.error_code.as_deref(), Some("unreadable_file"));
            assert!(!row.error_detail.unwrap().contains("secret"));
            assert!(row.blob_keys.is_empty());
        }
    }

    pub(crate) async fn a_storage_failure_midway_tracks_every_key_tried_and_deletes_them(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // The first part is stored, the second put fails.
        *storage.fail_stores_from.lock().unwrap() = Some(1);
        let env = env(registry.clone(), storage.clone());
        let out = prepare_csv(&env, &request(source, 40)).await.unwrap();
        let (f, row) = failed_row(&registry, source, out).await;
        assert_eq!(f.code, reason::STORAGE);
        assert!(!f.detail.contains("secret") && !f.detail.contains("chat-attachments"));
        // The key of the put that failed is tracked, with the one that worked.
        for k in part_keys(source, 2) {
            assert!(row.blob_keys.contains(&k), "{k} untracked");
        }
        // Everything tracked was deleted; the source is untouched.
        assert_eq!(storage.keys(), vec![source.to_string()]);
        for k in part_keys(source, 2) {
            assert!(storage.deleted.lock().unwrap().contains(&k));
        }
        assert!(!storage
            .deleted
            .lock()
            .unwrap()
            .contains(&source.to_string()));
    }

    pub(crate) async fn a_manifest_that_could_not_be_stored_never_makes_the_row_ready(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // Three parts are stored; the manifest is the fourth put and fails.
        *storage.fail_stores_from.lock().unwrap() = Some(3);
        let env = env(registry.clone(), storage.clone());
        let out = prepare_csv(&env, &request(source, 40)).await.unwrap();
        let (f, row) = failed_row(&registry, source, out).await;
        assert_eq!(f.code, reason::STORAGE);
        assert_eq!(row.manifest_key, None);
        let manifest_key = format!("{}/manifest.json", root_of(source));
        assert!(row.blob_keys.contains(&manifest_key));
        assert_eq!(row.blob_keys.len(), TRACK_AHEAD + 1);
        assert_eq!(storage.keys(), vec![source.to_string()]);
    }

    /// A sleeper that records what it was asked for and fires when told.
    pub(crate) struct Gate {
        /// Durations asked for, other than the progress interval.
        pub asked: std::sync::Mutex<Vec<Duration>>,
        /// Ends the time budget.
        pub fire: tokio::sync::Notify,
        /// Ends one progress interval.
        pub tick: tokio::sync::Notify,
        /// Ends the bound of one terminal step.
        pub step: tokio::sync::Notify,
    }

    pub(crate) fn gated(env: PrepareEnv) -> (PrepareEnv, Arc<Gate>) {
        let gate = Arc::new(Gate {
            asked: std::sync::Mutex::new(Vec::new()),
            fire: tokio::sync::Notify::new(),
            tick: tokio::sync::Notify::new(),
            step: tokio::sync::Notify::new(),
        });
        let g = gate.clone();
        let env = env.with_sleeper(Arc::new(move |d| {
            let g = g.clone();
            Box::pin(async move {
                if d == PROGRESS_INTERVAL {
                    g.tick.notified().await;
                } else if d == TERMINAL_STEP {
                    g.step.notified().await;
                } else {
                    g.asked.lock().unwrap().push(d);
                    g.fire.notified().await;
                }
            })
        }));
        (env, gate)
    }

    pub(crate) async fn the_budget_ends_a_stuck_run_with_the_time_reason_and_removes_its_output(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // The first part is stored; the second put never completes.
        *storage.hang_stores_from.lock().unwrap() = Some(1);
        let (env, gate) =
            gated(env(registry.clone(), storage.clone()).with_budget(Duration::from_secs(300)));
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.hung.notified().await;
        gate.fire.notify_one();
        let out = run.await.unwrap();
        let (f, row) = failed_row(&registry, source, out).await;
        assert_eq!(f.code, reason::TIME);
        // Asked for exactly the budget, once.
        assert_eq!(*gate.asked.lock().unwrap(), vec![Duration::from_secs(300)]);
        // The part that was stored and the one that hung are tracked, then removed.
        for k in part_keys(source, 2) {
            assert!(row.blob_keys.contains(&k), "{k} untracked");
            assert!(storage.deleted.lock().unwrap().contains(&k));
        }
        assert_eq!(storage.keys(), vec![source.to_string()]);
        assert_eq!(row.manifest_key, None);
    }

    pub(crate) async fn a_source_that_never_yields_ends_with_the_time_reason(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        *storage.stall_source.lock().unwrap() = true;
        let (env, gate) = gated(env(registry.clone(), storage.clone()));
        gate.fire.notify_one();
        let out = prepare_csv(&env, &request(source, 40)).await.unwrap();
        let (f, row) = failed_row(&registry, source, out).await;
        assert_eq!(f.code, reason::TIME);
        assert_eq!(
            (f.detail.as_str(), row.blob_keys.len()),
            ("the preparation did not finish within its time budget", 0)
        );
        assert_eq!(storage.keys(), vec![source.to_string()]);
    }

    /// Another job takes the row of `source`.
    async fn take_over(registry: &Arc<dyn PreparationRegistry>, source: &str) {
        registry.delete(source).await.unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let claim = ClaimRequest {
            source_key: source.to_string(),
            source_bytes: 1,
            format_version: FORMAT_VERSION,
            owner: "other".into(),
            lease: lease_for(chrono::Duration::seconds(300)),
            now,
        };
        assert!(registry.claim(claim).await.unwrap().is_some());
    }

    /// Runs a preparation whose second put hangs, lets `during` change the
    /// registry while it hangs, then lets it go on.
    async fn run_hung_at<F, Fut>(
        registry: &Arc<dyn PreparationRegistry>,
        storage: &Arc<PlacedStorage>,
        source: &str,
        hang_at: usize,
        during: F,
    ) -> PrepareOutcome
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ()>,
    {
        *storage.hang_stores_from.lock().unwrap() = Some(hang_at);
        let env = env(registry.clone(), storage.clone());
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.hung.notified().await;
        during().await;
        storage.release_hang.notify_one();
        run.await.unwrap()
    }

    pub(crate) async fn a_job_whose_row_was_deleted_stops_before_its_next_part_and_leaves_no_row(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let out = run_hung_at(&registry, &storage, source, 1, || async {
            registry.delete(source).await.unwrap();
        })
        .await;
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        // The second part went through (it was in flight), the third was never tried.
        assert_eq!(*storage.stores.lock().unwrap(), 2);
        // Nothing is recorded, and what it wrote belongs to nobody: removed.
        assert!(registry.get(source).await.unwrap().is_none());
        assert_eq!(storage.keys(), vec![source.to_string()]);
        for k in part_keys(source, 2) {
            assert!(storage.deleted.lock().unwrap().contains(&k));
        }
    }

    pub(crate) async fn a_job_whose_lease_was_taken_stops_and_leaves_the_new_owners_row_and_objects(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let out = run_hung_at(&registry, &storage, source, 1, || async {
            take_over(&registry, source).await;
        })
        .await;
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        assert_eq!(*storage.stores.lock().unwrap(), 2);
        // The new owner's row is untouched: running, not failed, nothing deleted
        // (the keys are the deterministic ones it writes now).
        let row = registry.get(source).await.unwrap().unwrap();
        assert_eq!(row.status, PrepareStatus::Running);
        assert_eq!(row.lease_owner.as_deref(), Some("other"));
        assert_eq!(row.error_code, None);
        assert!(storage.deleted.lock().unwrap().is_empty());
        assert_eq!(objects_but_source(&storage, source).len(), 2);
    }

    pub(crate) async fn a_completion_that_finds_the_row_gone_is_cancelled_and_never_ready(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // Three parts, then the manifest is the fourth put: the row is deleted
        // while it is being stored, after the ownership check before it passed.
        let out = run_hung_at(&registry, &storage, source, 3, || async {
            registry.delete(source).await.unwrap();
        })
        .await;
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        assert!(registry.get(source).await.unwrap().is_none());
        assert_eq!(storage.keys(), vec![source.to_string()]);
    }

    pub(crate) async fn an_unopenable_source_releases_the_row_without_a_failure(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = Arc::new(PlacedStorage::default());
        let env = env(registry.clone(), storage.clone());
        let out = prepare_csv(&env, &request(source, 40)).await.unwrap();
        assert!(matches!(out, PrepareOutcome::SourceGone), "{out:?}");
        assert!(registry.get(source).await.unwrap().is_none());
        assert_eq!(*storage.stores.lock().unwrap(), 0);
    }

    pub(crate) async fn a_source_deleted_during_a_restart_removes_what_was_written_and_releases_the_row(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let mut first = b"id\n".to_vec();
        for i in 0..10_000 {
            first.extend_from_slice(format!("{i}\n").as_bytes());
        }
        first.extend_from_slice(b"late\n");
        let storage = PlacedStorage::with_source(source, first);
        *storage.remove_source_on_second_open.lock().unwrap() = true;
        let env = env(registry.clone(), storage.clone()).with_writer(WriterConfig {
            max_rows: 2000,
            max_bytes: usize::MAX,
        });
        let out = prepare_csv(&env, &request(source, 60_000)).await.unwrap();
        assert!(matches!(out, PrepareOutcome::SourceGone), "{out:?}");
        assert!(registry.get(source).await.unwrap().is_none());
        // The four parts the first run wrote are gone, and so is the source.
        assert!(storage.keys().is_empty());
        for k in part_keys(source, 4) {
            assert!(storage.deleted.lock().unwrap().contains(&k));
        }
    }

    /// Remembers every report and says when one arrives.
    #[derive(Default)]
    pub(crate) struct Recording {
        pub seen: std::sync::Mutex<Vec<PrepareProgressInfo>>,
        pub arrived: tokio::sync::Notify,
    }

    #[async_trait]
    impl PrepareProgress for Recording {
        async fn report(&self, _key: &str, info: PrepareProgressInfo) {
            self.seen.lock().unwrap().push(info);
            self.arrived.notify_one();
        }
        async fn read(&self, _key: &str) -> Option<PrepareProgressInfo> {
            self.seen.lock().unwrap().last().cloned()
        }
    }

    /// Runs a preparation held at its second put, ticks the progress interval once
    /// and returns the report that tick produced, checking the registry was not
    /// written for it. A report that never comes fails the test instead of hanging it.
    async fn one_tick(
        registry: &Arc<dyn PreparationRegistry>,
        declared: u64,
    ) -> PrepareProgressInfo {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let progress = Arc::new(Recording::default());
        *storage.hang_stores_from.lock().unwrap() = Some(1);
        let (env, gate) =
            gated(env(registry.clone(), storage.clone()).with_progress(progress.clone()));
        let req = request(source, declared);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.hung.notified().await;
        let before = registry.get(source).await.unwrap().unwrap();
        gate.tick.notify_one();
        // The start report, then the tick's.
        let arrived = async {
            while progress.seen.lock().unwrap().len() < 2 {
                progress.arrived.notified().await;
            }
        };
        tokio::time::timeout(Duration::from_secs(30), arrived)
            .await
            .expect("the tick produced no report");
        let tick = progress.seen.lock().unwrap()[1].clone();
        // The registry was not written for it.
        assert_eq!(registry.get(source).await.unwrap().unwrap(), before);
        storage.release_hang.notify_one();
        assert!(matches!(run.await.unwrap(), PrepareOutcome::Ready(_)));
        tick
    }

    pub(crate) async fn progress_is_reported_every_interval_to_the_port_and_never_written_to_the_registry(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        // The whole file was read (the sample reads it before any part).
        let read = five_rows().len() as u64;
        let tick = one_tick(&registry, 40).await;
        assert_eq!(tick.state, ProgressState::Running);
        assert_eq!((tick.done, tick.total), (read, Some(40)));
        // A declared size smaller than what was read never makes done pass total.
        // Done stays under the total until the preparation is ready, so a bar
        // never shows complete while the output is still being written.
        let tick = one_tick(&registry, read).await;
        assert_eq!((tick.done, tick.total), (read - 1, Some(read)));
        // A declared size smaller than what was read is clamped the same way.
        let tick = one_tick(&registry, 20).await;
        assert_eq!((tick.done, tick.total), (19, Some(20)));
    }

    pub(crate) async fn a_finished_preparation_reports_its_final_state_to_the_port(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let progress = Arc::new(Recording::default());
        // Ready.
        let ready = fresh_source();
        let storage = PlacedStorage::with_source(&ready, five_rows());
        let env1 = env(registry.clone(), storage).with_progress(progress.clone());
        assert!(matches!(
            prepare_csv(&env1, &request(&ready, 40)).await.unwrap(),
            PrepareOutcome::Ready(_)
        ));
        assert_eq!(
            progress.read("").await,
            Some(PrepareProgressInfo {
                state: ProgressState::Ready,
                done: 40,
                total: Some(40)
            })
        );
        // Failed.
        let source = fresh_source();
        let storage = PlacedStorage::with_source(&source, Vec::new());
        let env2 = env(registry.clone(), storage).with_progress(progress.clone());
        assert!(matches!(
            prepare_csv(&env2, &request(&source, 0)).await.unwrap(),
            PrepareOutcome::Failed(_)
        ));
        assert_eq!(
            progress.read("").await.unwrap().state,
            ProgressState::Failed
        );
        // Cancelled: the source does not exist.
        let source = fresh_source();
        let env3 = env(registry.clone(), Arc::new(PlacedStorage::default()))
            .with_progress(progress.clone());
        assert!(matches!(
            prepare_csv(&env3, &request(&source, 40)).await.unwrap(),
            PrepareOutcome::SourceGone
        ));
        assert_eq!(
            progress.read("").await.unwrap().state,
            ProgressState::Cancelled
        );
        // Not claimed: nothing is reported for it.
        let reports = progress.seen.lock().unwrap().len();
        assert!(matches!(
            prepare_csv(&env1, &request(&ready, 40)).await.unwrap(),
            PrepareOutcome::NotClaimed
        ));
        assert_eq!(progress.seen.lock().unwrap().len(), reports);
    }

    pub(crate) async fn the_inline_trigger_runs_a_csv_request_through_the_driver(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::ports::{InlineTrigger, PrepareTrigger};
        let progress = Arc::new(Recording::default());
        for mime in ["text/csv", "Text/CSV; charset=utf-8"] {
            let source = fresh_source();
            let storage = PlacedStorage::with_source(&source, five_rows());
            let env = env(registry.clone(), storage).with_progress(progress.clone());
            let config = PrepareConfig::from_switch(Some("on"));
            let runner = Arc::new(CsvPrepareRunner::new(Arc::new(env), &config));
            let trigger = InlineTrigger::new(runner);
            let mut req = request(&source, 40);
            req.mime_type = mime.to_string();
            let before = progress.seen.lock().unwrap().len();
            trigger.request(req).await.unwrap();
            // The request runs detached: wait for its final report, with a bound.
            let done = async {
                loop {
                    let seen = progress.seen.lock().unwrap().len();
                    if seen > before
                        && progress.read("").await.unwrap().state == ProgressState::Ready
                    {
                        break;
                    }
                    progress.arrived.notified().await;
                }
            };
            tokio::time::timeout(Duration::from_secs(30), done)
                .await
                .expect("the request never finished");
            let row = registry.get(&source).await.unwrap().unwrap();
            assert_eq!(row.status, PrepareStatus::Ready, "{mime}");
        }
    }

    pub(crate) async fn with_the_switch_off_or_another_mime_the_runner_touches_nothing(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let xlsx = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
        for (switch, mime) in [
            ("off", "text/csv"),
            ("on", xlsx),
            ("on", "text/csvx"),
            ("on", ""),
        ] {
            let source = fresh_source();
            let storage = PlacedStorage::with_source(&source, five_rows());
            let env = env(registry.clone(), storage.clone());
            let config = PrepareConfig::from_switch(Some(switch));
            let runner = CsvPrepareRunner::new(Arc::new(env), &config);
            let mut req = request(&source, 40);
            req.mime_type = mime.to_string();
            runner.run(req).await;
            assert!(
                registry.get(&source).await.unwrap().is_none(),
                "{switch} {mime}"
            );
            assert_eq!(*storage.opens.lock().unwrap(), 0);
            assert_eq!(*storage.stores.lock().unwrap(), 0);
        }
    }

    pub(crate) async fn dropping_the_prepare_future_mid_run_leaves_every_stored_object_tracked_in_the_row(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // The second put never completes; the whole preparation is then dropped.
        *storage.hang_stores_from.lock().unwrap() = Some(1);
        let env = env(registry.clone(), storage.clone());
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await });
        storage.hung.notified().await;
        run.abort();
        let _ = run.await;
        let row = registry.get(source).await.unwrap().unwrap();
        assert_eq!(row.status, PrepareStatus::Running);
        // Every object in storage is listed, and so are the one in flight and
        // the manifest that was never reached.
        let manifest = format!("{}/manifest.json", root_of(source));
        for k in objects_but_source(&storage, source) {
            assert!(row.blob_keys.contains(&k), "{k} stored but not tracked");
        }
        for k in part_keys(source, 2).into_iter().chain([manifest]) {
            assert!(row.blob_keys.contains(&k), "{k} not tracked ahead");
        }
    }

    pub(crate) async fn a_part_beyond_the_first_batch_is_tracked_before_its_put_a_batch_ahead(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        // Forty-two rows of two per part: twenty-one parts.
        let mut body = b"id\n".to_vec();
        for i in 0..42 {
            body.extend_from_slice(format!("{i}\n").as_bytes());
        }
        let storage = PlacedStorage::with_source(source, body);
        // The seventeenth put (part 16) is the first one past the first batch.
        *storage.hang_stores_from.lock().unwrap() = Some(TRACK_AHEAD);
        let env = env(registry.clone(), storage.clone());
        let req = request(source, 100);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await });
        storage.hung.notified().await;
        run.abort();
        let _ = run.await;
        let row = registry.get(source).await.unwrap().unwrap();
        // Part 16 is not stored yet, and it is listed with the whole second batch.
        assert_eq!(objects_but_source(&storage, source).len(), TRACK_AHEAD);
        for k in part_keys(source, 2 * TRACK_AHEAD) {
            assert!(row.blob_keys.contains(&k), "{k} not tracked ahead");
        }
        assert_eq!(row.blob_keys.len(), 2 * TRACK_AHEAD + 1);
    }

    pub(crate) async fn a_row_that_vanishes_before_the_first_tracking_write_stops_the_job_before_any_object(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::registry_faults::FaultyRegistry;
        use std::sync::atomic::Ordering::SeqCst;
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let faulty = FaultyRegistry::new(registry.clone());
        faulty.faults.delete_before_track.store(true, SeqCst);
        let env = env(faulty, storage.clone());
        let out = prepare_csv(&env, &request(source, 40)).await.unwrap();
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        assert_eq!(*storage.stores.lock().unwrap(), 0);
        assert!(registry.get(source).await.unwrap().is_none());
    }

    pub(crate) async fn a_row_deleted_before_the_budget_ends_the_run_still_gets_its_objects_deleted(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        *storage.hang_stores_from.lock().unwrap() = Some(1);
        let (env, gate) = gated(env(registry.clone(), storage.clone()));
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.hung.notified().await;
        registry.delete(source).await.unwrap();
        gate.fire.notify_one();
        let out = run.await.unwrap();
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        // The first part was stored; with no row it belongs to nobody.
        assert_eq!(storage.keys(), vec![source.to_string()]);
        assert!(registry.get(source).await.unwrap().is_none());
    }

    pub(crate) async fn a_row_deleted_before_a_storage_failure_still_gets_its_objects_deleted(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // The second put is held; the row goes; the put then fails.
        *storage.hang_stores_from.lock().unwrap() = Some(1);
        *storage.fail_stores_from.lock().unwrap() = Some(1);
        let out = run_hung_at(&registry, &storage, source, 1, || async {
            registry.delete(source).await.unwrap();
        })
        .await;
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        assert_eq!(storage.keys(), vec![source.to_string()]);
        assert!(registry.get(source).await.unwrap().is_none());
    }

    /// A late word forces a second read of the file; `during` runs while that
    /// second open is stopped.
    async fn run_paused_at_the_second_open<F, Fut>(
        registry: &Arc<dyn PreparationRegistry>,
        storage: &Arc<PlacedStorage>,
        source: &str,
        during: F,
    ) -> PrepareOutcome
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ()>,
    {
        *storage.pause_second_open.lock().unwrap() = true;
        let env = env(registry.clone(), storage.clone()).with_writer(WriterConfig {
            max_rows: 2000,
            max_bytes: usize::MAX,
        });
        let req = request(source, 60_000);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.second_open_reached.notified().await;
        during().await;
        storage.second_open_go.notify_one();
        run.await.unwrap()
    }

    fn late_word_file() -> Vec<u8> {
        let mut body = b"id\n".to_vec();
        for i in 0..10_000 {
            body.extend_from_slice(format!("{i}\n").as_bytes());
        }
        body.extend_from_slice(b"late\n");
        body
    }

    pub(crate) async fn a_row_deleted_before_a_source_read_failure_still_gets_its_objects_deleted(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, late_word_file());
        *storage.fail_second_open.lock().unwrap() = true;
        let out = run_paused_at_the_second_open(&registry, &storage, source, || async {
            registry.delete(source).await.unwrap();
        })
        .await;
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        // The four parts the first run wrote are removed.
        assert_eq!(storage.keys(), vec![source.to_string()]);
        assert!(registry.get(source).await.unwrap().is_none());
    }

    pub(crate) async fn a_lease_taken_before_the_source_turns_out_missing_deletes_nothing_and_releases_nothing(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, late_word_file());
        *storage.remove_source_on_second_open.lock().unwrap() = true;
        let out = run_paused_at_the_second_open(&registry, &storage, source, || async {
            take_over(&registry, source).await;
        })
        .await;
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        // The new owner's row stays, and the keys it writes are not touched.
        let row = registry.get(source).await.unwrap().unwrap();
        assert_eq!(row.lease_owner.as_deref(), Some("other"));
        assert!(storage.deleted.lock().unwrap().is_empty());
        assert_eq!(objects_but_source(&storage, source).len(), 4);
    }

    pub(crate) async fn a_row_taken_before_the_budget_ends_the_run_keeps_the_new_owners_objects(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        *storage.hang_stores_from.lock().unwrap() = Some(1);
        let (env, gate) = gated(env(registry.clone(), storage.clone()));
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.hung.notified().await;
        take_over(&registry, source).await;
        gate.fire.notify_one();
        let out = run.await.unwrap();
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        assert!(storage.deleted.lock().unwrap().is_empty());
        let row = registry.get(source).await.unwrap().unwrap();
        assert_eq!(row.lease_owner.as_deref(), Some("other"));
        assert_eq!(row.error_code, None);
    }

    /// Waits for a preparation that must end by its own bounds; a bound that does
    /// not hold fails the test instead of hanging it.
    async fn joined(run: tokio::task::JoinHandle<PrepareOutcome>) -> PrepareOutcome {
        tokio::time::timeout(Duration::from_secs(30), run)
            .await
            .expect("the job outlived its bound")
            .unwrap()
    }

    pub(crate) async fn a_manifest_put_that_never_completes_is_ended_by_the_same_budget(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // Three parts are stored; the manifest, the fourth put, never completes.
        *storage.hang_stores_from.lock().unwrap() = Some(3);
        let (env, gate) = gated(env(registry.clone(), storage.clone()));
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.hung.notified().await;
        gate.fire.notify_one();
        let out = joined(run).await;
        let (f, row) = failed_row(&registry, source, out).await;
        assert_eq!(f.code, reason::TIME);
        // One budget for the whole job: it was asked for once.
        assert_eq!(*gate.asked.lock().unwrap(), vec![PREP_TIMEOUT]);
        assert_eq!(row.manifest_key, None);
        assert_eq!(storage.keys(), vec![source.to_string()]);
    }

    pub(crate) async fn a_completion_that_never_returns_is_ended_by_its_own_bound_and_deletes_nothing(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::registry_faults::FaultyRegistry;
        use std::sync::atomic::Ordering::SeqCst;
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let faulty = FaultyRegistry::new(registry.clone());
        faulty.faults.hang_complete.store(true, SeqCst);
        let (env, gate) = gated(env(faulty.clone(), storage.clone()));
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await });
        faulty.faults.complete_reached.notified().await;
        gate.step.notify_one();
        let out = tokio::time::timeout(Duration::from_secs(30), run)
            .await
            .expect("the job outlived its bound")
            .unwrap();
        assert!(out.is_err(), "{out:?}");
        // Whether it was applied is unknown: nothing deleted, everything listed.
        assert!(storage.deleted.lock().unwrap().is_empty());
        let row = registry.get(source).await.unwrap().unwrap();
        for k in objects_but_source(&storage, source) {
            assert!(row.blob_keys.contains(&k), "{k} not tracked");
        }
    }

    pub(crate) async fn a_delete_that_never_returns_does_not_hold_the_failure_back(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // The manifest cannot be stored, and the cleanup that follows never returns.
        *storage.fail_stores_from.lock().unwrap() = Some(3);
        *storage.hang_delete.lock().unwrap() = true;
        let (env, gate) = gated(env(registry.clone(), storage.clone()));
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.delete_reached.notified().await;
        gate.step.notify_one();
        let out = joined(run).await;
        let (f, _row) = failed_row(&registry, source, out).await;
        assert_eq!(f.code, reason::STORAGE);
    }

    pub(crate) async fn the_lease_is_the_budget_plus_the_grace_and_outlasts_the_bounded_job(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        *storage.hang_stores_from.lock().unwrap() = Some(1);
        let budget = Duration::from_secs(300);
        let env = env(registry.clone(), storage.clone()).with_budget(budget);
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await });
        storage.hung.notified().await;
        let row = registry.get(source).await.unwrap().unwrap();
        run.abort();
        let _ = run.await;
        let claimed = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        // Budget plus the registry's grace, not the budget alone.
        assert_eq!(
            row.lease_until,
            Some(
                claimed
                    + chrono::Duration::seconds(300)
                    + crate::tabular_prepare::registry::LEASE_GRACE
            )
        );
        // The job after the budget is at most MAX_TERMINAL_STEPS bounded steps.
        let after = TERMINAL_STEP * MAX_TERMINAL_STEPS;
        assert!(
            chrono::Duration::from_std(after).unwrap()
                < crate::tabular_prepare::registry::LEASE_GRACE
        );
    }

    pub(crate) async fn a_cleanup_of_a_deleted_source_that_does_not_answer_keeps_the_row_and_its_keys(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, late_word_file());
        *storage.remove_source_on_second_open.lock().unwrap() = true;
        *storage.hang_delete.lock().unwrap() = true;
        let (env, gate) = gated(
            env(registry.clone(), storage.clone()).with_writer(WriterConfig {
                max_rows: 2000,
                max_bytes: usize::MAX,
            }),
        );
        let req = request(source, 60_000);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        let steps = async {
            // The delete of the deleted source's objects, then the one of the failure.
            for _ in 0..2 {
                storage.delete_reached.notified().await;
                gate.step.notify_one();
            }
        };
        tokio::time::timeout(Duration::from_secs(30), steps)
            .await
            .expect("the job did not go on after the delete timed out");
        let out = joined(run).await;
        // The objects could not be removed, so the row stays, failed, listing them.
        let (f, row) = failed_row(&registry, source, out).await;
        assert_eq!(f.code, reason::STORAGE);
        for k in part_keys(source, 4) {
            assert!(row.blob_keys.contains(&k), "{k} not tracked");
        }
    }

    /// Collects what the tracing events of this thread write.
    #[derive(Clone, Default)]
    struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Captured;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    pub(crate) async fn logs_carry_a_fixed_sentence_a_kind_and_an_opaque_id_never_a_key_or_adapter_text(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::registry_faults::FaultyRegistry;
        use std::sync::atomic::Ordering::SeqCst;
        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(captured.clone())
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let config = PrepareConfig::from_switch(Some("on"));
        let mut ids = Vec::new();
        // 1. The registry cannot record the outcome (its text names a key).
        let source = fresh_source();
        let storage = PlacedStorage::with_source(&source, five_rows());
        let faulty = FaultyRegistry::new(registry.clone());
        faulty.faults.fail_complete.store(true, SeqCst);
        let runner = CsvPrepareRunner::new(Arc::new(env(faulty, storage)), &config);
        runner.run(request(&source, 40)).await;
        ids.push(opaque_id(&source));
        // 2. The failure is recorded and the cleanup that follows is refused with
        // text that names a key.
        let source = fresh_source();
        let storage = PlacedStorage::with_source(&source, five_rows());
        *storage.fail_stores_from.lock().unwrap() = Some(1);
        *storage.fail_delete.lock().unwrap() = true;
        let runner = CsvPrepareRunner::new(Arc::new(env(registry.clone(), storage)), &config);
        runner.run(request(&source, 40)).await;
        ids.push(opaque_id(&source));
        // 3. A request that is not a CSV.
        let source = fresh_source();
        let storage = PlacedStorage::with_source(&source, five_rows());
        let runner = CsvPrepareRunner::new(Arc::new(env(registry.clone(), storage)), &config);
        let mut req = request(&source, 40);
        req.mime_type = "application/pdf".into();
        runner.run(req).await;
        ids.push(opaque_id(&source));
        // 4. No derived root.
        let source = fresh_source();
        let storage = PlacedStorage::with_source(&source, five_rows());
        *storage.no_root.lock().unwrap() = true;
        let runner = CsvPrepareRunner::new(Arc::new(env(registry.clone(), storage)), &config);
        runner.run(request(&source, 40)).await;
        ids.push(opaque_id(&source));

        let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        for forbidden in ["secret", "chat-attachments", ".csv", "prepared/"] {
            assert!(
                !text.contains(forbidden),
                "{forbidden:?} in the log:\n{text}"
            );
        }
        // The identifier is there, opaque and stable, and each line has a kind or a reason.
        for id in &ids {
            assert_eq!(id.len(), 12);
            // Hexadecimal digits of a digest, not a piece of the key.
            assert!(id.bytes().all(|b| b.is_ascii_hexdigit()), "{id}");
            assert!(!"chat-attachments/u/s/".contains(id.as_str()));
            assert!(text.contains(id.as_str()), "{id} missing:\n{text}");
        }
        assert!(
            text.contains("kind=\"registry\"") && text.contains("kind=\"upload_failed\""),
            "{text}"
        );
    }

    pub(crate) async fn a_source_key_that_cannot_be_a_key_is_refused_without_a_row(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let storage = Arc::new(PlacedStorage::default());
        let env = env(registry, storage.clone());
        let out = prepare_csv(&env, &request("chat-attachments/../x.csv", 40))
            .await
            .unwrap();
        assert!(matches!(
            out,
            PrepareOutcome::Refused(PrepareStartError::InvalidSourceKey)
        ));
        assert_eq!(*storage.opens.lock().unwrap(), 0);
    }

    pub(crate) async fn a_registry_error_at_completion_leaves_the_stored_objects_tracked_and_not_deleted(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::registry_faults::FaultyRegistry;
        use std::sync::atomic::Ordering::SeqCst;
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let faulty = FaultyRegistry::new(registry.clone());
        faulty.faults.fail_complete.store(true, SeqCst);
        let env = env(faulty, storage.clone());
        let err = prepare_csv(&env, &request(source, 40)).await.unwrap_err();
        let _ = err;
        // Whether `complete` applied is unknown, so nothing is deleted; every
        // object is listed in the row instead.
        let row = registry.get(source).await.unwrap().unwrap();
        assert!(storage.deleted.lock().unwrap().is_empty());
        let stored = objects_but_source(&storage, source);
        assert_eq!(stored.len(), 4);
        for k in stored {
            assert!(row.blob_keys.contains(&k), "{k} stored but not tracked");
        }
    }

    pub(crate) async fn a_registry_error_recording_a_failure_still_deletes_the_objects_that_are_tracked(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::registry_faults::FaultyRegistry;
        use std::sync::atomic::Ordering::SeqCst;
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // The manifest cannot be stored, and the failure cannot be recorded.
        *storage.fail_stores_from.lock().unwrap() = Some(3);
        let faulty = FaultyRegistry::new(registry.clone());
        faulty.faults.fail_fail.store(true, SeqCst);
        let env = env(faulty, storage.clone());
        assert!(prepare_csv(&env, &request(source, 40)).await.is_err());
        // The failure is safe to act on whether or not it was recorded.
        assert_eq!(storage.keys(), vec![source.to_string()]);
        let row = registry.get(source).await.unwrap().unwrap();
        assert_eq!(row.status, PrepareStatus::Running);
        assert_eq!(row.blob_keys.len(), TRACK_AHEAD + 1);
    }

    pub(crate) async fn a_restart_with_fewer_parts_leaves_stale_keys_the_manifest_never_names(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        // Ten thousand integers and a late word: the first run writes four
        // parts (8,000 rows; the rest of its batch was never flushed), then the column becomes text and the file is read again; it
        // changed in between, and the second run writes one part.
        let mut first = b"id\n".to_vec();
        for i in 0..10_000 {
            first.extend_from_slice(format!("{i}\n").as_bytes());
        }
        first.extend_from_slice(b"late\n");
        let storage = PlacedStorage::with_source(source, first);
        *storage.swap_on_second_open.lock().unwrap() = Some(Bytes::from_static(b"id\nx\ny\n"));
        let env = env(registry.clone(), storage.clone()).with_writer(WriterConfig {
            max_rows: 2000,
            max_bytes: usize::MAX,
        });
        let PrepareOutcome::Ready(table) =
            prepare_csv(&env, &request(source, 60_000)).await.unwrap()
        else {
            panic!("expected a ready table");
        };
        assert_eq!(table.manifest.tables[0].parts, 1);
        assert_eq!(table.stale_keys, part_keys(source, 4)[1..].to_vec());
        let row = registry.get(source).await.unwrap().unwrap();
        // Tracked for cleanup, still stored, and not referenced.
        for k in &table.stale_keys {
            assert!(row.blob_keys.contains(k));
            assert!(storage.objects.lock().unwrap().contains_key(k));
        }
        for k in part_keys(source, 4) {
            assert!(row.blob_keys.contains(&k));
        }
        assert!(row
            .blob_keys
            .contains(&format!("{}/manifest.json", root_of(source))));
    }
}

#[cfg(test)]
mod registry_tests {
    use super::cases::*;
    use crate::tabular_prepare::registry::PreparationRegistry;
    use crate::tabular_prepare::sqlite_registry::SqlitePreparationRegistry;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::sync::Arc;
    use std::time::Duration;

    async fn sqlite() -> (Arc<dyn PreparationRegistry>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new()
            .filename(dir.path().join("registry.db"))
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::migrate!("migrations/sqlite")
            .run(&pool)
            .await
            .unwrap();
        (
            Arc::new(SqlitePreparationRegistry::from_pool(Arc::new(pool))),
            dir,
        )
    }

    macro_rules! sqlite_case {
        ($name:ident, $case:ident) => {
            #[tokio::test]
            async fn $name() {
                let (registry, _dir) = sqlite().await;
                $case(registry).await;
            }
        };
    }

    sqlite_case!(
        tabular_prepare_a_prepared_table_is_ready_with_the_manifest_stored_last,
        a_prepared_table_is_ready_with_the_manifest_stored_last
    );
    sqlite_case!(
        tabular_prepare_without_a_derived_root_nothing_is_written_not_even_a_row,
        without_a_derived_root_nothing_is_written_not_even_a_row
    );
    sqlite_case!(
        tabular_prepare_the_result_carries_what_the_conversion_reports,
        the_result_carries_what_the_conversion_reports
    );
    sqlite_case!(
        tabular_prepare_a_second_preparation_is_not_claimed_and_stores_nothing,
        a_second_preparation_is_not_claimed_and_stores_nothing
    );
    sqlite_case!(
        tabular_prepare_a_file_that_is_not_a_csv_fails_with_a_fixed_text_and_no_cell,
        a_file_that_is_not_a_csv_fails_with_a_fixed_text_and_no_cell
    );
    sqlite_case!(
        tabular_prepare_a_storage_failure_midway_tracks_every_key_tried_and_deletes_them,
        a_storage_failure_midway_tracks_every_key_tried_and_deletes_them
    );
    sqlite_case!(
        tabular_prepare_a_manifest_that_could_not_be_stored_never_makes_the_row_ready,
        a_manifest_that_could_not_be_stored_never_makes_the_row_ready
    );
    sqlite_case!(
        tabular_prepare_a_restart_with_fewer_parts_leaves_stale_keys_the_manifest_never_names,
        a_restart_with_fewer_parts_leaves_stale_keys_the_manifest_never_names
    );
    sqlite_case!(
        tabular_prepare_the_budget_ends_a_stuck_run_with_the_time_reason_and_removes_its_output,
        the_budget_ends_a_stuck_run_with_the_time_reason_and_removes_its_output
    );
    sqlite_case!(
        tabular_prepare_a_source_that_never_yields_ends_with_the_time_reason,
        a_source_that_never_yields_ends_with_the_time_reason
    );
    sqlite_case!(
        tabular_prepare_a_job_whose_row_was_deleted_stops_before_its_next_part_and_leaves_no_row,
        a_job_whose_row_was_deleted_stops_before_its_next_part_and_leaves_no_row
    );
    sqlite_case!(
        tabular_prepare_a_job_whose_lease_was_taken_stops_and_leaves_the_new_owners_row_and_objects,
        a_job_whose_lease_was_taken_stops_and_leaves_the_new_owners_row_and_objects
    );
    sqlite_case!(
        tabular_prepare_a_completion_that_finds_the_row_gone_is_cancelled_and_never_ready,
        a_completion_that_finds_the_row_gone_is_cancelled_and_never_ready
    );
    sqlite_case!(
        tabular_prepare_an_unopenable_source_releases_the_row_without_a_failure,
        an_unopenable_source_releases_the_row_without_a_failure
    );
    sqlite_case!(
        tabular_prepare_a_source_deleted_during_a_restart_removes_what_was_written_and_releases_the_row,
        a_source_deleted_during_a_restart_removes_what_was_written_and_releases_the_row
    );
    sqlite_case!(
        tabular_prepare_progress_is_reported_every_interval_to_the_port_and_never_written_to_the_registry,
        progress_is_reported_every_interval_to_the_port_and_never_written_to_the_registry
    );
    sqlite_case!(
        tabular_prepare_a_finished_preparation_reports_its_final_state_to_the_port,
        a_finished_preparation_reports_its_final_state_to_the_port
    );
    sqlite_case!(
        tabular_prepare_the_inline_trigger_runs_a_csv_request_through_the_driver,
        the_inline_trigger_runs_a_csv_request_through_the_driver
    );
    sqlite_case!(
        tabular_prepare_with_the_switch_off_or_another_mime_the_runner_touches_nothing,
        with_the_switch_off_or_another_mime_the_runner_touches_nothing
    );
    sqlite_case!(
        tabular_prepare_dropping_the_prepare_future_mid_run_leaves_every_stored_object_tracked_in_the_row,
        dropping_the_prepare_future_mid_run_leaves_every_stored_object_tracked_in_the_row
    );
    sqlite_case!(
        tabular_prepare_a_registry_error_at_completion_leaves_the_stored_objects_tracked_and_not_deleted,
        a_registry_error_at_completion_leaves_the_stored_objects_tracked_and_not_deleted
    );
    sqlite_case!(
        tabular_prepare_a_registry_error_recording_a_failure_still_deletes_the_objects_that_are_tracked,
        a_registry_error_recording_a_failure_still_deletes_the_objects_that_are_tracked
    );
    sqlite_case!(
        tabular_prepare_a_part_beyond_the_first_batch_is_tracked_before_its_put_a_batch_ahead,
        a_part_beyond_the_first_batch_is_tracked_before_its_put_a_batch_ahead
    );
    sqlite_case!(
        tabular_prepare_a_row_that_vanishes_before_the_first_tracking_write_stops_the_job_before_any_object,
        a_row_that_vanishes_before_the_first_tracking_write_stops_the_job_before_any_object
    );
    sqlite_case!(
        tabular_prepare_a_row_deleted_before_the_budget_ends_the_run_still_gets_its_objects_deleted,
        a_row_deleted_before_the_budget_ends_the_run_still_gets_its_objects_deleted
    );
    sqlite_case!(
        tabular_prepare_a_row_deleted_before_a_storage_failure_still_gets_its_objects_deleted,
        a_row_deleted_before_a_storage_failure_still_gets_its_objects_deleted
    );
    sqlite_case!(
        tabular_prepare_a_row_deleted_before_a_source_read_failure_still_gets_its_objects_deleted,
        a_row_deleted_before_a_source_read_failure_still_gets_its_objects_deleted
    );
    sqlite_case!(
        tabular_prepare_a_lease_taken_before_the_source_turns_out_missing_deletes_nothing_and_releases_nothing,
        a_lease_taken_before_the_source_turns_out_missing_deletes_nothing_and_releases_nothing
    );
    sqlite_case!(
        tabular_prepare_a_row_taken_before_the_budget_ends_the_run_keeps_the_new_owners_objects,
        a_row_taken_before_the_budget_ends_the_run_keeps_the_new_owners_objects
    );
    sqlite_case!(
        tabular_prepare_a_manifest_put_that_never_completes_is_ended_by_the_same_budget,
        a_manifest_put_that_never_completes_is_ended_by_the_same_budget
    );
    sqlite_case!(
        tabular_prepare_a_completion_that_never_returns_is_ended_by_its_own_bound_and_deletes_nothing,
        a_completion_that_never_returns_is_ended_by_its_own_bound_and_deletes_nothing
    );
    sqlite_case!(
        tabular_prepare_a_delete_that_never_returns_does_not_hold_the_failure_back,
        a_delete_that_never_returns_does_not_hold_the_failure_back
    );
    sqlite_case!(
        tabular_prepare_the_lease_is_the_budget_plus_the_grace_and_outlasts_the_bounded_job,
        the_lease_is_the_budget_plus_the_grace_and_outlasts_the_bounded_job
    );
    sqlite_case!(
        tabular_prepare_a_cleanup_of_a_deleted_source_that_does_not_answer_keeps_the_row_and_its_keys,
        a_cleanup_of_a_deleted_source_that_does_not_answer_keeps_the_row_and_its_keys
    );
    sqlite_case!(
        tabular_prepare_logs_carry_a_fixed_sentence_a_kind_and_an_opaque_id_never_a_key_or_adapter_text,
        logs_carry_a_fixed_sentence_a_kind_and_an_opaque_id_never_a_key_or_adapter_text
    );
    sqlite_case!(
        tabular_prepare_a_source_key_that_cannot_be_a_key_is_refused_without_a_row,
        a_source_key_that_cannot_be_a_key_is_refused_without_a_row
    );
}
