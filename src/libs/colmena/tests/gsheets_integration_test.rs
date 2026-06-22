//! Integration test for Google Sheets — hits the real API. Gated by
//! `#[ignore]` so it doesn't run in CI without explicit opt-in.
//!
//! Required env:
//!   GOOGLE_APPLICATION_CREDENTIALS=/path/to/sa.json
//!   COLMENA_GSHEETS_TEST_SPREADSHEET_ID=<id of an empty test sheet>
//!
//! The test sheet must be shared (Edit) with the SA email. Tests create
//! temporary tabs prefixed "e2e_" / "formula_" and delete them at the end.
//!
//! Run locally:
//!   source .env
//!   cargo test --test gsheets_integration_test -- --ignored --nocapture

use colmena::gsheets::application::format::{
    a1_to_grid_range, build_format_requests, BorderSide, Borders, FormatSpec, TextFormat,
};
use colmena::gsheets::domain::{
    CellValue, ReadOptions, SheetsClient, SheetsError, SpreadsheetId, ValueRenderOption,
};
use colmena::gsheets::infrastructure::config::GSheetsConfig;
use colmena::gsheets::infrastructure::http_client::GoogleSheetsHttpClient;

fn test_id() -> SpreadsheetId {
    SpreadsheetId(
        std::env::var("COLMENA_GSHEETS_TEST_SPREADSHEET_ID")
            .expect("COLMENA_GSHEETS_TEST_SPREADSHEET_ID required for integration test"),
    )
}

fn client() -> GoogleSheetsHttpClient {
    GoogleSheetsHttpClient::from_config(&GSheetsConfig::from_env()).expect("build client")
}

