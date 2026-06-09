//! Persistent storage of the last revision the agent saw per
//! `(agent_session_id, document_id)`, optionally augmented with the full
//! [`DocumentSnapshot`] at that revision so the co-edit guard can show
//! paragraph-level diffs of intervening human edits.
//!
//! Postgres-backed in production via `PostgresRevisionStore`;
//! `InMemoryRevisionStore` is available for unit tests.
//!
//! v1.1 (2026-06-09): added optional snapshot persistence. The
//! `last_snapshot_json` / `last_snapshot_size_bytes` columns are added via
//! migration `20260609000000_gdocs_session_state_snapshot.sql`. Instances
//! that boot against a database without the migration applied will detect
//! it via `information_schema.columns` and degrade to v1 behavior
//! (revision-only persistence; empty diffs in the guard).

use crate::gdocs::domain::{DocsError, DocumentId, DocumentSnapshot, RevisionId};
use async_trait::async_trait;
use sqlx::PgPool;

/// Maximum serialized snapshot size to persist. Beyond this, the snapshot
/// is dropped (NULL) and the co-edit guard degrades to v1 behavior
/// (revisionId equality only; empty diffs) for that doc.
///
/// Default 1 MiB. Override via `COLMENA_GDOCS_MAX_SNAPSHOT_BYTES` env var.
pub const DEFAULT_MAX_SNAPSHOT_BYTES: usize = 1_048_576;

/// Read the effective cap, allowing operators to override at runtime.
pub fn max_snapshot_bytes() -> usize {
    std::env::var("COLMENA_GDOCS_MAX_SNAPSHOT_BYTES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_SNAPSHOT_BYTES)
}

/// Persistent revision-tracking port. The co-edit guard reads stored
/// revision + snapshot before each edit; successful writes update both.
///
/// The legacy `get` / `put` are still on the trait as backward-compat
/// shims (default implementations delegate to the new methods).
#[async_trait]
pub trait RevisionStore: Send + Sync {
    /// Return both the last revisionId we observed for `(session, doc)` and
    /// the full snapshot, if either has been persisted.
    async fn get_with_snapshot(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
    ) -> Result<(Option<RevisionId>, Option<DocumentSnapshot>), DocsError>;

    /// Persist the revision and optionally the snapshot. Passing `None`
    /// for `snapshot` clears any previously stored snapshot for that key.
    async fn put_with_snapshot(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
        rev: &RevisionId,
        snapshot: Option<&DocumentSnapshot>,
    ) -> Result<(), DocsError>;

    /// Backward-compat shim — fetches only the revision.
    async fn get(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
    ) -> Result<Option<RevisionId>, DocsError> {
        let (rev, _) = self.get_with_snapshot(session_id, doc_id).await?;
        Ok(rev)
    }

    /// Backward-compat shim — persists only the revision (clears snapshot).
    async fn put(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
        rev: &RevisionId,
    ) -> Result<(), DocsError> {
        self.put_with_snapshot(session_id, doc_id, rev, None).await
    }
}

/// Production implementation backed by Postgres.
pub struct PostgresRevisionStore {
    pool: PgPool,
    has_snapshot_col: bool,
}

impl PostgresRevisionStore {
    /// Construct against a live pool, probing `information_schema` for the
    /// v1.1 snapshot columns. If they are absent (older migration set),
    /// degrade silently to v1 behavior and log a single warn.
    pub async fn new(pool: PgPool) -> Self {
        let has_snapshot_col = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (\
                 SELECT 1 FROM information_schema.columns \
                 WHERE table_name = 'gdocs_session_state' \
                 AND column_name = 'last_snapshot_json'\
             )",
        )
        .fetch_one(&pool)
        .await
        .unwrap_or(false);

        if !has_snapshot_col {
            tracing::warn!(
                "gdocs: last_snapshot_json column missing on gdocs_session_state; \
                 co-edit guard degrades to v1 (revisionId equality only). \
                 Apply migration 20260609000000_gdocs_session_state_snapshot.sql"
            );
        }
        Self {
            pool,
            has_snapshot_col,
        }
    }
}

