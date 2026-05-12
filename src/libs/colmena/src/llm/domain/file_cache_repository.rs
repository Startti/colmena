//! Port para persistir referencias a archivos subidos al Files API.
//! Implementación por defecto en `llm/infrastructure/files/postgres_file_cache.rs`.

use crate::llm::domain::{LlmError, ProviderFileRef, ProviderKind};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone)]
pub struct CachedFileEntry {
    pub document_id: String,
    pub provider: ProviderKind,
    pub provider_file_id: String,
    pub mime_type: String,
    pub filename: String,
    pub size_bytes: Option<i64>,
    pub uploaded_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: DateTime<Utc>,
}

impl CachedFileEntry {
    /// Heurística: si tenemos expires_at y ya pasó (menos margen de 5 min),
    /// asumimos expirado sin llamar al provider.
    pub fn is_likely_alive(&self, now: DateTime<Utc>) -> bool {
        match self.expires_at {
            None => true,
            Some(exp) => now < exp - Duration::minutes(5),
        }
    }

    pub fn into_ref(self) -> ProviderFileRef {
        ProviderFileRef {
            provider: self.provider,
            provider_file_id: self.provider_file_id,
            mime_type: self.mime_type,
            filename: self.filename,
            expires_at: self.expires_at,
        }
    }
}

#[async_trait]
pub trait FileCacheRepository: Send + Sync {
    async fn lookup(
        &self,
        document_id: &str,
        provider: ProviderKind,
    ) -> Result<Option<CachedFileEntry>, LlmError>;

    async fn upsert(&self, entry: &CachedFileEntry) -> Result<(), LlmError>;

    async fn invalidate(&self, document_id: &str, provider: ProviderKind) -> Result<(), LlmError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Dyn-safety guard: ensures FileCacheRepository can be stored as Arc<dyn ...>.
    const _: fn() = || {
        fn _dyn_safe(_: &dyn FileCacheRepository) {}
    };

    fn entry_with_expiry(expires_at: Option<DateTime<Utc>>) -> CachedFileEntry {
        let now = Utc::now();
        CachedFileEntry {
            document_id: "doc-1".into(),
            provider: ProviderKind::Google,
            provider_file_id: "files/abc".into(),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_bytes: Some(1024),
            uploaded_at: now,
            expires_at,
            last_used_at: now,
        }
    }

    #[test]
    fn alive_when_expires_at_is_none() {
        let e = entry_with_expiry(None);
        assert!(e.is_likely_alive(Utc::now()));
    }

    #[test]
    fn alive_when_expires_at_in_future_beyond_margin() {
        let now = Utc::now();
        let e = entry_with_expiry(Some(now + Duration::hours(2)));
        assert!(e.is_likely_alive(now));
    }

    #[test]
    fn expired_when_within_5min_margin() {
        let now = Utc::now();
        let e = entry_with_expiry(Some(now + Duration::minutes(3)));
        assert!(!e.is_likely_alive(now));
    }

    #[test]
    fn expired_when_in_past() {
        let now = Utc::now();
        let e = entry_with_expiry(Some(now - Duration::hours(1)));
        assert!(!e.is_likely_alive(now));
    }

    #[test]
    fn into_ref_preserves_fields() {
        let e = entry_with_expiry(None);
        let r = e.clone().into_ref();
        assert_eq!(r.provider_file_id, e.provider_file_id);
        assert_eq!(r.provider, e.provider);
    }

    struct InMemoryCache {
        entries: std::sync::Mutex<Vec<CachedFileEntry>>,
    }

    #[async_trait]
    impl FileCacheRepository for InMemoryCache {
        async fn lookup(
            &self,
            doc_id: &str,
            p: ProviderKind,
        ) -> Result<Option<CachedFileEntry>, LlmError> {
            Ok(self
                .entries
                .lock()
                .unwrap()
                .iter()
                .find(|e| e.document_id == doc_id && e.provider == p)
                .cloned())
        }

        async fn upsert(&self, e: &CachedFileEntry) -> Result<(), LlmError> {
            let mut v = self.entries.lock().unwrap();
            v.retain(|x| !(x.document_id == e.document_id && x.provider == e.provider));
            v.push(e.clone());
            Ok(())
        }

        async fn invalidate(&self, doc_id: &str, p: ProviderKind) -> Result<(), LlmError> {
            self.entries
                .lock()
                .unwrap()
                .retain(|x| !(x.document_id == doc_id && x.provider == p));
            Ok(())
        }
    }

    #[tokio::test]
    async fn in_memory_cache_lookup_upsert_invalidate_round_trip() {
        let cache = InMemoryCache {
            entries: std::sync::Mutex::new(Vec::new()),
        };
        let entry = entry_with_expiry(None);
        assert!(cache
            .lookup("doc-1", ProviderKind::Google)
            .await
            .unwrap()
            .is_none());
        cache.upsert(&entry).await.unwrap();
        let got = cache.lookup("doc-1", ProviderKind::Google).await.unwrap();
        assert!(got.is_some());
        assert_eq!(got.unwrap().provider_file_id, "files/abc");
        cache
            .invalidate("doc-1", ProviderKind::Google)
            .await
            .unwrap();
        assert!(cache
            .lookup("doc-1", ProviderKind::Google)
            .await
            .unwrap()
            .is_none());
    }
}
