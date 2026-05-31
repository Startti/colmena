//! Snapshot tests for HTML renderer per theme.
//! Stores the rendered HTML for a canonical IR per theme and detects
//! accidental regressions. To regenerate snapshots: delete the file
//! and run the test once (it panics on first creation, passes on re-run).

use colmena::documents::application::apply_patch::ApplyPatchInput;
use colmena::documents::application::create_document::CreateDocumentInput;
use colmena::documents::application::runtime::DocumentRuntime;
use colmena::documents::domain::ids::{ArtifactKind, SessionId};
use colmena::documents::domain::ir::html::{FooterConfig, Theme};
use colmena::documents::domain::patch::{Patch, PatchOp, PatchSource};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

async fn render_canonical(theme: Theme) -> String {
    let tmp = tempdir().unwrap();
    let cfg = json!({
        "storage_root": tmp.path().join("a").to_str().unwrap(),
        "asset_storage_root": tmp.path().join("b").to_str().unwrap()
    });
    let rt = DocumentRuntime::from_config(&cfg).await.unwrap();
    let created = rt
        .create
        .execute(CreateDocumentInput {
            kind: ArtifactKind::Html,
            session_id: SessionId::new("s"),
            label: None,
            retention_limit: None,
            initial_ir: None,
            source: PatchSource::Agent,
        })
        .await
        .unwrap();
    let _ = rt
        .apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: created.artifact_id.0.clone(),
                base_version: "v1".into(),
                source: PatchSource::Agent,
                ops: vec![
                    PatchOp::SetTheme { theme },
                    PatchOp::SetFooter {
                        footer: FooterConfig {
                            enabled: true,
                            page_numbers: true,
                            custom_text: Some("Test".into()),
                        },
                    },
                ],
            },
        })
        .await
        .unwrap();
    let data = rt.store.read_current(&created.artifact_id).await.unwrap();
    String::from_utf8(data.rendered_binary).unwrap()
}

fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.html"))
}

fn assert_or_write_snapshot(name: &str, content: &str) {
    let path = snapshot_path(name);
    if let Ok(existing) = fs::read_to_string(&path) {
        assert_eq!(
            existing.trim(),
            content.trim(),
            "snapshot mismatch for {name}; delete {path:?} to regenerate"
        );
    } else {
        fs::write(&path, content).unwrap();
        panic!("snapshot {name} created at {path:?}. Re-run the test to assert.");
    }
}

#[tokio::test]
async fn snapshot_executive_report() {
    let html = render_canonical(Theme::Executive).await;
    assert_or_write_snapshot("executive_report", &html);
}

#[tokio::test]
async fn snapshot_minimal_report() {
    let html = render_canonical(Theme::Minimal).await;
    assert_or_write_snapshot("minimal_report", &html);
}

#[tokio::test]
async fn snapshot_vibrant_report() {
    let html = render_canonical(Theme::Vibrant).await;
    assert_or_write_snapshot("vibrant_report", &html);
}

#[tokio::test]
async fn snapshot_dark_report() {
    let html = render_canonical(Theme::Dark).await;
    assert_or_write_snapshot("dark_report", &html);
}
