//! Abstraction over "where SQL queries land" for the recent-changes
//! subsystem. Two impls: `DirectBackend` (local/shared mode → direct
//! `ChangeTrackerStore`) and `RestBackend` (ws_peer mode → HTTP to the
//! CRDT documents server).

use crate::crdt_documents::{
    change_tracker_store::{
        ChangeTrackerStore, NewEvent, StoredArtifact, StoredEvent, StoreError,
    },
    ArtifactId,
};
use async_trait::async_trait;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("http: {0}")]
    Http(String),
    #[error("decode: {0}")]
    Decode(String),
}

#[async_trait]
pub trait CrdtBackend: Send + Sync + std::any::Any {
    async fn record_event(&self, ev: NewEvent) -> Result<u64, BackendError>;

    async fn events_since(
        &self,
        artifact_id: &ArtifactId,
        since_event_id: u64,
        sheet_id_filter: Option<&str>,
        exclude_origin: Option<&str>,
        limit: u32,
    ) -> Result<Vec<StoredEvent>, BackendError>;

    async fn upsert_cursor(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        last_event_id: u64,
    ) -> Result<(), BackendError>;

    async fn cursor_for(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
    ) -> Result<Option<u64>, BackendError>;

    async fn artifacts_for_session(
        &self,
        session_id: &str,
        limit: u32,
    ) -> Result<Vec<StoredArtifact>, BackendError>;

    async fn touch_artifact(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        name: Option<&str>,
    ) -> Result<(), BackendError>;
}

// ── DirectBackend ────────────────────────────────────────────────────────

pub struct DirectBackend {
    pub store: Arc<dyn ChangeTrackerStore>,
}

#[async_trait]
impl CrdtBackend for DirectBackend {
    async fn record_event(&self, ev: NewEvent) -> Result<u64, BackendError> {
        Ok(self.store.insert_event(ev).await?)
    }
    async fn events_since(
        &self,
        artifact_id: &ArtifactId,
        since_event_id: u64,
        sheet_id_filter: Option<&str>,
        exclude_origin: Option<&str>,
        limit: u32,
    ) -> Result<Vec<StoredEvent>, BackendError> {
        Ok(self
            .store
            .events_since(artifact_id, since_event_id, sheet_id_filter, exclude_origin, limit)
            .await?)
    }
    async fn upsert_cursor(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        last_event_id: u64,
    ) -> Result<(), BackendError> {
        Ok(self.store.upsert_cursor(session_id, artifact_id, last_event_id).await?)
    }
    async fn cursor_for(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
    ) -> Result<Option<u64>, BackendError> {
        Ok(self.store.cursor_for(session_id, artifact_id).await?)
    }
    async fn artifacts_for_session(
        &self,
        session_id: &str,
        limit: u32,
    ) -> Result<Vec<StoredArtifact>, BackendError> {
        Ok(self.store.artifacts_for_session(session_id, limit).await?)
    }
    async fn touch_artifact(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        name: Option<&str>,
    ) -> Result<(), BackendError> {
        Ok(self.store.touch_artifact(session_id, artifact_id, name).await?)
    }
}

// ── RestBackend ──────────────────────────────────────────────────────────

pub struct RestBackend {
    pub client: reqwest::Client,
    /// Base URL like `http://crdt-service:8090` (NO trailing slash).
    pub base_url: String,
}

impl RestBackend {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }
}

#[async_trait]
impl CrdtBackend for RestBackend {
    async fn record_event(&self, ev: NewEvent) -> Result<u64, BackendError> {
        let url = format!("{}/documents/{}/events", self.base_url, ev.artifact_id);
        let body = serde_json::json!({
            "sheet_id": ev.sheet_id,
            "origin": ev.origin,
            "summary": ev.summary,
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| BackendError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(BackendError::Http(format!("status {}", resp.status())));
        }
        let val: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| BackendError::Decode(e.to_string()))?;
        Ok(val["id"].as_u64().unwrap_or(0))
    }

