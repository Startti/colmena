# `for_each` — Deterministic List Tool Execution — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `for_each` node that runs one embedded target tool over N configurations deterministically (iteration in Rust, not the LLM ReAct loop), usable both as a graph node and as an LLM tool.

**Architecture:** One `ExecutableNode` (`for_each`) holds an embedded `target` tool config and iterates it over a list. A pure `ListToolExecutor` owns iteration/concurrency/policy/ordering/events. Per row, the node reuses the *same* node_schema merge as `DagToolExecutor::execute_inner` (extracted into a shared helper) and dispatches to the target node via an injected `NodeRegistryPort` handle (mirroring the `subgraph` OnceLock injection). Progress streams via two new typed SSE events.

**Tech Stack:** Rust (`colmena_dag_engine` crate), `async_trait`, `futures` (`buffer_unordered`), `serde_json`, `tokio`. Spec: [`docs/superpowers/specs/2026-07-20-deterministic-list-tool-execution-design.md`](../specs/2026-07-20-deterministic-list-tool-execution-design.md).

## Global Constraints

- Crate name is `colmena_dag_engine`. Run module tests with `cargo test --lib <module>`; run `cargo test --verbose` before any push (CI uses it; `--lib` hides doctest/integration failures).
- `[lints.rust] warnings = "deny"` — any rustc warning fails the build (unused import, dead code). No `#[allow(...)]` on production code.
- Rust toolchain pinned to `1.95.0` (`rust-toolchain.toml`).
- Domain layer has ZERO infrastructure dependencies. `parse_node_schema` is domain; `resolve_value_templates` is infrastructure — the merge helper therefore lives in **infrastructure**.
- All DAG nodes implement `ExecutableNode`; node outputs use the `{ "output": ... }` convention.
- Node `execute` signature (verbatim):
  ```rust
  async fn execute(&self, inputs: &NodeInputs, config: &Value, state: &mut Value,
      observer: Option<Arc<dyn ExecutionObserver>>) -> Result<Value, Box<dyn StdError + Send + Sync>>;
  ```
  where `NodeInputs = HashMap<String, Value>`. When a node runs as a tool, `DagToolExecutor` passes `config = json!({})` and folds everything into `inputs`.
- Recursion guard: reuse `MAX_SUBGRAPH_TOOL_DEPTH = 5` (defined in `nodes/subgraph.rs:52`).
- Tests that need env vars (`DATABASE_URL`, gsheets OAuth) MUST be `#[ignore = "requires X"]`.
- Test graphs use real registered node types only — never `log` as a tool backing.

---

## File Structure

**Create:**
- `src/libs/colmena/src/dag_engine/infrastructure/node_schema_merge.rs` — extracted pure merge helper (Task 1).
- `src/libs/colmena/src/dag_engine/application/list_tool_executor.rs` — `ExecPolicy`, `ItemResult`, `ListToolExecutor` (Tasks 2–3).
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs` — the `ForEachNode` (Tasks 5–7).
- `tests/graphs/agents/for_each_http_tool.json`, `tests/graphs/agents/for_each_subgraph_tool.json`, `tests/graphs/basic/for_each_node.json` — E2E graphs (Task 9).

**Modify:**
- `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` — make two fns `pub(crate)`; replace inline merge with helper call (Task 1).
- `src/libs/colmena/src/dag_engine/domain/observer.rs` — 2 new `NodeEvent` variants (Task 4).
- `src/libs/colmena/src/dag_engine/domain/events.rs` — 2 new `DagExecutionEvent` variants + `node_id()`/`advances_heartbeat_clock()` (Task 4).
- `src/libs/colmena/src/dag_engine/application/run_use_case.rs` — 2 conversion arms `NodeEvent → DagExecutionEvent` (Task 4).
- `src/libs/colmena/src/dag_engine/infrastructure/registry.rs` — register `for_each`, add `set_foreach_registry` (Task 8).
- `src/libs/colmena/src/dag_engine/infrastructure/engine.rs` — call `set_foreach_registry` next to `set_subgraph_executor` (Task 8).
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs` and `application/mod.rs` and `infrastructure/mod.rs` — `pub mod` declarations for new files.
- Docs (Task 9).

---

## Task 1: Extract the node_schema merge helper

Reuse the exact merge `execute_inner` performs (`fixed_values` seed → per-arg container placement → `${VAR}` resolution) so `for_each` produces identical inputs per row.

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/node_schema_merge.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs:193` (visibility) and `:1735-1802` (call the helper)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/mod.rs` (add `pub mod node_schema_merge;`)

**Interfaces:**
- Produces: `pub(crate) fn merge_args_into_schema(node_schema: &Value, args: HashMap<String, Value>) -> Result<HashMap<String, Value>, String>`

- [ ] **Step 1: Make the two template fns `pub(crate)`**

In `dag_tool_executor.rs`, change `fn resolve_value_templates` (line 193) and its sibling `fn resolve_template_string` to `pub(crate) fn`:
```rust
    pub(crate) fn resolve_value_templates(value: &Value, inputs: &HashMap<String, Value>) -> Value {
```
```rust
    pub(crate) fn resolve_template_string(s: &str, inputs: &HashMap<String, Value>) -> String {
```

- [ ] **Step 2: Write the failing test for the helper**

