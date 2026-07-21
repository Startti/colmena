//! `for_each` — runs an embedded target tool once per row of a list,
//! deterministically (iteration in Rust, not the LLM loop). Usable as a graph
//! node and as an LLM tool. See spec 2026-07-20-deterministic-list-tool-execution.

use crate::colmena_log;
use crate::dag_engine::application::list_tool_executor::{
    run_list, ExecPolicy, ItemStatus, OnError, DEFAULT_MAX_ITEMS,
};
use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::{ExecutionObserver, NodeEvent};
use crate::dag_engine::domain::tool_configuration::parse_node_schema;
use crate::dag_engine::infrastructure::node_schema_merge::merge_args_into_schema;
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    dispatch_gsheets_create_spreadsheet, dispatch_gsheets_set_range,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::{Arc, OnceLock};

/// Upper clamp on `concurrency` — a defensive ceiling so a misconfigured or
/// LLM-supplied huge value can't spawn unbounded concurrent dispatches.
const MAX_CONCURRENCY: usize = 64;

/// Ambient context keys forwarded from `for_each`'s own `inputs` into every
/// per-row target dispatch (when present, and only if the target's merged
/// args don't already set them). This keeps recursion/session context alive
/// across a `for_each` boundary — e.g. `__colmena_subgraph_depth` so a
/// `subgraph` target reached via `for_each` doesn't reset `MAX_SUBGRAPH_TOOL_DEPTH`
/// to 0. Deliberately excludes `__node_id` / `__colmena_node_id_path`, which
/// are for_each-specific and could collide with the target's own path logic.
const FORWARDED_CONTEXT_KEYS: [&str; 3] = [
    "__colmena_subgraph_depth",
    "__colmena_session_id",
    "__colmena_agent_session_id",
];

pub struct ForEachNode {
    pub registry: Arc<OnceLock<Arc<dyn NodeRegistryPort>>>,
}

impl Default for ForEachNode {
    fn default() -> Self {
        Self::new()
    }
}

impl ForEachNode {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(OnceLock::new()),
        }
    }
}

/// Select a single column from each row, renaming it to `as_name` (or the
/// column name itself if `as_name` is absent). With no `column`, rows pass
/// through unchanged.
pub(crate) fn apply_column_selection(
    rows: Vec<Value>,
    column: Option<&str>,
    as_name: Option<&str>,
) -> Vec<Value> {
    let Some(col) = column else { return rows };
    let key = as_name.unwrap_or(col);
    rows.into_iter()
        .map(|row| {
            let val = row.get(col).cloned().unwrap_or(Value::Null);
            json!({ key: val })
        })
        .collect()
}

/// Read a config field, falling back to the inputs map. Mirrors the
/// `suspend` node's `cfg_or_input` — config-first so graph-node static
/// config is honored, inputs-fallback so the tool path (where the
/// executor folds everything into `inputs` and passes `config = {}`)
/// keeps working unchanged.
fn cfg_or_input<'a>(config: &'a Value, inputs: &'a NodeInputs, key: &str) -> Option<&'a Value> {
    config.get(key).or_else(|| inputs.get(key))
}

/// A short JSON type name for error messages (`items` type-mismatch).
fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Read the list of rows from config or inputs: `items` (inline array) →
/// `items_from` (data-source handle) → default input edge.
async fn resolve_rows_async(config: &Value, inputs: &NodeInputs) -> Result<Vec<Value>, String> {
    if let Some(v) = cfg_or_input(config, inputs, "items") {
        return match v {
            Value::Array(arr) => Ok(arr.clone()),
            other => Err(format!(
                "for_each: `items` must be an array of row objects, got {}",
                value_type_name(other)
            )),
        };
    }

    if let Some(handle) = cfg_or_input(config, inputs, "items_from") {
        let source = handle.get("source").and_then(|v| v.as_str()).unwrap_or("");
        let column = handle.get("column").and_then(|v| v.as_str());
        let as_name = handle.get("as").and_then(|v| v.as_str());
        let rows = match source {
            "sheet" => {
                use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::dispatch_gsheets_read;
                let reference = handle.get("ref").and_then(|v| v.as_str()).unwrap_or("");
                // `ref` = "<spreadsheet_id>|<sheet>|<range?>"
                let mut parts = reference.split('|');
                let spreadsheet_id = parts.next().unwrap_or("").to_string();
                let sheet = parts.next().unwrap_or("").to_string();
                let range = parts.next().map(|s| s.to_string());
                let mut args = json!({ "spreadsheet_id": spreadsheet_id, "sheet": sheet, "format": "json", "as_records": true });
                if let Some(r) = range {
                    args["range"] = json!(r);
                }
                let res = dispatch_gsheets_read(args).await;
                if res.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                    return Err(format!("for_each items_from sheet failed: {res}"));
                }
                res.get("values")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .ok_or("for_each items_from sheet: no `values` in response")?
            }
            other => {
                return Err(format!(
                    "for_each items_from: unknown source '{other}' (v1: sheet)"
                ))
            }
        };
        return Ok(apply_column_selection(rows, column, as_name));
    }

    if let Some(Value::Array(arr)) = inputs.get("input") {
        return Ok(arr.clone());
    }
    if let Some(Value::Array(arr)) = inputs.get("default") {
        return Ok(arr.clone());
    }
    Err("for_each: no list found — provide `items`, `items_from`, or an input edge carrying an array".into())
}

