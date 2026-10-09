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

/// The model's code wrapped for the large path: the same imports and the same
/// `result` convention as the small path, with `tables` and the `df` guard in
/// place of a loaded DataFrame.
pub fn wrap_large_code(code: &str) -> String {
    format!(
        "\nimport pandas as pd\nimport numpy as np\nimport scipy.stats as stats\n\n{PRELUDE}\nresult = None\n\n{code}\n\n{}\n",
        postlude()
    )
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
    inputs
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
        assert_eq!(tail(&wrapped), tail(&small));
        assert!(!wrapped.contains("_attachment_records"));
        assert!(wrapped.contains(PRELUDE));
        // The prelude comes before the model's code, so `tables` exists for it.
        assert!(wrapped.find(PRELUDE).unwrap() < wrapped.find("result = tables.names").unwrap());
    }
}