Create `node_schema_merge.rs` with only the test module first:
```rust
//! Pure merge of LLM/row args into a `node_schema`, extracted from
//! `DagToolExecutor::execute_inner` so `for_each` reuses identical semantics.

use crate::dag_engine::domain::tool_configuration::parse_node_schema;
use crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor;
use serde_json::Value;
use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn places_fixed_and_row_args() {
        let schema = json!({
            "base_url": { "fixed": "https://api.example.com" },
            "user_id":  { "required": true, "description": "id" }
        });
        let mut row = HashMap::new();
        row.insert("user_id".to_string(), json!(42));
        let out = merge_args_into_schema(&schema, row).unwrap();
        assert_eq!(out.get("base_url").unwrap(), &json!("https://api.example.com"));
        assert_eq!(out.get("user_id").unwrap(), &json!(42));
    }

    #[test]
    fn row_arg_cannot_override_fixed() {
        let schema = json!({ "secret": { "fixed": "keep" }, "x": { "required": true } });
        let mut row = HashMap::new();
        row.insert("secret".to_string(), json!("evil"));
        row.insert("x".to_string(), json!(1));
        let out = merge_args_into_schema(&schema, row).unwrap();
        assert_eq!(out.get("secret").unwrap(), &json!("keep"));
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test --lib node_schema_merge`
Expected: FAIL — `cannot find function merge_args_into_schema`.

- [ ] **Step 4: Implement the helper (move the block verbatim from execute_inner)**

Add above the test module in `node_schema_merge.rs`:
```rust
/// Merge caller-supplied args (LLM tool args, or a `for_each` row) into a
/// parsed `node_schema`: seed all `fixed` values, place each arg via
/// `param_to_container`, refuse to override fixed fields, then resolve
/// `${VAR}` templates. Identical to `execute_inner`'s PATH 0.
pub(crate) fn merge_args_into_schema(
    node_schema: &Value,
    args: HashMap<String, Value>,
) -> Result<HashMap<String, Value>, String> {
    let parsed = parse_node_schema(node_schema)
        .map_err(|e| format!("Invalid node_schema: {e}"))?;
    let mut result: HashMap<String, Value> = HashMap::new();

    for (k, v) in &parsed.fixed_values {
        result.insert(k.clone(), v.clone());
    }

    for (param_name, param_value) in &args {
        if let Some(container) = parsed.param_to_container.get(param_name) {
            let entry = result
                .entry(container.clone())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Value::Object(map) = entry {
                let real_key = if let Some(dot_pos) = param_name.find('.') {
                    &param_name[dot_pos + 1..]
                } else {
                    param_name.as_str()
                };
                if let (Some(Value::Object(existing)), Value::Object(incoming)) =
                    (map.get(real_key), param_value)
                {
                    let mut merged = existing.clone();
                    for (k, v) in incoming {
                        merged.insert(k.clone(), v.clone());
                    }
                    map.insert(real_key.to_string(), Value::Object(merged));
                } else {
                    map.insert(real_key.to_string(), param_value.clone());
                }
            }
        } else if parsed.fixed_values.contains_key(param_name) {
            // A supplied arg must NEVER override an operator-declared `fixed` field.
            eprintln!(
                "⚠️ [node_schema_merge] Ignoring arg '{param_name}' — collides with a fixed field."
            );
        } else {
            result.insert(param_name.clone(), param_value.clone());
        }
    }

    let resolved = result
        .iter()
        .map(|(k, v)| (k.clone(), DagToolExecutor::resolve_value_templates(v, &result)))
        .collect::<HashMap<String, Value>>();
    Ok(resolved)
}
```
Add `pub mod node_schema_merge;` to `infrastructure/mod.rs`.

- [ ] **Step 5: Run to verify the helper passes**

Run: `cargo test --lib node_schema_merge`
Expected: PASS (2 tests).

- [ ] **Step 6: Replace the inline block in `execute_inner`**

In `dag_tool_executor.rs`, the PATH 0 branch (starts `if let Some(schema) = tool_cfg.and_then(|c| c.node_schema.as_ref())` ~line 1735) — replace the whole inline body (down to the `resolved_result` return, ~line 1802) with:
```rust
        let inputs = if let Some(schema) = tool_cfg.and_then(|c| c.node_schema.as_ref()) {
            crate::dag_engine::infrastructure::node_schema_merge::merge_args_into_schema(schema, args.clone())
                .map_err(|e| LlmError::InvalidToolCall {
                    reason: format!("Invalid node_schema for tool {node_type}: {e}"),
                })?
        } else if let Some(fixed) = fixed_config.as_ref() {
```
(Leave the `$DYNAMIC` and legacy branches untouched.)

- [ ] **Step 7: Verify no behavior change + commit**

Run: `cargo test --lib dag_tool_executor && cargo test --lib tool_configuration`
Expected: PASS (all existing merge tests green).
```bash
git add src/libs/colmena/src/dag_engine/infrastructure/node_schema_merge.rs \
        src/libs/colmena/src/dag_engine/infrastructure/mod.rs \
        src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs
git commit -m "refactor: extract node_schema merge into reusable helper"
```

---

## Task 2: `ListToolExecutor` core — sequential (continue / abort)

**Files:**
- Create: `src/libs/colmena/src/dag_engine/application/list_tool_executor.rs`
- Modify: `src/libs/colmena/src/dag_engine/application/mod.rs` (add `pub mod list_tool_executor;`)

**Interfaces:**
- Produces:
  ```rust
  pub enum OnError { Continue, Abort }
  pub struct ExecPolicy { pub on_error: OnError, pub concurrency: usize, pub max_items: usize }
  pub enum ItemStatus { Ok, Err }
  pub struct ItemResult { pub index: usize, pub input: Value, pub status: ItemStatus, pub output: Option<Value>, pub error: Option<String> }
  pub async fn run_list<F, Fut>(rows: Vec<Value>, policy: &ExecPolicy, dispatch: F) -> Vec<ItemResult>
      where F: Fn(usize, Value) -> Fut, Fut: Future<Output = Result<Value, String>>;
  ```

- [ ] **Step 1: Write the failing tests (sequential behavior)**

