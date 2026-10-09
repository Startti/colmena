//! The Python side of the large path: what the model's code finds when it runs
//! over prepared tables, and how that code is wrapped.
//!
//! The prelude (`prelude.py`) is trusted code that runs before the model's code
//! in the same restricted sandbox. It defines `tables`, lazy handles over the
//! Parquet parts staged at `/data`, and a `df` that refuses every use, so a
//! large file is never loaded whole by default. It reads metadata from inputs
//! the trusted side injects (the verified manifest), never from a file the
//! sandbox could have changed, and it imports nothing restricted mode forbids:
//! pandas reads the Parquet (pyarrow is loaded by pandas, never imported here).

use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_run_python::wrap_user_code;
use crate::tabular_prepare::manifest::Manifest;
use serde_json::{json, Map, Value};

/// The prelude's source.
pub const PRELUDE: &str = include_str!("prelude.py");

/// Where the call finds its prepared tables.
pub const DATA_DIR: &str = "/data";

/// The most memory one `read` may be estimated to need (`READ_MAX_MB`, half of
/// the heavy run's 3,072 MiB). An estimate, to be calibrated against the spike.
pub const READ_MAX_BYTES: u64 = 1536 * 1024 * 1024;

const POSTLUDE_MARKER: &str = "# === colmena auto-postlude ===";

/// The small path's postlude (it turns `result` into `output`), taken from the
/// small path's own wrapper so the two cannot drift apart.
fn postlude() -> String {
    let small = wrap_user_code("");
    let at = small
        .find(POSTLUDE_MARKER)
        .expect("the wrapper has a postlude");
    small[at..].to_string()
}

/// What follows the postlude: when the code wrote files with `emit_table`, the
/// answer carries their report next to the result, under a marker the runtime
/// unwraps. Nothing is added when no file was written.
const EMITTED_TAIL: &str =
    "if _ct_emitted:\n    output = {'__colmena_emitted': _ct_emitted, 'result': output}\n";

/// The key the report travels under.
pub const EMITTED_KEY: &str = "__colmena_emitted";

/// The model's code wrapped for the large path: the same imports and the same
/// `result` convention as the small path, with `tables` and the `df` guard in
/// place of a loaded DataFrame.
pub fn wrap_large_code(code: &str) -> String {
    format!(
        "\nimport pandas as pd\nimport numpy as np\nimport scipy.stats as stats\n\n{PRELUDE}\nresult = None\n\n{code}\n\n{}\n{EMITTED_TAIL}",
        postlude()
    )
}

/// What the code says about a file it wrote with `emit_table`. Untrusted: it
/// comes from the sandbox, so it is only ever shown beside a file the reader kept,
/// matched by name, and every field is cleaned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitReport {
    pub name: String,
    pub rows: Option<u64>,
    pub dtypes: Vec<(String, String)>,
}

/// Most columns of dtypes kept, and the longest name or dtype text.
const MAX_REPORT_COLUMNS: usize = 200;
const MAX_REPORT_TEXT: usize = 64;

/// Splits the answer of a wrapped run into the code's result and the reports of
/// the files it wrote. An answer without the marker is the result as it is.
pub fn unwrap_emitted(output: Value) -> (Value, Vec<EmitReport>) {
    let Value::Object(mut map) = output else {
        return (output, vec![]);
    };
    if map.len() != 2 || !map.contains_key(EMITTED_KEY) || !map.contains_key("result") {
        return (Value::Object(map), vec![]);
    }
    let result = map.remove("result").unwrap_or(Value::Null);
    let clean = |text: &str| crate::llm::domain::large_tabular::inert_text(text, MAX_REPORT_TEXT);
    let reports = map
        .remove(EMITTED_KEY)
        .and_then(|v| match v {
            Value::Array(items) => Some(items),
            _ => None,
        })
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?.to_string();
            let dtypes = item
                .get("dtypes")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .take(MAX_REPORT_COLUMNS)
                        .filter_map(|(k, v)| Some((clean(k), clean(v.as_str()?))))
                        .collect()
                })
                .unwrap_or_default();
            Some(EmitReport {
                name,
                rows: item.get("rows").and_then(Value::as_u64),
                dtypes,
            })
        })
        .collect();
    (result, reports)
}

