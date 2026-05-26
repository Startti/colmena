//! Plan A — Task 12: end-to-end integration tests for the attachment uniform
//! resolution path. Verifies the full chain `AttachmentRegistry +
//! OutputStorageRepository + AttachmentStreamResolver → http_request multipart`
//! works for all three attachment origins:
//!   1. Inline (user-uploaded `data:` URI bytes).
//!   2. Signed URL (caller passes a URL; we persist + register the bytes).
//!   3. Generated artifact (image_generation / image_edit / tts auto-register).
//!
//! These tests are deterministic and require no external services or API keys.
//! They drive `HttpNode::execute` directly with a real `AttachmentStreamResolverImpl`
//! built from `SqliteAttachmentRegistry` (in-memory) + `LocalCacheStorageAdapter`.
//! The destination is a `wiremock::MockServer` that captures the multipart POST.
//!
//! Companion JSON graphs in `tests/graphs/agents/` model what an LLM-driven
//! end-to-end run looks like, but the LLM path itself is exercised by other
//! integration tests + manual `cargo run --bin dag_engine -- run …` smoke tests
//! (skipped here because driving a real LLM would require API keys and live
//! network — see CLAUDE.md `#[ignore]` convention).

use std::collections::HashMap;
use std::sync::Arc;

use colmena::dag_engine::domain::node::ExecutableNode;
use colmena::dag_engine::infrastructure::nodes::http::HttpNode;
use colmena::llm::domain::attachments::AttachmentStreamResolver;
use colmena::llm::domain::attachments::{
    AttachmentRegistry, AttachmentSource, UpsertAttachmentInput,
};
use colmena::llm::domain::ProviderKind;
use colmena::llm::infrastructure::attachments::AttachmentStreamResolverImpl;
use colmena::llm::infrastructure::persistence::SqliteAttachmentRegistry;
use colmena::storage::domain::{OutputStorageRepository, StoreRequest};
use colmena::storage::infrastructure::LocalCacheStorageAdapter;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const AGENT_SESSION_ID: &str = "test_agent_session";

/// Parameters for `seed_attachment`. Bundled into a struct to keep the helper
/// under clippy's `too_many_arguments` ceiling and to make call sites
/// self-documenting.
struct SeedSpec {
    document_id: &'static str,
    bytes: Vec<u8>,
    mime: &'static str,
    filename: &'static str,
    source: AttachmentSource,
    origin: &'static str,
}

/// Persist the bytes in the storage adapter and register the resulting
/// `storage_key` in the attachment registry. Mirrors what tasks 3 / 4 / 5 / 6
/// do during real flows (inline persistence in `llm.rs`, auto-register in
/// `image_generation`/`image_edit`/`tts`).
async fn seed_attachment(
    registry: &SqliteAttachmentRegistry,
    storage: &LocalCacheStorageAdapter,
    spec: SeedSpec,
) {
    let size = spec.bytes.len() as u64;
    let stored = storage
        .store(StoreRequest {
            bytes: spec.bytes,
            mime_type: spec.mime.to_string(),
            filename: spec.filename.to_string(),
            session_id: Some(AGENT_SESSION_ID.to_string()),
            agent_session_id: Some(AGENT_SESSION_ID.to_string()),
        })
        .await
        .expect("storage.store ok");

    registry
        .upsert(UpsertAttachmentInput {
            agent_session_id: AGENT_SESSION_ID.to_string(),
            document_id: spec.document_id.to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-test".to_string(),
            mime_type: spec.mime.to_string(),
            filename: spec.filename.to_string(),
            size_bytes: Some(size),
            label: None,
            description: None,
            source: spec.source,
            storage_key: Some(stored.storage_key),
            origin: Some(spec.origin.to_string()),
        })
        .await
        .expect("registry.upsert ok");
}

/// Build the http_request multipart config that POSTs to `<server>/documents`.
fn mk_config(server_uri: &str, body: Value) -> Value {
    json!({
        "base_url": server_uri,
        "endpoint": "/documents",
        "method": "POST",
        "headers": { "Content-Type": "multipart/form-data" },
        "allow_http_urls": true,
        "body": body,
    })
}

/// Mount the destination POST handler. Returns 201 with a JSON body.
async fn mount_kb_endpoint(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/documents"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "kb-doc-1" })))
        .mount(server)
        .await;
}