Create `list_tool_executor.rs`:
```rust
//! Deterministic iteration engine for `for_each`: runs a dispatch closure over
//! N rows with a policy (error handling + concurrency) and stable ordering.

use serde_json::Value;
use std::future::Future;

pub const DEFAULT_MAX_ITEMS: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnError { Continue, Abort }

#[derive(Debug, Clone, Copy)]
pub struct ExecPolicy { pub on_error: OnError, pub concurrency: usize, pub max_items: usize }

impl Default for ExecPolicy {
    fn default() -> Self { Self { on_error: OnError::Continue, concurrency: 1, max_items: DEFAULT_MAX_ITEMS } }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus { Ok, Err }

#[derive(Debug, Clone)]
pub struct ItemResult {
    pub index: usize,
    pub input: Value,
    pub status: ItemStatus,
    pub output: Option<Value>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn continue_collects_ok_and_err_in_order() {
        let rows = vec![json!({"n":1}), json!({"n":2}), json!({"n":3})];
        let policy = ExecPolicy { on_error: OnError::Continue, concurrency: 1, max_items: DEFAULT_MAX_ITEMS };
        let out = run_list(rows, &policy, |_i, row| async move {
            let n = row["n"].as_i64().unwrap();
            if n == 2 { Err("boom".into()) } else { Ok(json!({"double": n * 2})) }
        }).await;
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].index, 0);
        assert_eq!(out[0].status, ItemStatus::Ok);
        assert_eq!(out[1].status, ItemStatus::Err);
        assert_eq!(out[1].error.as_deref(), Some("boom"));
        assert_eq!(out[2].output.as_ref().unwrap(), &json!({"double": 6}));
    }

    #[tokio::test]
    async fn abort_stops_after_first_error() {
        let rows = vec![json!({"n":1}), json!({"n":2}), json!({"n":3})];
        let policy = ExecPolicy { on_error: OnError::Abort, concurrency: 1, max_items: DEFAULT_MAX_ITEMS };
        let out = run_list(rows, &policy, |_i, row| async move {
            let n = row["n"].as_i64().unwrap();
            if n == 2 { Err("stop".into()) } else { Ok(json!(n)) }
        }).await;
        assert_eq!(out.len(), 2); // item 3 never ran
        assert_eq!(out[1].status, ItemStatus::Err);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib list_tool_executor`
Expected: FAIL — `cannot find function run_list`.

- [ ] **Step 3: Implement `run_list` (sequential path only for now)**

Add above the test module:
```rust
/// Run `dispatch` over each row sequentially (concurrency handled in a later
/// task). `Continue` collects every row's result; `Abort` stops after the
/// first error. Results are index-ordered.
pub async fn run_list<F, Fut>(rows: Vec<Value>, policy: &ExecPolicy, dispatch: F) -> Vec<ItemResult>
where
    F: Fn(usize, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let mut results = Vec::with_capacity(rows.len());
    for (index, row) in rows.into_iter().enumerate() {
        let res = dispatch(index, row.clone()).await;
        let item = match res {
            Ok(output) => ItemResult { index, input: row, status: ItemStatus::Ok, output: Some(output), error: None },
            Err(error) => ItemResult { index, input: row, status: ItemStatus::Err, output: None, error: Some(error) },
        };
        let is_err = item.status == ItemStatus::Err;
        results.push(item);
        if is_err && policy.on_error == OnError::Abort { break; }
    }
    results
}
```
Add `pub mod list_tool_executor;` to `application/mod.rs`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test --lib list_tool_executor`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**
```bash
git add src/libs/colmena/src/dag_engine/application/list_tool_executor.rs \
        src/libs/colmena/src/dag_engine/application/mod.rs
git commit -m "feat: add ListToolExecutor sequential iteration engine"
```

---

## Task 3: `ListToolExecutor` — bounded concurrency

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/list_tool_executor.rs`

**Interfaces:**
- Consumes: `run_list` from Task 2 (signature unchanged).
- Produces: same `run_list`, now honoring `policy.concurrency > 1` via `futures::stream::buffer_unordered`, results re-sorted by `index`.

- [ ] **Step 1: Write the failing test (parallel preserves order, runs concurrently)**

Add to the test module:
```rust
    #[tokio::test]
    async fn parallel_preserves_index_order() {
        let rows: Vec<Value> = (0..10).map(|i| json!({"n": i})).collect();
        let policy = ExecPolicy { on_error: OnError::Continue, concurrency: 4, max_items: DEFAULT_MAX_ITEMS };
        let out = run_list(rows, &policy, |_i, row| async move {
            let n = row["n"].as_i64().unwrap();
            // Later items sleep longer; without re-sort they'd finish out of order.
            tokio::time::sleep(std::time::Duration::from_millis((10 - n) as u64)).await;
            Ok(json!(n))
        }).await;
        for (i, item) in out.iter().enumerate() {
            assert_eq!(item.index, i);
            assert_eq!(item.output.as_ref().unwrap(), &json!(i as i64));
        }
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib list_tool_executor::tests::parallel_preserves_index_order`
Expected: FAIL — items out of order (sequential impl sleeps 10..1, still ordered → actually passes ordering but is slow). To make it a real failure first, temporarily assert timing. Simpler: proceed — this test guards the concurrent impl; run after Step 3.

- [ ] **Step 3: Add the concurrent branch**

