#![cfg(target_os = "linux")]
//! Golden equivalence between Python executors. Every case runs in-process
//! and through the executor the environment selects
//! (`COLMENA_PYTHON_EXECUTOR`, `COLMENA_PYTHON_EXECUTOR_MODES`,
//! `COLMENA_PYTHON_EXECUTOR_BIN`). Each side must give the case's expected
//! outcome (a result, or a given kind of error), and then the whole results
//! must match. With the default (`inprocess`) there is nothing to compare and
//! the tests say so.
//! The subprocess executor needs root and CAP_SYS_ADMIN, and the cases need
//! pandas, numpy and scipy.

use colmena::dag_engine::domain::python_executor::{
    ExecutorKind, PythonExecutor, PythonRunError, PythonRunRequest, PythonRunResult,
};
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    attachment_run_python, crdt_doc_run_python, data_run_python, gsheets_run_python,
};
use colmena::dag_engine::infrastructure::python_exec::{
    self,
    config::{ExecutorConfig, ModesPolicy},
    inprocess::InProcessExecutor,
};
use serde_json::{json, Map, Value};
use std::time::Duration;

/// The environment's executor configuration once it is installed as the
/// process executor and ready, as a host does it, or `None` when it is the
/// in-process one.
async fn isolated_config() -> Option<ExecutorConfig> {
    pyo3::Python::initialize();
    let cfg = ExecutorConfig::from_env().expect("valid executor configuration");
    let kind = python_exec::install_from_env().expect("the configured executor builds");
    assert_eq!(kind, cfg.kind);
    python_exec::wait_until_ready()
        .await
        .expect("the executor is ready");
    if kind == ExecutorKind::InProcess {
        eprintln!("skipped: COLMENA_PYTHON_EXECUTOR is inprocess; nothing to compare");
        return None;
    }
    Some(cfg)
}

/// Whether the process executor sends a call in `mode` to the isolated
/// executor rather than running it in-process.
fn routed(cfg: &ExecutorConfig, mode: &str) -> bool {
    mode != "none" || cfg.modes == ModesPolicy::All
}

fn inputs(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn case(code: String, mode: &str, pairs: &[(&str, Value)]) -> PythonRunRequest {
    PythonRunRequest {
        code,
        mode: mode.into(),
        timeout: Some(Duration::from_secs(60)),
        inputs: inputs(pairs),
    }
}

fn rows(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|i| json!({"id": i, "grp": format!("g{}", i % 7), "amount": (i as f64) * 1.5}))
            .collect(),
    )
}

fn columns(var: &str) -> Value {
    json!({ var: ["id", "grp", "amount"] })
}

