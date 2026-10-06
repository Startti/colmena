//! The ownership rule of the attachment registry, written once over the trait so
//! SQLite (plain tests) and Postgres (ignored tests, `DATABASE_URL`) run the same
//! cases.
//!
//! A row either references an object the HOST owns (`origin = host_storage_ref`,
//! the key is the host's) or an object the engine stored. That class is IMMUTABLE
//! for an existing `(session, document, provider)`: a write of the other class
//! changes nothing and says so, so the class, the key and the provider file id can
//! never end up mixed (a host key under an engine origin would let the GC delete
//! the host's object; an engine copy lost behind a host key would leak).

use crate::llm::domain::attachments::{
    origin, AttachmentRegistry, AttachmentSource, UpsertAttachmentInput, UpsertOutcome,
};
use crate::llm::domain::ProviderKind;
use std::sync::Arc;

const HOST_KEY: &str = "hosts/user-1/sales.csv";
const COPY_KEY: &str = "engine/copy-1";

fn host(sid: &str, doc: &str, provider: ProviderKind) -> UpsertAttachmentInput {
    UpsertAttachmentInput {
        agent_session_id: sid.into(),
        document_id: doc.into(),
        provider,
        provider_file_id: String::new(),
        mime_type: "text/csv".into(),
        filename: "sales.csv".into(),
        size_bytes: Some(60 * 1024 * 1024),
        label: None,
        description: None,
        source: AttachmentSource::Path(HOST_KEY.into()),
        storage_key: Some(HOST_KEY.into()),
        origin: Some(origin::HOST_STORAGE_REF.into()),
    }
}

fn ordinary(
    sid: &str,
    doc: &str,
    provider: ProviderKind,
    key: Option<&str>,
) -> UpsertAttachmentInput {
    UpsertAttachmentInput {
        provider_file_id: "pf-1".into(),
        filename: "other.csv".into(),
        size_bytes: Some(10),
        source: AttachmentSource::SignedUrl("https://example.invalid/x".into()),
        storage_key: key.map(str::to_string),
        origin: Some(origin::USER_UPLOAD.into()),
        ..host(sid, doc, provider)
    }
}

async fn row(
    reg: &dyn AttachmentRegistry,
    sid: &str,
    doc: &str,
    provider: ProviderKind,
) -> crate::llm::domain::ConversationAttachment {
    reg.lookup(sid, doc, provider).await.unwrap().expect("row")
}

/// A host row stays whole when it is re-registered as an ordinary file, with or
/// without a key, and the caller is told.
pub(crate) async fn a_host_row_is_not_rewritten_as_an_ordinary_file(
    reg: &dyn AttachmentRegistry,
    sid: &str,
) {
    assert_eq!(
        reg.upsert_checked(host(sid, "d", ProviderKind::OpenAi))
            .await
            .unwrap(),
        UpsertOutcome::Written
    );
    let before = row(reg, sid, "d", ProviderKind::OpenAi).await;
    for key in [None, Some(COPY_KEY)] {
        let outcome = reg
            .upsert_checked(ordinary(sid, "d", ProviderKind::OpenAi, key))
            .await
            .unwrap();
        assert_eq!(outcome, UpsertOutcome::OwnershipConflict, "key {key:?}");
        let after = row(reg, sid, "d", ProviderKind::OpenAi).await;
        assert_eq!(after.origin.as_deref(), Some(origin::HOST_STORAGE_REF));
        assert_eq!(after.storage_key.as_deref(), Some(HOST_KEY));
        assert_eq!(after.provider_file_id, "");
        assert_eq!(after.filename, before.filename);
        assert_eq!(after.size_bytes, before.size_bytes);
        assert_eq!(after.source, before.source);
        assert_eq!(
            after.refreshed_at, before.refreshed_at,
            "untouched, not even refreshed"
        );
    }
}

