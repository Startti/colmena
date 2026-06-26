//! Live E2E harness for the gsheets read/write fixes — all tests are
//! `#[ignore]`-gated and hit the REAL Google API, so they never run in CI.
//! They drive a real sheet via the OAuth client and are parameterised entirely
//! by env vars (no real spreadsheet IDs in the repo).
//!
//! - `fix_full_read_is_row_bounded_under_cap` — regression test: an oversized
//!   whole-sheet read stays under the tool-result cap with truncated metadata.
//! - `baseline_full_read_vs_cap` / `dump_sheet_as_colmena_sees_it` — print-only
//!   diagnostics (what the model actually receives).
//! - `locate_client_id` / `reset_cells` — utilities for write-validation runs.
//!
//! Run (per the gsheets E2E runbook — OAuth creds injected in-memory):
//!   export COLMENA_GOOGLE_OAUTH_CLIENT_ID=... CLIENT_SECRET=... REFRESH_TOKEN=...
//!   export EXP_SPREADSHEET_ID=<id>            # required
//!   export EXP_SHEET="hoja 16"               # required
//!   cargo test --test gsheets_truncation_baseline -- --ignored --nocapture

use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::gsheets_tools::dispatch_read_with_client;
use colmena::gsheets::domain::{CellValue, ReadOptions, SheetsClient, SpreadsheetId};
use colmena::gsheets::infrastructure::config::GSheetsConfig;
use colmena::gsheets::infrastructure::http_client::GoogleSheetsHttpClient;

/// The same per-string cap the LLM tool-result scrubber applies
/// (`DEFAULT_MAX_TOOL_RESULT_STRING_BYTES`).
const TOOL_RESULT_CAP: usize = 51_200;