Replace `run_list` body with a dispatch that branches on concurrency:
```rust
pub async fn run_list<F, Fut>(rows: Vec<Value>, policy: &ExecPolicy, dispatch: F) -> Vec<ItemResult>
where
    F: Fn(usize, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    if policy.concurrency <= 1 {
        return run_sequential(rows, policy, dispatch).await;
    }
    use futures::stream::{self, StreamExt};
    let mut results: Vec<ItemResult> = stream::iter(rows.into_iter().enumerate())
        .map(|(index, row)| {
            let fut = dispatch(index, row.clone());
            async move {
                match fut.await {
                    Ok(output) => ItemResult { index, input: row, status: ItemStatus::Ok, output: Some(output), error: None },
                    Err(error) => ItemResult { index, input: row, status: ItemStatus::Err, output: None, error: Some(error) },
                }
            }
        })
        .buffer_unordered(policy.concurrency)
        .collect()
        .await;
    results.sort_by_key(|r| r.index);
    results
}

async fn run_sequential<F, Fut>(rows: Vec<Value>, policy: &ExecPolicy, dispatch: F) -> Vec<ItemResult>
where
    F: Fn(usize, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let mut results = Vec::with_capacity(rows.len());
    for (index, row) in rows.into_iter().enumerate() {
        let item = match dispatch(index, row.clone()).await {
            Ok(output) => ItemResult { index, input: row, status: ItemStatus::Ok, output: Some(output), error: None },
            Err(error) => ItemResult { index, input: row, status: ItemStatus::Err, output: None, error: Some(error) },
        };
        let is_err = item.status == ItemStatus::Err;
        results.push(item);
        if is_err && policy.on_error == OnError::Abort { break; }
    }
    results
}
```
> Note: `Abort` under concurrency is best-effort — in-flight items complete; no NEW items start after an error is observed. v1 documents this. (Strict cancellation = backlog.)

- [ ] **Step 4: Run to verify all pass**

Run: `cargo test --lib list_tool_executor`
Expected: PASS (3 tests). Confirm `futures` is a dependency: `grep '^futures' src/libs/colmena/Cargo.toml` (it is — used elsewhere).

- [ ] **Step 5: Commit**
```bash
git add src/libs/colmena/src/dag_engine/application/list_tool_executor.rs
git commit -m "feat: add bounded-concurrency path to ListToolExecutor"
```

---

## Task 4: Progress events (`batch-progress`, `batch-item-finished`)

Follows the exact precedent of `SkillLoaded`/`ToolDescribed`, which exist in BOTH `NodeEvent` (observer.rs) and `DagExecutionEvent` (events.rs) with a conversion arm in run_use_case.rs.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/observer.rs` (2 `NodeEvent` variants)
- Modify: `src/libs/colmena/src/dag_engine/domain/events.rs` (2 `DagExecutionEvent` variants + 2 impl methods)
- Modify: `src/libs/colmena/src/dag_engine/application/run_use_case.rs` (2 conversion arms)

**Interfaces:**
- Produces: `NodeEvent::BatchProgress { node_id, total, completed, ok, err, in_flight }` and `NodeEvent::BatchItemFinished { node_id, index, key, status }`; identical `DagExecutionEvent` variants serialized as `"batch_progress"` / `"batch_item_finished"`.

- [ ] **Step 1: Add `NodeEvent` variants**

In `observer.rs`, inside `enum NodeEvent`, after `ToolDescribed { ... }`:
```rust
    /// Coarse batch progress emitted by `for_each` at start, per item, and end.
    BatchProgress {
        node_id: String,
        total: usize,
        completed: usize,
        ok: usize,
        err: usize,
        in_flight: usize,
    },
    /// Emitted by `for_each` the moment a single item finishes.
    BatchItemFinished {
        node_id: String,
        index: usize,
        key: String,
        status: String, // "ok" | "err"
    },
```

- [ ] **Step 2: Add `DagExecutionEvent` variants**

In `events.rs`, inside `enum DagExecutionEvent`, after the `ToolDescribed` variant:
```rust
    #[serde(rename = "batch_progress")]
    BatchProgress {
        node_id: String,
        total: usize,
        completed: usize,
        ok: usize,
        err: usize,
        in_flight: usize,
    },
    #[serde(rename = "batch_item_finished")]
    BatchItemFinished {
        node_id: String,
        index: usize,
        key: String,
        status: String,
    },
```

- [ ] **Step 3: Extend the two impl methods**

In `events.rs`, in `fn node_id(&self) -> Option<&str>`, add arms so both new variants return their `node_id`:
```rust
            DagExecutionEvent::BatchProgress { node_id, .. } => Some(node_id),
            DagExecutionEvent::BatchItemFinished { node_id, .. } => Some(node_id),
```
In `fn advances_heartbeat_clock(&self) -> bool`, add both as `true` (they are real activity):
```rust
            DagExecutionEvent::BatchProgress { .. } => true,
            DagExecutionEvent::BatchItemFinished { .. } => true,
```
> If `node_id()` / `advances_heartbeat_clock()` use a catch-all `_ =>`, these explicit arms may be unnecessary — check for exhaustiveness; add only if the match is exhaustive (no `_`).

- [ ] **Step 4: Add the conversion arms**

In `run_use_case.rs`, find the `match` that converts `NodeEvent` → `DagExecutionEvent` (grep: `NodeEvent::ToolDescribed` — the SkillLoaded/ToolDescribed conversion). Add alongside:
```rust
                    NodeEvent::BatchProgress { node_id, total, completed, ok, err, in_flight } => {
                        DagExecutionEvent::BatchProgress { node_id, total, completed, ok, err, in_flight }
                    }
                    NodeEvent::BatchItemFinished { node_id, index, key, status } => {
                        DagExecutionEvent::BatchItemFinished { node_id, index, key, status }
                    }
```

- [ ] **Step 5: Write a serialization test**

Add to `events.rs` test module (or create one):
```rust
    #[test]
    fn batch_progress_serializes_with_event_tag() {
        let ev = DagExecutionEvent::BatchProgress {
            node_id: "fe1".into(), total: 10, completed: 3, ok: 2, err: 1, in_flight: 2,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["event"], "batch_progress");
        assert_eq!(v["data"]["completed"], 3);
    }