    async fn events_since(
        &self,
        artifact_id: &ArtifactId,
        since_event_id: u64,
        sheet_id_filter: Option<&str>,
        exclude_origin: Option<&str>,
        limit: u32,
    ) -> Result<Vec<StoredEvent>, BackendError> {
        let mut url = format!(
            "{}/documents/{}/changes?since={}&limit={}",
            self.base_url, artifact_id, since_event_id, limit
        );
        if let Some(s) = sheet_id_filter {
            url.push_str(&format!("&sheet_id={s}"));
        }
        if let Some(s) = exclude_origin {
            url.push_str(&format!("&exclude_origin={s}"));
        }
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| BackendError::Http(e.to_string()))?;
        let val: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| BackendError::Decode(e.to_string()))?;
        let events = val["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|e| StoredEvent {
                id: e["id"].as_u64().unwrap_or(0),
                artifact_id: e["artifact_id"].as_str().unwrap_or("").to_string(),
                sheet_id: e["sheet_id"].as_str().map(String::from),
                origin: e["origin"].as_str().unwrap_or("").to_string(),
                summary: e["summary"].as_str().unwrap_or("").to_string(),
                created_at: e["created_at"].as_str().unwrap_or("").to_string(),
            })
            .collect();
        Ok(events)
    }

    async fn upsert_cursor(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        last_event_id: u64,
    ) -> Result<(), BackendError> {
        let url = format!("{}/documents/{}/cursor", self.base_url, artifact_id);
        let body = serde_json::json!({
            "agent_session_id": session_id,
            "last_event_id": last_event_id,
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| BackendError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(BackendError::Http(format!("status {}", resp.status())));
        }
        Ok(())
    }

    async fn cursor_for(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
    ) -> Result<Option<u64>, BackendError> {
        let url = format!(
            "{}/documents/{}/cursor?agent_session_id={}",
            self.base_url, artifact_id, session_id
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| BackendError::Http(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let val: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| BackendError::Decode(e.to_string()))?;
        Ok(val["last_event_id"].as_u64())
    }

    async fn artifacts_for_session(
        &self,
        session_id: &str,
        limit: u32,
    ) -> Result<Vec<StoredArtifact>, BackendError> {
        let url = format!(
            "{}/documents/by-session/{}?limit={}",
            self.base_url, session_id, limit
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| BackendError::Http(e.to_string()))?;
        let val: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| BackendError::Decode(e.to_string()))?;
        Ok(val["artifacts"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|a| StoredArtifact {
                artifact_id: a["artifact_id"].as_str().unwrap_or("").to_string(),
                name: a["name"].as_str().unwrap_or("").to_string(),
                created_at: a["created_at"].as_str().unwrap_or("").to_string(),
                last_accessed_at: a["last_accessed_at"].as_str().unwrap_or("").to_string(),
            })
            .collect())
    }

    async fn touch_artifact(
        &self,
        _session_id: &str,
        _artifact_id: &ArtifactId,
        _name: Option<&str>,
    ) -> Result<(), BackendError> {
        // ws_peer mode: touch is done by the server on POST /documents
        // (which includes agent_session_id in the body). The client
        // explicitly setting touch is a no-op.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::change_tracker_store::InMemoryChangeTrackerStore;

    #[tokio::test]
    async fn direct_backend_records_and_queries() {
        let store: Arc<dyn ChangeTrackerStore> = Arc::new(InMemoryChangeTrackerStore::new());
        let backend = DirectBackend { store };
        let aid = ArtifactId::new();
        let id = backend
            .record_event(NewEvent {
                artifact_id: aid.clone(),
                sheet_id: Some("sh_test".to_string()),
                origin: "agent:s1".to_string(),
                summary: "hello".to_string(),
            })
            .await
            .unwrap();
        assert!(id > 0);
        let evs = backend
            .events_since(&aid, 0, None, None, 10)
            .await
            .unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].summary, "hello");
    }
}