/// A stable per-row key for progress/checklist events: first scalar field, else index.
fn row_key(row: &Value, index: usize) -> String {
    if let Value::Object(map) = row {
        if let Some((k, v)) = map.iter().find(|(_, v)| v.is_string() || v.is_number()) {
            return format!("{k}={v}");
        }
    }
    format!("index={index}")
}

fn parse_policy(config: &Value, inputs: &NodeInputs) -> ExecPolicy {
    let on_error = match cfg_or_input(config, inputs, "on_error").and_then(|v| v.as_str()) {
        Some("abort") => OnError::Abort,
        _ => OnError::Continue,
    };
    let concurrency = (cfg_or_input(config, inputs, "concurrency")
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .max(1) as usize)
        .min(MAX_CONCURRENCY);
    let max_items = cfg_or_input(config, inputs, "max_items")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_MAX_ITEMS as u64) as usize;
    ExecPolicy {
        on_error,
        concurrency,
        max_items,
    }
}

/// Parsed `results_to` sink config. v1 supports only `sink: "sheet"` — a NEW
/// spreadsheet created once before iteration, never the input sheet.
#[derive(Debug, Clone)]
struct ResultsSink {
    title: String,
    mode: String,
}

/// Parse a `results_to` value into a `ResultsSink`, or an error message if
/// the sink is unsupported. Pure — no I/O.
fn parse_results_to(v: &Value) -> Result<ResultsSink, String> {
    let sink = v.get("sink").and_then(|s| s.as_str()).unwrap_or("");
    if sink != "sheet" {
        return Err(format!(
            "for_each results_to: unknown sink '{sink}' (v1: sheet)"
        ));
    }
    let title = v
        .get("title")
        .and_then(|s| s.as_str())
        .unwrap_or("for_each results")
        .to_string();
    let mode = v.get("mode").and_then(|s| s.as_str()).unwrap_or("final");
    if mode != "final" && mode != "incremental" {
        return Err(format!(
            "for_each results_to: unknown mode '{mode}' (expected 'final' or 'incremental')"
        ));
    }
    Ok(ResultsSink {
        title,
        mode: mode.to_string(),
    })
}

/// Header row for the results sheet: `["index"] + <input columns> + ["status", "result"]`.
/// Input columns are the sorted keys of the first row's input object (rows are
/// homogeneous); if the first row is a scalar (or the list is empty), a
/// single `"input"` column is used instead. Pure — no I/O.
fn results_sheet_header(rows: &[Value]) -> (Vec<String>, Vec<Value>) {
    let input_cols: Vec<String> = match rows.first() {
        Some(Value::Object(map)) => {
            let mut cols: Vec<String> = map.keys().cloned().collect();
            cols.sort();
            cols
        }
        _ => vec!["input".to_string()],
    };
    let mut header: Vec<Value> = vec![json!("index")];
    header.extend(input_cols.iter().map(|c| json!(c)));
    header.push(json!("status"));
    header.push(json!("result"));
    (input_cols, header)
}

/// A single data row for the results sheet, matching `results_sheet_header`'s
/// column order: `[index, <input[col] for each input col>, status, result]`.
/// When `input_cols == ["input"]` and `input` is not an object, the scalar
/// value itself fills that column. Pure — no I/O.
fn results_sheet_row(
    index: usize,
    input: &Value,
    status: &str,
    result_cell: Value,
    input_cols: &[String],
) -> Vec<Value> {
    let mut row: Vec<Value> = vec![json!(index)];
    for col in input_cols {
        let cell = match input {
            Value::Object(map) => map.get(col).cloned().unwrap_or(Value::Null),
            other if col == "input" => other.clone(),
            _ => Value::Null,
        };
        row.push(cell);
    }
    row.push(json!(status));
    row.push(result_cell);
    row
}

