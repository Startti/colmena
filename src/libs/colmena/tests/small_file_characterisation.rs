//! Characterisation of today's small-file Python and attachment behaviour
//! (slice C0 of the large tabular files chain).
//!
//! Every test pins what the code does NOW for files at or below the current
//! limits, so later slices that add a large-file path can prove they left the
//! small-file path untouched. Nothing here changes behaviour: if a test fails
//! after an unrelated change, that change altered small-file behaviour.
//!
//! The pins that need crate-private items live beside them in
//! `dag_engine::infrastructure::nodes::llm::characterisation` (run both with
//! `cargo test --lib characterisation` and
//! `cargo test --test small_file_characterisation`).

mod small_file_support;

use colmena::dag_engine::domain::python_executor::{
    PythonExecutor, PythonRunError, PythonRunRequest,
};
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_run_python;
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::data_run_python::{
    self, EnabledSources,
};
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::sql_bulk_tools::parse_attachment_to_records;
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::tabular_bindings::AttachmentFetcher;
use colmena::dag_engine::infrastructure::python_exec::config::{RemoteAuthConfig, RemoteConfig};
use colmena::dag_engine::infrastructure::python_exec::protocol::{
    WireResponse, WireStatus, CRASHED_MESSAGE,
};
use colmena::dag_engine::infrastructure::python_exec::remote::RemoteExecutor;
use colmena::dag_engine::infrastructure::python_exec::scope;
use colmena::llm::domain::{FunctionCall, ToolCall, ToolExecutor};
use serde_json::{json, Map, Value};
use small_file_support::{executor_with, Recording};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

const MIB: usize = 1024 * 1024;
const CSV_MIME: &str = "text/csv";
const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// The snippets whose wrapped form is pinned byte for byte by the goldens.
const ATTACHMENT_SNIPPET: &str = "print(df.shape)\nresult = df.head(2)";
const DATA_SNIPPET: &str = "df = pd.DataFrame(sales)\noutput = len(df)";
const ATTACHMENT_WRAP_GOLDEN: &str = include_str!("golden/attachment_run_python_wrap.golden");
const DATA_WRAP_GOLDEN: &str = include_str!("golden/data_run_python_wrap.golden");

fn attachment_call(code: &str) -> ToolCall {
    let args = json!({"attachment_id": "doc-1", "code": code});
    ToolCall::new(
        "call-1".to_string(),
        FunctionCall::new("attachment_run_python".to_string(), args.to_string()),
    )
}

/// Runs `attachment_run_python` on `bytes` with the recording executor in
/// scope; returns the parsed tool output (minus the wall-clock field) and the
/// requests that reached the executor.
async fn run_attachment_tool(
    bytes: Vec<u8>,
    mime: &str,
    filename: &str,
    code: &str,
    executor: Arc<Recording>,
) -> (Value, Vec<PythonRunRequest>) {
    let tools = executor_with(bytes, mime, filename);
    let call = attachment_call(code);
    let result = scope(executor.clone(), async { tools.execute(&call).await })
        .await
        .expect("the dispatcher answers");
    assert!(result.success, "tool envelope is a success: {result:?}");
    let mut out: Value = serde_json::from_str(&result.output).expect("JSON tool output");
    if let Some(obj) = out.as_object_mut() {
        obj.remove("duration_ms");
    }
    (out, executor.requests())
}

fn csv_with_rows(n: usize) -> Vec<u8> {
    let mut csv = String::from("a\n");
    for i in 0..n {
        csv.push_str(&format!("{i}\n"));
    }
    csv.into_bytes()
}

fn small_xlsx() -> Vec<u8> {
    let mut book = rust_xlsxwriter::Workbook::new();
    let sheet = book.add_worksheet();
    sheet.set_name("Inventory").unwrap();
    for (col, head) in ["sku", "price", "qty"].iter().enumerate() {
        sheet.write_string(0, col as u16, *head).unwrap();
    }
    sheet.write_string(1, 0, "A001").unwrap();
    sheet.write_number(1, 1, 9.5).unwrap();
    sheet.write_number(1, 2, 3.0).unwrap();
    sheet.write_string(2, 0, "B002").unwrap();
    sheet.write_number(2, 2, 5.0).unwrap();
    book.save_to_buffer().unwrap()
}

