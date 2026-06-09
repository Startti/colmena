//! Persistent storage of the last revision the agent saw per
//! `(agent_session_id, document_id)`. Postgres-backed in production;
//! `InMemoryRevisionStore` available for unit tests.

use crate::gdocs::domain::{DocsError, DocumentId, RevisionId};
use async_trait::async_trait;
use sqlx::PgPool;

/// Persistent revision-tracking port. The co-edit guard (Task 15) reads
/// the stored revision before each edit; successful writes update it.
#[async_trait]
pub trait RevisionStore: Send + Sync {
    async fn get(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
    ) -> Result<Option<RevisionId>, DocsError>;

    async fn put(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
        rev: &RevisionId,
    ) -> Result<(), DocsError>;
}

/// Production implementation backed by Postgres.
pub struct PostgresRevisionStore {
    pool: PgPool,
}

impl PostgresRevisionStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl RevisionStore for PostgresRevisionStore {
    async fn get(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
    ) -> Result<Option<RevisionId>, DocsError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT last_revision_id FROM gdocs_session_state \
             WHERE agent_session_id = $1 AND document_id = $2",
        )
        .bind(session_id)
        .bind(&doc_id.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DocsError::Internal(format!("revision_store.get: {e}")))?;
        Ok(row.map(|(s,)| RevisionId(s)))
    }

    async fn put(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
        rev: &RevisionId,
    ) -> Result<(), DocsError> {
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
        Ok(())
    }
}

/// In-memory `RevisionStore` for unit tests.
#[cfg(test)]
pub struct InMemoryRevisionStore {
    map: tokio::sync::RwLock<std::collections::HashMap<(String, String), RevisionId>>,
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
    async fn get(&self, sid: &str, doc: &DocumentId) -> Result<Option<RevisionId>, DocsError> {
        Ok(self
            .map
            .read()
            .await
            .get(&(sid.into(), doc.0.clone()))
            .cloned())
    }

    async fn put(&self, sid: &str, doc: &DocumentId, rev: &RevisionId) -> Result<(), DocsError> {
        self.map
            .write()
            .await
            .insert((sid.into(), doc.0.clone()), rev.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_round_trip() {
        let store = InMemoryRevisionStore::new();
        let doc = DocumentId("doc1".into());
        let rev = RevisionId("rev_5".into());
        assert!(store.get("s1", &doc).await.unwrap().is_none());
        store.put("s1", &doc, &rev).await.unwrap();
        assert_eq!(store.get("s1", &doc).await.unwrap(), Some(rev));
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
}