#[async_trait]
impl RevisionStore for PostgresRevisionStore {
    async fn get_with_snapshot(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
    ) -> Result<(Option<RevisionId>, Option<DocumentSnapshot>), DocsError> {
        if self.has_snapshot_col {
            let row: Option<(String, Option<serde_json::Value>)> = sqlx::query_as(
                "SELECT last_revision_id, last_snapshot_json \
                 FROM gdocs_session_state \
                 WHERE agent_session_id = $1 AND document_id = $2",
            )
            .bind(session_id)
            .bind(&doc_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| DocsError::Internal(format!("revision_store.get: {e}")))?;
            match row {
                None => Ok((None, None)),
                Some((rev, snap_json)) => {
                    let snap = snap_json.and_then(|v| serde_json::from_value(v).ok());
                    Ok((Some(RevisionId(rev)), snap))
                }
            }
        } else {
            let row: Option<(String,)> = sqlx::query_as(
                "SELECT last_revision_id FROM gdocs_session_state \
                 WHERE agent_session_id = $1 AND document_id = $2",
            )
            .bind(session_id)
            .bind(&doc_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| DocsError::Internal(format!("revision_store.get: {e}")))?;
            Ok((row.map(|(s,)| RevisionId(s)), None))
        }
    }

    async fn put_with_snapshot(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
        rev: &RevisionId,
        snapshot: Option<&DocumentSnapshot>,
    ) -> Result<(), DocsError> {
        if self.has_snapshot_col {
            let snap_json = snapshot.and_then(|s| serde_json::to_value(s).ok());
            let snap_bytes = snap_json
                .as_ref()
                .and_then(|v| serde_json::to_string(v).ok().map(|s| s.len()));
            let cap = max_snapshot_bytes();
            let (stored_json, stored_bytes) = match snap_bytes {
                Some(b) if b > cap => {
                    tracing::warn!(
                        bytes = b,
                        cap = cap,
                        doc_id = %doc_id.0,
                        "gdocs.snapshot.too_large — dropping snapshot, guard degrades to v1 for this doc"
                    );
                    (None, None)
                }
                Some(b) => (snap_json, Some(b as i32)),
                None => (None, None),
            };

            sqlx::query(
                "INSERT INTO gdocs_session_state \
                    (agent_session_id, document_id, last_revision_id, last_edit_at, \
                     last_snapshot_json, last_snapshot_size_bytes) \
                 VALUES ($1, $2, $3, now(), $4, $5) \
                 ON CONFLICT (agent_session_id, document_id) \
                 DO UPDATE SET last_revision_id         = EXCLUDED.last_revision_id, \
                               last_edit_at             = now(), \
                               last_snapshot_json       = EXCLUDED.last_snapshot_json, \
                               last_snapshot_size_bytes = EXCLUDED.last_snapshot_size_bytes",
            )
            .bind(session_id)
            .bind(&doc_id.0)
            .bind(&rev.0)
            .bind(stored_json)
            .bind(stored_bytes)
            .execute(&self.pool)
            .await
            .map_err(|e| DocsError::Internal(format!("revision_store.put: {e}")))?;
        } else {
            sqlx::query(
                "INSERT INTO gdocs_session_state \
                    (agent_session_id, document_id, last_revision_id, last_edit_at) \
                 VALUES ($1, $2, $3, now()) \
                 ON CONFLICT (agent_session_id, document_id) \
                 DO UPDATE SET last_revision_id = EXCLUDED.last_revision_id, \
                               last_edit_at     = now()",
            )
            .bind(session_id)
            .bind(&doc_id.0)
            .bind(&rev.0)
            .execute(&self.pool)
            .await
            .map_err(|e| DocsError::Internal(format!("revision_store.put: {e}")))?;
        }
        Ok(())
    }
}

#[cfg(test)]
type RevSnapshotEntry = (RevisionId, Option<DocumentSnapshot>);
#[cfg(test)]
type RevStoreMap = std::collections::HashMap<(String, String), RevSnapshotEntry>;

/// In-memory `RevisionStore` for unit tests.
#[cfg(test)]
pub struct InMemoryRevisionStore {
    map: tokio::sync::RwLock<RevStoreMap>,
}

#[cfg(test)]
impl Default for InMemoryRevisionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl InMemoryRevisionStore {
    pub fn new() -> Self {
        Self {
            map: tokio::sync::RwLock::new(Default::default()),
        }
    }
}

#[cfg(test)]
#[async_trait]
impl RevisionStore for InMemoryRevisionStore {
    async fn get_with_snapshot(
        &self,
        sid: &str,
        doc: &DocumentId,
    ) -> Result<(Option<RevisionId>, Option<DocumentSnapshot>), DocsError> {
        match self.map.read().await.get(&(sid.into(), doc.0.clone())) {
            None => Ok((None, None)),
            Some((rev, snap)) => Ok((Some(rev.clone()), snap.clone())),
        }
    }