// ── C0.1: wrap_user_code, byte for byte ─────────────────────────────────

#[test]
fn attachment_run_python_wrap_matches_the_golden() {
    let wrapped = attachment_run_python::wrap_user_code(ATTACHMENT_SNIPPET);
    assert_eq!(wrapped, ATTACHMENT_WRAP_GOLDEN);
}

#[test]
fn data_run_python_wrap_matches_the_golden() {
    let wrapped = data_run_python::wrap_user_code(DATA_SNIPPET);
    assert_eq!(wrapped, DATA_WRAP_GOLDEN);
}

#[test]
fn the_wraps_embed_the_user_code_verbatim_between_prelude_and_postlude() {
    // A second snippet proves the goldens are not hard-coded to one input.
    let other = "result = int(df['qty'].astype(int).sum())";
    let attachment = attachment_run_python::wrap_user_code(other);
    let (head, tail) = ATTACHMENT_WRAP_GOLDEN
        .split_once(ATTACHMENT_SNIPPET)
        .expect("golden holds the snippet once");
    assert_eq!(attachment, format!("{head}{other}{tail}"));
    let data = data_run_python::wrap_user_code(other);
    let (head, tail) = DATA_WRAP_GOLDEN
        .split_once(DATA_SNIPPET)
        .expect("golden holds the snippet once");
    assert_eq!(data, format!("{head}{other}{tail}"));
}

// ── C0.2: the PythonRunRequest of a small CSV and a small XLSX ──────────

#[tokio::test]
async fn small_csv_request_and_result_shape_are_pinned() {
    let csv = b"sku,price,qty\nA001,9.99,3\nB002,,5\n".to_vec();
    let (out, requests) = run_attachment_tool(
        csv,
        CSV_MIME,
        "inventory.csv",
        ATTACHMENT_SNIPPET,
        Recording::ok(),
    )
    .await;

    assert_eq!(requests.len(), 1, "exactly one Python call");
    let req = &requests[0];
    assert_eq!(req.code, ATTACHMENT_WRAP_GOLDEN);
    assert_eq!(req.mode, "restricted");
    assert_eq!(req.timeout, Some(Duration::from_secs(30)));
    let keys: Vec<&String> = req.inputs.keys().collect();
    assert_eq!(keys, ["_attachment_records"]);
    // Every cell travels as a string; an empty cell is null.
    assert_eq!(
        req.inputs["_attachment_records"],
        json!([
            {"sku": "A001", "price": "9.99", "qty": "3"},
            {"sku": "B002", "price": null, "qty": "5"}
        ])
    );
    assert_eq!(
        out,
        json!({
            "stdout": "(2, 3)\n",
            "result": {"rows": 2},
            "row_count": 2,
            "columns": ["sku", "price", "qty"]
        })
    );
}

#[tokio::test]
async fn ten_mebibyte_csv_still_goes_whole_into_one_request() {
    // 2048 rows of a 5 KiB cell: just over 10 MiB, well under every row limit.
    let blob = "x".repeat(5 * 1024);
    let mut csv = String::from("id,blob\n");
    for i in 0..2048 {
        csv.push_str(&format!("{i},{blob}\n"));
    }
    assert!(csv.len() > 10 * MIB);
    let (out, requests) = run_attachment_tool(
        csv.into_bytes(),
        CSV_MIME,
        "big.csv",
        ATTACHMENT_SNIPPET,
        Recording::ok(),
    )
    .await;

    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    assert_eq!(req.timeout, Some(Duration::from_secs(30)));
    assert_eq!(req.mode, "restricted");
    let records = req.inputs["_attachment_records"].as_array().unwrap();
    assert_eq!(records.len(), 2048);
    assert!(records
        .iter()
        .all(|r| r["id"].is_string() && r["blob"].is_string()));
    assert_eq!(records[2047]["id"], "2047");
    assert_eq!(records[0]["blob"].as_str().unwrap().len(), 5 * 1024);
    assert_eq!(out["row_count"], 2048);
    assert_eq!(out["columns"], json!(["id", "blob"]));
}