#[async_trait::async_trait]
impl ExecutableNode for ForEachNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let node_id = inputs
            .get("__node_id")
            .and_then(|v| v.as_str())
            .unwrap_or("for_each")
            .to_string();

        let target = cfg_or_input(config, inputs, "target")
            .cloned()
            .ok_or("for_each: missing `target` (embedded tool config)")?;
        let target_type = target
            .get("node_type")
            .and_then(|v| v.as_str())
            .ok_or("for_each: `target.node_type` is required")?
            .to_string();
        if target_type == "for_each" {
            return Err("for_each: a for_each cannot target itself".into());
        }
        let target_schema = target
            .get("node_schema")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let policy = parse_policy(config, inputs);
        let mut rows = resolve_rows_async(config, inputs)
            .await
            .map_err(|e| -> Box<dyn StdError + Send + Sync> { e.into() })?;
        if rows.len() > policy.max_items {
            colmena_log!(
                "⚠️ [for_each] {} rows exceeds max_items={}, truncating.",
                rows.len(),
                policy.max_items
            );
            rows.truncate(policy.max_items);
        }
        let total = rows.len();

        let results_to = match cfg_or_input(config, inputs, "results_to") {
            Some(v) => Some(
                parse_results_to(v).map_err(|e| -> Box<dyn StdError + Send + Sync> { e.into() })?,
            ),
            None => None,
        };

        // `results_to` sheet sink: create the destination spreadsheet ONCE,
        // before iterating, and write the header row upfront. Never touches
        // the input sheet — always a brand-new spreadsheet.
        let mut results_sheet_info: Option<Value> = None;
        // (spreadsheet_id, sheet_name, mode, input_cols) — captured for the
        // per-row `incremental` write and the post-loop `final` write.
        let mut sink_ctx: Option<(String, String, String, Vec<String>)> = None;
        if let Some(sink) = &results_to {
            let create_res =
                dispatch_gsheets_create_spreadsheet(json!({ "title": sink.title })).await;
            if create_res.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                return Err(format!(
                    "for_each results_to: failed to create results spreadsheet: {create_res}"
                )
                .into());
            }
            let spreadsheet_id = create_res
                .get("spreadsheet_id")
                .and_then(|v| v.as_str())
                .ok_or("for_each results_to: create_spreadsheet response missing spreadsheet_id")?
                .to_string();
            let url = create_res
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let sheet_name = create_res
                .get("sheets")
                .and_then(|s| s.as_array())
                .and_then(|a| a.first())
                .and_then(|s| s.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("Sheet1")
                .to_string();

            let (input_cols, header) = results_sheet_header(&rows);
            let header_res = dispatch_gsheets_set_range(json!({
                "spreadsheet_id": spreadsheet_id,
                "sheet": sheet_name,
                "start": "A1",
                "values": [header],
            }))
            .await;
            if header_res.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                colmena_log!(
                    "⚠️ [for_each] results_to: failed to write header row: {}",
                    header_res
                );
            }

            results_sheet_info = Some(json!({ "spreadsheet_id": spreadsheet_id, "url": url }));
            sink_ctx = Some((spreadsheet_id, sheet_name, sink.mode.clone(), input_cols));
        }
        // Only the `incremental` mode writes inside the per-row dispatch closure.
        let incremental_sink: Option<(String, String, Vec<String>)> =
            sink_ctx.as_ref().and_then(|(id, sheet, mode, cols)| {
                (mode == "incremental").then(|| (id.clone(), sheet.clone(), cols.clone()))
            });

        let registry = self
            .registry
            .get()
            .ok_or("for_each: NodeRegistryPort not initialized")?
            .clone();

        if let Some(obs) = &observer {
            obs.on_event(NodeEvent::BatchProgress {
                node_id: node_id.clone(),
                total,
                completed: 0,
                ok: 0,
                err: 0,
                in_flight: 0,
            });
        }

        // Ambient recursion/session context from for_each's own inputs, forwarded
        // into every per-row target dispatch (see FORWARDED_CONTEXT_KEYS doc).
        // Cloned into an owned map upfront so the dispatch closure below doesn't
        // need to borrow `inputs` across the `.await` points inside `run_list`.
        let forwarded_context: HashMap<String, Value> = FORWARDED_CONTEXT_KEYS
            .iter()
            .filter_map(|&key| inputs.get(key).map(|v| (key.to_string(), v.clone())))
            .collect();

        // Per-row dispatch: merge the row into the target schema, run the target node.
        let dispatch = |index: usize, row: Value| {
            let registry = registry.clone();
            let target_type = target_type.clone();
            let target_schema = target_schema.clone();
            let observer = observer.clone();
            let forwarded_context = forwarded_context.clone();
            let incremental_sink = incremental_sink.clone();
            async move {
                let dispatch_result: Result<Value, String> = async {
                    let row_map: HashMap<String, Value> = match &row {
                        Value::Object(m) => m.clone().into_iter().collect(),
                        other => {
                            let mut h = HashMap::new();
                            h.insert("value".to_string(), other.clone());
                            h
                        }
                    };
                    let mut merged = merge_args_into_schema(&target_schema, row_map)
                        .map_err(|e| format!("row {index}: {e}"))?;
                    for (key, value) in forwarded_context {
                        merged.entry(key).or_insert(value);
                    }
                    if let Ok(node_schema) = serde_json::from_value::<
                        crate::dag_engine::domain::tool_configuration::NodeSchema,
                    >(target_schema.clone())
                    {
                        if let Ok(parsed) = parse_node_schema(&node_schema) {
                            for req in &parsed.required_params {
                                // A required param may live at the top level of `merged`, or
                                // (when it's a container child, e.g. an http_request's
                                // `body.user_id`) nested inside its container object. Mirrors
                                // the `real_key` logic in `merge_args_into_schema`.
                                let present =
                                    if let Some(container) = parsed.param_to_container.get(req) {
                                        let real_key = req
                                            .find('.')
                                            .map(|p| &req[p + 1..])
                                            .unwrap_or(req.as_str());
                                        merged
                                            .get(container)
                                            .and_then(|v| v.as_object())
                                            .is_some_and(|m| m.contains_key(real_key))
                                    } else {
                                        merged.contains_key(req)
                                    };
                                if !present {
                                    return Err(format!(
                                        "row {index}: missing required param '{req}'"
                                    ));
                                }
                            }
                        }
                    }
                    let node = registry.get_node(&target_type).ok_or_else(|| {
                        format!("row {index}: unknown target node_type '{target_type}'")
                    })?;
                    let mut item_state = json!({});
                    let result = node
                        .execute(&merged, &json!({}), &mut item_state, observer.clone())
                        .await
                        .map_err(|e| format!("row {index}: {e}"))?;
                    // HITL fail-closed: a SUSPENDED result inside a fan-out is an error.
                    if result.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED")
                    {
                        return Err(format!(
                            "row {index}: target suspended (HITL not supported inside for_each)"
                        ));
                    }
                    Ok(result)
                }
                .await;

                // `results_to` incremental sink: write this row's own address the
                // moment it finishes, both ok and err, so the sheet reflects
                // failures too. Distinct per-row ranges avoid concurrent-write races.
                if let Some((spreadsheet_id, sheet_name, input_cols)) = &incremental_sink {
                    let (status, cell) = match &dispatch_result {
                        Ok(output) => (
                            "ok",
                            json!(serde_json::to_string(output).unwrap_or_default()),
                        ),
                        Err(e) => ("err", json!(e.clone())),
                    };
                    let row_values = results_sheet_row(index, &row, status, cell, input_cols);
                    let write_res = dispatch_gsheets_set_range(json!({
                        "spreadsheet_id": spreadsheet_id,
                        "sheet": sheet_name,
                        "start": format!("A{}", index + 2),
                        "values": [row_values],
                    }))
                    .await;
                    if write_res.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                        colmena_log!(
                            "⚠️ [for_each] results_to incremental write failed for row {}: {}",
                            index,
                            write_res
                        );
                    }
                }

                dispatch_result
            }
        };

        let results = run_list(rows, &policy, dispatch).await;

        // Emit per-item + final progress.
        let mut ok = 0usize;
        let mut err = 0usize;
        let mut out_rows: Vec<Value> = Vec::with_capacity(results.len());
        for r in &results {
            match r.status {
                ItemStatus::Ok => ok += 1,
                ItemStatus::Err => err += 1,
            }
            if let Some(obs) = &observer {
                obs.on_event(NodeEvent::BatchItemFinished {
                    node_id: node_id.clone(),
                    index: r.index,
                    key: row_key(&r.input, r.index),
                    status: if r.status == ItemStatus::Ok {
                        "ok".into()
                    } else {
                        "err".into()
                    },
                });
            }
            let mut m = Map::new();
            m.insert("index".into(), json!(r.index));
            m.insert("input".into(), r.input.clone());
            m.insert(
                "status".into(),
                json!(if r.status == ItemStatus::Ok {
                    "ok"
                } else {
                    "err"
                }),
            );
            if let Some(o) = &r.output {
                m.insert("output".into(), o.clone());
            }
            if let Some(e) = &r.error {
                m.insert("error".into(), json!(e));
            }
            out_rows.push(Value::Object(m));
        }
        if let Some(obs) = &observer {
            obs.on_event(NodeEvent::BatchProgress {
                node_id: node_id.clone(),
                total,
                completed: results.len(),
                ok,
                err,
                in_flight: 0,
            });
        }

        // `results_to` final-mode sink: one bulk write of every data row after
        // all rows finish (as opposed to `incremental`, which already wrote
        // per-row inside the dispatch closure above).
        let mut results_sheet_error: Option<String> = None;
        if let Some((spreadsheet_id, sheet_name, mode, input_cols)) = &sink_ctx {
            if mode == "final" && !results.is_empty() {
                let data_rows: Vec<Vec<Value>> = results
                    .iter()
                    .map(|r| {
                        let (status, cell) = match r.status {
                            ItemStatus::Ok => (
                                "ok",
                                json!(serde_json::to_string(&r.output).unwrap_or_default()),
                            ),
                            ItemStatus::Err => ("err", json!(r.error.clone().unwrap_or_default())),
                        };
                        results_sheet_row(r.index, &r.input, status, cell, input_cols)
                    })
                    .collect();
                let write_res = dispatch_gsheets_set_range(json!({
                    "spreadsheet_id": spreadsheet_id,
                    "sheet": sheet_name,
                    "start": "A2",
                    "values": data_rows,
                }))
                .await;
                if write_res.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                    results_sheet_error = Some(format!("{write_res}"));
                }
            }
        }

        let mut output = json!({ "total": total, "ok": ok, "err": err, "results": out_rows });
        if let Some(info) = results_sheet_info {
            output["results_sheet"] = info;
        }
        if let Some(e) = results_sheet_error {
            output["results_sheet_error"] = json!(e);
        }
        Ok(json!({ "output": output }))
    }

    fn default_output(&self) -> Option<&str> {
        Some("output")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "for_each",
            "inputs": {
                "target": "object (embedded tool config: {node_type, node_schema})",
                "items": "array of row objects (optional if an input edge carries the list)",
                "on_error": "continue | abort",
                "concurrency": "integer >= 1",
                "max_items": "integer"
            },
            "outputs": { "output": "object { total, ok, err, results[] }" }
        })
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Run an embedded target tool once per row of a list, deterministically. \
              Provide the list via `items` (inline array) or `items_from` \
              ({ source: \"sheet\", ref: \"<spreadsheet_id>|<sheet>|<range?>\", column?, as? }) \
              to read rows from a Google Sheet without the model re-typing them.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::infrastructure::nodes::math::AddNode;

    struct StubRegistry {
        add: Arc<dyn ExecutableNode>,
        echo: Option<Arc<dyn ExecutableNode>>,
    }
    impl NodeRegistryPort for StubRegistry {
        fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
            match node_type {
                "add" => Some(self.add.clone()),
                "echo" => self.echo.clone(),
                _ => None,
            }
        }
        fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
            HashMap::new()
        }
    }

    /// A tiny stub node that echoes back exactly the inputs it received (as
    /// its `output`). Used to observe what a target dispatch actually gets,
    /// e.g. whether ambient context keys were forwarded.
    struct EchoNode;
    #[async_trait::async_trait]
    impl ExecutableNode for EchoNode {
        async fn execute(
            &self,
            inputs: &NodeInputs,
            _config: &Value,
            _state: &mut Value,
            _observer: Option<Arc<dyn ExecutionObserver>>,
        ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
            let map: Map<String, Value> = inputs.clone().into_iter().collect();
            Ok(json!({ "output": Value::Object(map) }))
        }

        fn schema(&self) -> Value {
            json!({})
        }
    }

    #[test]
    fn column_selection_maps_scalar_rows() {
        let rows = vec![
            json!({"user_id": 1, "name": "a"}),
            json!({"user_id": 2, "name": "b"}),
        ];
        let picked = super::apply_column_selection(rows, Some("user_id"), Some("uid"));
        assert_eq!(picked[0], json!({"uid": 1}));
        assert_eq!(picked[1], json!({"uid": 2}));
    }

    #[test]
    fn column_selection_no_column_returns_rows_unchanged() {
        let rows = vec![json!({"a": 1}), json!({"a": 2})];
        let picked = super::apply_column_selection(rows.clone(), None, None);
        assert_eq!(picked, rows);
    }

    #[test]
    fn column_selection_defaults_key_to_column_name() {
        let rows = vec![json!({"user_id": 1, "name": "a"})];
        let picked = super::apply_column_selection(rows, Some("user_id"), None);
        assert_eq!(picked[0], json!({"user_id": 1}));
    }

    #[tokio::test]
    async fn runs_add_target_over_inline_items() {
        let node = ForEachNode::new();
        node.registry
            .set(Arc::new(StubRegistry {
                add: Arc::new(AddNode),
                echo: None,
            }) as Arc<dyn NodeRegistryPort>)
            .ok();

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "target".to_string(),
            json!({
                "node_type": "add",
                "node_schema": {
                    "a": { "type": "number", "required": true },
                    "b": { "type": "number", "required": true }
                }
            }),
        );
        inputs.insert("items".to_string(), json!([{"a":1,"b":2},{"a":10,"b":20}]));

        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        let results = out["output"]["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(out["output"]["ok"], 2);
        assert_eq!(results[0]["status"], "ok");
        assert_eq!(results[0]["output"], json!({"output": 3.0}));
        assert_eq!(results[1]["output"], json!({"output": 30.0}));
    }

    #[tokio::test]
    async fn runs_as_graph_node_reading_config() {
        let node = ForEachNode::new();
        node.registry
            .set(Arc::new(StubRegistry {
                add: Arc::new(AddNode),
                echo: None,
            }) as Arc<dyn NodeRegistryPort>)
            .ok();

        let config = json!({
            "target": {
                "node_type": "add",
                "node_schema": {
                    "a": { "type": "number", "required": true },
                    "b": { "type": "number", "required": true }
                }
            },
            "items": [{"a":1,"b":2},{"a":10,"b":20}]
        });
        let inputs: NodeInputs = HashMap::new();

        let mut state = json!({});
        let out = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .unwrap();
        let results = out["output"]["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(out["output"]["ok"], 2);
        assert_eq!(results[0]["status"], "ok");
        assert_eq!(results[0]["output"], json!({"output": 3.0}));
        assert_eq!(results[1]["output"], json!({"output": 30.0}));
    }

    #[tokio::test]
    async fn missing_required_param_becomes_err_row() {
        let node = ForEachNode::new();
        node.registry
            .set(Arc::new(StubRegistry {
                add: Arc::new(AddNode),
                echo: None,
            }) as Arc<dyn NodeRegistryPort>)
            .ok();

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "target".to_string(),
            json!({
                "node_type": "add",
                "node_schema": {
                    "a": { "type": "number", "required": true },
                    "b": { "type": "number", "required": true }
                }
            }),
        );
        inputs.insert("items".to_string(), json!([{"a":1}])); // missing b
        inputs.insert("on_error".to_string(), json!("continue"));

        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(out["output"]["err"], 1);
        assert_eq!(out["output"]["results"][0]["status"], "err");
    }

    #[tokio::test]
    async fn required_param_nested_in_container_is_not_falsely_missing() {
        // Regression: a target `node_schema` with a container field (e.g. an
        // http_request's `body.user_id`) folds LLM-visible children into the
        // container object at merge time (`merged["body"]["user_id"]`), not at
        // the top level (`merged["user_id"]`). The required-param check must
        // look inside the container, not `merged.contains_key(<child name>)`
        // directly — otherwise every row with a present-but-nested required
        // field is wrongly flagged as missing it.
        let node = ForEachNode::new();
        node.registry
            .set(Arc::new(StubRegistry {
                add: Arc::new(AddNode),
                echo: None,
            }) as Arc<dyn NodeRegistryPort>)
            .ok();

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "target".to_string(),
            json!({
                "node_type": "add",
                "node_schema": {
                    "payload": {
                        "properties": {
                            "a": { "type": "number", "required": true },
                            "b": { "type": "number", "required": true }
                        }
                    }
                }
            }),
        );
        inputs.insert("items".to_string(), json!([{"a": 1, "b": 2}]));
        inputs.insert("on_error".to_string(), json!("continue"));

        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        // AddNode still fails (it reads top-level `a`/`b`, not `payload.a`/`payload.b`),
        // but the failure must NOT be the required-param guard — proving the guard
        // correctly saw `a`/`b` nested inside `payload`.
        let error = out["output"]["results"][0]["error"].as_str().unwrap();
        assert!(
            !error.contains("missing required param"),
            "required-param guard falsely rejected a container-nested field: {error}"
        );
    }

    #[tokio::test]
    async fn empty_list_is_not_an_error() {
        let node = ForEachNode::new();
        node.registry
            .set(Arc::new(StubRegistry {
                add: Arc::new(AddNode),
                echo: None,
            }) as Arc<dyn NodeRegistryPort>)
            .ok();

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "target".to_string(),
            json!({ "node_type": "add", "node_schema": {} }),
        );
        inputs.insert("items".to_string(), json!([]));

        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(out["output"]["total"], 0);
        assert!(out["output"]["results"].as_array().unwrap().is_empty());
    }

    #[test]
    fn concurrency_is_clamped_to_max_concurrency() {
        let config = json!({ "concurrency": 1000 });
        let inputs: NodeInputs = HashMap::new();
        let policy = super::parse_policy(&config, &inputs);
        assert_eq!(policy.concurrency, MAX_CONCURRENCY);
    }

    #[test]
    fn concurrency_below_one_is_raised_to_one() {
        let config = json!({ "concurrency": 0 });
        let inputs: NodeInputs = HashMap::new();
        let policy = super::parse_policy(&config, &inputs);
        assert_eq!(policy.concurrency, 1);
    }

    #[tokio::test]
    async fn forwards_subgraph_depth_into_target_dispatch() {
        // A for_each whose target is reached via a subgraph-like tool must not
        // silently reset the ambient recursion depth — otherwise a
        // self-referencing `child_graph_path` routed through for_each could
        // bypass MAX_SUBGRAPH_TOOL_DEPTH. Use an echo target to observe exactly
        // what context the per-row dispatch receives.
        let node = ForEachNode::new();
        node.registry
            .set(Arc::new(StubRegistry {
                add: Arc::new(AddNode),
                echo: Some(Arc::new(EchoNode)),
            }) as Arc<dyn NodeRegistryPort>)
            .ok();

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "target".to_string(),
            json!({ "node_type": "echo", "node_schema": {} }),
        );
        inputs.insert("items".to_string(), json!([{"a": 1}]));
        inputs.insert("__colmena_subgraph_depth".to_string(), json!(3));
        // Not forwarded — for_each-specific, must not leak into the target.
        inputs.insert("__node_id".to_string(), json!("for_each_1"));

        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        let result_output = &out["output"]["results"][0]["output"]["output"];
        assert_eq!(result_output["__colmena_subgraph_depth"], json!(3));
        assert!(result_output.get("__node_id").is_none());
    }

    #[test]
    fn results_sheet_header_derives_sorted_input_columns() {
        let rows = vec![json!({"b": 1, "a": 2})];
        let (cols, header) = super::results_sheet_header(&rows);
        assert_eq!(cols, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            header,
            vec![
                json!("index"),
                json!("a"),
                json!("b"),
                json!("status"),
                json!("result")
            ]
        );
    }

    #[test]
    fn results_sheet_header_scalar_input_uses_single_column() {
        let rows = vec![json!(42)];
        let (cols, header) = super::results_sheet_header(&rows);
        assert_eq!(cols, vec!["input".to_string()]);
        assert_eq!(
            header,
            vec![
                json!("index"),
                json!("input"),
                json!("status"),
                json!("result")
            ]
        );
    }

    #[test]
    fn results_sheet_header_empty_rows_falls_back_to_input_column() {
        let rows: Vec<Value> = vec![];
        let (cols, header) = super::results_sheet_header(&rows);
        assert_eq!(cols, vec!["input".to_string()]);
        assert_eq!(
            header,
            vec![
                json!("index"),
                json!("input"),
                json!("status"),
                json!("result")
            ]
        );
    }

    #[test]
    fn results_sheet_row_builds_ok_row_from_object_input() {
        let input = json!({"a": 2, "b": 1});
        let cols = vec!["a".to_string(), "b".to_string()];
        let row = super::results_sheet_row(0, &input, "ok", json!("{\"output\":3.0}"), &cols);
        assert_eq!(
            row,
            vec![
                json!(0),
                json!(2),
                json!(1),
                json!("ok"),
                json!("{\"output\":3.0}")
            ]
        );
    }

    #[test]
    fn results_sheet_row_builds_err_row_with_error_string() {
        let input = json!({"a": 1});
        let cols = vec!["a".to_string()];
        let row = super::results_sheet_row(2, &input, "err", json!("row 2: boom"), &cols);
        assert_eq!(
            row,
            vec![json!(2), json!(1), json!("err"), json!("row 2: boom")]
        );
    }

    #[test]
    fn results_sheet_row_scalar_input_uses_input_value() {
        let input = json!(42);
        let cols = vec!["input".to_string()];
        let row = super::results_sheet_row(0, &input, "ok", json!("42"), &cols);
        assert_eq!(row, vec![json!(0), json!(42), json!("ok"), json!("42")]);
    }

    #[test]
    fn parse_results_to_accepts_valid_modes_and_rejects_unknown() {
        // Absent mode defaults to "final".
        let sink = super::parse_results_to(&json!({ "sink": "sheet" })).unwrap();
        assert_eq!(sink.mode, "final");
        // Explicit valid modes are accepted.
        assert_eq!(
            super::parse_results_to(&json!({ "sink": "sheet", "mode": "final" }))
                .unwrap()
                .mode,
            "final"
        );
        assert_eq!(
            super::parse_results_to(&json!({ "sink": "sheet", "mode": "incremental" }))
                .unwrap()
                .mode,
            "incremental"
        );
        // An unrecognized mode fails loudly rather than silently writing nothing.
        let err =
            super::parse_results_to(&json!({ "sink": "sheet", "mode": "Final" })).unwrap_err();
        assert!(
            err.contains("unknown mode 'Final'")
                && err.contains("expected 'final' or 'incremental'"),
            "unexpected error message: {err}"
        );
    }

    #[tokio::test]
    async fn results_to_unknown_mode_returns_targeted_error() {
        let node = ForEachNode::new();
        node.registry
            .set(Arc::new(StubRegistry {
                add: Arc::new(AddNode),
                echo: None,
            }) as Arc<dyn NodeRegistryPort>)
            .ok();

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "target".to_string(),
            json!({ "node_type": "add", "node_schema": {} }),
        );
        inputs.insert("items".to_string(), json!([{"a":1,"b":2}]));
        inputs.insert(
            "results_to".to_string(),
            json!({"sink": "sheet", "mode": "increment"}),
        );

        let mut state = json!({});
        let err = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("unknown mode 'increment'"),
            "unexpected error message: {err}"
        );
    }

    #[tokio::test]
    async fn results_to_absent_leaves_output_unchanged() {
        let node = ForEachNode::new();
        node.registry
            .set(Arc::new(StubRegistry {
                add: Arc::new(AddNode),
                echo: None,
            }) as Arc<dyn NodeRegistryPort>)
            .ok();

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "target".to_string(),
            json!({
                "node_type": "add",
                "node_schema": {
                    "a": { "type": "number", "required": true },
                    "b": { "type": "number", "required": true }
                }
            }),
        );
        inputs.insert("items".to_string(), json!([{"a":1,"b":2}]));

        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert!(out["output"].get("results_sheet").is_none());
        assert!(out["output"].get("results_sheet_error").is_none());
    }

    #[tokio::test]
    async fn results_to_unknown_sink_returns_targeted_error() {
        let node = ForEachNode::new();
        node.registry
            .set(Arc::new(StubRegistry {
                add: Arc::new(AddNode),
                echo: None,
            }) as Arc<dyn NodeRegistryPort>)
            .ok();

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "target".to_string(),
            json!({ "node_type": "add", "node_schema": {} }),
        );
        inputs.insert("items".to_string(), json!([{"a":1,"b":2}]));
        inputs.insert("results_to".to_string(), json!({"sink": "database"}));

        let mut state = json!({});
        let err = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("unknown sink 'database'"),
            "unexpected error message: {err}"
        );
    }

    #[tokio::test]
    async fn non_array_items_yields_targeted_error() {
        let node = ForEachNode::new();
        node.registry
            .set(Arc::new(StubRegistry {
                add: Arc::new(AddNode),
                echo: None,
            }) as Arc<dyn NodeRegistryPort>)
            .ok();

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(
            "target".to_string(),
            json!({ "node_type": "add", "node_schema": {} }),
        );
        inputs.insert("items".to_string(), json!("not-an-array"));

        let mut state = json!({});
        let err = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("`items` must be an array of row objects"),
            "unexpected error message: {msg}"
        );
        assert!(msg.contains("string"), "expected type name in error: {msg}");
    }
}