    async fn put_with_snapshot(
        &self,
        sid: &str,
        doc: &DocumentId,
        rev: &RevisionId,
        snapshot: Option<&DocumentSnapshot>,
    ) -> Result<(), DocsError> {
        self.map.write().await.insert(
            (sid.into(), doc.0.clone()),
            (rev.clone(), snapshot.cloned()),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdocs::domain::{ParagraphKind, ParagraphSnapshot, TabSnapshot};

    fn make_snapshot(rev: &str) -> DocumentSnapshot {
        DocumentSnapshot {
            doc_id: DocumentId("doc1".into()),
            revision_id: RevisionId(rev.into()),
            title: "t".into(),
            tabs: vec![TabSnapshot {
                tab_id: None,
                paragraphs: vec![ParagraphSnapshot {
                    n: 1,
                    kind: ParagraphKind::Paragraph,
                    text: "hello".into(),
                    start_index: 1,
                    end_index: 6,
                }],
            }],
        }
    }

    #[tokio::test]
    async fn in_memory_round_trip_legacy_api() {
        let store = InMemoryRevisionStore::new();
        let doc = DocumentId("doc1".into());
        let rev = RevisionId("rev_5".into());
        assert!(store.get("s1", &doc).await.unwrap().is_none());
        store.put("s1", &doc, &rev).await.unwrap();
        assert_eq!(store.get("s1", &doc).await.unwrap(), Some(rev));
    }

    #[tokio::test]
    async fn in_memory_round_trip_with_snapshot() {
        let store = InMemoryRevisionStore::new();
        let doc = DocumentId("doc1".into());
        let rev = RevisionId("rev_5".into());
        let snap = make_snapshot("rev_5");
        store
            .put_with_snapshot("s1", &doc, &rev, Some(&snap))
            .await
            .unwrap();
        let (got_rev, got_snap) = store.get_with_snapshot("s1", &doc).await.unwrap();
        assert_eq!(got_rev, Some(rev));
        assert_eq!(got_snap, Some(snap));
    }

    #[tokio::test]
    async fn in_memory_put_without_snapshot_clears_old_snapshot() {
        let store = InMemoryRevisionStore::new();
        let doc = DocumentId("doc1".into());
        let snap = make_snapshot("r1");
        store
            .put_with_snapshot("s1", &doc, &RevisionId("r1".into()), Some(&snap))
            .await
            .unwrap();
        // Now put without snapshot — snapshot should be cleared.
        store
            .put_with_snapshot("s1", &doc, &RevisionId("r2".into()), None)
            .await
            .unwrap();
        let (_, got_snap) = store.get_with_snapshot("s1", &doc).await.unwrap();
        assert_eq!(got_snap, None);
    }

    #[tokio::test]
    async fn in_memory_scoped_by_session() {
        let store = InMemoryRevisionStore::new();
        let doc = DocumentId("doc1".into());
        store
            .put("s1", &doc, &RevisionId("ra".into()))
            .await
            .unwrap();
        store
            .put("s2", &doc, &RevisionId("rb".into()))
            .await
            .unwrap();
        assert_eq!(store.get("s1", &doc).await.unwrap().unwrap().0, "ra");
        assert_eq!(store.get("s2", &doc).await.unwrap().unwrap().0, "rb");
    }

    #[tokio::test]
    async fn in_memory_overwrite_same_key() {
        let store = InMemoryRevisionStore::new();
        let doc = DocumentId("d".into());
        store
            .put("s", &doc, &RevisionId("r1".into()))
            .await
            .unwrap();
        store
            .put("s", &doc, &RevisionId("r2".into()))
            .await
            .unwrap();
        assert_eq!(store.get("s", &doc).await.unwrap().unwrap().0, "r2");
    }

    #[test]
    fn max_snapshot_bytes_uses_env_override() {
        // SAFETY: tests do not run in parallel within a single test
        // binary by default; we set + clear the var inside this test.
        std::env::set_var("COLMENA_GDOCS_MAX_SNAPSHOT_BYTES", "42");
        assert_eq!(max_snapshot_bytes(), 42);
        std::env::remove_var("COLMENA_GDOCS_MAX_SNAPSHOT_BYTES");
        assert_eq!(max_snapshot_bytes(), DEFAULT_MAX_SNAPSHOT_BYTES);
    }
}