fn cell_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Minimal markdown table renderer — mirrors the shape (and therefore the
/// byte size) of what `gsheets_read` emits, so we can measure it vs the cap.
fn to_markdown(values: &serde_json::Value) -> String {
    let Some(rows) = values.as_array() else {
        return String::new();
    };
    if rows.is_empty() {
        return String::new();
    }
    let width = rows
        .iter()
        .filter_map(|r| r.as_array().map(|a| a.len()))
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for (i, row) in rows.iter().enumerate() {
        let cells = row.as_array().cloned().unwrap_or_default();
        let mut line: Vec<String> = (0..width)
            .map(|c| cells.get(c).map(cell_to_string).unwrap_or_default())
            .collect();
        out.push_str(&format!("| {} |\n", line.join(" | ")));
        if i == 0 {
            line = (0..width).map(|_| "---".to_string()).collect();
            out.push_str(&format!("| {} |\n", line.join(" | ")));
        }
    }
    out
}

fn env_ready() -> bool {
    std::env::var("COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN").is_ok()
        && std::env::var("EXP_SPREADSHEET_ID").is_ok()
        && std::env::var("EXP_SHEET").is_ok()
}

/// Print-only: dump EXP_RANGE of EXP_SHEET showing the business columns
/// (CLIENT ID / Cantidad=S / Tarifa=U / Importe=V) per sheet row — to verify a
/// write landed in the right column. Set EXP_RANGE (A1, e.g. "20:27").
#[tokio::test]
#[ignore = "requires OAuth creds + EXP_SPREADSHEET_ID + EXP_SHEET + EXP_RANGE"]
async fn dump_range() {
    if !env_ready() || std::env::var("EXP_RANGE").is_err() {
        eprintln!("SKIP: need OAuth + EXP_SPREADSHEET_ID + EXP_SHEET + EXP_RANGE");
        return;
    }
    let client =
        GoogleSheetsHttpClient::from_config(&GSheetsConfig::from_env()).expect("build client");
    let id = SpreadsheetId(std::env::var("EXP_SPREADSHEET_ID").unwrap());
    let sheet = std::env::var("EXP_SHEET").unwrap();
    let range = std::env::var("EXP_RANGE").unwrap();
    let first_row: usize = range
        .split(':')
        .next()
        .and_then(|s| s.trim_matches(|c: char| !c.is_ascii_digit()).parse().ok())
        .unwrap_or(1);
    let r = client
        .read_range(&id, &sheet, Some(&range), ReadOptions::default())
        .await
        .expect("read ok");
    println!("\n===== {sheet}!{range} (CLIENT ID | S=Cantidad | U=Tarifa | V=Importe) =====");
    for (i, row) in r.values.as_array().into_iter().flatten().enumerate() {
        let cells = row.as_array().cloned().unwrap_or_default();
        let get = |idx: usize| cells.get(idx).map(cell_to_string).unwrap_or_default();
        let cid = get(0);
        let cid = if cid.len() > 20 {
            format!("{}…", &cid[..20])
        } else {
            cid
        };
        println!(
            "row {:>3} | {:<21} | S={:<10} | U={:<12} | V={}",
            first_row + i,
            cid,
            get(18),
            get(20),
            get(21)
        );
    }
    println!("=====================================================================\n");
}

#[tokio::test]
#[ignore = "requires OAuth creds + EXP_SPREADSHEET_ID + EXP_SHEET"]
async fn baseline_full_read_vs_cap() {
    if !env_ready() {
        eprintln!("SKIP: env not configured (OAuth + EXP_SPREADSHEET_ID + EXP_SHEET)");
        return;
    }
    let client =
        GoogleSheetsHttpClient::from_config(&GSheetsConfig::from_env()).expect("build client");
    let id = SpreadsheetId(std::env::var("EXP_SPREADSHEET_ID").unwrap());
    let sheet = std::env::var("EXP_SHEET").unwrap();

    // 1. Full read (range omitted = whole used area) — the case that gets nuked.
    let full = client
        .read_range(&id, &sheet, None, ReadOptions::default())
        .await
        .expect("full read ok");
    let rows = full.values.as_array().map(|a| a.len()).unwrap_or(0);
    let cols = full
        .values
        .as_array()
        .and_then(|a| a.first())
        .and_then(|r| r.as_array())
        .map(|r| r.len())
        .unwrap_or(0);
    let md_full = to_markdown(&full.values);

    println!("\n========== BASELINE: full read of '{sheet}' ==========");
    println!("dimensions: {rows} rows x {cols} cols");
    println!("full markdown bytes: {}", md_full.len());
    println!("tool-result cap: {TOOL_RESULT_CAP} bytes");
    println!(
        "EXCEEDS CAP? {}  -> with current scrubber the WHOLE markdown becomes the [truncated] marker (0 rows reach the model)",
        md_full.len() > TOOL_RESULT_CAP
    );
    println!("\n--- real header (columns the model SHOULD see) ---");
    if let Some(header) = full.values.as_array().and_then(|a| a.first()) {
        println!(
            "{}",
            cell_to_string(&serde_json::Value::Array(
                header.as_array().cloned().unwrap_or_default()
            ))
        );
        if let Some(h) = header.as_array() {
            for (i, c) in h.iter().enumerate() {
                println!("  col[{i}] = {:?}", cell_to_string(c));
            }
        }
    }
    println!("\n--- first 12 markdown lines (what a head-preserving truncation keeps) ---");
    for l in md_full.lines().take(12) {
        println!("{l}");
    }

    // 2. Bounded preview (rows 1..=30) — the shape the fix should emit.
    let preview = client
        .read_range(&id, &sheet, Some("1:30"), ReadOptions::default())
        .await
        .expect("preview read ok");
    let md_prev = to_markdown(&preview.values);
    println!("\n========== bounded preview (range 1:30) ==========");
    println!("preview markdown bytes: {}", md_prev.len());
    println!(
        "fits under cap? {}  (this is what a row-bounded gsheets_read would return)",
        md_prev.len() <= TOOL_RESULT_CAP
    );
    println!("=====================================================\n");
}

/// Verifies THE FIX end-to-end against the real sheet: the production
/// `dispatch_read_with_client` path now row-bounds an oversized markdown read
/// so the `markdown` field stays under the scrubber cap (never nuked) and
/// carries `truncated`/`rows_shown`/`total_rows`.
#[tokio::test]
#[ignore = "requires OAuth creds + EXP_SPREADSHEET_ID + EXP_SHEET"]
async fn fix_full_read_is_row_bounded_under_cap() {
    if !env_ready() {
        eprintln!("SKIP: env not configured (OAuth + EXP_SPREADSHEET_ID + EXP_SHEET)");
        return;
    }
    let client =
        GoogleSheetsHttpClient::from_config(&GSheetsConfig::from_env()).expect("build client");
    let spreadsheet_id = std::env::var("EXP_SPREADSHEET_ID").unwrap();
    let sheet = std::env::var("EXP_SHEET").unwrap();

    // Whole-sheet read (range omitted) through the REAL tool dispatcher.
    let args = serde_json::json!({ "spreadsheet_id": spreadsheet_id, "sheet": sheet });
    let out = dispatch_read_with_client(args, &client).await;

    println!("\n========== FIX: dispatch_read_with_client (whole sheet) ==========");
    let md = out.get("markdown").and_then(|v| v.as_str()).unwrap_or("");
    println!("markdown bytes: {}", md.len());
    println!("truncated: {:?}", out.get("truncated"));
    println!("rows_shown: {:?}", out.get("rows_shown"));
    println!("total_rows: {:?}", out.get("total_rows"));
    println!("dimensions: {:?}", out.get("dimensions"));
    println!("--- first 6 markdown lines (real columns reach the model now) ---");
    for l in md.lines().take(6) {
        println!("{}", &l[..l.len().min(300)]);
    }
    println!("==================================================================\n");

    assert_eq!(out.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert!(
        md.len() <= TOOL_RESULT_CAP,
        "bounded markdown ({} bytes) must stay under the scrubber cap so it is NOT discarded",
        md.len()
    );
    assert_eq!(
        out.get("truncated").and_then(|v| v.as_bool()),
        Some(true),
        "an oversized sheet must be flagged truncated"
    );
    let rows_shown = out
        .get("rows_shown")
        .and_then(|v| v.as_u64())
        .expect("rows_shown present");
    assert!(rows_shown >= 1, "at least one data row reaches the model");
    // Real columns must be present (the whole point of the fix).
    assert!(
        md.starts_with("| "),
        "result is a real markdown table with the sheet's columns"
    );
}

/// Print-only: dumps EXACTLY what `gsheets_read` returns for a tab (the
/// production dispatcher envelope + the first markdown rows), so we can eyeball
/// "what Colmena sees". No assertions. Set EXP_SHEET to the tab to inspect.
#[tokio::test]
#[ignore = "requires OAuth creds + EXP_SPREADSHEET_ID + EXP_SHEET"]
async fn dump_sheet_as_colmena_sees_it() {
    if !env_ready() {
        eprintln!("SKIP: env not configured (OAuth + EXP_SPREADSHEET_ID + EXP_SHEET)");
        return;
    }
    let client =
        GoogleSheetsHttpClient::from_config(&GSheetsConfig::from_env()).expect("build client");
    let spreadsheet_id = std::env::var("EXP_SPREADSHEET_ID").unwrap();
    let sheet = std::env::var("EXP_SHEET").unwrap();

    let args = serde_json::json!({ "spreadsheet_id": spreadsheet_id, "sheet": sheet });
    let out = dispatch_read_with_client(args, &client).await;

    println!("\n========== gsheets_read envelope for tab '{sheet}' ==========");
    println!("ok: {:?}", out.get("ok"));
    println!("dimensions: {:?}", out.get("dimensions"));
    println!("truncated: {:?}", out.get("truncated"));
    println!("rows_shown: {:?}", out.get("rows_shown"));
    println!("total_rows: {:?}", out.get("total_rows"));
    if let Some(err) = out.get("error") {
        println!("ERROR: {err:?} message={:?}", out.get("message"));
    }
    let md = out.get("markdown").and_then(|v| v.as_str()).unwrap_or("");
    println!("markdown bytes: {}", md.len());
    println!("--- first 15 markdown lines (each line capped at 400 chars for display) ---");
    for l in md.lines().take(15) {
        println!("{}", &l[..l.len().min(400)]);
    }
    println!("=============================================================\n");
}

/// Deterministic reset helper for the write-validation runs: sets the cells in
/// EXP_RESET_CELLS (comma-separated A1, e.g. "S20,S21,S22") of EXP_SHEET to the
/// number EXP_RESET_VALUE (default 0), so an agent's write is observable.
#[tokio::test]
#[ignore = "requires OAuth creds + EXP_SPREADSHEET_ID + EXP_SHEET + EXP_RESET_CELLS"]
async fn reset_cells() {
    if std::env::var("COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN").is_err()
        || std::env::var("EXP_SPREADSHEET_ID").is_err()
        || std::env::var("EXP_SHEET").is_err()
        || std::env::var("EXP_RESET_CELLS").is_err()
    {
        eprintln!("SKIP: need OAuth + EXP_SPREADSHEET_ID + EXP_SHEET + EXP_RESET_CELLS");
        return;
    }
    let client =
        GoogleSheetsHttpClient::from_config(&GSheetsConfig::from_env()).expect("build client");
    let id = SpreadsheetId(std::env::var("EXP_SPREADSHEET_ID").unwrap());
    let sheet = std::env::var("EXP_SHEET").unwrap();
    let value: f64 = std::env::var("EXP_RESET_VALUE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    for cell in std::env::var("EXP_RESET_CELLS").unwrap().split(',') {
        let addr = cell.trim();
        if addr.is_empty() {
            continue;
        }
        client
            .set_cell(&id, &sheet, addr, CellValue::Number(value))
            .await
            .unwrap_or_else(|e| panic!("set {addr} failed: {e:?}"));
        println!("reset {sheet}!{addr} = {value}");
    }
}

/// Print-only: locate a CLIENT ID across all tabs and report the row + the
/// current value of the `Cantidad` column (its A1 cell), so a write test can
/// target it precisely AND we capture the original value for a possible revert.
/// Set EXP_FIND_ID to the CLIENT ID substring to search for.
#[tokio::test]
#[ignore = "requires OAuth creds + EXP_SPREADSHEET_ID + EXP_FIND_ID"]
async fn locate_client_id() {
    if std::env::var("COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN").is_err()
        || std::env::var("EXP_SPREADSHEET_ID").is_err()
        || std::env::var("EXP_FIND_ID").is_err()
    {
        eprintln!("SKIP: need OAuth + EXP_SPREADSHEET_ID + EXP_FIND_ID");
        return;
    }
    let client =
        GoogleSheetsHttpClient::from_config(&GSheetsConfig::from_env()).expect("build client");
    let id = SpreadsheetId(std::env::var("EXP_SPREADSHEET_ID").unwrap());
    let needle = std::env::var("EXP_FIND_ID").unwrap();

    let sheets = client.list_sheets(&id).await.expect("list_sheets ok");
    println!(
        "\n========== locating '{needle}' across {} tabs ==========",
        sheets.len()
    );
    fn col_to_a1(mut idx: usize) -> String {
        // 0 -> A, 25 -> Z, 26 -> AA ...
        let mut s = String::new();
        idx += 1;
        while idx > 0 {
            let rem = (idx - 1) % 26;
            s.insert(0, (b'A' + rem as u8) as char);
            idx = (idx - 1) / 26;
        }
        s
    }
    for sm in &sheets {
        let resp = match client
            .read_range(&id, &sm.title, None, ReadOptions::default())
            .await
        {
            Ok(r) => r,
            Err(e) => {
                println!("  tab '{}' read error: {e:?}", sm.title);
                continue;
            }
        };
        let Some(rows) = resp.values.as_array() else {
            continue;
        };
        let Some(header) = rows.first().and_then(|h| h.as_array()) else {
            continue;
        };
        let idx_client = header.iter().position(|c| {
            c.as_str()
                .map(|s| s.eq_ignore_ascii_case("CLIENT ID"))
                .unwrap_or(false)
        });
        let idx_cant = header.iter().position(|c| {
            c.as_str()
                .map(|s| s.eq_ignore_ascii_case("Cantidad"))
                .unwrap_or(false)
        });
        let (Some(ci), Some(qi)) = (idx_client, idx_cant) else {
            continue;
        };
        for (ri, row) in rows.iter().enumerate().skip(1) {
            let cells = row.as_array().cloned().unwrap_or_default();
            let client_cell = cells.get(ci).map(cell_to_string).unwrap_or_default();
            if client_cell.contains(&needle) {
                let cant = cells.get(qi).map(cell_to_string).unwrap_or_default();
                let row_1based = ri + 1; // sheet row number (header = row 1)
                println!("  FOUND in tab '{}':", sm.title);
                println!(
                    "    sheet row = {row_1based}  (CLIENT ID cell = {:?})",
                    client_cell
                );
                println!(
                    "    Cantidad column = index {qi} = col {} -> cell {}{}",
                    col_to_a1(qi),
                    col_to_a1(qi),
                    row_1based
                );
                println!(
                    "    CURRENT Cantidad value = {:?}  <-- SAVE THIS to revert",
                    cant
                );
            }
        }
    }
    println!("=========================================================\n");
}
