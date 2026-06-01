//! Demo: generate rich HTML outputs (one per theme) to /tmp/colmena_html_demo/.
//! Open them in any browser to verify the renderer visually.
//!
//! Run with:
//!   cargo run --example html_renderer_demo -p colmena_dag_engine
//!
//! After running, open:
//!   open /tmp/colmena_html_demo/report_executive.html
//!   open /tmp/colmena_html_demo/report_minimal.html
//!   open /tmp/colmena_html_demo/report_vibrant.html
//!   open /tmp/colmena_html_demo/report_dark.html

use colmena::documents::application::apply_patch::ApplyPatchInput;
use colmena::documents::application::create_document::CreateDocumentInput;
use colmena::documents::application::runtime::DocumentRuntime;
use colmena::documents::application::upload_asset::UploadAssetInput;
use colmena::documents::domain::ids::{ArtifactKind, SessionId};
use colmena::documents::domain::ir::html::{FooterConfig, Locale, SlideLayout, Theme};
use colmena::documents::domain::patch::{Patch, PatchOp, PatchSource};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

// Minimal 1×1 transparent PNG (68 bytes)
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG sig
    0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR length + type
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1×1
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, // RGBA, CRC
    0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, // IDAT length + type
    0x78, 0x9C, 0x62, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, // deflate
    0x0D, 0x0A, 0x2D, 0xB4, // CRC
    0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, // IEND length + type
    0xAE, 0x42, 0x60, 0x82, // CRC
];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from("/tmp/colmena_html_demo");
    if out_dir.exists() {
        fs::remove_dir_all(&out_dir)?;
    }
    fs::create_dir_all(&out_dir)?;

    for theme in [
        Theme::Executive,
        Theme::Minimal,
        Theme::Vibrant,
        Theme::Dark,
    ] {
        generate(theme, &out_dir).await?;
    }

    println!("\n=== Generated demo HTMLs ===");
    println!("Open any of these in your browser:");
    let mut entries: Vec<_> = fs::read_dir(&out_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "html").unwrap_or(false))
        .collect();
    entries.sort();
    for path in &entries {
        println!("  open {}", path.display());
    }
    println!("\nQuick one-liner:");
    println!("  open {}/report_executive.html", out_dir.display());
    Ok(())
}