#[tokio::test]
async fn small_xlsx_request_and_result_shape_are_pinned() {
    let (out, requests) = run_attachment_tool(
        small_xlsx(),
        XLSX_MIME,
        "inventory.xlsx",
        ATTACHMENT_SNIPPET,
        Recording::ok(),
    )
    .await;

    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    assert_eq!(req.code, ATTACHMENT_WRAP_GOLDEN);
    assert_eq!(req.mode, "restricted");
    assert_eq!(req.timeout, Some(Duration::from_secs(30)));
    // Numbers are rendered as text, an empty cell is null.
    assert_eq!(
        req.inputs["_attachment_records"],
        json!([
            {"sku": "A001", "price": "9.5", "qty": "3"},
            {"sku": "B002", "price": null, "qty": "5"}
        ])
    );
    assert_eq!(out["row_count"], 2);
    assert_eq!(out["columns"], json!(["sku", "price", "qty"]));
}

#[tokio::test]
async fn data_run_python_request_is_pinned() {
    let recording = Recording::ok();
    let args = json!({
        "bindings": [{"var": "sales", "data": [{"n": 2}, {"n": 3}]}],
        "code": DATA_SNIPPET,
    });
    let attach: AttachmentFetcher<'_> = Box::new(|_| Box::pin(async { Err("unused".to_string()) }));
    let registrar: colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_writer::AttachmentRegistrar<'_> =
        Box::new(|_, _| Box::pin(async { Err("unused".to_string()) }));
    let out = scope(
        recording.clone(),
        data_run_python::dispatch_core(
            args,
            &EnabledSources::default(),
            None,
            None,
            &attach,
            &registrar,
            None,
        ),
    )
    .await;

    let requests = recording.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].code, DATA_WRAP_GOLDEN);
    assert_eq!(requests[0].mode, "restricted");
    assert_eq!(requests[0].timeout, Some(Duration::from_secs(30)));
    let keys: Vec<&String> = requests[0].inputs.keys().collect();
    assert_eq!(keys, ["sales", "_loaded_columns"]);
    assert_eq!(requests[0].inputs["sales"], json!([{"n": 2}, {"n": 3}]));
    assert_eq!(out["error"], Value::Null);
    assert_eq!(out["stdout"], "(2, 3)\n");
}

// ── C0.3: row limits and the 30 s timeout ───────────────────────────────

const ATTACHMENT_ROW_LIMIT_ERROR: &str = "CSV exceeds 100000 rows — run_python cannot load the full DataFrame. Use sql_bulk_insert_from_attachment for large files, or filter the data before upload.";
const DATA_ROW_LIMIT_ERROR: &str = "CSV exceeds 500000 rows — run_python cannot load the full DataFrame. Use sql_bulk_insert_from_attachment for large files, or filter the data before upload.";

#[tokio::test]
async fn attachment_run_python_accepts_100k_rows_and_refuses_one_more() {
    let recording = Recording::ok();
    let (out, requests) = run_attachment_tool(
        csv_with_rows(100_000),
        CSV_MIME,
        "rows.csv",
        ATTACHMENT_SNIPPET,
        recording,
    )
    .await;
    assert_eq!(requests.len(), 1);
    assert_eq!(out["row_count"], 100_000);

    let refusing = Recording::ok();
    let (out, requests) = run_attachment_tool(
        csv_with_rows(100_001),
        CSV_MIME,
        "rows.csv",
        ATTACHMENT_SNIPPET,
        refusing,
    )
    .await;
    assert!(
        requests.is_empty(),
        "no Python call when the file is refused"
    );
    assert_eq!(
        out,
        json!({"error": ATTACHMENT_ROW_LIMIT_ERROR, "source": "execution"})
    );
}

