//! End-to-end test for the HTML documents module (spec §16).
//! Verifies create → apply (theme + slide + image + chart blocks) → read renders
//! produce a self-contained HTML that contains the expected markers.

use colmena::documents::application::apply_patch::ApplyPatchInput;
use colmena::documents::application::create_document::CreateDocumentInput;
use colmena::documents::application::runtime::DocumentRuntime;
use colmena::documents::application::upload_asset::UploadAssetInput;
use colmena::documents::domain::ids::{ArtifactKind, SessionId};
use colmena::documents::domain::ir::html::{Locale, SlideLayout, Theme};
use colmena::documents::domain::patch::{Patch, PatchOp, PatchSource};
use serde_json::json;
use tempfile::tempdir;

// Minimal 1x1 transparent PNG (89 bytes) for asset tests.
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
    0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
    0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x62, 0x00,
    0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
    0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[tokio::test]
async fn end_to_end_html_module() {
    let tmp = tempdir().unwrap();
    let cfg = json!({
        "storage_root": tmp.path().join("artifacts").to_str().unwrap(),
        "asset_storage_root": tmp.path().join("assets").to_str().unwrap(),
    });
    let rt = DocumentRuntime::from_config(&cfg).await.unwrap();
    let session = SessionId::new("test_session");

    // 1. Upload an asset (logo PNG)
    let upload = rt
        .upload_asset
        .execute(UploadAssetInput {
            session_id: session.clone(),
            bytes: TINY_PNG.to_vec(),
            mime: "image/png".into(),
            label: Some("company_logo".into()),
        })
        .await
        .unwrap();

    // 2. Create HTML doc (v1)
    let created = rt
        .create
        .execute(CreateDocumentInput {
            kind: ArtifactKind::Html,
            session_id: session.clone(),
            label: Some("Q3 Report".into()),
            retention_limit: None,
            initial_ir: None,
            source: PatchSource::Agent,
        })
        .await
        .unwrap();

    // 3. v1 → v2: set theme + props
    let _v2 = rt
        .apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: created.artifact_id.0.clone(),
                base_version: "v1".into(),
                source: PatchSource::Agent,
                ops: vec![
                    PatchOp::SetTheme {
                        theme: Theme::Executive,
                    },
                    PatchOp::SetDocProps {
                        title: Some("Q3 Results".into()),
                        author: Some("Daniel".into()),
                        date: Some("2026-10-30".into()),
                        locale: Some(Locale::Es),
                    },
                ],
            },
        })
        .await
        .unwrap();

    // 4. v2 → v3: add a title slide
    let v3 = rt
        .apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: created.artifact_id.0.clone(),
                base_version: "v2".into(),
                source: PatchSource::Agent,
                ops: vec![PatchOp::AddSlide {
                    layout: SlideLayout::Title,
                    at_index: None,
                    title: Some("Q3 Results".into()),
                    subtitle: Some("2026".into()),
                }],
            },
        })
        .await
        .unwrap();

    // Extract the slide_id assigned by AddSlide from the structured summary.
    // Each entry looks like: {"op_index": N, "op": "add_slide", "assigned_ids": {"slide": "sl_xxx"}}
    let new_slide_id = v3
        .summary
        .structured
        .iter()
        .find_map(|e| {
            e.get("assigned_ids")
                .and_then(|a| a.get("slide"))
                .and_then(|s| s.as_str())
                .map(String::from)
        })
        .expect("add_slide must return slide_id in summary.structured");

    // 5. v3 → v4: insert image (referencing the asset) and a chart on the new slide
    let _v4 = rt
        .apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: created.artifact_id.0.clone(),
                base_version: v3.version_id.0.clone(),
                source: PatchSource::Agent,
                ops: vec![
                    PatchOp::InsertHtmlBlock {
                        slide_id: new_slide_id.clone(),
                        before: None,
                        after: None,
                        block: json!({
                            "kind": "image",
                            "src": { "kind": "asset", "asset_id": upload.asset_id.as_str() },
                            "alt": "logo",
                            "caption": null,
                            "position": "hero"
                        }),
                    },
                    PatchOp::InsertHtmlBlock {
                        slide_id: new_slide_id.clone(),
                        before: None,
                        after: None,
                        block: json!({
                            "kind": "chart",
                            "chart": {
                                "chart_type": "bar",
                                "series": [{"name":"Sales","data":[10.0,20.0,30.0]}],
                                "x_axis": {"categories":["Q1","Q2","Q3"]},
                                "legend": true
                            },
                            "title": "Q3 Sales",
                            "size": "medium"
                        }),
                    },
                ],
            },
        })
        .await
        .unwrap();

    // 6. Read the latest bytes from the artifact store
    let data = rt.store.read_current(&created.artifact_id).await.unwrap();
    let html = String::from_utf8(data.rendered_binary).unwrap();

    // 7. Assertions — verify the full stack produced a coherent HTML document.
    assert!(
        html.starts_with("<!DOCTYPE html>"),
        "expected DOCTYPE at start, got: {}",
        &html[..html.len().min(50)]
    );
    assert!(
        html.contains("Q3 Results"),
        "title text missing from rendered HTML"
    );
    assert!(
        html.contains("lang=\"es\""),
        "locale es not reflected in lang attribute"
    );
    assert!(
        html.contains("--font-heading"),
        "executive theme CSS variable --font-heading missing"
    );
    assert!(
        html.contains("data:image/png;base64,"),
        "asset not base64-inlined in rendered HTML"
    );
    assert!(
        html.contains("new Chart"),
        "Chart.js init script missing from rendered HTML"
    );
}
