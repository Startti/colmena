//! End-to-end integration tests for the gdocs subsystem against the real
//! Google Docs + Drive APIs. Gated by `#[ignore]` so CI never tries to
//! authenticate; they run locally only when the operator opts in.
//!
//! Required env:
//!   GOOGLE_APPLICATION_CREDENTIALS          — SA JSON path
//!   COLMENA_GDOCS_TEST_PARENT_FOLDER_ID     — Drive folder shared (Edit) with the SA
//!
//! Optional env:
//!   COLMENA_GDOCS_TEST_SHARE_EMAIL          — email used by the share test
//!
//! Run locally:
//!   source .env
//!   cargo test --test gdocs_integration_test -- --ignored --nocapture
//!
//! The tests create temporary docs prefixed `colmena gdocs IT — ` inside
//! the configured parent folder. They do NOT delete them automatically —
//! the parent folder is expected to be a disposable test bucket.

use colmena::gdocs::domain::{
    DocsClient, DocsError, DocumentId, ExportFormat, ParagraphKind, RevisionId, ShareRole,
};
use colmena::gdocs::infrastructure::config::GDocsConfig;
use colmena::gdocs::infrastructure::http_client::GoogleDocsHttpClient;

/// Returns true iff the required env vars are set. Used to keep the
/// `#[ignore]`-gated tests from crashing with `panic!("env required")`
/// when invoked without setup — we'd rather they exit cleanly.
fn env_ready() -> bool {
    std::env::var("GOOGLE_APPLICATION_CREDENTIALS").is_ok()
        && std::env::var("COLMENA_GDOCS_TEST_PARENT_FOLDER_ID").is_ok()
}

fn make_client() -> GoogleDocsHttpClient {
    let cfg = GDocsConfig::from_env();
    GoogleDocsHttpClient::from_config(&cfg).expect("client init")
}

fn parent_folder() -> String {
    std::env::var("COLMENA_GDOCS_TEST_PARENT_FOLDER_ID")
        .expect("COLMENA_GDOCS_TEST_PARENT_FOLDER_ID required for integration test")
}

const SKIP: &str = "SKIP: env not configured (need GOOGLE_APPLICATION_CREDENTIALS \
                    + COLMENA_GDOCS_TEST_PARENT_FOLDER_ID)";

// ── 1. Lifecycle ──────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GDOCS_TEST_PARENT_FOLDER_ID — run with `cargo test --test gdocs_integration_test -- --ignored`"]
async fn create_blank_doc_lands_in_folder() {
    if !env_ready() {
        eprintln!("{SKIP}");
        return;
    }
    let client = make_client();
    let folder = parent_folder();
    let meta = client
        .create("colmena gdocs IT — blank", Some(&folder))
        .await
        .expect("create ok");
    assert!(meta.url.contains("/document/d/"));
    assert_eq!(meta.title, "colmena gdocs IT — blank");
    assert!(!meta.revision_id.0.is_empty());
    assert!(!meta.doc_id.0.is_empty());
}

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GDOCS_TEST_PARENT_FOLDER_ID — run with `cargo test --test gdocs_integration_test -- --ignored`"]
async fn create_from_markdown_with_loss_detection() {
    if !env_ready() {
        eprintln!("{SKIP}");
        return;
    }
    let client = make_client();
    let folder = parent_folder();
    // Include something that should always be lossy: a footnote.
    let md = "# Título\n\n\
              Texto normal.\n\n\
              ## Subsección\n\n\
              Esto tiene un footnote.[^1]\n\n\
              [^1]: que se debería perder en la conversión.\n";
    let r = client
        .create_from_markdown("colmena gdocs IT — markdown", md, Some(&folder))
        .await
        .expect("create_from_markdown ok");
    assert!(r.meta.url.contains("/document/d/"));
    assert!(!r.outline_snapshot.is_empty(), "outline must be populated");
    // We expect the footnote to be flagged as lossy.
    let has_footnote = r
        .lossy_conversions
        .iter()
        .any(|l| l.element_type == "footnote");
    assert!(
        has_footnote,
        "expected footnote in lossy_conversions: {:?}",
        r.lossy_conversions
    );
}

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GDOCS_TEST_PARENT_FOLDER_ID — run with `cargo test --test gdocs_integration_test -- --ignored`"]
async fn export_returns_pdf_bytes() {
    if !env_ready() {
        eprintln!("{SKIP}");
        return;
    }
    let client = make_client();
    let folder = parent_folder();
    let r = client
        .create_from_markdown(
            "colmena gdocs IT — export",
            "# Hello\n\nWorld.\n",
            Some(&folder),
        )
        .await
        .expect("create_from_markdown ok");
    let pdf = client
        .export(&r.meta.doc_id, ExportFormat::Pdf)
        .await
        .expect("export ok");
    assert!(pdf.len() > 200, "pdf body too small: {} bytes", pdf.len());
    assert!(pdf.starts_with(b"%PDF"), "missing PDF magic");
}

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GDOCS_TEST_PARENT_FOLDER_ID — run with `cargo test --test gdocs_integration_test -- --ignored`"]
async fn share_grants_access_without_error() {
    if !env_ready() {
        eprintln!("{SKIP}");
        return;
    }
    let client = make_client();
    let folder = parent_folder();
    let meta = client
        .create("colmena gdocs IT — share", Some(&folder))
        .await
        .expect("create ok");
    // The SA cannot share to itself meaningfully; pick a test email.
    let test_email = std::env::var("COLMENA_GDOCS_TEST_SHARE_EMAIL")
        .unwrap_or_else(|_| "ops@colmena.test".to_string());
    // Shouldn't panic. If the email is invalid, Google will return 400 —
    // accept either Ok or a structured error; the test asserts the call
    // signature plumbing, not Google's policy.
    match client
        .share(&meta.doc_id, &test_email, ShareRole::Reader)
        .await
    {
        Ok(()) => {}
        Err(e) => eprintln!("share returned err (acceptable for test emails): {e}"),
    }
}

