//! Is the prepared copy of this file one the engine may stage and run over?
//!
//! The answer comes from the registry, not from the model and not from the
//! storage alone: the row must be this source's, `ready`, in the layout this
//! reader reads, and the manifest in storage must be the one the row recorded.
//! Nothing is read from storage before the row has passed, and a prepared copy
//! that is too large for one call is refused from the row's recorded size,
//! before any part is opened.

use super::refusal::{FailureReason, Invalid, RunRefusal, Unavailable};
use crate::storage::domain::OutputStorageRepository;
use crate::tabular_prepare::manifest::{
    part_path, Manifest, TableInfo, MANIFEST_MAX_BYTES, MANIFEST_PATH,
};
use crate::tabular_prepare::registry::{
    PreparationRegistry, PrepareStatus, PreparedRow, FORMAT_VERSION, MAX_ATTEMPTS,
};
use futures::StreamExt;
use std::collections::HashSet;

/// Most bytes of prepared parts one call may stage (`D_max`, 1 GiB). An estimate
/// until the instance is measured (spike item 5); the executor's staged-volume
/// budget bounds the output side separately.
pub const DATA_MAX_BYTES: u64 = 1024 * 1024 * 1024;

/// A prepared copy the registry vouched for. Holds no data: only the verified
/// manifest and where the parts live.
#[derive(Debug, Clone)]
pub struct PreparedTables {
    root: String,
    manifest_key: String,
    manifest: Manifest,
    prepared_bytes: u64,
}

impl PreparedTables {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn tables(&self) -> &[TableInfo] {
        &self.manifest.tables
    }

    /// What the registry recorded for the size of the live parts and manifest.
    pub fn prepared_bytes(&self) -> u64 {
        self.prepared_bytes
    }

    pub fn manifest_key(&self) -> &str {
        &self.manifest_key
    }

    /// Storage key of a part: the derived root and the canonical relative path.
    pub fn part_key(&self, table: usize, part: usize) -> Result<String, RunRefusal> {
        let rel = part_path(table, part).map_err(|_| RunRefusal::Invalid(Invalid::Manifest))?;
        Ok(format!("{}/{rel}", self.root))
    }

    /// Indexes of the tables to stage: all of them for no names, else the named
    /// ones (matched ignoring case), each once, in manifest order.
    pub fn select(&self, names: &[String]) -> Result<Vec<usize>, RunRefusal> {
        if names.is_empty() {
            return Ok((0..self.manifest.tables.len()).collect());
        }
        let mut chosen = HashSet::new();
        for name in names {
            let wanted = name.to_lowercase();
            match self
                .manifest
                .tables
                .iter()
                .position(|t| t.name.to_lowercase() == wanted)
            {
                Some(i) => {
                    chosen.insert(i);
                }
                None => return Err(RunRefusal::NoSuchTable { name: name.clone() }),
            }
        }
        let mut chosen: Vec<usize> = chosen.into_iter().collect();
        chosen.sort_unstable();
        Ok(chosen)
    }
}

/// The registry's answer for `source_key`, judged: a ready row of this source
/// in this layout, or why not. Pure: no I/O.
pub fn judge_row(row: Option<PreparedRow>, source_key: &str) -> Result<PreparedRow, RunRefusal> {
    let Some(row) = row else {
        return Err(RunRefusal::NotPrepared);
    };
    match row.status {
        PrepareStatus::Running => Err(RunRefusal::StillPreparing { percent: None }),
        PrepareStatus::Deleting => Err(RunRefusal::BeingRemoved),
        PrepareStatus::Failed => Err(RunRefusal::PreparationFailed {
            reason: FailureReason::from_code(row.error_code.as_deref().unwrap_or("")),
            final_failure: row.attempts >= MAX_ATTEMPTS,
        }),
        PrepareStatus::Ready => {
            let ours = row.source_storage_key == source_key;
            let current = row.format_version == FORMAT_VERSION;
            if ours && current && row.manifest_key.is_some() {
                Ok(row)
            } else {
                Err(RunRefusal::Invalid(Invalid::Record))
            }
        }
    }
}