/// An engine row keeps its copy when a host reference arrives for the same id.
pub(crate) async fn an_engine_row_is_not_rewritten_as_a_host_reference(
    reg: &dyn AttachmentRegistry,
    sid: &str,
) {
    reg.upsert(ordinary(sid, "d", ProviderKind::OpenAi, Some(COPY_KEY)))
        .await
        .unwrap();
    let outcome = reg
        .upsert_checked(host(sid, "d", ProviderKind::OpenAi))
        .await
        .unwrap();
    assert_eq!(outcome, UpsertOutcome::OwnershipConflict);
    let after = row(reg, sid, "d", ProviderKind::OpenAi).await;
    assert_eq!(after.origin.as_deref(), Some(origin::USER_UPLOAD));
    assert_eq!(
        after.storage_key.as_deref(),
        Some(COPY_KEY),
        "the copy stays reachable"
    );
    assert_eq!(after.provider_file_id, "pf-1");
}

/// Same-class writes behave exactly as they always did: a host row refreshes
/// every field, an ordinary row keeps its key and origin when the new write has
/// none (the COALESCE), and the outcome is `Written`.
pub(crate) async fn same_class_writes_are_unchanged(reg: &dyn AttachmentRegistry, sid: &str) {
    reg.upsert(host(sid, "h", ProviderKind::OpenAi))
        .await
        .unwrap();
    let mut refreshed = host(sid, "h", ProviderKind::OpenAi);
    refreshed.filename = "renamed.csv".into();
    refreshed.size_bytes = Some(70 * 1024 * 1024);
    assert_eq!(
        reg.upsert_checked(refreshed).await.unwrap(),
        UpsertOutcome::Written
    );
    let h = row(reg, sid, "h", ProviderKind::OpenAi).await;
    assert_eq!(
        (h.filename.as_str(), h.size_bytes),
        ("renamed.csv", Some(70 * 1024 * 1024))
    );
    assert_eq!(h.origin.as_deref(), Some(origin::HOST_STORAGE_REF));

    reg.upsert(ordinary(sid, "o", ProviderKind::OpenAi, Some(COPY_KEY)))
        .await
        .unwrap();
    let mut again = ordinary(sid, "o", ProviderKind::OpenAi, None);
    again.origin = None;
    assert_eq!(
        reg.upsert_checked(again).await.unwrap(),
        UpsertOutcome::Written
    );
    let o = row(reg, sid, "o", ProviderKind::OpenAi).await;
    assert_eq!(
        o.storage_key.as_deref(),
        Some(COPY_KEY),
        "COALESCE keeps the key"
    );
    assert_eq!(
        o.origin.as_deref(),
        Some(origin::USER_UPLOAD),
        "COALESCE keeps the origin"
    );
}

/// The class belongs to a `(session, document, provider)` row: another provider's
/// row for the same document is independent.
pub(crate) async fn rows_of_another_provider_are_independent(
    reg: &dyn AttachmentRegistry,
    sid: &str,
) {
    reg.upsert(host(sid, "d", ProviderKind::OpenAi))
        .await
        .unwrap();
    let outcome = reg
        .upsert_checked(ordinary(sid, "d", ProviderKind::Anthropic, Some(COPY_KEY)))
        .await
        .unwrap();
    assert_eq!(outcome, UpsertOutcome::Written);
    assert_eq!(
        row(reg, sid, "d", ProviderKind::OpenAi)
            .await
            .storage_key
            .as_deref(),
        Some(HOST_KEY)
    );
    assert_eq!(
        row(reg, sid, "d", ProviderKind::Anthropic)
            .await
            .storage_key
            .as_deref(),
        Some(COPY_KEY)
    );
}

/// Both classes race for the same new id: exactly one wins, whole.
pub(crate) async fn concurrent_registration_of_both_classes_never_mixes(
    reg: Arc<dyn AttachmentRegistry>,
    sid: &str,
) {
    for n in 0..25 {
        let doc = format!("race-{n}");
        let (a, b) = (reg.clone(), reg.clone());
        let (sa, sb) = (sid.to_string(), sid.to_string());
        let (da, db) = (doc.clone(), doc.clone());
        let t1 =
            tokio::spawn(
                async move { a.upsert_checked(host(&sa, &da, ProviderKind::OpenAi)).await },
            );
        let t2 = tokio::spawn(async move {
            b.upsert_checked(ordinary(&sb, &db, ProviderKind::OpenAi, Some(COPY_KEY)))
                .await
        });
        let (o1, o2) = (t1.await.unwrap().unwrap(), t2.await.unwrap().unwrap());
        assert!(
            (o1 == UpsertOutcome::Written) != (o2 == UpsertOutcome::Written),
            "exactly one writer wins: {o1:?} {o2:?}"
        );
        let r = row(&*reg, sid, &doc, ProviderKind::OpenAi).await;
        let whole_host = r.origin.as_deref() == Some(origin::HOST_STORAGE_REF)
            && r.storage_key.as_deref() == Some(HOST_KEY)
            && r.provider_file_id.is_empty();
        let whole_engine = r.origin.as_deref() == Some(origin::USER_UPLOAD)
            && r.storage_key.as_deref() == Some(COPY_KEY)
            && r.provider_file_id == "pf-1";
        assert!(whole_host || whole_engine, "never a mix: {r:?}");
    }
}