```

- [ ] **Step 6: Build + test + commit**

Run: `cargo test --lib events && cargo build`
Expected: PASS + clean build (deny-warnings). Fix any non-exhaustive `match NodeEvent`/`match DagExecutionEvent` the compiler flags.
```bash
git add src/libs/colmena/src/dag_engine/domain/observer.rs \
        src/libs/colmena/src/dag_engine/domain/events.rs \
        src/libs/colmena/src/dag_engine/application/run_use_case.rs
git commit -m "feat: add batch_progress and batch_item_finished SSE events"
```

---

## Task 5: `ForEachNode` — struct, registry injection, node-target dispatch, inline/edge lists

Delivers a working `for_each` for `items` inline + input-edge, dispatching to a node target. `items_from` (Task 6) and guards (Task 7) come next.

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs` (add `pub mod for_each;`)

**Interfaces:**
- Consumes: `merge_args_into_schema` (Task 1); `run_list`, `ExecPolicy`, `OnError`, `ItemResult`, `ItemStatus` (Tasks 2–3); `NodeEvent::{BatchProgress, BatchItemFinished}` (Task 4); `NodeRegistryPort::get_node` (`application/ports.rs:14`).
- Produces:
  ```rust
  pub struct ForEachNode { pub registry: Arc<OnceLock<Arc<dyn NodeRegistryPort>>> }
  impl ForEachNode { pub fn new() -> Self; }
  ```
  Output value shape: `{ "output": { "total", "ok", "err", "results": [ {index,input,status,output|error} ] } }`.

- [ ] **Step 1: Write the failing test (iterate a real `add` target over inline items)**

Create `for_each.rs`:
```rust
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
    fn default() -> Self { Self::new() }
}

impl ForEachNode {
    pub fn new() -> Self { Self { registry: Arc::new(OnceLock::new()) } }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::infrastructure::nodes::math::MathNode; // real `add` node
    use crate::dag_engine::infrastructure::registry::HashMapNodeRegistry;
    // Build a tiny registry stub exposing just what get_node needs.

    // NOTE: use the real registry via a lightweight constructor in Task 8's test,
    // or a hand-rolled stub implementing NodeRegistryPort here.
    struct StubRegistry { add: Arc<dyn ExecutableNode> }
    impl NodeRegistryPort for StubRegistry {
        fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
            if node_type == "add" { Some(self.add.clone()) } else { None }
        }
        fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> { HashMap::new() }
    }

    #[tokio::test]
    async fn runs_add_target_over_inline_items() {
        let node = ForEachNode::new();
        node.registry.set(Arc::new(StubRegistry { add: Arc::new(MathNode::new("add")) }) as Arc<dyn NodeRegistryPort>).ok();

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert("target".to_string(), json!({
            "node_type": "add",
            "node_schema": { "a": { "required": true }, "b": { "required": true } }
        }));
        inputs.insert("items".to_string(), json!([{"a":1,"b":2},{"a":10,"b":20}]));

        let mut state = json!({});
        let out = node.execute(&inputs, &json!({}), &mut state, None).await.unwrap();
        let results = out["output"]["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(out["output"]["ok"], 2);
        assert_eq!(results[0]["status"], "ok");
    }
}
```
> Confirm `MathNode::new("add")` matches the real constructor — check `nodes/math.rs`. If the math node uses a different constructor/output key, adjust the assertion to its actual output shape.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib for_each`
Expected: FAIL — `execute` not implemented for `ForEachNode`.

- [ ] **Step 3: Implement `execute` + `ExecutableNode`**

Add to `for_each.rs`:
```rust
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

        let target = inputs.get("target").cloned()
            .ok_or("for_each: missing `target` (embedded tool config)")?;
        let target_type = target.get("node_type").and_then(|v| v.as_str())
            .ok_or("for_each: `target.node_type` is required")?.to_string();
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

        let registry = self.registry.get()
            .ok_or("for_each: NodeRegistryPort not initialized")?
            .clone();

        if let Some(obs) = &observer {
            obs.on_event(NodeEvent::BatchProgress { node_id: node_id.clone(), total, completed: 0, ok: 0, err: 0, in_flight: 0 });
        }

        // Per-row dispatch: merge the row into the target schema, run the target node.
        let dispatch = |index: usize, row: Value| {
            let registry = registry.clone();
            let target_type = target_type.clone();
            let target_schema = target_schema.clone();
            let observer = observer.clone();
            let node_id = node_id.clone();
            async move {
                let row_map: HashMap<String, Value> = match &row {
                    Value::Object(m) => m.clone().into_iter().collect(),
                    other => { let mut h = HashMap::new(); h.insert("value".to_string(), other.clone()); h }
                };
                let merged = merge_args_into_schema(&target_schema, row_map)
                    .map_err(|e| format!("row {index}: {e}"))?;
                let node = registry.get_node(&target_type)
                    .ok_or_else(|| format!("row {index}: unknown target node_type '{target_type}'"))?;
                let mut item_state = json!({});
                let result = node.execute(&merged, &json!({}), &mut item_state, observer.clone()).await
                    .map_err(|e| format!("row {index}: {e}"))?;
                // HITL fail-closed: a SUSPENDED result inside a fan-out is an error.
                if result.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED")
                    || result.get("status").and_then(|v| v.as_str()) == Some("SUSPENDED") {
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
            match r.status { ItemStatus::Ok => ok += 1, ItemStatus::Err => err += 1 }
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
            if let Some(o) = &r.output { m.insert("output".into(), o.clone()); }
            if let Some(e) = &r.error { m.insert("error".into(), json!(e)); }
            out_rows.push(Value::Object(m));
        }
        if let Some(obs) = &observer {
            obs.on_event(NodeEvent::BatchProgress { node_id: node_id.clone(), total, completed: results.len(), ok, err, in_flight: 0 });
        }

        Ok(json!({ "output": { "total": total, "ok": ok, "err": err, "results": out_rows } }))
    }

    fn default_output(&self) -> Option<&str> { Some("output") }

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
        Some("Run an embedded target tool once per row of a list, deterministically. \
              Provide the list via `items` (array) or `items_from` (a data-source handle). \
              Prefer `items_from` for lists that come from data — the model never re-types them.")
    }
}
```
Add `pub mod for_each;` to `nodes/mod.rs`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test --lib for_each`
Expected: PASS. Fix `MathNode` constructor/output-key mismatches surfaced by the test.

