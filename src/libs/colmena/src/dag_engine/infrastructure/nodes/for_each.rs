//! `for_each` — runs an embedded target tool once per row of a list,
//! deterministically (iteration in Rust, not the LLM loop). Usable as a graph
//! node and as an LLM tool. See spec 2026-07-20-deterministic-list-tool-execution.

use crate::dag_engine::application::list_tool_executor::{run_list, ExecPolicy, ItemStatus, OnError, DEFAULT_MAX_ITEMS};
use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::{ExecutionObserver, NodeEvent};
use crate::dag_engine::infrastructure::node_schema_merge::merge_args_into_schema;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::{Arc, OnceLock};

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
        Self { registry: Arc::new(OnceLock::new()) }
    }
}

/// Read the list of rows from inputs: `items` (inline array) → default input edge.
/// (`items_from` handles are added in a later task.)
fn resolve_rows(inputs: &NodeInputs) -> Result<Vec<Value>, String> {
    if let Some(Value::Array(arr)) = inputs.get("items") {
        return Ok(arr.clone());
    }
    if let Some(Value::Array(arr)) = inputs.get("input") {
        return Ok(arr.clone());
    }
    if let Some(Value::Array(arr)) = inputs.get("default") {
        return Ok(arr.clone());
    }
    Err("for_each: no list found — provide `items` (array) or an input edge carrying an array".into())
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

fn parse_policy(inputs: &NodeInputs) -> ExecPolicy {
    let on_error = match inputs.get("on_error").and_then(|v| v.as_str()) {
        Some("abort") => OnError::Abort,
        _ => OnError::Continue,
    };
    let concurrency = inputs.get("concurrency").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
    let max_items = inputs.get("max_items").and_then(|v| v.as_u64()).unwrap_or(DEFAULT_MAX_ITEMS as u64) as usize;
    ExecPolicy { on_error, concurrency, max_items }
}

#[async_trait::async_trait]
impl ExecutableNode for ForEachNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let node_id = inputs.get("__node_id").and_then(|v| v.as_str()).unwrap_or("for_each").to_string();

        let target = inputs.get("target").cloned().ok_or("for_each: missing `target` (embedded tool config)")?;
        let target_type = target
            .get("node_type")
            .and_then(|v| v.as_str())
            .ok_or("for_each: `target.node_type` is required")?
            .to_string();
        if target_type == "for_each" {
            return Err("for_each: a for_each cannot target itself".into());
        }
        let target_schema = target.get("node_schema").cloned().unwrap_or_else(|| json!({}));

        let policy = parse_policy(inputs);
        let mut rows = resolve_rows(inputs).map_err(|e| -> Box<dyn StdError + Send + Sync> { e.into() })?;
        if rows.len() > policy.max_items {
            eprintln!("⚠️ [for_each] {} rows exceeds max_items={}, truncating.", rows.len(), policy.max_items);
            rows.truncate(policy.max_items);
        }
        let total = rows.len();

        let registry = self.registry.get().ok_or("for_each: NodeRegistryPort not initialized")?.clone();

        if let Some(obs) = &observer {
            obs.on_event(NodeEvent::BatchProgress { node_id: node_id.clone(), total, completed: 0, ok: 0, err: 0, in_flight: 0 });
        }

        // Per-row dispatch: merge the row into the target schema, run the target node.
        let dispatch = |index: usize, row: Value| {
            let registry = registry.clone();
            let target_type = target_type.clone();
            let target_schema = target_schema.clone();
            let observer = observer.clone();
            async move {
                let row_map: HashMap<String, Value> = match &row {
                    Value::Object(m) => m.clone().into_iter().collect(),
                    other => {
                        let mut h = HashMap::new();
                        h.insert("value".to_string(), other.clone());
                        h
                    }
                };
                let merged = merge_args_into_schema(&target_schema, row_map).map_err(|e| format!("row {index}: {e}"))?;
                let node = registry
                    .get_node(&target_type)
                    .ok_or_else(|| format!("row {index}: unknown target node_type '{target_type}'"))?;
                let mut item_state = json!({});
                let result = node
                    .execute(&merged, &json!({}), &mut item_state, observer.clone())
                    .await
                    .map_err(|e| format!("row {index}: {e}"))?;
                // HITL fail-closed: a SUSPENDED result inside a fan-out is an error.
                if result.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED")
                    || result.get("status").and_then(|v| v.as_str()) == Some("SUSPENDED")
                {
                    return Err(format!("row {index}: target suspended (HITL not supported inside for_each)"));
                }
                Ok(result)
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
                    status: if r.status == ItemStatus::Ok { "ok".into() } else { "err".into() },
                });
            }
            let mut m = Map::new();
            m.insert("index".into(), json!(r.index));
            m.insert("input".into(), r.input.clone());
            m.insert("status".into(), json!(if r.status == ItemStatus::Ok { "ok" } else { "err" }));
            if let Some(o) = &r.output {
                m.insert("output".into(), o.clone());
            }
            if let Some(e) = &r.error {
                m.insert("error".into(), json!(e));
            }
            out_rows.push(Value::Object(m));
        }
        if let Some(obs) = &observer {
            obs.on_event(NodeEvent::BatchProgress { node_id: node_id.clone(), total, completed: results.len(), ok, err, in_flight: 0 });
        }

        Ok(json!({ "output": { "total": total, "ok": ok, "err": err, "results": out_rows } }))
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
              Provide the list via `items` (array) or `items_from` (a data-source handle). \
              Prefer `items_from` for lists that come from data — the model never re-types them.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::infrastructure::nodes::math::AddNode;

    struct StubRegistry {
        add: Arc<dyn ExecutableNode>,
    }
    impl NodeRegistryPort for StubRegistry {
        fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
            if node_type == "add" {
                Some(self.add.clone())
            } else {
                None
            }
        }
        fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
            HashMap::new()
        }
    }

    #[tokio::test]
    async fn runs_add_target_over_inline_items() {
        let node = ForEachNode::new();
        node.registry.set(Arc::new(StubRegistry { add: Arc::new(AddNode) }) as Arc<dyn NodeRegistryPort>).ok();

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
        let out = node.execute(&inputs, &json!({}), &mut state, None).await.unwrap();
        let results = out["output"]["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(out["output"]["ok"], 2);
        assert_eq!(results[0]["status"], "ok");
        assert_eq!(results[0]["output"], json!({"output": 3.0}));
        assert_eq!(results[1]["output"], json!({"output": 30.0}));
    }
}