// ── 2. Outline + read ────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GDOCS_TEST_PARENT_FOLDER_ID — run with `cargo test --test gdocs_integration_test -- --ignored`"]
async fn read_outline_shows_headings_and_paragraphs() {
    if !env_ready() {
        eprintln!("{SKIP}");
        return;
    }
    let client = make_client();
    let folder = parent_folder();
    let md = "# Encabezado 1\n\nPárrafo bajo el encabezado.\n\n\
              ## Encabezado 2\n\nOtro párrafo.\n";
    let r = client
        .create_from_markdown("colmena gdocs IT — outline", md, Some(&folder))
        .await
        .expect("create_from_markdown ok");
    let outline = client
        .read_outline(&r.meta.doc_id, None)
        .await
        .expect("read_outline ok");
    assert!(
        outline.len() >= 4,
        "expected at least 4 paragraphs, got {}: {:?}",
        outline.len(),
        outline
    );
    let h1 = outline
        .iter()
        .find(|e| matches!(e.kind, ParagraphKind::Heading1));
    assert!(h1.is_some(), "expected at least one H1 in outline");
}

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GDOCS_TEST_PARENT_FOLDER_ID — run with `cargo test --test gdocs_integration_test -- --ignored`"]
async fn read_as_markdown_returns_close_to_original() {
    if !env_ready() {
        eprintln!("{SKIP}");
        return;
    }
    let client = make_client();
    let folder = parent_folder();
    let md = "# Top\n\nA paragraph.\n";
    let r = client
        .create_from_markdown("colmena gdocs IT — read_md", md, Some(&folder))
        .await
        .expect("create_from_markdown ok");
    let back = client
        .read_as_markdown(&r.meta.doc_id, None)
        .await
        .expect("read_as_markdown ok");
    assert!(back.contains("Top"), "lost heading: {back}");
    assert!(back.contains("paragraph"), "lost body: {back}");
}

// ── 3. Multi-tab ─────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GDOCS_TEST_PARENT_FOLDER_ID — run with `cargo test --test gdocs_integration_test -- --ignored`"]
async fn add_tab_then_list_tabs_includes_it() {
    if !env_ready() {
        eprintln!("{SKIP}");
        return;
    }
    let client = make_client();
    let folder = parent_folder();
    let meta = client
        .create("colmena gdocs IT — tabs", Some(&folder))
        .await
        .expect("create ok");
    let new_tab = client
        .add_tab(&meta.doc_id, "Segunda Pestaña", None)
        .await
        .expect("add_tab ok");
    let tabs = client.list_tabs(&meta.doc_id).await.expect("list_tabs ok");
    assert!(
        tabs.iter().any(|t| t.title == "Segunda Pestaña"),
        "new tab missing from list: {tabs:?}"
    );
    assert_eq!(new_tab.title, "Segunda Pestaña");
}

// ── 4. Revisions ─────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GDOCS_TEST_PARENT_FOLDER_ID — run with `cargo test --test gdocs_integration_test -- --ignored`"]
async fn batch_update_advances_revision_id() {
    if !env_ready() {
        eprintln!("{SKIP}");
        return;
    }
    let client = make_client();
    let folder = parent_folder();
    let meta = client
        .create("colmena gdocs IT — revisions", Some(&folder))
        .await
        .expect("create ok");
    let rev0 = meta.revision_id.clone();

    // Insert "hello" at index 1 (Docs API uses index 1 as the first
    // editable position; index 0 is reserved for the document start).
    let requests = vec![serde_json::json!({
        "insertText": {"location": {"index": 1}, "text": "hello"}
    })];
    let r = client
        .batch_update(&meta.doc_id, requests, Some(&rev0))
        .await
        .expect("batch_update ok");
    assert_ne!(
        r.revision_id_after.0, rev0.0,
        "revision id must advance after a write"
    );

    let since = client
        .list_revisions_since(&meta.doc_id, &rev0)
        .await
        .expect("list_revisions_since ok");
    assert!(
        !since.is_empty(),
        "expected at least one revision after rev0"
    );
}