- [ ] **Step 5: Commit**
```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs
git commit -m "feat: add for_each node with inline/edge lists and node-target dispatch"
```

---

## Task 6: `items_from` — attachment (CSV/XLSX), sheet, single-column selection

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs` (list resolution + storage injection)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs` (pass storage when constructing `ForEachNode` — done in Task 8)

**Interfaces:**
- Consumes: `parse_attachment_to_records(bytes, mime_type, filename, delimiter, sheet_name, header_row) -> Result<(Vec<String>, Vec<Map<String,Value>>), String>` (`nodes/llm_synthetic_tools/sql_bulk_tools.rs:562`); `dispatch_gsheets_read(args: Value) -> Value` (re-export of `dispatch_read`, `nodes/llm_synthetic_tools/gsheets_tools.rs`, json mode returns `{ "ok", "values" }`).
- Produces: extended `ForEachNode` with `storage: Option<Arc<dyn OutputStorageRepository>>` and async `resolve_rows` supporting `items_from`.

- [ ] **Step 1: Add storage field + builder**

In `for_each.rs`, extend the struct:
```rust
use crate::storage::domain::OutputStorageRepository;

pub struct ForEachNode {
    pub registry: Arc<OnceLock<Arc<dyn NodeRegistryPort>>>,
    pub storage: Option<Arc<dyn OutputStorageRepository>>,
}
impl ForEachNode {
    pub fn new() -> Self { Self { registry: Arc::new(OnceLock::new()), storage: None } }
    pub fn with_storage(mut self, storage: Arc<dyn OutputStorageRepository>) -> Self { self.storage = Some(storage); self }
}
```

- [ ] **Step 2: Write the failing test (sheet source, mocked) + column selection unit test**

Add a pure unit test for column selection (no I/O):
```rust
    #[test]
    fn column_selection_maps_scalar_rows() {
        let rows = vec![json!({"user_id": 1, "name": "a"}), json!({"user_id": 2, "name": "b"})];
        let picked = super::apply_column_selection(rows, Some("user_id"), Some("uid"));
        assert_eq!(picked[0], json!({"uid": 1}));
        assert_eq!(picked[1], json!({"uid": 2}));
    }
```

- [ ] **Step 3: Implement column selection + `items_from` resolution**

Add helpers and make `resolve_rows` async:
```rust
pub(crate) fn apply_column_selection(rows: Vec<Value>, column: Option<&str>, as_name: Option<&str>) -> Vec<Value> {
    let Some(col) = column else { return rows };
    let key = as_name.unwrap_or(col);
    rows.into_iter().map(|row| {
        let val = row.get(col).cloned().unwrap_or(Value::Null);
        json!({ key: val })
    }).collect()
}

async fn resolve_rows_async(
    inputs: &NodeInputs,
    storage: &Option<Arc<dyn OutputStorageRepository>>,
) -> Result<Vec<Value>, String> {
    if let Some(Value::Array(arr)) = inputs.get("items") { return Ok(arr.clone()); }

    if let Some(handle) = inputs.get("items_from") {
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
                if let Some(r) = range { args["range"] = json!(r); }
                let res = dispatch_gsheets_read(args).await;
                if res.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                    return Err(format!("for_each items_from sheet failed: {res}"));
                }
                res.get("values").and_then(|v| v.as_array()).cloned()
                    .ok_or("for_each items_from sheet: no `values` in response")?
            }
            "attachment" => {
                use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::sql_bulk_tools::parse_attachment_to_records;
                let storage = storage.as_ref().ok_or("for_each items_from attachment: no storage adapter configured")?;
                let doc_id = handle.get("ref").and_then(|v| v.as_str())
                    .ok_or("for_each items_from attachment: missing `ref` document_id")?;
                let bytes = storage.read(doc_id).await.map_err(|e| format!("read attachment: {e}"))?;
                let filename = handle.get("filename").and_then(|v| v.as_str()).unwrap_or(doc_id);
                let mime = handle.get("mime_type").and_then(|v| v.as_str()).unwrap_or("text/csv");
                let (_cols, records) = parse_attachment_to_records(&bytes, mime, filename, None, None, None)?;
                records.into_iter().map(Value::Object).collect()
            }
            other => return Err(format!("for_each items_from: unknown source '{other}' (v1: sheet | attachment)")),
        };
        return Ok(apply_column_selection(rows, column, as_name));
    }

    for edge_key in ["input", "default"] {
        if let Some(Value::Array(arr)) = inputs.get(edge_key) { return Ok(arr.clone()); }
    }
    Err("for_each: no list found — provide `items`, `items_from`, or an input edge carrying an array".into())
}
```
> Confirm the exact `OutputStorageRepository` read method name/signature (`storage.read(doc_id)` → bytes). Check `src/libs/colmena/src/storage/domain/`; adjust the call to the real method (e.g. `read`, `get`, `read_bytes`) and its `Result` type.

- [ ] **Step 4: Wire `execute` to use `resolve_rows_async`**

Replace the `resolve_rows(inputs)` call in `execute` with:
```rust
        let mut rows = resolve_rows_async(inputs, &self.storage).await
            .map_err(|e| -> Box<dyn StdError + Send + Sync> { e.into() })?;
```
Remove the now-unused sync `resolve_rows` (deny-warnings).

- [ ] **Step 5: Run tests + commit**