/// Reads one object that must fit in `cap` bytes, never holding more than the
/// cap: the declared size is checked first, and the stream is cut off at the cap
/// whatever it declared. `too_big` is the refusal for an object over the cap.
async fn read_capped(
    storage: &dyn OutputStorageRepository,
    key: &str,
    cap: usize,
    too_big: RunRefusal,
) -> Result<Vec<u8>, RunRefusal> {
    let mut stream = storage
        .read_stream(key)
        .await
        .map_err(|_| RunRefusal::Storage)?;
    if stream.size_bytes > cap as u64 {
        return Err(too_big);
    }
    let mut out = Vec::with_capacity((stream.size_bytes as usize).min(cap));
    while let Some(chunk) = stream.stream.next().await {
        let chunk = chunk.map_err(|_| RunRefusal::Storage)?;
        if out.len() + chunk.len() > cap {
            return Err(too_big);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// The manifest's table list is the row's, byte for byte. A list that cannot be
/// serialised, or a row without one, is a mismatch, never a match.
fn same_table_list(manifest: &Manifest, recorded: Option<&str>) -> bool {
    matches!((manifest.tables_json(), recorded), (Ok(ours), Some(theirs)) if ours == theirs)
}

/// Verifies the prepared copy of `source_key` through the registry, then reads
/// and checks its manifest. Returns the plan, or a refusal the model can read.
///
/// The key must come from the session's own catalog row, never from the model.
pub async fn verify_prepared(
    registry: &dyn PreparationRegistry,
    storage: &dyn OutputStorageRepository,
    source_key: &str,
) -> Result<PreparedTables, RunRefusal> {
    let row = registry
        .get(source_key)
        .await
        .map_err(|_| RunRefusal::Unavailable(Unavailable::Registry))?;
    let row = judge_row(row, source_key)?;

    let root = storage
        .derived_root(source_key)
        .ok_or(RunRefusal::Invalid(Invalid::NoRoot))?;
    let manifest_key = format!("{root}/{MANIFEST_PATH}");
    if row.manifest_key.as_deref() != Some(manifest_key.as_str())
        || !row.blob_keys.contains(&manifest_key)
    {
        return Err(RunRefusal::Invalid(Invalid::Record));
    }
    let prepared_bytes = u64::try_from(row.prepared_bytes.unwrap_or(-1))
        .map_err(|_| RunRefusal::Invalid(Invalid::Record))?;
    // The call's data limit is applied at staging, to the tables actually chosen
    // and on the sizes their parts declare: a whole-copy cap here would refuse a
    // file whose chosen tables fit.

    let bytes = read_capped(
        storage,
        &manifest_key,
        MANIFEST_MAX_BYTES,
        RunRefusal::Invalid(Invalid::Manifest),
    )
    .await?;
    let manifest =
        Manifest::from_json(&bytes).map_err(|_| RunRefusal::Invalid(Invalid::Manifest))?;
    // The registry and the storage must tell the same story.
    if !same_table_list(&manifest, row.tables_json.as_deref()) {
        return Err(RunRefusal::Invalid(Invalid::Manifest));
    }
    let tracked: HashSet<&str> = row.blob_keys.iter().map(String::as_str).collect();
    let plan = PreparedTables {
        root,
        manifest_key,
        manifest,
        prepared_bytes,
    };
    for (t, table) in plan.manifest.tables.iter().enumerate() {
        for p in 0..table.parts as usize {
            if !tracked.contains(plan.part_key(t, p)?.as_str()) {
                return Err(RunRefusal::Invalid(Invalid::Parts));
            }
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::super::testkit::*;
    use super::*;
    use crate::tabular_prepare::registry::ReadyInfo;
    use chrono::Utc;

    fn base_row(status: PrepareStatus) -> PreparedRow {
        let now = Utc::now();
        PreparedRow {
            source_storage_key: SOURCE.into(),
            status,
            format_version: FORMAT_VERSION,
            manifest_key: Some(format!("{ROOT}/manifest.json")),
            blob_keys: vec![],
            tables_json: None,
            source_bytes: 1,
            prepared_bytes: Some(1),
            error_code: None,
            error_detail: None,
            lease_owner: None,
            lease_until: None,
            attempts: 1,
            created_at: now,
            updated_at: now,
            last_used_at: None,
        }
    }

    #[test]
    fn a_row_that_is_not_ready_says_why_and_never_stages() {
        assert_eq!(
            judge_row(None, SOURCE).unwrap_err(),
            RunRefusal::NotPrepared
        );
        assert_eq!(
            judge_row(Some(base_row(PrepareStatus::Running)), SOURCE).unwrap_err(),
            RunRefusal::StillPreparing { percent: None }
        );
        assert_eq!(
            judge_row(Some(base_row(PrepareStatus::Deleting)), SOURCE).unwrap_err(),
            RunRefusal::BeingRemoved
        );
        let mut failed = base_row(PrepareStatus::Failed);
        failed.error_code = Some("time".into());
        failed.error_detail = Some("gs://secret/key".into());
        failed.attempts = 1;
        assert_eq!(
            judge_row(Some(failed.clone()), SOURCE).unwrap_err(),
            RunRefusal::PreparationFailed {
                reason: FailureReason::Time,
                final_failure: false
            }
        );
        failed.attempts = MAX_ATTEMPTS;
        assert_eq!(
            judge_row(Some(failed), SOURCE).unwrap_err(),
            RunRefusal::PreparationFailed {
                reason: FailureReason::Time,
                final_failure: true
            }
        );
    }

    #[test]
    fn a_ready_row_of_another_source_or_layout_or_without_a_manifest_is_invalid() {
        let ok = base_row(PrepareStatus::Ready);
        assert!(judge_row(Some(ok.clone()), SOURCE).is_ok());
        let mut other = ok.clone();
        other.source_storage_key = "chat-attachments/u2/s9/doc-9".into();
        let mut old = ok.clone();
        old.format_version = FORMAT_VERSION - 1;
        let mut bare = ok;
        bare.manifest_key = None;
        for row in [other, old, bare] {
            assert_eq!(
                judge_row(Some(row), SOURCE).unwrap_err(),
                RunRefusal::Invalid(Invalid::Record)
            );
        }
    }

    #[tokio::test]
    async fn a_ready_copy_is_verified_and_lists_its_parts() {
        let p = prepared(&[("sales", 2), ("Stores", 1)], 10).await;
        let plan = verify_prepared(&*p.registry, &*p.storage, SOURCE)
            .await
            .unwrap();
        assert_eq!(plan.tables().len(), 2);
        assert_eq!(
            plan.part_key(1, 0).unwrap(),
            format!("{ROOT}/t1/part-00000.parquet")
        );
        assert_eq!(
            plan.prepared_bytes(),
            3 * 10 + p.manifest.to_json().unwrap().len() as u64
        );
        // Only the manifest was read: no part is opened by a verification.
        assert_eq!(
            p.storage.reads.lock().unwrap().as_slice(),
            &[format!("{ROOT}/manifest.json")]
        );
    }

    #[tokio::test]
    async fn the_registry_is_asked_before_the_storage_is_touched() {
        let (registry, _dir) = sqlite_registry().await;
        let storage = FakeStorage::new();
        // No row at all.
        let err = verify_prepared(&*registry, &*storage, SOURCE)
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::NotPrepared);
        assert!(storage.reads.lock().unwrap().is_empty());
        // A row that is still running.
        claim(&registry, FORMAT_VERSION).await;
        let err = verify_prepared(&*registry, &*storage, SOURCE)
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::StillPreparing { percent: None });
        assert!(storage.reads.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_copy_in_an_older_layout_is_invalid_not_staged() {
        let p = prepared_with(&[("sales", 1)], 10, FORMAT_VERSION - 1, |_| {}).await;
        let err = verify_prepared(&*p.registry, &*p.storage, SOURCE)
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Invalid(Invalid::Record));
        assert!(p.storage.reads.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_storage_that_cannot_say_where_the_copy_lives_is_refused() {
        let p = prepared(&[("sales", 1)], 10).await;
        let none = FakeStorage::without_root();
        let err = verify_prepared(&*p.registry, &*none, SOURCE)
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Invalid(Invalid::NoRoot));
    }

    /// A row whose manifest key is not at the derived root is not the copy this
    /// source's storage placed there.
    #[tokio::test]
    async fn a_manifest_key_outside_the_derived_root_is_invalid() {
        let p = prepared_with(&[("sales", 1)], 10, FORMAT_VERSION, |info| {
            info.manifest_key = "chat-attachments/u9/s9/prepared/doc-9/manifest.json".into();
            info.blob_keys.push(info.manifest_key.clone());
        })
        .await;
        let err = verify_prepared(&*p.registry, &*p.storage, SOURCE)
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Invalid(Invalid::Record));
        assert!(p.storage.reads.lock().unwrap().is_empty());
    }

    /// The data limit is not applied to the whole copy here: a file whose chosen
    /// tables fit must not be refused for the size of the others.
    #[tokio::test]
    async fn a_copy_recorded_over_the_data_limit_still_verifies() {
        let p = prepared_with(&[("sales", 1)], 10, FORMAT_VERSION, |info| {
            info.prepared_bytes = DATA_MAX_BYTES as i64 * 3;
        })
        .await;
        assert!(verify_prepared(&*p.registry, &*p.storage, SOURCE)
            .await
            .is_ok());
    }

    #[test]
    fn a_table_list_that_cannot_be_serialised_or_is_absent_never_matches() {
        let ok = crate::tabular_run::testkit::manifest_with(&[("sales", 1)]);
        let text = ok.tables_json().unwrap();
        assert!(same_table_list(&ok, Some(&text)));
        assert!(!same_table_list(&ok, None), "no recorded list");
        assert!(!same_table_list(&ok, Some("[]")));
        // Too many columns for the registry row: serialisation fails on our side too.
        let mut huge = ok.clone();
        huge.tables[0].columns = (0..3000)
            .map(|i| {
                let mut c = huge.tables[0].columns[0].clone();
                c.name = format!("column_number_{i:05}");
                c
            })
            .collect();
        assert!(huge.tables_json().is_err());
        assert!(
            !same_table_list(&huge, None),
            "Err on our side and None on theirs is not a match"
        );
    }

    #[tokio::test]
    async fn a_manifest_that_disagrees_with_the_row_is_invalid() {
        let p = prepared_with(&[("sales", 1)], 10, FORMAT_VERSION, |info| {
            info.tables_json = "[]".into();
        })
        .await;
        let err = verify_prepared(&*p.registry, &*p.storage, SOURCE)
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Invalid(Invalid::Manifest));
    }

    #[tokio::test]
    async fn a_manifest_that_is_not_json_is_invalid_and_its_text_is_not_echoed() {
        let p = prepared(&[("sales", 1)], 10).await;
        p.storage
            .put(&format!("{ROOT}/manifest.json"), b"{secret cell}".to_vec());
        let err = verify_prepared(&*p.registry, &*p.storage, SOURCE)
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Invalid(Invalid::Manifest));
        assert!(!err.message().contains("secret"));
    }

    /// The manifest is read under its cap whatever the storage declares: an
    /// object that lies about its size is cut off, not buffered.
    #[tokio::test]
    async fn an_oversized_manifest_is_refused_at_the_cap() {
        let p = prepared(&[("sales", 1)], 10).await;
        p.storage.put(
            &format!("{ROOT}/manifest.json"),
            vec![b' '; MANIFEST_MAX_BYTES + 1],
        );
        let err = verify_prepared(&*p.registry, &*p.storage, SOURCE)
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Invalid(Invalid::Manifest));
    }

    #[tokio::test]
    async fn a_part_the_row_does_not_track_is_invalid() {
        let p = prepared_with(
            &[("sales", 2)],
            10,
            FORMAT_VERSION,
            |info: &mut ReadyInfo| {
                info.blob_keys
                    .retain(|k| !k.ends_with("part-00001.parquet"));
            },
        )
        .await;
        let err = verify_prepared(&*p.registry, &*p.storage, SOURCE)
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Invalid(Invalid::Parts));
    }

    #[tokio::test]
    async fn a_storage_failure_on_the_manifest_hides_the_adapters_text() {
        let p = prepared(&[("sales", 1)], 10).await;
        p.storage
            .broken
            .lock()
            .unwrap()
            .push(format!("{ROOT}/manifest.json"));
        let err = verify_prepared(&*p.registry, &*p.storage, SOURCE)
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Storage);
        assert!(!err.message().contains("secret"));
        assert!(!err.message().contains(ROOT));
    }

    fn plan(tables: &[(&str, u32)]) -> PreparedTables {
        let tables = tables
            .iter()
            .map(|(name, parts)| TableInfo {
                name: (*name).to_string(),
                rows: u64::from(*parts) * 10,
                parts: *parts,
                columns: vec![],
            })
            .collect();
        PreparedTables {
            root: ROOT.into(),
            manifest_key: format!("{ROOT}/manifest.json"),
            manifest: Manifest::new(tables),
            prepared_bytes: 7,
        }
    }

    #[test]
    fn a_part_key_is_the_derived_root_and_the_canonical_relative_path() {
        let p = plan(&[("sales", 2), ("stores", 1)]);
        assert_eq!(
            p.part_key(1, 0).unwrap(),
            format!("{ROOT}/t1/part-00000.parquet")
        );
        assert_eq!(
            p.part_key(0, 1).unwrap(),
            format!("{ROOT}/t0/part-00001.parquet")
        );
        // A part index no manifest may hold is refused, never formatted.
        assert_eq!(
            p.part_key(0, 100_000).unwrap_err(),
            RunRefusal::Invalid(Invalid::Manifest)
        );
        assert_eq!(p.manifest_key(), format!("{ROOT}/manifest.json"));
        assert_eq!(p.prepared_bytes(), 7);
        assert_eq!(p.tables().len(), 2);
    }

    #[test]
    fn tables_are_selected_by_name_ignoring_case_and_once() {
        let p = plan(&[("sales", 1), ("Stores", 1), ("tax", 1)]);
        assert_eq!(p.select(&[]).unwrap(), vec![0, 1, 2]);
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            p.select(&names(&["STORES", "sales", "stores"])).unwrap(),
            vec![0, 1]
        );
        let err = p.select(&names(&["sales", "nope"])).unwrap_err();
        assert_eq!(
            err,
            RunRefusal::NoSuchTable {
                name: "nope".into()
            }
        );
    }

    #[test]
    fn the_data_limit_is_one_gibibyte() {
        assert_eq!(DATA_MAX_BYTES, 1_073_741_824);
    }
}