#[tokio::test]
async fn data_run_python_attachment_binding_accepts_500k_rows_and_refuses_one_more() {
    let fetcher_for = |rows: usize| -> AttachmentFetcher<'static> {
        Box::new(move |_id| {
            Box::pin(async move { Ok((csv_with_rows(rows), CSV_MIME.to_string())) })
        })
    };
    let registrar: colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_writer::AttachmentRegistrar<'_> =
        Box::new(|_, _| Box::pin(async { Err("unused".to_string()) }));
    let args = json!({
        "bindings": [{"var": "rows", "attachment_id": "doc-1"}],
        "code": "output = 1",
    });

    let attach = fetcher_for(500_001);
    let recording = Recording::ok();
    let out = scope(
        recording.clone(),
        data_run_python::dispatch_core(
            args,
            &EnabledSources::default(),
            None,
            None,
            &attach,
            &registrar,
            None,
        ),
    )
    .await;
    assert!(
        recording.requests().is_empty(),
        "no Python call when refused"
    );
    assert_eq!(out["error"], "AttachmentParseFailed", "got: {out}");
    assert_eq!(out["binding"], "rows", "got: {out}");
    assert_eq!(out["message"], DATA_ROW_LIMIT_ERROR, "got: {out}");

    // The ceiling itself is accepted by the parser the binding uses.
    let (columns, records) = parse_attachment_to_records(
        &csv_with_rows(500_000),
        CSV_MIME,
        "rows.csv",
        None,
        None,
        None,
        500_000,
    )
    .expect("exactly 500000 rows load");
    assert_eq!(columns, ["a"]);
    assert_eq!(records.len(), 500_000);
}

#[tokio::test]
async fn both_python_tools_report_the_same_30_second_timeout_text() {
    let timing_out = Recording::answering(Err(PythonRunError::Timeout));
    let (out, requests) = run_attachment_tool(
        b"a\n1\n".to_vec(),
        CSV_MIME,
        "t.csv",
        ATTACHMENT_SNIPPET,
        timing_out,
    )
    .await;
    assert_eq!(requests[0].timeout, Some(Duration::from_secs(30)));
    assert_eq!(
        out,
        json!({"error": "code execution exceeded 30s timeout", "source": "execution"})
    );

    let recording = Recording::answering(Err(PythonRunError::Timeout));
    let attach: AttachmentFetcher<'_> = Box::new(|_| Box::pin(async { Err("unused".to_string()) }));
    let registrar: colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_writer::AttachmentRegistrar<'_> =
        Box::new(|_, _| Box::pin(async { Err("unused".to_string()) }));
    let out = scope(
        recording.clone(),
        data_run_python::dispatch_core(
            json!({"bindings": [{"var": "t", "data": [{"a": 1}]}], "code": "output = 1"}),
            &EnabledSources::default(),
            None,
            None,
            &attach,
            &registrar,
            None,
        ),
    )
    .await;
    assert_eq!(
        recording.requests()[0].timeout,
        Some(Duration::from_secs(30))
    );
    assert_eq!(out["error"], "code execution exceeded 30s timeout");
    assert_eq!(out["output"], Value::Null);
}

// ── C0.4: the v1 wire as the remote client puts it ──────────────────────

fn remote_config(uri: &str, max_request_bytes: usize, max_wire: Option<usize>) -> RemoteConfig {
    RemoteConfig {
        url: uri.parse().unwrap(),
        auth: RemoteAuthConfig::None,
        max_request_bytes,
        max_response_bytes: 8 * MIB,
        max_wire_bytes: max_wire,
    }
}

fn wire_request() -> PythonRunRequest {
    let mut inputs = Map::new();
    inputs.insert("x".into(), json!(20));
    inputs.insert("name".into(), json!("ñandú"));
    PythonRunRequest {
        code: "output = x + 1".to_string(),
        mode: "restricted".to_string(),
        timeout: Some(Duration::from_secs(30)),
        inputs,
    }
}