Run: `cargo test --lib for_each`
Expected: PASS (column-selection unit test + Task 5 tests). Sheet/attachment paths are exercised in E2E (Task 9), `#[ignore]`-gated for live creds.
```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs
git commit -m "feat: for_each items_from attachment/sheet + single-column selection"
```

---

## Task 7: Row validation, recursion guard, empty-list, HITL — hardening

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`

- [ ] **Step 1: Write failing tests**
```rust
    #[tokio::test]
    async fn missing_required_param_becomes_err_row() {
        let node = ForEachNode::new();
        node.registry.set(Arc::new(StubRegistry { add: Arc::new(MathNode::new("add")) }) as Arc<dyn NodeRegistryPort>).ok();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert("target".to_string(), json!({ "node_type": "add",
            "node_schema": { "a": { "required": true }, "b": { "required": true } } }));
        inputs.insert("items".to_string(), json!([{"a":1}])); // missing b
        inputs.insert("on_error".to_string(), json!("continue"));
        let mut state = json!({});
        let out = node.execute(&inputs, &json!({}), &mut state, None).await.unwrap();
        assert_eq!(out["output"]["err"], 1);
        assert_eq!(out["output"]["results"][0]["status"], "err");
    }

    #[tokio::test]
    async fn empty_list_is_not_an_error() {
        let node = ForEachNode::new();
        node.registry.set(Arc::new(StubRegistry { add: Arc::new(MathNode::new("add")) }) as Arc<dyn NodeRegistryPort>).ok();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert("target".to_string(), json!({ "node_type": "add", "node_schema": {} }));
        inputs.insert("items".to_string(), json!([]));
        let mut state = json!({});
        let out = node.execute(&inputs, &json!({}), &mut state, None).await.unwrap();
        assert_eq!(out["output"]["total"], 0);
        assert!(out["output"]["results"].as_array().unwrap().is_empty());
    }
```

- [ ] **Step 2: Add required-param validation to the dispatch closure**

In the per-row `dispatch` (Task 5), after `merge_args_into_schema`, validate required params from the parsed schema before running the target:
```rust
                use crate::dag_engine::domain::tool_configuration::parse_node_schema;
                if let Ok(parsed) = parse_node_schema(&target_schema) {
                    for req in &parsed.required_params {
                        if !merged.contains_key(req) {
                            return Err(format!("row {index}: missing required param '{req}'"));
                        }
                    }
                }
```
> Confirm the field name on `ParsedNodeSchema` for required params (`required_params` per the tool_configuration.rs types). Adjust if different.

- [ ] **Step 3: Run + commit**

Run: `cargo test --lib for_each`
Expected: PASS (empty-list + required-param + earlier tests). The self-target guard and HITL fail-closed from Task 5 are already covered by code; add a quick assertion test if desired.
```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs
git commit -m "feat: for_each row validation, empty-list, recursion/HITL guards"
```

---

## Task 8: Register `for_each` + inject the registry handle

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/engine.rs:299`

**Interfaces:**
- Consumes: `ForEachNode::new()` / `with_storage` (Tasks 5–6).
- Produces: `HashMapNodeRegistry::set_foreach_registry(&self, registry: Arc<dyn NodeRegistryPort>)`.

- [ ] **Step 1: Register the node + keep a handle**

In `registry.rs`, add a field to `HashMapNodeRegistry`:
```rust
    foreach_node: Option<Arc<crate::dag_engine::infrastructure::nodes::for_each::ForEachNode>>,
```
In `new_with_secure_values`, near the SubGraph registration, construct + register `for_each` (with storage when available):
```rust
            // --- Registrar ForEach ---
            let mut fe = crate::dag_engine::infrastructure::nodes::for_each::ForEachNode::new();
            if let Some(storage_arc) = storage.clone() {
                fe = fe.with_storage(storage_arc);
            }
            let fe_node = Arc::new(fe);
            nodes.insert("for_each".to_string(), fe_node.clone() as Arc<dyn ExecutableNode>);
```
Add `foreach_node: Some(fe_node),` to the `Self { ... }` literal (and `foreach_node: None` anywhere a registry is built without it, if such a path exists).

- [ ] **Step 2: Add the injection setter (mirror `set_subgraph_executor`)**

In `registry.rs`, next to `set_subgraph_executor`:
```rust
    pub fn set_foreach_registry(&self, registry: Arc<dyn NodeRegistryPort>) {
        if let Some(fe) = &self.foreach_node {
            let _ = fe.registry.set(registry);
        }
    }
```

- [ ] **Step 3: Call the setter after the registry exists**

In `engine.rs`, right after `node_registry.set_subgraph_executor(use_case.clone());` (line 299):
```rust
        node_registry.set_foreach_registry(node_registry.clone());
```
> `node_registry` is `Arc<HashMapNodeRegistry>` which implements `NodeRegistryPort`. Passing `node_registry.clone()` creates a self-referential `Arc` inside the node's `OnceLock`. This is acceptable for process lifetime (the registry lives as long as the engine). If a leak-free variant is required, change the field to `Weak<dyn NodeRegistryPort>` and pass `Arc::downgrade(&node_registry)`, upgrading at dispatch time — note this in the code comment.

- [ ] **Step 4: Integration test — `for_each` via the real registry**

Add a test in `for_each.rs` (or `tests/`) building the real registry, or verify via the E2E graph in Task 9. Minimal check:

Run: `cargo build && cargo test --lib for_each`
Expected: clean build (deny-warnings) + PASS.

- [ ] **Step 5: Commit**
```bash
git add src/libs/colmena/src/dag_engine/infrastructure/registry.rs \
        src/libs/colmena/src/dag_engine/infrastructure/engine.rs
git commit -m "feat: register for_each node and inject its registry handle"
```

---

## Task 9: E2E graphs + documentation