/// Run http_request multipart with the `$attachment:<document_id>` body and
/// assert the destination received the exact bytes seeded into storage.
async fn run_upload_and_assert(
    registry: Arc<SqliteAttachmentRegistry>,
    storage: Arc<LocalCacheStorageAdapter>,
    document_id: &str,
    expected_bytes: &[u8],
) {
    let server = MockServer::start().await;
    mount_kb_endpoint(&server).await;

    let resolver: Arc<dyn AttachmentStreamResolver> = Arc::new(AttachmentStreamResolverImpl::new(
        registry as Arc<dyn AttachmentRegistry>,
        storage,
    ));
    let node = HttpNode::new().with_attachment_resolver(resolver);

    let body = json!({ "file": format!("$attachment:{document_id}") });
    let config = mk_config(&server.uri(), body);

    let mut inputs: HashMap<String, Value> = HashMap::new();
    inputs.insert(
        "__colmena_agent_session_id".to_string(),
        json!(AGENT_SESSION_ID),
    );

    let out = node
        .execute(&inputs, &config, &mut json!({}), None)
        .await
        .expect("execute ok");
    assert_eq!(out["status"], 201, "downstream should return 201");

    let received = server
        .received_requests()
        .await
        .expect("received_requests ok");
    let req = received
        .iter()
        .find(|r| r.url.path() == "/documents")
        .expect("POST /documents received");
    let ct = req.headers.get("content-type").unwrap().to_str().unwrap();
    assert!(
        ct.starts_with("multipart/form-data"),
        "expected multipart, got {ct}"
    );
    assert!(ct.contains("boundary="), "boundary missing in {ct}");

    // The body should contain the bytes we seeded — multipart wraps them in a
    // boundary envelope, but the raw payload bytes appear verbatim inside.
    let body_bytes = &req.body;
    let window = expected_bytes.len();
    assert!(
        body_bytes.windows(window).any(|w| w == expected_bytes),
        "downstream body did not contain the seeded bytes (size={window})"
    );
}

#[tokio::test]
async fn inline_doc_can_be_forwarded_via_multipart() {
    let registry = Arc::new(
        SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .expect("sqlite registry ok"),
    );
    let storage = Arc::new(LocalCacheStorageAdapter::new());

    let bytes: Vec<u8> = (0..256u32)
        .map(|i| (i % 256) as u8)
        .cycle()
        .take(1024)
        .collect();
    seed_attachment(
        &registry,
        &storage,
        SeedSpec {
            document_id: "doc_inline_1",
            bytes: bytes.clone(),
            mime: "application/pdf",
            filename: "inline.pdf",
            source: AttachmentSource::Inline,
            origin: "user_upload",
        },
    )
    .await;

    run_upload_and_assert(registry, storage, "doc_inline_1", &bytes).await;
}

#[tokio::test]
async fn signed_url_doc_can_be_forwarded_via_multipart() {
    let registry = Arc::new(
        SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .expect("sqlite registry ok"),
    );
    let storage = Arc::new(LocalCacheStorageAdapter::new());

    // Simulate task 3: caller resolved a signed URL → fetched the bytes →
    // persisted them in storage + registered with source=SignedUrl.
    let bytes: Vec<u8> = [0xDEu8, 0xAD, 0xBE, 0xEF].repeat(512); // 2048 bytes
    seed_attachment(
        &registry,
        &storage,
        SeedSpec {
            document_id: "doc_signed_1",
            bytes: bytes.clone(),
            mime: "application/pdf",
            filename: "from_signed_url.pdf",
            source: AttachmentSource::SignedUrl("https://example.com/signed/foo".to_string()),
            origin: "user_upload",
        },
    )
    .await;

    run_upload_and_assert(registry, storage, "doc_signed_1", &bytes).await;
}

#[tokio::test]
async fn generated_artifact_can_be_forwarded_via_multipart() {
    let registry = Arc::new(
        SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .expect("sqlite registry ok"),
    );
    let storage = Arc::new(LocalCacheStorageAdapter::new());

    // Simulate tasks 4/5/6: image_generation produced an image, persisted the
    // bytes, and auto-registered the row with origin=generated_by:image_generation.
    let bytes: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
    seed_attachment(
        &registry,
        &storage,
        SeedSpec {
            document_id: "doc_generated_1",
            bytes: bytes.clone(),
            mime: "image/png",
            filename: "generated.png",
            // Generated artifacts have no recoverable source — Inline is the
            // current placeholder used by image_generation auto-register.
            source: AttachmentSource::Inline,
            origin: "generated_by:image_generation",
        },
    )
    .await;

    run_upload_and_assert(registry, storage, "doc_generated_1", &bytes).await;
}

/// Negative path: resolver fires NotFound when the document_id is unknown,
/// the downstream endpoint never receives a request, and the http_request
/// node surfaces the error verbatim.
#[tokio::test]
async fn unknown_document_id_errors_before_downstream_post() {
    let registry = Arc::new(
        SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .expect("sqlite registry ok"),
    );
    let storage = Arc::new(LocalCacheStorageAdapter::new());
    let server = MockServer::start().await;
    mount_kb_endpoint(&server).await;

    let resolver: Arc<dyn AttachmentStreamResolver> = Arc::new(AttachmentStreamResolverImpl::new(
        registry as Arc<dyn AttachmentRegistry>,
        storage,
    ));
    let node = HttpNode::new().with_attachment_resolver(resolver);

    let body = json!({ "file": "$attachment:doc_does_not_exist" });
    let config = mk_config(&server.uri(), body);
    let mut inputs: HashMap<String, Value> = HashMap::new();
    inputs.insert(
        "__colmena_agent_session_id".to_string(),
        json!(AGENT_SESSION_ID),
    );

    let err = node
        .execute(&inputs, &config, &mut json!({}), None)
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("AttachmentResolveError") || msg.contains("not found"),
        "expected resolver error, got: {msg}"
    );

    let received = server
        .received_requests()
        .await
        .expect("received_requests ok");
    assert!(
        received.iter().all(|r| r.url.path() != "/documents"),
        "downstream must not receive a POST when resolution fails"
    );
}