#[cfg(test)]
mod sqlite {
    use super::*;
    use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;

    async fn registry() -> (Arc<dyn AttachmentRegistry>, tempfile::TempDir) {
        // A file database: the race needs more than one connection.
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}?mode=rwc", dir.path().join("o.db").display());
        let reg: Arc<dyn AttachmentRegistry> =
            Arc::new(SqliteAttachmentRegistry::new(&url).await.unwrap());
        (reg, dir)
    }

    #[tokio::test]
    async fn host_to_ordinary_is_refused() {
        let (reg, _d) = registry().await;
        a_host_row_is_not_rewritten_as_an_ordinary_file(&*reg, "s").await;
    }
    #[tokio::test]
    async fn ordinary_to_host_is_refused() {
        let (reg, _d) = registry().await;
        an_engine_row_is_not_rewritten_as_a_host_reference(&*reg, "s").await;
    }
    #[tokio::test]
    async fn same_class_is_unchanged() {
        let (reg, _d) = registry().await;
        same_class_writes_are_unchanged(&*reg, "s").await;
    }
    #[tokio::test]
    async fn other_provider_is_independent() {
        let (reg, _d) = registry().await;
        rows_of_another_provider_are_independent(&*reg, "s").await;
    }
    #[tokio::test]
    async fn the_race_never_mixes() {
        let (reg, _d) = registry().await;
        concurrent_registration_of_both_classes_never_mixes(reg, "s").await;
    }
}

/// Postgres parity: `DATABASE_URL=postgres://... cargo test --lib ownership_contract -- --ignored`.
#[cfg(test)]
mod postgres {
    use super::*;
    use crate::dag_engine::infrastructure::pool_registry::{PgPoolRegistry, PoolConfig};
    use crate::llm::infrastructure::persistence::PostgresAttachmentRegistry;

    async fn registry() -> (Arc<dyn AttachmentRegistry>, String) {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        let pools = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
        let pool = pools.get_or_create(&url).await.unwrap();
        sqlx::migrate!("migrations/postgres")
            .set_ignore_missing(true)
            .run(&*pool)
            .await
            .unwrap();
        let reg = PostgresAttachmentRegistry::new(pools, &url).await.unwrap();
        (Arc::new(reg), format!("own_{}", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL"]
    async fn host_to_ordinary_is_refused() {
        let (reg, sid) = registry().await;
        a_host_row_is_not_rewritten_as_an_ordinary_file(&*reg, &sid).await;
    }
    #[tokio::test]
    #[ignore = "requires DATABASE_URL"]
    async fn ordinary_to_host_is_refused() {
        let (reg, sid) = registry().await;
        an_engine_row_is_not_rewritten_as_a_host_reference(&*reg, &sid).await;
    }
    #[tokio::test]
    #[ignore = "requires DATABASE_URL"]
    async fn same_class_is_unchanged() {
        let (reg, sid) = registry().await;
        same_class_writes_are_unchanged(&*reg, &sid).await;
    }
    #[tokio::test]
    #[ignore = "requires DATABASE_URL"]
    async fn other_provider_is_independent() {
        let (reg, sid) = registry().await;
        rows_of_another_provider_are_independent(&*reg, &sid).await;
    }
    #[tokio::test]
    #[ignore = "requires DATABASE_URL"]
    async fn the_race_never_mixes() {
        let (reg, sid) = registry().await;
        concurrent_registration_of_both_classes_never_mixes(reg, &sid).await;
    }
}