/// What a case must produce on each side before the two are compared, so two
/// identical failures cannot pass for equivalence.
#[derive(Debug, Clone, Copy)]
enum Expect {
    Ok,
    /// A Python error whose text starts with this.
    Err(&'static str),
}

const VIOLATION_IMPORT: Expect = Expect::Err("SandboxViolation: import 'os' is not allowed");
const VIOLATION_BUILTIN: Expect = Expect::Err("SandboxViolation: 'open' is not allowed");
const SYNTAX: Expect = Expect::Err("SyntaxError: ");
const KEY_ERROR: Expect = Expect::Err("Python execution error: KeyError: 'missing'");
const CONVERSION: Expect = Expect::Err("Failed to convert Python 'output' to JSON: ");

fn assert_outcome(
    name: &str,
    side: &str,
    r: &Result<PythonRunResult, PythonRunError>,
    expect: Expect,
) {
    let held = match (expect, r) {
        (Expect::Ok, Ok(_)) => true,
        (Expect::Err(prefix), Err(PythonRunError::Python(m))) => m.starts_with(prefix),
        _ => false,
    };
    assert!(
        held,
        "golden case {name}: {side} gave {r:?}, expected {expect:?}"
    );
}

fn cases() -> Vec<(&'static str, Expect, PythonRunRequest)> {
    let data = |code: &str, n: usize| {
        let pairs = [("sales", rows(n)), ("_loaded_columns", columns("sales"))];
        case(data_run_python::wrap_user_code(code), "restricted", &pairs)
    };
    vec![
        ("scalar-none", Expect::Ok, case("output = 6 * 7".into(), "none", &[])),
        ("unset-output", Expect::Ok, case("x = 1".into(), "restricted", &[])),
        ("none-output", Expect::Ok, case("output = None".into(), "restricted", &[])),
        (
            "stdout-unicode",
            Expect::Ok,
            case(
                "print('ñandú 😀')\nprint('línea 2')\noutput = 'ok'".into(),
                "restricted",
                &[],
            ),
        ),
        (
            "json-boundary",
            Expect::Ok,
            case(
                "import numpy as np\noutput = {'i': int(np.int64(5)), 'f': float(np.float64(0.1)), 'nested': [[1, {'a': None}], True], 's': 'ü'}".into(),
                "restricted",
                &[],
            ),
        ),
        (
            "big-int-conversion-error",
            CONVERSION,
            case("output = 2**70".into(), "restricted", &[]),
        ),
        ("violation", VIOLATION_IMPORT, case("import os".into(), "restricted", &[])),
        (
            "banned-builtin",
            VIOLATION_BUILTIN,
            case("output = open('x')".into(), "restricted", &[]),
        ),
        ("syntax", SYNTAX, case("output = (".into(), "restricted", &[])),
        (
            "exception",
            KEY_ERROR,
            case("output = {}['missing']".into(), "restricted", &[]),
        ),
        (
            "hmac-signing",
            Expect::Ok,
            case(
                "import hmac, hashlib, base64\noutput = base64.b64encode(hmac.new(key.encode(), msg.encode(), hashlib.sha256).digest()).decode()".into(),
                "restricted",
                &[("key", json!("s3cr3t")), ("msg", json!("GET&/x&a=1"))],
            ),
        ),
        (
            "stdlib-none-csv-io",
            Expect::Ok,
            case(
                "import csv, io\nb = io.StringIO()\nw = csv.writer(b)\nw.writerows([['a', 'b'], [1, 2]])\noutput = b.getvalue()".into(),
                "none",
                &[],
            ),
        ),
        (
            "data-run-python-groupby",
            Expect::Ok,
            data(
                "df = pd.DataFrame(sales)\noutput = df.groupby('grp')['amount'].sum().round(6).to_dict()",
                10_000,
            ),
        ),
        (
            "data-run-python-sinks",
            Expect::Ok,
            data(
                "df = pd.DataFrame(sales)\noutput_tables = {'s.t': df.head(3)}\noutput_sheets = {'Hoja': df.tail(2)}\noutput_attachments = {'out.csv': df.head(1)}\noutput = len(df)",
                50,
            ),
        ),
        (
            "gsheets-run-python",
            Expect::Ok,
            case(
                gsheets_run_python::wrap_user_code(
                    "df = pd.DataFrame(data)\noutput_sheets = {'Resumen': df.describe().reset_index()}\noutput = int(df['id'].max())",
                ),
                "restricted",
                &[("data", rows(200)), ("_gsheets_loaded_columns", columns("data"))],
            ),
        ),
        (
            "attachment-run-python",
            Expect::Ok,
            case(
                attachment_run_python::wrap_user_code("print(df.shape)\nresult = df.head(2)"),
                "restricted",
                &[("_attachment_records", rows(100))],
            ),
        ),
        (
            "crdt-doc-run-python",
            Expect::Ok,
            case(
                crdt_doc_run_python::wrap_user_code("d = dfs['s1']\noutput = int(d['id'].sum())"),
                "restricted",
                &[("_dfs_raw", json!({"s1": rows(100)}))],
            ),
        ),
    ]
}

/// The whole result, keeping an unset `output` apart from `output = None`.
fn normalize(r: Result<PythonRunResult, PythonRunError>) -> Value {
    match r {
        Ok(ok) => json!({"ok": {
            "output_set": ok.output.is_some(),
            "output": ok.output,
            "stdout": ok.stdout,
        }}),
        Err(e) => json!({ "err": format!("{e:?}") }),
    }
}

#[tokio::test]
async fn every_case_matches_the_in_process_result() {
    let Some(cfg) = isolated_config().await else {
        return;
    };
    let (mut isolated, mut local) = (0, 0);
    for (name, expect, req) in cases() {
        let side = if routed(&cfg, &req.mode) {
            isolated += 1;
            cfg.kind.as_str()
        } else {
            local += 1;
            "inprocess"
        };
        let expected = InProcessExecutor.run(req.clone()).await;
        assert_outcome(name, "the in-process executor", &expected, expect);
        let actual = python_exec::run(req).await;
        assert_outcome(name, &format!("the {side} executor"), &actual, expect);
        assert_eq!(normalize(actual), normalize(expected), "golden case {name}");
    }
    eprintln!(
        "golden: {} cases match; {isolated} ran through the {} executor, {local} in-process on both sides (modes {:?})",
        isolated + local,
        cfg.kind.as_str(),
        cfg.modes,
    );
}

/// More calls at once than `serve` takes in flight (2 × slots) wait, not fail.
#[tokio::test]
async fn a_burst_of_calls_completes() {
    let Some(cfg) = isolated_config().await else {
        return;
    };
    let calls = (0..12).map(|i| printer("restricted", &format!("burst-{i}")));
    let results = futures::future::join_all(calls.map(python_exec::run)).await;
    assert!(results.iter().all(Result::is_ok), "{results:?}");
    let kind = cfg.kind.as_str();
    eprintln!("burst: 12 calls at once completed ({kind} executor)");
}

const MARKERS: [&str; 2] = ["first-call", "second-call"];

/// Prints `marker` five times, 0.1 s apart, and returns when it started and
/// ended. `restricted` code cannot import `time`, so it waits by polling the
/// clock; the interpreter still switches threads every few milliseconds.
fn printer(mode: &str, marker: &str) -> PythonRunRequest {
    let pause = if mode == "none" {
        "import time\ndef pause():\n    time.sleep(0.1)\n"
    } else {
        "def pause():\n    t = datetime.datetime.now()\n    while (datetime.datetime.now() - t).total_seconds() < 0.1:\n        pass\n"
    };
    let code = format!(
        "import datetime\n{pause}start = datetime.datetime.now().timestamp()\nfor _ in range(5):\n    print(marker)\n    pause()\noutput = [start, datetime.datetime.now().timestamp()]"
    );
    case(code, mode, &[("marker", json!(marker))])
}

/// What is wrong with one pair of calls made at the same time: each stdout
/// must hold its own marker five times and nothing else. The calls must have
/// overlapped, or there is nothing to judge.
fn stdout_problems(
    mode: &str,
    results: [Result<PythonRunResult, PythonRunError>; 2],
) -> Vec<String> {
    let results: Vec<PythonRunResult> = results
        .into_iter()
        .zip(MARKERS)
        .map(|(r, marker)| r.unwrap_or_else(|e| panic!("{mode} call {marker} failed: {e:?}")))
        .collect();
    let spans: Vec<(f64, f64)> = results
        .iter()
        .map(|r| {
            serde_json::from_value(r.output.clone().unwrap_or_default()).expect("[start, end]")
        })
        .collect();
    let ((s0, e0), (s1, e1)) = (spans[0], spans[1]);
    assert!(
        s0 < e1 && s1 < e0,
        "{mode}: the two calls did not run at the same time: {spans:?}"
    );
    results
        .iter()
        .zip(MARKERS)
        .filter(|(r, marker)| r.stdout != format!("{marker}\n").repeat(5))
        .map(|(r, marker)| format!("{mode}: the {marker} call captured {:?}", r.stdout))
        .collect()
}

/// Real runs overlap: `for_each` runs its items concurrently and a host may
/// run several graphs at once. Both calls go through the environment's
/// executor; a mode the current `COLMENA_PYTHON_EXECUTOR_MODES` keeps
/// in-process is skipped (see the ignored in-process test below).
#[tokio::test]
async fn concurrent_calls_keep_their_own_stdout() {
    let Some(cfg) = isolated_config().await else {
        return;
    };
    if cfg.subprocess.slots < 2 {
        eprintln!("skipped: COLMENA_PYTHON_EXECUTOR_SLOTS is below 2, so calls cannot overlap");
        return;
    }
    let mut problems = Vec::new();
    for mode in ["restricted", "none"] {
        if !routed(&cfg, mode) {
            eprintln!(
                "skipped {mode}: COLMENA_PYTHON_EXECUTOR_MODES ({:?}) runs it in-process",
                cfg.modes
            );
            continue;
        }
        let (a, b) = tokio::join!(
            python_exec::run(printer(mode, MARKERS[0])),
            python_exec::run(printer(mode, MARKERS[1])),
        );
        let found = stdout_problems(mode, [a, b]);
        if found.is_empty() {
            eprintln!(
                "concurrent {mode}: each call kept its own stdout ({} executor)",
                cfg.kind.as_str()
            );
        }
        problems.extend(found);
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[tokio::test]
#[ignore = "the in-process executor swaps the process-wide sys.stdout for each call, so \
            concurrent calls share it and their captured stdout mixes; a known gap"]
async fn concurrent_in_process_calls_keep_their_own_stdout() {
    pyo3::Python::initialize();
    let mut problems = Vec::new();
    for mode in ["restricted", "none"] {
        let (a, b) = tokio::join!(
            InProcessExecutor.run(printer(mode, MARKERS[0])),
            InProcessExecutor.run(printer(mode, MARKERS[1])),
        );
        problems.extend(stdout_problems(mode, [a, b]));
    }
    assert!(problems.is_empty(), "{problems:#?}");
}