/// The inputs the prelude reads: the chosen tables (name, position in the
/// manifest, rows, parts, columns with type and decoded size), where the parts
/// are, and the read limit. Built from the verified manifest, so nothing the
/// sandbox wrote is in it.
pub fn prelude_inputs(manifest: &Manifest, tables: &[usize]) -> Map<String, Value> {
    let chosen: Vec<Value> = tables
        .iter()
        .filter_map(|&i| manifest.tables.get(i).map(|t| (i, t)))
        .map(|(i, t)| {
            json!({
                "name": t.name,
                "index": i,
                "rows": t.rows,
                "parts": t.parts,
                "columns": t.columns.iter().map(|c| json!({
                    "name": c.name,
                    "type": serde_json::to_value(c.column_type).unwrap_or(Value::Null),
                    "in_memory_bytes": c.in_memory_bytes,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let mut inputs = Map::new();
    inputs.insert("_ct_tables".into(), Value::Array(chosen));
    inputs.insert("_ct_data_dir".into(), Value::from(DATA_DIR));
    inputs.insert("_ct_read_max".into(), Value::from(READ_MAX_BYTES));
    let out = crate::tabular_run::collect::CollectLimits::default();
    inputs.insert("_ct_out_dir".into(), Value::from("/out"));
    inputs.insert("_ct_out_files".into(), Value::from(out.max_files));
    inputs.insert("_ct_out_file_max".into(), Value::from(out.file_bytes));
    inputs.insert("_ct_out_total_max".into(), Value::from(out.total_bytes));
    inputs
}

/// What the tool tells the model about the tables it can use: names, rows and
/// column types, never a size or a path.
pub fn tables_summary(manifest: &Manifest, tables: &[usize]) -> Value {
    Value::Array(
        tables
            .iter()
            .filter_map(|&i| manifest.tables.get(i))
            .map(|t| {
                json!({
                    "name": t.name,
                    "rows": t.rows,
                    "columns": t.columns.iter().map(|c| json!({
                        "name": c.name,
                        "type": serde_json::to_value(c.column_type).unwrap_or(Value::Null),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::infrastructure::nodes::python_node::execute_sandboxed_helper;
    use crate::tabular_prepare::manifest::{ColumnInfo, ColumnType, Manifest, TableInfo};

    fn col(name: &str, t: ColumnType, bytes: u64) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            column_type: t,
            uncompressed_bytes: 1,
            in_memory_bytes: bytes,
        }
    }

    fn manifest() -> Manifest {
        Manifest::new(vec![
            TableInfo {
                name: "sales".into(),
                rows: 1000,
                parts: 2,
                columns: vec![
                    col("id", ColumnType::Int, 8000),
                    col("note", ColumnType::String, 20_000),
                    col("paid", ColumnType::Bool, 125),
                    col("day", ColumnType::Date, 4000),
                ],
            },
            TableInfo {
                name: "Stores".into(),
                rows: 10,
                parts: 1,
                columns: vec![col("store", ColumnType::String, 200)],
            },
        ])
    }

    /// The prelude, then `body`, in restricted mode (the mode the tool runs in).
    fn run(body: &str, inputs: Map<String, Value>) -> Result<Value, String> {
        pyo3::Python::initialize();
        let code = format!("{PRELUDE}\n{body}");
        execute_sandboxed_helper(&code, "restricted", 30, &inputs)
            .map(|r| r.output.unwrap_or(Value::Null))
    }

    fn inputs() -> Map<String, Value> {
        prelude_inputs(&manifest(), &[0, 1])
    }

    /// The error text of a body that must raise.
    fn failure(body: &str, inputs: Map<String, Value>) -> String {
        let wrapped =
            format!("try:\n    {body}\n    output = 'no error'\nexcept LargeTableError as e:\n    output = str(e)");
        run(&wrapped, inputs)
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .unwrap()
    }

    #[test]
    fn the_prelude_passes_restricted_mode_and_names_the_tables() {
        let out = run("output = tables.names", inputs()).unwrap();
        assert_eq!(out, serde_json::json!(["sales", "Stores"]));
    }

    #[test]
    fn schema_has_names_and_types_and_never_a_size() {
        let out = run("output = tables.schema('sales')", inputs()).unwrap();
        assert_eq!(out["rows"], 1000);
        assert_eq!(out["parts"], 2);
        assert_eq!(
            out["columns"][1],
            serde_json::json!({"name": "note", "type": "string"})
        );
        assert!(!out.to_string().contains("bytes"), "{out}");
        // The match ignores case when it is unambiguous.
        let out = run("output = tables.schema('stores')['name']", inputs()).unwrap();
        assert_eq!(out, "Stores");
        let msg = failure("tables['nope']", inputs());
        assert!(
            msg.contains("no table named 'nope'") && msg.contains("'sales', 'Stores'"),
            "{msg}"
        );
    }

    /// A handle is not a DataFrame: every way of using it as one says how to
    /// read the table instead.
    #[test]
    fn a_handle_used_as_a_dataframe_says_how_to_read_it() {
        for body in [
            "tables['sales'].groupby('id')",
            "tables['sales']['id']",
            "len(tables['sales'])",
            "list(tables['sales'])",
            "tables['sales'].sum()",
        ] {
            let msg = failure(body, inputs());
            assert!(
                msg.contains("handle to a large table, not a DataFrame"),
                "{body}: {msg}"
            );
            assert!(
                msg.contains("read(columns=[...]") && msg.contains(".parts(columns="),
                "{body}: {msg}"
            );
        }
        let meta = run(
            "t = tables['sales']\noutput = [t.name, t.rows, t.n_parts, t.columns, t.dtypes['day']]",
            inputs(),
        )
        .unwrap();
        assert_eq!(meta[0], "sales");
        assert_eq!(meta[2], 2);
        assert_eq!(meta[3], serde_json::json!(["id", "note", "paid", "day"]));
        assert_eq!(meta[4], "date");
    }

    #[test]
    fn df_is_not_loaded_and_every_use_says_so() {
        for body in ["df.head()", "df['a']", "len(df)", "list(df)", "repr(df)"] {
            let msg = failure(body, inputs());
            assert!(msg.contains("`df` is not loaded"), "{body}: {msg}");
            assert!(msg.contains("tables.names"), "{body}: {msg}");
        }
    }

    #[test]
    fn the_wrapper_keeps_the_small_paths_imports_result_and_postlude() {
        let wrapped = wrap_large_code("result = tables.names");
        let small = wrap_user_code("result = tables.names");
        for must in [
            "import pandas as pd\nimport numpy as np\nimport scipy.stats as stats",
            "result = None\n\nresult = tables.names\n",
        ] {
            assert!(wrapped.contains(must), "{must}");
        }
        // The same postlude, byte for byte.
        let tail = |s: &str| s[s.find(POSTLUDE_MARKER).unwrap()..].trim_end().to_string();
        assert_eq!(
            tail(&wrapped)
                .trim_end_matches(EMITTED_TAIL.trim_end())
                .trim_end(),
            tail(&small)
        );
        assert!(!wrapped.contains("_attachment_records"));
        assert!(wrapped.contains(PRELUDE));
        // The prelude comes before the model's code, so `tables` exists for it.
        assert!(wrapped.find(PRELUDE).unwrap() < wrapped.find("result = tables.names").unwrap());
    }

    fn with_limit(mut m: Map<String, Value>, limit: u64) -> Map<String, Value> {
        m.insert("_ct_read_max".into(), Value::from(limit));
        m
    }

    #[test]
    fn read_needs_known_columns_in_a_list() {
        let t = "tables['sales']";
        let msg = failure(&format!("{t}.read()"), inputs());
        assert!(
            msg.contains("read() needs `columns=[...]`") && msg.contains("'id', 'note'"),
            "{msg}"
        );
        let msg = failure(&format!("{t}.read(columns=[])"), inputs());
        assert!(msg.contains("non-empty list"), "{msg}");
        let msg = failure(&format!("{t}.read(columns='id')"), inputs());
        assert!(msg.contains("non-empty list"), "{msg}");
        let msg = failure(&format!("{t}.read(columns=['id', 'nope'])"), inputs());
        assert!(
            msg.contains("no column named 'nope'") && msg.contains("'paid'"),
            "{msg}"
        );
        let msg = failure(&format!("{t}.read(columns=['id'], filters='x')"), inputs());
        assert!(msg.contains("`filters` must be a list"), "{msg}");
        let msg = failure(&format!("{t}.head(n=0)"), inputs());
        assert!(msg.contains("1 to 1000"), "{msg}");
    }

    /// The estimate applies the per-type multipliers of the developer guide and
    /// doubles for the moment Arrow's table and the frame exist together.
    #[test]
    fn the_read_estimate_applies_the_type_multipliers() {
        let est = |cols: &str| {
            run(
                &format!("output = _ct_estimate(tables._tables[0], {cols})"),
                inputs(),
            )
            .unwrap()
            .as_u64()
            .unwrap()
        };
        assert_eq!(est("['id']"), 2 * 8000);
        assert_eq!(est("['paid']"), 2 * 125 * 16);
        assert_eq!(est("['day']"), 2 * 4000 * 2);
        // A string adds the Python object header for every row: 57 * 1000.
        assert_eq!(est("['note']"), 2 * (20_000 + 57 * 1000));
        assert_eq!(est("['id', 'paid']"), 2 * (8000 + 125 * 16));
    }

    /// Over the limit nothing is read: the data directory does not exist, so an
    /// attempt to read would fail differently.
    #[test]
    fn a_read_over_the_limit_is_refused_with_the_guidance_and_reads_nothing() {
        let mut m = inputs();
        m.insert(
            "_ct_data_dir".into(),
            Value::from("/nonexistent-colmena-data"),
        );
        let limit = 2 * 1024 * 1024;
        // 2 * (20,000 + 57,000) is far under 2 MiB; make the limit tiny.
        let tiny = with_limit(m.clone(), 100_000);
        let msg = failure("tables['sales'].read(columns=['note'])", tiny);
        assert_eq!(
            msg,
            "`sales` is too large to load whole (1000 rows, about 1 MiB for these columns; limit 0 MiB). \
             Select fewer columns with `tables['sales'].read(columns=[...], filters=[...])`, \
             or loop `for part in tables['sales'].parts(columns=[...])` and combine per-part results."
        );
        // Under the limit the read is attempted (and fails on the missing directory).
        let roomy = with_limit(m, limit);
        let err = run("tables['sales'].read(columns=['id'])", roomy).unwrap_err();
        assert!(!err.contains("too large to load whole"), "{err}");
    }

    #[test]
    fn the_summary_for_the_model_has_names_rows_and_types_only() {
        let s = tables_summary(&manifest(), &[0, 1]);
        assert_eq!(s[0]["name"], "sales");
        assert_eq!(s[0]["rows"], 1000);
        assert_eq!(
            s[0]["columns"][3],
            serde_json::json!({"name": "day", "type": "date"})
        );
        let text = s.to_string();
        for leaked in ["bytes", "index", "part", "chat-attachments", "/data"] {
            assert!(!text.contains(leaked), "{leaked} in {text}");
        }
    }

    #[test]
    fn the_read_limit_is_half_of_the_heavy_memory() {
        assert_eq!(READ_MAX_BYTES, 1536 * 1024 * 1024);
    }

    #[test]
    fn the_report_of_written_files_is_split_from_the_result_and_cleaned() {
        let out = json!({
            "__colmena_emitted": [
                {"name": "a.csv", "rows": 3, "dtypes": {"x": "int64", "bad`col": "ob\nject"}, "size": 9},
                {"rows": 1},
                "junk"
            ],
            "result": {"total": 5}
        });
        let (result, reports) = unwrap_emitted(out);
        assert_eq!(result, json!({"total": 5}));
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].rows, Some(3));
        assert_eq!(
            reports[0].dtypes[1],
            ("badcol".to_string(), "ob ject".to_string())
        );
        // No marker: the answer is the result, whatever its shape.
        for plain in [
            json!({"a": 1}),
            json!([1]),
            json!(null),
            json!({"result": 1, "other": 2}),
        ] {
            assert_eq!(unwrap_emitted(plain.clone()), (plain, vec![]));
        }
    }

    fn emit_failure(body: &str) -> String {
        let mut m = inputs();
        m.insert("_ct_out_files".into(), Value::from(2));
        m.insert("_ct_out_dir".into(), Value::from("/nonexistent-out"));
        failure(body, m)
    }

    /// The checks that need no pandas: format, name, count and duplicates.
    #[test]
    fn emit_table_refuses_a_bad_format_name_or_count_before_touching_anything() {
        let msg = emit_failure("emit_table(None, 'a', 'xlsx')");
        assert!(msg.contains("'csv' or 'parquet'"), "{msg}");
        for bad in [
            "",
            "a b",
            "../x",
            "x.csv",
            "é",
            "a\n",
            "ok\n",
            &"n".repeat(49),
        ] {
            let msg = emit_failure(&format!("emit_table(None, {bad:?})"));
            assert!(msg.contains("1 to 48 letters"), "{bad:?}: {msg}");
        }
        let msg = emit_failure("_ct_emitted.extend([{'name': 'a.csv', 'size': 0}, {'name': 'b.csv', 'size': 0}])\n    emit_table(None, 'c')");
        assert!(msg.contains("at most 2 files"), "{msg}");
        let msg = emit_failure(
            "_ct_emitted.append({'name': 'a.csv', 'size': 0})\n    emit_table(None, 'a')",
        );
        assert!(msg.contains("already written"), "{msg}");
    }

    /// `in` follows the same case rule as `tables[name]`.
    #[test]
    fn membership_follows_the_same_case_rule_as_lookup() {
        let out = run(
            "output = ['sales' in tables, 'STORES' in tables, 'Sales' in tables, 'nope' in tables]",
            inputs(),
        )
        .unwrap();
        assert_eq!(out, serde_json::json!([true, true, true, false]));
        assert!(run("output = tables['STORES'].name", inputs()).is_ok());
    }

    #[test]
    fn inputs_keep_the_manifest_position_of_the_chosen_tables_and_no_path_or_key() {
        let m = prelude_inputs(&manifest(), &[1]);
        let tables = m["_ct_tables"].as_array().unwrap();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0]["name"], "Stores");
        assert_eq!(tables[0]["index"], 1);
        assert_eq!(tables[0]["columns"][0]["type"], "string");
        assert_eq!(m["_ct_data_dir"], DATA_DIR);
        assert_eq!(m["_ct_read_max"], READ_MAX_BYTES);
        assert_eq!(m["_ct_out_dir"], "/out");
        assert_eq!(m["_ct_out_files"], 8);
        assert_eq!(m["_ct_out_file_max"], 64 * 1024 * 1024);
        assert_eq!(m["_ct_out_total_max"], 128 * 1024 * 1024);
        // An index the manifest does not have is dropped, not invented.
        assert_eq!(
            prelude_inputs(&manifest(), &[7])["_ct_tables"],
            serde_json::json!([])
        );
    }
}

/// What the prelude does with real Parquet parts. Needs python3 with pandas and
/// pyarrow in the test environment (the Linux CI image and the Docker image used
/// for the jail suites have them); without them each test prints a skip line.
#[cfg(test)]
mod reads {
    use super::*;
    use crate::dag_engine::infrastructure::nodes::python_node::execute_sandboxed_helper;
    use crate::tabular_prepare::manifest::{ColumnInfo, ColumnType, Manifest, TableInfo};

    fn python(code: &str, mode: &str, inputs: &Map<String, Value>) -> Result<Value, String> {
        pyo3::Python::initialize();
        execute_sandboxed_helper(code, mode, 60, inputs).map(|r| r.output.unwrap_or(Value::Null))
    }

    /// Two parts of five rows (`a` 0..9, `b` text) under `dir`, written by
    /// pandas, and the inputs the prelude is given for them. `None` when pandas
    /// or pyarrow is missing.
    fn staged(dir: &std::path::Path) -> Option<Map<String, Value>> {
        let mut inputs = Map::new();
        inputs.insert("_dir".into(), Value::from(dir.to_str().unwrap()));
        let make = "import os, pandas as pd, pyarrow\nos.makedirs(_dir + '/t0')\n\
                    for p in range(2):\n    pd.DataFrame({'a': range(5 * p, 5 * p + 5), 'b': list('vwxyz')})\
                    .to_parquet(_dir + '/t0/part-%05d.parquet' % p)\noutput = 1\n";
        if python(make, "none", &inputs).is_err() {
            eprintln!("skipped: python3 with pandas and pyarrow is needed to make Parquet parts");
            return None;
        }
        let manifest = Manifest::new(vec![TableInfo {
            name: "t".into(),
            rows: 10,
            parts: 2,
            columns: vec![
                ColumnInfo {
                    name: "a".into(),
                    column_type: ColumnType::Int,
                    uncompressed_bytes: 1,
                    in_memory_bytes: 80,
                },
                ColumnInfo {
                    name: "b".into(),
                    column_type: ColumnType::String,
                    uncompressed_bytes: 1,
                    in_memory_bytes: 100,
                },
            ],
        }]);
        let mut inputs = prelude_inputs(&manifest, &[0]);
        inputs.insert("_ct_data_dir".into(), Value::from(dir.to_str().unwrap()));
        Some(inputs)
    }

    fn run(body: &str, inputs: &Map<String, Value>) -> Value {
        python(&format!("{PRELUDE}\n{body}"), "restricted", inputs).unwrap()
    }

    /// Every row of every part is visited once, and nothing else is read.
    #[test]
    fn parts_visit_every_row_once() {
        let dir = tempfile::tempdir().unwrap();
        let Some(inputs) = staged(dir.path()) else {
            return;
        };
        let out = run(
            "ids = []\nfor part in tables['t'].parts(columns=['a']):\n    ids += [int(v) for v in part['a']]\n\
             output = [ids, list(part.columns)]",
            &inputs,
        );
        assert_eq!(out[0], serde_json::json!((0..10).collect::<Vec<_>>()));
        assert_eq!(out[1], serde_json::json!(["a"]));
    }

    #[test]
    fn read_returns_the_named_columns_and_filters_before_loading() {
        let dir = tempfile::tempdir().unwrap();
        let Some(inputs) = staged(dir.path()) else {
            return;
        };
        let out = run(
            "output = [int(v) for v in tables['t'].read(columns=['a'])['a']]",
            &inputs,
        );
        assert_eq!(out, serde_json::json!((0..10).collect::<Vec<_>>()));
        let out = run(
            "f = tables['t'].read(columns=['a', 'b'], filters=[('a', '>=', 7)])\n\
             output = [list(f.columns), [int(v) for v in f['a']]]",
            &inputs,
        );
        assert_eq!(out, serde_json::json!([["a", "b"], [7, 8, 9]]));
        let out = run("output = len(tables['t'].head(3))", &inputs);
        assert_eq!(out, 3);
    }

    /// The wrapper end to end, postlude and imports included.
    #[test]
    fn the_wrapped_code_returns_its_result_through_the_small_paths_postlude() {
        let dir = tempfile::tempdir().unwrap();
        let Some(inputs) = staged(dir.path()) else {
            return;
        };
        if python("import scipy", "none", &Map::new()).is_err() {
            eprintln!("skipped: scipy is needed by the wrapper's imports");
            return;
        }
        let code = wrap_large_code("result = int(tables['t'].read(columns=['a'])['a'].sum())");
        assert_eq!(python(&code, "restricted", &inputs).unwrap(), 45);
        // `df` is the guard, not a loaded frame.
        let code = wrap_large_code("result = df.shape");
        let err = python(&code, "restricted", &inputs).unwrap_err();
        assert!(err.contains("`df` is not loaded"), "{err}");
    }

    /// Inputs for the output helper, pointing at `out`.
    fn emitting(out: &std::path::Path) -> Option<Map<String, Value>> {
        let dir = tempfile::tempdir().unwrap();
        let mut inputs = staged(dir.path())?;
        std::mem::forget(dir);
        inputs.insert("_ct_out_dir".into(), Value::from(out.to_str().unwrap()));
        Some(inputs)
    }

    #[test]
    fn emit_table_writes_csv_from_a_frame_and_from_parts_with_one_header() {
        let out = tempfile::tempdir().unwrap();
        let Some(inputs) = emitting(out.path()) else {
            return;
        };
        let report = run(
            "emit_table(tables['t'].head(3, columns=['a']), 'head')\n\
             emit_table(tables['t'].parts(columns=['a', 'b']), 'all')\n\
             output = _ct_emitted",
            &inputs,
        );
        assert_eq!(report[0]["name"], "head.csv");
        assert_eq!(report[0]["rows"], 3);
        assert_eq!(report[1]["rows"], 10);
        assert_eq!(
            report[1]["dtypes"],
            serde_json::json!({"a": "int64", "b": "object"})
        );
        let all = std::fs::read_to_string(out.path().join("all.csv")).unwrap();
        assert_eq!(all.lines().count(), 11, "one header and ten rows: {all}");
        assert!(all.starts_with("a,b\n0,v\n"));
    }

    #[test]
    fn emit_table_writes_one_parquet_and_refuses_several() {
        let out = tempfile::tempdir().unwrap();
        let Some(inputs) = emitting(out.path()) else {
            return;
        };
        let ok = run(
            "emit_table(tables['t'].read(columns=['a']), 'one', 'parquet')\noutput = _ct_emitted[0]['rows']",
            &inputs,
        );
        assert_eq!(ok, 10);
        assert!(
            std::fs::metadata(out.path().join("one.parquet"))
                .unwrap()
                .len()
                > 0
        );
        let msg = python(
            &format!("{PRELUDE}\ntry:\n    emit_table(tables['t'].parts(), 'x', 'parquet')\nexcept LargeTableError as e:\n    output = str(e)"),
            "restricted",
            &inputs,
        )
        .unwrap();
        assert!(
            msg.as_str()
                .unwrap()
                .contains("parquet takes one DataFrame"),
            "{msg}"
        );
    }

    /// A file over its limit is refused in the sandbox with the way out, after
    /// the chunk that crossed it; the reader would drop it anyway.
    #[test]
    fn emit_table_refuses_a_file_over_the_limit_with_a_clear_error() {
        let out = tempfile::tempdir().unwrap();
        let Some(mut inputs) = emitting(out.path()) else {
            return;
        };
        inputs.insert("_ct_out_file_max".into(), Value::from(20));
        let msg = python(
            &format!("{PRELUDE}\ntry:\n    emit_table(tables['t'].parts(), 'big')\nexcept LargeTableError as e:\n    output = str(e)"),
            "restricted",
            &inputs,
        )
        .unwrap();
        let text = msg.as_str().unwrap();
        assert!(
            text.contains("big.csv is over the limit") && text.contains("aggregate"),
            "{text}"
        );
    }

    /// Through the wrapper: the report travels beside the result under the marker
    /// and `unwrap_emitted` splits it; with no file written the answer is the
    /// result alone.
    #[test]
    fn the_wrapped_answer_carries_the_report_only_when_a_file_was_written() {
        let out = tempfile::tempdir().unwrap();
        let Some(inputs) = emitting(out.path()) else {
            return;
        };
        if python("import scipy", "none", &Map::new()).is_err() {
            eprintln!("skipped: scipy is needed by the wrapper's imports");
            return;
        }
        let with = python(
            &wrap_large_code("emit_table(tables['t'].head(2, columns=['a']), 'h')\nresult = 7"),
            "restricted",
            &inputs,
        )
        .unwrap();
        let (result, reports) = unwrap_emitted(with);
        assert_eq!(result, 7);
        assert_eq!(
            (reports[0].name.as_str(), reports[0].rows),
            ("h.csv", Some(2))
        );
        let without = python(&wrap_large_code("result = 7"), "restricted", &inputs).unwrap();
        assert_eq!(without, 7);
    }

    /// Dates are datetime64 (not Python objects) and nullable booleans stay
    /// booleans, whatever the part holds: the factors of the estimate are what
    /// the frame is.
    #[test]
    fn reads_return_dates_as_datetime64_and_nullable_booleans_as_boolean() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut inputs) = staged(dir.path()) else {
            return;
        };
        inputs.insert("_dir2".into(), Value::from(dir.path().to_str().unwrap()));
        let make = "import datetime, pandas as pd\npd.DataFrame({'d': [datetime.date(2024, 1, 2), None], 'b': pd.array([True, None], dtype='boolean'), 'a': [1, 2]}).to_parquet(_dir2 + '/t0/part-00000.parquet')\noutput = 1";
        python(make, "none", &inputs).unwrap();
        inputs.insert(
            "_ct_tables".into(),
            serde_json::json!([{"name": "t", "index": 0, "rows": 2, "parts": 1, "columns": [
                {"name": "d", "type": "date", "in_memory_bytes": 8},
                {"name": "b", "type": "bool", "in_memory_bytes": 1},
                {"name": "a", "type": "int", "in_memory_bytes": 16}]}]),
        );
        let out = run("f = tables['t'].read(columns=['d', 'b'])\noutput = [str(f['d'].dtype), str(f['b'].dtype)]", &inputs);
        assert_eq!(out, serde_json::json!(["datetime64[ns]", "boolean"]));
    }

    /// `head` decodes one batch of the rows asked for, never the part: the reader
    /// is wrapped to see what it is asked.
    #[test]
    fn head_reads_one_batch_of_n_rows_not_the_part() {
        let dir = tempfile::tempdir().unwrap();
        let Some(inputs) = staged(dir.path()) else {
            return;
        };
        let out = run(
            "_orig = _ct_pyarrow\ncalls = []\ndef _ct_pyarrow():\n    pd, pa, pq = _orig()\n    class PF:\n        def __init__(self, path):\n            self._pf = pq.ParquetFile(path)\n        def iter_batches(self, **kw):\n            calls.append(kw['batch_size'])\n            return self._pf.iter_batches(**kw)\n    class PQ:\n        ParquetFile = PF\n        def read_table(self, *a, **k):\n            calls.append('read_table')\n            return pq.read_table(*a, **k)\n    return pd, pa, PQ\nrows = len(tables['t'].head(3, columns=['a']))\noutput = [rows, calls]",
            &inputs,
        );
        assert_eq!(out, serde_json::json!([3, [3]]));
    }

    /// `parts` with no columns is estimated too: a part too wide to load whole is
    /// refused with the way out, before anything is read.
    #[test]
    fn parts_without_columns_is_estimated_and_refused_when_too_wide() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut inputs) = staged(dir.path()) else {
            return;
        };
        inputs.insert("_ct_read_max".into(), Value::from(100));
        let out = python(
            &format!("{PRELUDE}\ntry:\n    next(tables['t'].parts())\nexcept LargeTableError as e:\n    output = str(e)"),
            "restricted",
            &inputs,
        )
        .unwrap();
        let text = out.as_str().unwrap();
        assert!(
            text.contains("one part of `t` is too large") && text.contains("parts(columns=[...])"),
            "{text}"
        );
    }

    /// A size that cannot be read is not a limit that is met.
    #[test]
    fn a_file_whose_size_cannot_be_checked_is_not_returned() {
        let out = tempfile::tempdir().unwrap();
        let Some(inputs) = emitting(out.path()) else {
            return;
        };
        let msg = python(
            &format!("{PRELUDE}\ntry:\n    _ct_file_size('/nonexistent-colmena/x.csv')\nexcept LargeTableError as e:\n    output = str(e)"),
            "restricted",
            &inputs,
        )
        .unwrap();
        assert!(
            msg.as_str().unwrap().contains("could not check the size"),
            "{msg}"
        );
    }
}