/// Returns true iff both required env vars are set. Used to keep the
/// `#[ignore]`-gated tests from crashing with `panic!("env required")`
/// when invoked without setup — we'd rather they exit cleanly.
fn env_ready() -> bool {
    std::env::var("GOOGLE_APPLICATION_CREDENTIALS").is_ok()
        && std::env::var("COLMENA_GSHEETS_TEST_SPREADSHEET_ID").is_ok()
}

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GSHEETS_TEST_SPREADSHEET_ID"]
async fn add_write_read_delete_sheet_round_trip() {
    if !env_ready() {
        eprintln!("SKIP: env not configured");
        return;
    }
    let c = client();
    let id = test_id();
    let tab = format!("e2e_{}", uuid::Uuid::new_v4().simple());
    let added = c.add_sheet(&id, &tab).await.expect("add ok");
    assert_eq!(added.title, tab);

    c.set_cell(&id, &tab, "A1", CellValue::Number(42.0))
        .await
        .expect("set ok");
    let r = c
        .read_range(&id, &tab, Some("A1"), ReadOptions::default())
        .await
        .expect("read ok");
    assert_eq!(r.values, serde_json::json!([[42.0]]));

    c.delete_sheet(&id, &tab).await.expect("delete ok");
}

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GSHEETS_TEST_SPREADSHEET_ID"]
async fn formula_evaluated_server_side() {
    if !env_ready() {
        eprintln!("SKIP: env not configured");
        return;
    }
    let c = client();
    let id = test_id();
    let tab = format!("formula_{}", uuid::Uuid::new_v4().simple());
    c.add_sheet(&id, &tab).await.expect("add");
    c.set_range(
        &id,
        &tab,
        "A1",
        vec![
            vec![CellValue::Number(10.0)],
            vec![CellValue::Number(20.0)],
            vec![CellValue::Number(30.0)],
        ],
    )
    .await
    .expect("seed");
    c.set_cell(
        &id,
        &tab,
        "B1",
        CellValue::String("=SUM(A1:A3)".to_string()),
    )
    .await
    .expect("write formula");

    // Read as evaluated number.
    let r = c
        .read_range(
            &id,
            &tab,
            Some("B1"),
            ReadOptions {
                value_render: ValueRenderOption::UnformattedValue,
                as_records: false,
            },
        )
        .await
        .expect("read evaluated");
    assert_eq!(r.values, serde_json::json!([[60.0]]));

    // Read as formula text.
    let f = c
        .read_range(
            &id,
            &tab,
            Some("B1"),
            ReadOptions {
                value_render: ValueRenderOption::Formula,
                as_records: false,
            },
        )
        .await
        .expect("read formula");
    assert_eq!(f.values, serde_json::json!([["=SUM(A1:A3)"]]));

    c.delete_sheet(&id, &tab).await.expect("cleanup");
}

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GSHEETS_TEST_SPREADSHEET_ID"]
async fn spreadsheet_not_found_for_bogus_id() {
    if !env_ready() {
        eprintln!("SKIP: env not configured");
        return;
    }
    let c = client();
    let result = c
        .list_sheets(&SpreadsheetId("totally-bogus-id-xxxxxx".to_string()))
        .await;
    assert!(matches!(
        result,
        Err(SheetsError::SpreadsheetNotFound(_)) | Err(SheetsError::Http(_))
    ));
}

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GSHEETS_TEST_SPREADSHEET_ID"]
async fn format_range_live_roundtrip() {
    if !env_ready() {
        eprintln!("SKIP: env not configured");
        return;
    }
    let c = client();

    // 1. Create a throwaway spreadsheet. There is no `delete_spreadsheet`
    //    primitive on the client (sibling live tests reuse a fixed sheet and
    //    only clean up *tabs* via `delete_sheet`); a created spreadsheet is
    //    left behind, matching `gsheets_collision_envelope_e2e`'s convention.
    let meta = c
        .create_spreadsheet("colmena fmt IT")
        .await
        .expect("create ok");
    let id = meta.spreadsheet_id.clone();

    // 2. Seed a small table on the first sheet at A1.
    let first = meta
        .sheets
        .first()
        .expect("at least one sheet")
        .title
        .clone();
    c.set_range(
        &id,
        &first,
        "A1",
        vec![
            vec![
                CellValue::String("Producto".to_string()),
                CellValue::String("Stock".to_string()),
            ],
            vec![
                CellValue::String("Lapices".to_string()),
                CellValue::String("10".to_string()),
            ],
            vec![
                CellValue::String("Cuadernos".to_string()),
                CellValue::String("5".to_string()),
            ],
        ],
    )
    .await
    .expect("seed ok");

    // 3. Resolve the first sheet's numeric sheet_id.
    let sheets = c.list_sheets(&id).await.expect("list ok");
    let sid = sheets.first().expect("at least one sheet").sheet_id.0;

    // 4. Build requests with the pure helpers.
    let header = build_format_requests(
        &a1_to_grid_range(sid, "A1:B1").unwrap(),
        &FormatSpec {
            text: Some(TextFormat {
                bold: Some(true),
                color: Some("#FFFFFF".into()),
                ..Default::default()
            }),
            background_color: Some("#4472C4".into()),
            horizontal_alignment: Some("CENTER".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let block = build_format_requests(
        &a1_to_grid_range(sid, "A1:B3").unwrap(),
        &FormatSpec {
            borders: Some(Borders {
                top: Some(BorderSide {
                    style: "SOLID".into(),
                    color: None,
                }),
                bottom: Some(BorderSide {
                    style: "SOLID".into(),
                    color: None,
                }),
                left: Some(BorderSide {
                    style: "SOLID".into(),
                    color: None,
                }),
                right: Some(BorderSide {
                    style: "SOLID".into(),
                    color: None,
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .unwrap();
    let width = build_format_requests(
        &a1_to_grid_range(sid, "A:A").unwrap(),
        &FormatSpec {
            column_width_px: Some(160),
            ..Default::default()
        },
    )
    .unwrap();
    let mut all = header;
    all.extend(block);
    all.extend(width);

    // 5. Apply all formatting in one batchUpdate round-trip.
    let result = c.batch_update(&id, all).await;
    assert!(result.is_ok(), "batch_update failed: {result:?}");

    // 6. No cleanup: no `delete_spreadsheet` primitive exists; throwaway
    //    spreadsheet is intentionally left behind (matches file convention).
}