async fn generate(theme: Theme, out_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempdir()?;
    let cfg = json!({
        "storage_root":       tmp.path().join("artifacts").to_str().unwrap(),
        "asset_storage_root": tmp.path().join("assets").to_str().unwrap(),
    });
    let rt = DocumentRuntime::from_config(&cfg).await?;
    let session = SessionId::new("demo");

    // --- Upload a tiny logo so the image block can reference an asset ----------
    let logo = rt
        .upload_asset
        .execute(UploadAssetInput {
            session_id: session.clone(),
            bytes: TINY_PNG.to_vec(),
            mime: "image/png".into(),
            label: Some("logo".into()),
        })
        .await?;

    // --- Create the artifact (v1) ----------------------------------------------
    let created = rt
        .create
        .execute(CreateDocumentInput {
            kind: ArtifactKind::Html,
            session_id: session.clone(),
            label: Some(format!("Demo — {} report", theme_name(theme))),
            retention_limit: None,
            initial_ir: None,
            source: PatchSource::Agent,
        })
        .await?;

    let aid = created.artifact_id.0.clone();

    // --- Patch 1 (v1→v2): doc-level props + theme + footer --------------------
    rt.apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: aid.clone(),
                base_version: "v1".into(),
                source: PatchSource::Agent,
                ops: vec![
                    PatchOp::SetTheme { theme },
                    PatchOp::SetDocProps {
                        title: Some(format!("Q3 2026 — {}", theme_name(theme))),
                        author: Some("Daniel Garcia".into()),
                        date: Some("2026-10-30".into()),
                        locale: Some(Locale::Es),
                    },
                    PatchOp::SetFooter {
                        footer: FooterConfig {
                            enabled: true,
                            page_numbers: true,
                            custom_text: Some("Colmena · Confidencial".into()),
                        },
                    },
                ],
            },
        })
        .await?;

    // --- Patch 2 (v2→v3): add 4 slides, capture minted ids -------------------
    let v3 = rt
        .apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: aid.clone(),
                base_version: "v2".into(),
                source: PatchSource::Agent,
                ops: vec![
                    PatchOp::AddSlide {
                        layout: SlideLayout::Title,
                        at_index: None,
                        title: Some(format!("Reporte Q3 2026 — {}", theme_name(theme))),
                        subtitle: Some("Resultados ejecutivos".into()),
                    },
                    PatchOp::AddSlide {
                        layout: SlideLayout::Content,
                        at_index: None,
                        title: Some("KPIs principales".into()),
                        subtitle: None,
                    },
                    PatchOp::AddSlide {
                        layout: SlideLayout::Content,
                        at_index: None,
                        title: Some("Tendencias de ventas".into()),
                        subtitle: None,
                    },
                    PatchOp::AddSlide {
                        layout: SlideLayout::SectionDivider,
                        at_index: None,
                        title: Some("Próximos pasos".into()),
                        subtitle: None,
                    },
                ],
            },
        })
        .await?;

    // Extract minted slide IDs from the structured summary entries.
    // Each entry: {"op_index": N, "op": "add_slide", "assigned_ids": {"slide": "slide_xxx"}}
    let slide_ids: Vec<String> = v3
        .summary
        .structured
        .iter()
        .filter_map(|entry| {
            entry
                .get("assigned_ids")
                .and_then(|a| a.get("slide"))
                .and_then(|s| s.as_str())
                .map(String::from)
        })
        .collect();

    if slide_ids.len() < 4 {
        return Err(format!(
            "expected 4 slide ids from AddSlide ops, got {} (structured: {})",
            slide_ids.len(),
            serde_json::to_string_pretty(&v3.summary.structured).unwrap_or_default()
        )
        .into());
    }
    let title_slide = &slide_ids[0];
    let kpis_slide = &slide_ids[1];
    let chart_slide = &slide_ids[2];
    // slide_ids[3] is the section divider — no blocks needed there

    // --- Patch 3 (v3→v4): rich content blocks ---------------------------------
    rt.apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: aid.clone(),
                base_version: v3.version_id.0.clone(),
                source: PatchSource::Agent,
                ops: vec![
                    // ----- Title slide: hero logo + intro paragraph -----
                    PatchOp::InsertHtmlBlock {
                        slide_id: title_slide.clone(),
                        before: None,
                        after: None,
                        block: json!({
                            "kind": "image",
                            "src": {
                                "kind": "asset",
                                "asset_id": logo.asset_id.as_str()
                            },
                            "alt": "Colmena logo",
                            "caption": null,
                            "position": "hero"
                        }),
                    },
                    PatchOp::InsertHtmlBlock {
                        slide_id: title_slide.clone(),
                        before: None,
                        after: None,
                        block: json!({
                            "kind": "paragraph",
                            "runs": [
                                {
                                    "text": "Generado automáticamente — ",
                                    "bold": false, "italic": false,
                                    "underline": false, "code": false
                                },
                                {
                                    "text": "ver fuente en GitHub",
                                    "bold": false, "italic": false,
                                    "underline": false, "code": false,
                                    "link": "https://github.com/startti/colmena"
                                }
                            ]
                        }),
                    },
                    // ----- KPIs slide: KPI grid + callout -----
                    PatchOp::InsertHtmlBlock {
                        slide_id: kpis_slide.clone(),
                        before: None,
                        after: None,
                        block: json!({
                            "kind": "kpi_grid",
                            "columns": 3,
                            "cards": [
                                {
                                    "label": "Ingresos",
                                    "value": "$1.2M",
                                    "delta": {"value": "+12.4%", "direction": "up"}
                                },
                                {
                                    "label": "Clientes activos",
                                    "value": "342",
                                    "delta": {"value": "+28", "direction": "up"}
                                },
                                {
                                    "label": "Churn rate",
                                    "value": "2.1%",
                                    "delta": {"value": "-0.3%", "direction": "down"}
                                }
                            ]
                        }),
                    },
                    PatchOp::InsertHtmlBlock {
                        slide_id: kpis_slide.clone(),
                        before: None,
                        after: None,
                        block: json!({
                            "kind": "callout",
                            "variant": "success",
                            "title": "Highlight del trimestre",
                            "runs": [
                                {
                                    "text": "Mejor resultado trimestral en 18 meses.",
                                    "bold": false, "italic": false,
                                    "underline": false, "code": false
                                }
                            ]
                        }),
                    },
                    PatchOp::InsertHtmlBlock {
                        slide_id: kpis_slide.clone(),
                        before: None,
                        after: None,
                        block: json!({
                            "kind": "callout",
                            "variant": "warning",
                            "title": "Atención",
                            "runs": [
                                {
                                    "text": "El margen operativo cayó 1.2 pp frente al trimestre anterior.",
                                    "bold": false, "italic": false,
                                    "underline": false, "code": false
                                }
                            ]
                        }),
                    },
                    // ----- Chart slide: bar chart + table -----
                    PatchOp::InsertHtmlBlock {
                        slide_id: chart_slide.clone(),
                        before: None,
                        after: None,
                        block: json!({
                            "kind": "chart",
                            "chart": {
                                "chart_type": "bar",
                                "series": [
                                    {
                                        "name": "Ventas 2025",
                                        "data": [200, 280, 310, 340]
                                    },
                                    {
                                        "name": "Ventas 2026",
                                        "data": [240, 320, 380, 420]
                                    }
                                ],
                                "x_axis": {
                                    "categories": ["Q1", "Q2", "Q3", "Q4"]
                                },
                                "legend": true
                            },
                            "title": "Comparativa anual de ventas",
                            "size": "large"
                        }),
                    },
                    PatchOp::InsertHtmlBlock {
                        slide_id: chart_slide.clone(),
                        before: None,
                        after: None,
                        block: json!({
                            "kind": "table",
                            "headers": ["Producto", "Unidades vendidas", "Revenue"],
                            "rows": [
                                {
                                    "id": "row_1",
                                    "cells": [
                                        {"type": "text",   "value": "Plan Pro"},
                                        {"type": "number", "value": 120},
                                        {"type": "text",   "value": "$480k"}
                                    ]
                                },
                                {
                                    "id": "row_2",
                                    "cells": [
                                        {"type": "text",   "value": "Plan Team"},
                                        {"type": "number", "value": 85},
                                        {"type": "text",   "value": "$425k"}
                                    ]
                                },
                                {
                                    "id": "row_3",
                                    "cells": [
                                        {"type": "text",   "value": "Plan Enterprise"},
                                        {"type": "number", "value": 12},
                                        {"type": "text",   "value": "$295k"}
                                    ]
                                }
                            ],
                            "caption": "Desglose de revenue por producto — Q3 2026"
                        }),
                    },
                ],
            },
        })
        .await?;

    // --- Read final rendered HTML and write to /tmp ---------------------------
    let data = rt.store.read_current(&created.artifact_id).await?;
    let html = String::from_utf8(data.rendered_binary)?;
    let filename = out_dir.join(format!("report_{}.html", theme_name(theme)));
    fs::write(&filename, &html)?;
    println!("  wrote {} ({} KB)", filename.display(), html.len() / 1024);
    Ok(())
}

fn theme_name(t: Theme) -> &'static str {
    match t {
        Theme::Executive => "executive",
        Theme::Minimal => "minimal",
        Theme::Vibrant => "vibrant",
        Theme::Dark => "dark",
    }
}