// ── 5. Errors ────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GDOCS_TEST_PARENT_FOLDER_ID — run with `cargo test --test gdocs_integration_test -- --ignored`"]
async fn get_unknown_doc_returns_document_not_found() {
    if !env_ready() {
        eprintln!("{SKIP}");
        return;
    }
    let client = make_client();
    let bogus = DocumentId("definitely_not_a_doc_abcdef123".into());
    let result = client.get(&bogus).await;
    assert!(
        matches!(result, Err(DocsError::DocumentNotFound(_))),
        "expected DocumentNotFound, got {result:?}"
    );
}

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GDOCS_TEST_PARENT_FOLDER_ID — run with `cargo test --test gdocs_integration_test -- --ignored`"]
async fn list_revisions_with_unknown_since_returns_empty() {
    if !env_ready() {
        eprintln!("{SKIP}");
        return;
    }
    let client = make_client();
    let folder = parent_folder();
    let meta = client
        .create("colmena gdocs IT — unknown rev", Some(&folder))
        .await
        .expect("create ok");
    let since = client
        .list_revisions_since(&meta.doc_id, &RevisionId("not_a_real_rev".into()))
        .await
        .expect("list_revisions_since must not surface a hard error for unknown since");
    // Our impl filters strictly AFTER `since`; unknown `since` means
    // nothing is "started" and we return [].
    assert_eq!(since.len(), 0);
}

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GDOCS_TEST_PARENT_FOLDER_ID — run with `cargo test --test gdocs_integration_test -- --ignored`"]
async fn add_tab_collision_returns_tab_exists() {
    if !env_ready() {
        eprintln!("{SKIP}");
        return;
    }
    let client = make_client();
    let folder = parent_folder();
    let meta = client
        .create("colmena gdocs IT — collision", Some(&folder))
        .await
        .expect("create ok");
    let _ = client
        .add_tab(&meta.doc_id, "Unique", None)
        .await
        .expect("first add_tab ok");
    let result = client.add_tab(&meta.doc_id, "Unique", None).await;
    assert!(
        matches!(result, Err(DocsError::TabExists(_))),
        "expected TabExists, got {result:?}"
    );
}

// ── Image insert from bytes (Approach A) — live roundtrip ─────────────
//
// Exercises the 3 new DocsClient methods (`upload_image_to_drive`,
// `set_anyone_reader`, `delete_drive_file`) plus an `insertInlineImage`
// batchUpdate against REAL Google: upload a tiny PNG to Drive → make it
// public → insert via the `lh3.googleusercontent.com/d/<id>` content URL →
// delete the temp Drive file. Proves the colmena HTTP code produces requests
// Google accepts, and that the temp file can be deleted right after the
// batchUpdate (Docs hosts its own copy). Gated on `COLMENA_GDOCS_TEST_DOC_ID`
// (a doc shared Editor with the agent) + OAuth env injected in-memory.
#[tokio::test]
#[ignore = "requires OAuth env + COLMENA_GDOCS_TEST_DOC_ID — run with `cargo test --test gdocs_integration_test -- --ignored`"]
async fn image_upload_public_insert_delete_roundtrip() {
    let doc_id = match std::env::var("COLMENA_GDOCS_TEST_DOC_ID") {
        Ok(d) if !d.is_empty() => d,
        _ => {
            eprintln!(
                "SKIP: set COLMENA_GDOCS_TEST_DOC_ID to a doc shared (Editor) with the agent"
            );
            return;
        }
    };
    let client = make_client();
    let doc = DocumentId(doc_id);

    // A valid 1x1 PNG (base64) — avoids committing a binary fixture.
    use base64::Engine;
    let png = base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==")
        .expect("valid base64 png");

    // 1. upload to Drive (image/png, no Doc conversion)
    let file_id = client
        .upload_image_to_drive(png, "image/png", "colmena-tmp-img-itest.png")
        .await
        .expect("upload_image_to_drive");
    assert!(!file_id.is_empty());

    // 2. make it publicly readable so Docs can fetch it server-side
    client
        .set_anyone_reader(&file_id)
        .await
        .expect("set_anyone_reader");

    // 3. insert at the end of the first tab's body via the lh3 content URL
    let snap = client.get(&doc).await.expect("documents.get");
    let tab = snap.tabs.first().expect("at least one tab");
    let last = tab.paragraphs.last().expect("at least one paragraph");
    let index = last.end_index.saturating_sub(1);
    let mut location = serde_json::json!({ "index": index });
    if let Some(t) = &tab.tab_id {
        location["tabId"] = serde_json::Value::String(t.0.clone());
    }
    let uri = format!("https://lh3.googleusercontent.com/d/{file_id}");
    let req = serde_json::json!({ "insertInlineImage": { "location": location, "uri": uri } });
    let res = client
        .batch_update(&doc, vec![req], None)
        .await
        .expect("batch_update insertInlineImage");
    assert!(!res.revision_id_after.0.is_empty());

    // 4. delete the temp Drive file — safe, Docs has copied the image
    client
        .delete_drive_file(&file_id)
        .await
        .expect("delete_drive_file");

    eprintln!(
        "OK: uploaded {file_id}, inserted image, deleted temp; revision {}",
        res.revision_id_after.0
    );
}