**Files:**
- Create: `tests/graphs/basic/for_each_node.json` (graph-node usage over a real node target)
- Create: `tests/graphs/agents/for_each_http_tool.json` (LLM-tool usage, http_request target)
- Create: `tests/graphs/agents/for_each_subgraph_tool.json` (LLM-tool usage, subgraph target)
- Create: `docs/developer_guide/49_for_each.md`
- Modify: `docs/node_configurations.json`, `docs/node_as_tools_reference.json`, `docs/agent_context/node_ports_reference.md`, `docs/developer_guide/41_builtin_tools_index.md`, `docs/DEVELOPER_GUIDE.md`, `docs/CHANGELOG_*.md`, root `CLAUDE.md` (Current Status).

- [ ] **Step 1: Graph-node E2E (no LLM)**

Create `tests/graphs/basic/for_each_node.json` — an `input` node feeding a list into `for_each` with an `add` target, output logged. Use only registered node types. Example `for_each` node config:
```json
{
  "id": "fe1", "node_type": "for_each",
  "config": {
    "target": { "node_type": "add", "node_schema": { "a": {"required": true}, "b": {"required": true} } },
    "items": [ {"a": 1, "b": 2}, {"a": 10, "b": 20} ],
    "concurrency": 2, "on_error": "continue"
  }
}
```
Run: `cargo run --bin dag_engine -- run tests/graphs/basic/for_each_node.json`
Expected: output with `results` length 2, both `ok`.

- [ ] **Step 2: LLM-tool E2E (http target)**

Create `tests/graphs/agents/for_each_http_tool.json` — an `llm_call` with a `tool_configurations` entry `batch_update_users` (`node_type: "for_each"`, `target` = an `http_request` to a real echo endpoint, `items`/`items_from` visible). Use the default LLM stack (google/gemini-2.5-flash) and a real prompt (realistic, not spoon-fed): e.g. "Update the plan for these users to pro: 101, 102, 103."
Run (source `.env` first):
```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/for_each_http_tool.json --agent-session-id agent_foreach_001
```
Save SSE to `/tmp/colmena_e2e/for_each_http.sse`; verify `batch_progress` + `batch_item_finished` frames and 3 result rows.

- [ ] **Step 3: LLM-tool E2E (subgraph target)**

Create `tests/graphs/agents/for_each_subgraph_tool.json` — same shape but `target.node_type: "subgraph"` with a `child_graph_inline` `llm_call`. Verify each row runs the sub-agent in isolation (Mode B).
Run + save SSE as above.

- [ ] **Step 4: Write `docs/developer_guide/49_for_each.md`**

Cover: what `for_each` is, the embedded `target` contract, `items` vs `items_from` (attachment/sheet/column), `on_error`/`concurrency`/`max_items`, the result table shape, the two progress events, HITL fail-closed, `MAX_SUBGRAPH_TOOL_DEPTH`, and both usage examples (graph node + tool). Language: Spanish (docs convention).

- [ ] **Step 5: Update the reference docs**

Add `for_each` to `node_configurations.json` (config fields), `node_as_tools_reference.json` (tool exposure with `target` fixed + `items`/`items_from` visible), `node_ports_reference.md` (default input `items`/edge, output `output`), and index it in `41_builtin_tools_index.md` + `DEVELOPER_GUIDE.md`. Add a CHANGELOG entry and a CLAUDE.md "Current Status" bullet.

- [ ] **Step 6: Full test sweep + commit**

Run: `cargo test --verbose && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all green (CI parity).
```bash
git add tests/graphs/ docs/ CLAUDE.md
git commit -m "docs: for_each guide, references, and E2E graphs"
```

---

## Self-Review

**Spec coverage:**
- §2 one node two ways → Tasks 5 (node) + 9 (tool exposure via graph). ✓
- §3 embedded target + merge reuse → Tasks 1 (helper) + 5 (dispatch). ✓
- §4 tool exposure (`target`/policy fixed, `items` visible) → Task 9 graphs; mechanism verified (Claim 2/3). ✓
- §5 list from items/items_from/edge + column selection → Tasks 5 + 6. ✓
- §6 result table + incremental in-memory → Task 5 output + Task 4 per-item events. ✓
- §7 Mode B, policy, HITL fail-closed, caps, 3 SSE granularities → Tasks 3, 4, 5, 7 (granularity 3 `batch-item[k]` is the child stream that flows through the existing subgraph observer path when target is a subgraph — no new code). ✓
- §8 node + engine + shared merge + injection + additive event enums → Tasks 1, 4, 5, 8. ✓
- §9 edge cases → Task 7 (validation, recursion, empty) + Task 5 (HITL, self-target). ✓
- §10 `node_type = for_each`, deferrals not built → respected (no target_tool/results_to/tool_result/checkpoint tasks). ✓
- §11 acceptance (both targets, both forms) → Task 9. ✓

**Placeholder scan:** No "TBD"/"add error handling"-style gaps; every code step shows real code. Three "confirm signature" notes (MathNode ctor, `OutputStorageRepository::read`, `ParsedNodeSchema.required_params`) are verification-at-implementation of names in existing code, not deferred logic — resolve by reading the cited file in that step.

**Type consistency:** `run_list`, `ExecPolicy{on_error,concurrency,max_items}`, `ItemResult{index,input,status,output,error}`, `ItemStatus`, `OnError`, `merge_args_into_schema`, `NodeEvent::{BatchProgress,BatchItemFinished}`, `DagExecutionEvent::{BatchProgress,BatchItemFinished}`, `ForEachNode{registry,storage}` are used consistently across tasks.

## Deferred (NOT in this plan — v1.1 / backlog per spec §10)
`target_tool` by-name; `results_to` sink (new Sheet); `items_from: tool_result`; durable checkpoint store; Mode A (shared memory).