async fn serve_once(response: WireResponse) -> MockServer {
    let server = MockServer::start().await;
    let body = serde_json::to_vec(&response).unwrap();
    Mock::given(path("/v1/run"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn remote_client_sends_the_v1_wire_bytes() {
    let mut ok = WireResponse::status_only(WireStatus::Ok, None);
    (ok.output_set, ok.output) = (true, Some(json!(21)));
    let server = serve_once(ok).await;
    let exec = RemoteExecutor::new(
        remote_config(&server.uri(), 32 * MIB, None),
        Duration::from_secs(60),
    )
    .unwrap();
    let result = exec.run(wire_request()).await.unwrap();
    assert_eq!(result.output, Some(json!(21)));

    let sent = server.received_requests().await.unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].url.path(), "/v1/run");
    assert_eq!(
        sent[0]
            .headers
            .get("content-encoding")
            .unwrap()
            .to_str()
            .unwrap(),
        "zstd"
    );
    // The body is the zstd (level 3) frame of exactly these JSON bytes.
    let golden = r#"{"v":1,"code":"output = x + 1","mode":"restricted","timeout_ms":30000,"inputs":{"x":20,"name":"ñandú"}}"#;
    assert_eq!(
        &sent[0].body[..4],
        &[0x28, 0xb5, 0x2f, 0xfd],
        "zstd frame magic"
    );
    let decoded = zstd::decode_all(&sent[0].body[..]).unwrap();
    assert_eq!(String::from_utf8(decoded).unwrap(), golden);
    assert_eq!(
        sent[0].body,
        zstd::bulk::compress(golden.as_bytes(), 3).unwrap()
    );
}

#[tokio::test]
async fn remote_client_uses_the_dispatcher_deadline_when_the_request_has_none() {
    let mut ok = WireResponse::status_only(WireStatus::Ok, None);
    ok.output_set = true;
    let server = serve_once(ok).await;
    let exec = RemoteExecutor::new(
        remote_config(&server.uri(), 32 * MIB, None),
        Duration::from_secs(60),
    )
    .unwrap();
    let mut req = wire_request();
    req.timeout = None;
    exec.run(req).await.unwrap();
    let sent = server.received_requests().await.unwrap();
    let decoded = zstd::decode_all(&sent[0].body[..]).unwrap();
    let wire: Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(wire["timeout_ms"], 60_000);
}

#[tokio::test]
async fn remote_client_refuses_a_request_over_32_mib_before_sending() {
    let server = MockServer::start().await;
    let exec = RemoteExecutor::new(
        remote_config(&server.uri(), 32 * MIB, None),
        Duration::from_secs(60),
    )
    .unwrap();
    let mut req = wire_request();
    req.inputs.insert("big".into(), json!("x".repeat(33 * MIB)));
    let err = exec.run(req).await.unwrap_err();
    assert_eq!(
        err,
        PythonRunError::Python(
            "Python execution error: the input exceeds the Python executor limit of 32 MiB"
                .to_string()
        )
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn remote_client_refuses_a_compressed_body_over_the_wire_cap() {
    let server = MockServer::start().await;
    let exec = RemoteExecutor::new(
        remote_config(&server.uri(), 32 * MIB, Some(MIB)),
        Duration::from_secs(60),
    )
    .unwrap();
    // Random hex barely compresses: 2 MiB of it stays above a 1 MiB wire cap.
    let noisy: String = (0..(64 * 1024))
        .map(|_| uuid::Uuid::new_v4().simple().to_string())
        .collect();
    let mut req = wire_request();
    req.inputs.insert("big".into(), json!(noisy));
    let err = exec.run(req).await.unwrap_err();
    assert_eq!(
        err,
        PythonRunError::Python(
            "Python execution error: the compressed input exceeds the Python executor transport limit of 1 MiB"
                .to_string()
        )
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_crashed_executor_reports_the_crash_text_verbatim() {
    let server = serve_once(WireResponse::status_only(WireStatus::Crashed, None)).await;
    let exec = RemoteExecutor::new(
        remote_config(&server.uri(), 32 * MIB, None),
        Duration::from_secs(60),
    )
    .unwrap();
    let err = exec.run(wire_request()).await.unwrap_err();
    assert_eq!(
        CRASHED_MESSAGE,
        "Python execution error: the Python process ended without returning a result (it may have exceeded its memory or CPU limit)"
    );
    assert_eq!(err, PythonRunError::Python(CRASHED_MESSAGE.to_string()));
}
