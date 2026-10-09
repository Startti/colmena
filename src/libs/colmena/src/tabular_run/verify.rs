//! Is the prepared copy of this file one the engine may stage and run over?
//!
//! The answer comes from the registry, not from the model and not from the
//! storage alone: the row must be this source's, `ready`, in the layout this
//! reader reads, and the manifest in storage must be the one the row recorded.
//! Nothing is read from storage before the row has passed, and a prepared copy
//! that is too large for one call is refused from the row's recorded size,
//! before any part is opened.

use super::refusal::{FailureReason, Invalid, RunRefusal};
use crate::tabular_prepare::manifest::{part_path, Manifest, TableInfo};
use crate::tabular_prepare::registry::{PrepareStatus, PreparedRow, FORMAT_VERSION, MAX_ATTEMPTS};
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    const SOURCE: &str = "chat-attachments/u1/s1/doc-1";
    const ROOT: &str = "chat-attachments/u1/s1/prepared/doc-1";

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
