# Tool-result structured digest (v1.1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a `tool` result is large AND structured (JSON), render the conversation-summary line as a deterministic structured digest (schema + row count + sample + key aggregates) instead of lossy NL prose, so the model keeps the SHAPE of the data (which columns/fields exist) as the message ages out of the recent window.

**Architecture:** A new pure function `digest_tool_result(content) -> Option<String>` in `application/tool_digest.rs` detects JSON arrays-of-objects, scalar arrays, and objects and emits a one-line shape summary. `build_compacted_messages` (in `history_compaction.rs`) calls it for `tool`-role messages in the old zone **before** the NL-summarizer path. Because the digest is deterministic and cheap, it is computed fresh each load — **never cached, never sent to an LLM**. Non-structured tool results (NL text) fall through unchanged to the existing semantic-summary path. Recall stays lossless (v1), so the digest being partial is safe.

**Tech Stack:** Rust, `serde_json` (already a dependency), `tokio`/`async-trait` (existing test infra). No DB migration, no repo-trait change, no public-API change → ADP unaffected.

---

## Why this is safe and small

- **No persistence change.** v1 added the `summary TEXT` column for *expensive* LLM summaries. Digests are deterministic and cheap, so we do NOT cache them — no migration, no `set_summary` call, no repo change.
- **No public API change.** The only change is the body of the per-message line built inside `build_compacted_messages`, plus one new pure module. ADP's worker is unaffected (internal wire-format only).
- **Recall already lossless (v1).** The digest is intentionally partial; the line cites `recall_history(turn=N)` so the model can pull the full structured result verbatim when it needs every row.
- **Strict fallback.** `digest_tool_result` returns `None` for anything that is not a JSON object / array-of-objects / scalar-array (e.g. a paragraph from web search). `None` → the existing NL semantic-summary path runs unchanged.

## File Structure

- **Create:** `src/libs/colmena/src/llm/application/tool_digest.rs` — the entire digest logic (pure, no async, no I/O). One responsibility: turn a structured-JSON tool result string into a one-line shape digest.
- **Modify:** `src/libs/colmena/src/llm/application/mod.rs` — register `pub mod tool_digest;`.
- **Modify:** `src/libs/colmena/src/llm/application/history_compaction.rs` — add one branch in the `ValueClass::Content` ladder of `build_compacted_messages` that prefers the digest for `tool`-role messages, plus a wiring test.
- **Modify (docs):** `docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md`, `docs/CHANGELOG_2026-06.md`, `docs/developer_guide/15_memory_guide.md`, `CLAUDE.md`.
- **Create (E2E):** `tests/graphs/agents/tool_digest_e2e.json`.

---

### Task 1: Pure digest core (`tool_digest.rs`)

Detect structured JSON and emit a shape digest. Handles: array-of-objects (tabular), array-of-scalars, single object (with field inventory, inline scalar values, nested markers, and drill-down into the dominant array-of-objects field). Bounded output. No aggregates yet (Task 2).

**Files:**
- Create: `src/libs/colmena/src/llm/application/tool_digest.rs`
- Modify: `src/libs/colmena/src/llm/application/mod.rs`

- [ ] **Step 1: Register the module**

In `src/libs/colmena/src/llm/application/mod.rs`, add the module line in alphabetical position (after `history_compaction`):

```rust
pub mod agent_service;
pub mod attachment_catalog;
pub mod history_compaction;
pub mod llm_call_use_case;
pub mod llm_health_check_use_case;
pub mod llm_stream_use_case;
pub mod tool_digest;
```

(Leave the existing `pub use ...;` lines unchanged — `tool_digest` is referenced by full path, like `history_compaction`.)

- [ ] **Step 2: Write the failing tests**

Create `src/libs/colmena/src/llm/application/tool_digest.rs` with ONLY the test module first (the function does not exist yet, so it fails to compile = a failing test):

```rust
//! Deterministic structured digests of tool results (v1.1).
//!
//! Pure, no LLM, no I/O. Used by `history_compaction` for `tool`-role
//! messages whose rendered content is large AND structured (JSON), so the
//! conversation summary preserves SHAPE (which columns/fields exist) instead
//! of lossy NL prose. The line is intentionally partial — recall_history
//! (lossless, v1) recovers the full result verbatim.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn array_of_objects_becomes_tabular_digest() {
        let rows: Vec<String> = (0..8)
            .map(|i| format!(r#"{{"region":"R{i}","revenue":{},"units":{}}}"#, 100_000 + i * 1000, 500 + i * 10))
            .collect();
        let content = format!("[{}]", rows.join(","));
        let d = digest_tool_result(&content).expect("structured");
        assert!(d.contains("8 filas"), "got: {d}");
        assert!(d.contains("cols: region, revenue, units"), "got: {d}");
        assert!(d.contains("region:R0"), "sample row missing: {d}");
    }

    #[test]
    fn array_of_scalars_becomes_count_and_sample() {
        let content = format!("[{}]", (0..40).map(|i| i.to_string()).collect::<Vec<_>>().join(","));
        let d = digest_tool_result(&content).expect("structured");
        assert!(d.contains("40 elementos"), "got: {d}");
        assert!(d.contains("muestra: [0, 1, 2"), "got: {d}");
    }

    #[test]
    fn object_lists_fields_inline_scalars_and_nested_markers() {
        let content = r#"{
            "order_id":"5512","status":"en transito","total":340,
            "items":[{"sku":"AB-1","qty":2},{"sku":"AB-2","qty":1}],
            "shipping_address":"Av Corrientes 1234","customer":{"email":"a@x.com","tier":"pro"}
        }"#;
        let d = digest_tool_result(content).expect("structured");
        assert!(d.contains("objeto"), "got: {d}");
        assert!(d.contains("order_id"), "got: {d}");
        assert!(d.contains("items[2]"), "nested array marker missing: {d}");
        assert!(d.contains("customer{2}"), "nested object marker missing: {d}");
        assert!(d.contains("status=en transito"), "inline scalar missing: {d}");
        // Drill-down surfaces the columns of the dominant array-of-objects field.
        assert!(d.contains("items[2] cols: sku, units") || d.contains("items[2] cols: sku, qty"), "drill-down missing: {d}");
    }

    #[test]
    fn non_json_returns_none() {
        assert!(digest_tool_result("the search returned three relevant articles about ...").is_none());
    }

    #[test]
    fn bare_scalar_returns_none() {
        assert!(digest_tool_result("96").is_none());
        assert!(digest_tool_result(r#""just a quoted string""#).is_none());
    }

    #[test]
    fn empty_collections_return_none() {
        assert!(digest_tool_result("[]").is_none());
        assert!(digest_tool_result("{}").is_none());
    }

    #[test]
    fn many_columns_are_capped_with_overflow_marker() {
        let pairs: Vec<String> = (0..20).map(|i| format!(r#""c{i}":{i}"#)).collect();
        let content = format!("[{{{}}}]", pairs.join(","));
        let d = digest_tool_result(&content).expect("structured");
        assert!(d.contains("+8 más"), "expected column overflow marker, got: {d}");
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p colmena_dag_engine --lib tool_digest`
Expected: FAIL to compile — `cannot find function digest_tool_result in this scope`.

- [ ] **Step 4: Implement the digest function**

Prepend the implementation ABOVE the `#[cfg(test)] mod tests` block in `tool_digest.rs`:

```rust
use serde_json::Value;

const MAX_COLUMNS: usize = 12;
const SAMPLE_ROWS: usize = 2;
const SCAN_ROWS_FOR_COLUMNS: usize = 50;
const MAX_INLINE_FIELDS: usize = 6;
const FIELD_VALUE_CHARS: usize = 40;
const DIGEST_CEILING_CHARS: usize = 400;

/// Returns a one-line structured digest of a tool result if `content` is
/// recognizably structured JSON (object, array-of-objects, or scalar array);
/// otherwise `None`, in which case the caller falls back to the NL summary.
///
/// The returned line carries NO turn tag and NO recall hint — the caller in
/// `history_compaction` prepends `[Tn] TOOL:` and appends the recall cue.
pub fn digest_tool_result(content: &str) -> Option<String> {
    let v: Value = serde_json::from_str(content.trim()).ok()?;
    let digest = match &v {
        Value::Array(arr) => digest_array(arr)?,
        Value::Object(_) => digest_object(&v)?,
        // Bare scalars (number / string / bool) are not "structured".
        _ => return None,
    };
    Some(cap(&digest, DIGEST_CEILING_CHARS))
}

fn digest_array(arr: &[Value]) -> Option<String> {
    if arr.is_empty() {
        return None;
    }
    // Array of objects → tabular digest.
    if arr.iter().take(SCAN_ROWS_FOR_COLUMNS).all(Value::is_object) {
        let cols = collect_columns(arr);
        let mut s = format!("{} filas · cols: {}", arr.len(), join_capped(&cols));
        let sample = sample_rows(arr, &cols);
        if !sample.is_empty() {
            s.push_str(&format!(" · muestra: {}", sample.join("; ")));
        }
        return Some(s);
    }
    // Array of scalars (or mixed) → count + small sample.
    let sample: Vec<String> = arr.iter().take(5).map(scalar_str).collect();
    let more = if arr.len() > 5 { ", …" } else { "" };
    Some(format!("{} elementos · muestra: [{}{}]", arr.len(), sample.join(", "), more))
}

fn digest_object(v: &Value) -> Option<String> {
    let map = v.as_object()?;
    if map.is_empty() {
        return None;
    }
    let mut fields: Vec<String> = Vec::new();
    let mut inline: Vec<String> = Vec::new();
    let mut drill: Option<(String, &Vec<Value>)> = None;
    for (k, val) in map.iter() {
        match val {
            Value::Array(a) => {
                fields.push(format!("{k}[{}]", a.len()));
                let is_obj_array =
                    !a.is_empty() && a.iter().take(SCAN_ROWS_FOR_COLUMNS).all(Value::is_object);
                if is_obj_array {
                    let better = drill.as_ref().map(|(_, prev)| a.len() > prev.len()).unwrap_or(true);
                    if better {
                        drill = Some((k.clone(), a));
                    }
                }
            }
            Value::Object(o) => fields.push(format!("{k}{{{}}}", o.len())),
            scalar => {
                fields.push(k.clone());
                if inline.len() < MAX_INLINE_FIELDS {
                    inline.push(format!("{k}={}", scalar_str(scalar)));
                }
            }
        }
    }
    let mut s = format!("objeto · campos: {}", join_capped(&fields));
    if !inline.is_empty() {
        s.push_str(&format!(" · {}", inline.join(", ")));
    }
    if let Some((k, a)) = drill {
        let cols = collect_columns(a);
        s.push_str(&format!(" · {k}[{}] cols: {}", a.len(), join_capped(&cols)));
    }
    Some(s)
}

/// Union of object keys across the first `SCAN_ROWS_FOR_COLUMNS` rows, in
/// first-seen order (deterministic).
fn collect_columns(arr: &[Value]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for row in arr.iter().take(SCAN_ROWS_FOR_COLUMNS) {
        if let Value::Object(map) = row {
            for k in map.keys() {
                if !seen.iter().any(|s| s == k) {
                    seen.push(k.clone());
                }
            }
        }
    }
    seen
}

fn join_capped(cols: &[String]) -> String {
    if cols.len() <= MAX_COLUMNS {
        cols.join(", ")
    } else {
        let shown = cols[..MAX_COLUMNS].join(", ");
        format!("{shown}, +{} más", cols.len() - MAX_COLUMNS)
    }
}

fn sample_rows(arr: &[Value], cols: &[String]) -> Vec<String> {
    arr.iter()
        .take(SAMPLE_ROWS)
        .filter_map(|row| {
            row.as_object().map(|map| {
                let fields: Vec<String> = cols
                    .iter()
                    .take(MAX_INLINE_FIELDS)
                    .filter_map(|c| map.get(c.as_str()).map(|v| format!("{c}:{}", scalar_str(v))))
                    .collect();
                format!("{{{}}}", fields.join(", "))
            })
        })
        .collect()
}

fn scalar_str(v: &Value) -> String {
    let raw = match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(a) => return format!("[{}]", a.len()),
        Value::Object(o) => return format!("{{{}}}", o.len()),
    };
    cap(&raw, FIELD_VALUE_CHARS)
}

/// Char-safe truncation with an ellipsis. Safe to truncate a digest because
/// the full result is recoverable verbatim via recall_history (lossless, v1).
fn cap(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max).collect();
        format!("{kept}…")
    }
}
```

> **Note on the drill-down test assertion:** the sample object uses `items:[{sku,qty},...]`, so the drill-down emits `items[2] cols: sku, qty`. The test accepts either `sku, qty` or `sku, units` only to be resilient to wording; with this implementation it will be `sku, qty`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p colmena_dag_engine --lib tool_digest`
Expected: PASS (7 tests).

- [ ] **Step 6: Lint + format**

Run: `cargo fmt && cargo clippy -p colmena_dag_engine --lib 2>&1 | tail -20`
Expected: no warnings (the crate denies warnings).

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/llm/application/tool_digest.rs src/libs/colmena/src/llm/application/mod.rs
git commit -m "$(cat <<'EOF'
feat(memory): add deterministic structured digest of tool results (v1.1 core)

Pure tool_digest::digest_tool_result turns a structured-JSON tool result
into a one-line shape digest (columns + row count + sample, field inventory
+ nested markers + drill-down for objects). Returns None for non-structured
content so the NL summary path is unchanged. No DB / repo / API change.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Numeric aggregates (min/max) for tabular digests

Append `col: min M max X` for up to 3 numeric columns so a later turn can answer "which region had the lowest margin?" without recalling every row. Applies to top-level array-of-objects and the drilled-in array field of an object.

**Files:**
- Modify: `src/libs/colmena/src/llm/application/tool_digest.rs`

- [ ] **Step 1: Write the failing tests**

Add these two tests inside the existing `mod tests` block in `tool_digest.rs`:

```rust
    #[test]
    fn tabular_digest_includes_numeric_min_max() {
        let rows = r#"[
            {"region":"Norte","revenue":420000,"margin":18},
            {"region":"Sur","revenue":310000,"margin":22},
            {"region":"Este","revenue":120000,"margin":9}
        ]"#;
        let d = digest_tool_result(rows).expect("structured");
        assert!(d.contains("revenue: min 120000 max 420000"), "got: {d}");
        assert!(d.contains("margin: min 9 max 22"), "got: {d}");
    }

    #[test]
    fn drilled_array_in_object_includes_aggregates() {
        let content = r#"{"row_count":2,"rows":[{"sku":"A","qty":5},{"sku":"B","qty":12}]}"#;
        let d = digest_tool_result(content).expect("structured");
        assert!(d.contains("rows[2] cols: sku, qty"), "got: {d}");
        assert!(d.contains("qty: min 5 max 12"), "got: {d}");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p colmena_dag_engine --lib tool_digest`
Expected: FAIL — the two new assertions don't find the `min`/`max` substrings.

- [ ] **Step 3: Implement the aggregates helper and wire it in**

Add the helpers below `sample_rows` in `tool_digest.rs`:

```rust
const MAX_AGG_COLS: usize = 3;

/// For up to `MAX_AGG_COLS` numeric columns, compute min/max across all rows.
fn numeric_aggregates(arr: &[Value], cols: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in cols {
        if out.len() >= MAX_AGG_COLS {
            break;
        }
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        let mut any = false;
        for row in arr {
            if let Some(n) = row.get(c.as_str()).and_then(Value::as_f64) {
                any = true;
                if n < min {
                    min = n;
                }
                if n > max {
                    max = n;
                }
            }
        }
        if any {
            out.push(format!("{c}: min {} max {}", fmt_num(min), fmt_num(max)));
        }
    }
    out
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n:.2}")
    }
}
```

Then append aggregates in BOTH tabular sites. In `digest_array`, after the `muestra` block and before `return Some(s)`:

```rust
        for agg in numeric_aggregates(arr, &cols) {
            s.push_str(&format!(" · {agg}"));
        }
        return Some(s);
```

In `digest_object`, inside the `if let Some((k, a)) = drill { ... }` block, after the `cols:` push:

```rust
        for agg in numeric_aggregates(a, &cols) {
            s.push_str(&format!(" · {agg}"));
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p colmena_dag_engine --lib tool_digest`
Expected: PASS (9 tests).

- [ ] **Step 5: Lint + format**

Run: `cargo fmt && cargo clippy -p colmena_dag_engine --lib 2>&1 | tail -20`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/llm/application/tool_digest.rs
git commit -m "$(cat <<'EOF'
feat(memory): add numeric min/max aggregates to tool-result digests

Tabular digests (top-level array-of-objects and drilled object array fields)
now append `col: min M max X` for up to 3 numeric columns, so a later turn
can reason about ranges without recalling every row.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Wire the digest into `build_compacted_messages`

Prefer the digest for `tool`-role messages in the old zone, ahead of the cache-hit and LLM-summarizer branches. Never cache, never call the summarizer for a structured tool result.

**Files:**
- Modify: `src/libs/colmena/src/llm/application/history_compaction.rs:178-217` (the `ValueClass::Content` branch) and the imports/test module.

- [ ] **Step 1: Write the failing test**

Add this test inside the existing `mod tests` block in `history_compaction.rs` (it reuses `ckey`, `InMemoryConversationRepository`, `Arc`, `async_trait` already imported in that module). First add a panicking summarizer struct right after the existing `StubSummarizer` definition:

```rust
    struct FailSummarizer;
    #[async_trait]
    impl MessageSummarizer for FailSummarizer {
        async fn summarize(
            &self,
            _text: &str,
            _t: usize,
        ) -> Result<String, crate::llm::domain::LlmError> {
            panic!("summarizer must NOT be called for a structured tool result");
        }
    }
```

Then the test:

```rust
    #[tokio::test]
    async fn structured_tool_result_becomes_digest_without_calling_summarizer() {
        let repo = Arc::new(InMemoryConversationRepository::new());
        let k = ckey();

        // idx 0,1 = keep_first (short user msgs).
        repo.add_message(&k, LlmMessage::user("x".into()).unwrap()).await.unwrap();
        repo.add_message(&k, LlmMessage::user("x".into()).unwrap()).await.unwrap();
        // idx 2 = a large structured tool result (≥250 chars) → must become a digest.
        let rows: Vec<String> = (0..8)
            .map(|i| format!(r#"{{"region":"R{i}","revenue":{},"units":{}}}"#, 100_000 + i * 1000, 500 + i * 10))
            .collect();
        let tool_content = format!("[{}]", rows.join(","));
        assert!(tool_content.len() >= 250);
        repo.add_message(&k, LlmMessage::tool("call_1".into(), tool_content).unwrap())
            .await
            .unwrap();
        // idx 3,4,5 = short recents.
        for _ in 0..3 {
            repo.add_message(&k, LlmMessage::user("x".into()).unwrap()).await.unwrap();
        }

        let stored = repo.get_with_summaries(&k).await.unwrap();
        let fail: Arc<dyn MessageSummarizer> = Arc::new(FailSummarizer);

        // Tiny budget pushes the tool result (idx 2) into the old zone.
        let out = build_compacted_messages(&stored, &k, repo.as_ref(), Some(&fail), 5).await;

        let summary = out
            .iter()
            .find(|m| m.role() == &MessageRole::System)
            .expect("summary block present");
        let body = summary.content();
        assert!(body.contains("[T2]"), "digest turn tag missing: {body}");
        assert!(body.contains("8 filas"), "row count missing: {body}");
        assert!(body.contains("cols: region, revenue, units"), "columns missing: {body}");
        assert!(body.contains("recall_history(turn=2)"), "recall hint missing: {body}");

        // The structured tool result must NOT have been persisted as a summary.
        let after = repo.get_with_summaries(&k).await.unwrap();
        assert_eq!(after[2].summary, None, "digest must not be cached in summary column");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p colmena_dag_engine --lib structured_tool_result_becomes_digest`
Expected: FAIL — either `panic!` from `FailSummarizer` (the old code routes the tool result to the summarizer) or the digest substrings are absent.

- [ ] **Step 3: Add the import**

At the top of `history_compaction.rs`, beside the other `use crate::llm::domain::...` line, add:

```rust
use crate::llm::application::tool_digest::digest_tool_result;
```

- [ ] **Step 4: Insert the digest branch**

In `build_compacted_messages`, inside the `ValueClass::Content` arm, the current ladder is:

```rust
                if let Some(tcs) = msg.tool_calls() {
                    // ... structural line ...
                } else if rendered_size(msg) < SUMMARY_SKIP_THRESHOLD_CHARS {
                    format!("[T{idx}] {}: {}", role_tag(msg), msg.content())
                } else if let Some(cached) = stored[idx].summary.as_deref() {
```

Insert a new `else if` between the `rendered_size < threshold` arm and the `cached` arm:

```rust
                } else if rendered_size(msg) < SUMMARY_SKIP_THRESHOLD_CHARS {
                    format!("[T{idx}] {}: {}", role_tag(msg), msg.content())
                } else if let Some(d) = matches!(msg.role(), MessageRole::Tool)
                    .then(|| digest_tool_result(msg.content()))
                    .flatten()
                {
                    // Structured tool result → deterministic digest. No LLM, no cache;
                    // the full result is recoverable verbatim via recall_history (lossless).
                    format!(
                        "[T{idx}] {}: {d} · recall_history(turn={idx}) para el detalle",
                        role_tag(msg)
                    )
                } else if let Some(cached) = stored[idx].summary.as_deref() {
```

(Leave the rest of the ladder — `cached`, summarizer, fallback — unchanged.)

- [ ] **Step 5: Run the targeted test, then the whole module**

Run: `cargo test -p colmena_dag_engine --lib structured_tool_result_becomes_digest`
Expected: PASS.

Run: `cargo test -p colmena_dag_engine --lib history_compaction`
Expected: PASS (all existing compaction tests still green — short messages, scaffolding, recent boundary, the NL-summary-and-cache test).

- [ ] **Step 6: Lint + format**

Run: `cargo fmt && cargo clippy -p colmena_dag_engine --lib 2>&1 | tail -20`
Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/llm/application/history_compaction.rs
git commit -m "$(cat <<'EOF'
feat(memory): prefer structured digest for tool results in history compaction

build_compacted_messages now renders a deterministic digest for tool-role
messages whose content is structured JSON, ahead of the cache-hit and
LLM-summarizer branches. NL tool results and all other roles are unchanged.
The digest line cites recall_history(turn=N) for the lossless full result.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Documentation

Mark v1.1 shipped in the spec, add a CHANGELOG entry, document the digest in the memory guide, and update the CLAUDE.md status section.

**Files:**
- Modify: `docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md`
- Modify: `docs/CHANGELOG_2026-06.md`
- Modify: `docs/developer_guide/15_memory_guide.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update the spec — move v1.1 from "future" to "shipped"**

In `docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md`, in the "Enhancements futuros (fuera de v1)" section, change the first bullet from a future item to a shipped pointer:

```markdown
- **Digest estructurado de tool-results (v1.1) — SHIPPED 2026-06-19:** para
  resultados de tools estructurados (JSON object / array-of-objects / scalar
  array), `tool_digest::digest_tool_result` produce un digest determinista
  (esquema + N filas + muestra + min/max) en vez de prosa NL. Sin LLM, sin
  cache, sin cambio de DB. Resultados NL caen al resumen semántico v1. Ver
  [`docs/superpowers/plans/2026-06-19-tool-result-structured-digest-v1-1.md`](../plans/2026-06-19-tool-result-structured-digest-v1-1.md).
```

Also, in the "Política de compactación por rol/tipo" table, update the structured-tool-result row:

```markdown
| `tool` result **estructurado** (JSON/filas) | **v1.1 (shipped):** digest determinista (esquema + N filas + muestra + min/max), sin LLM. NL cae a resumen semántico. |
```

- [ ] **Step 2: Add a CHANGELOG entry**

In `docs/CHANGELOG_2026-06.md`, add a new top section (match the existing numbered-section format; use the next number after the highest present — the example below assumes §19, adjust to the actual next number):

```markdown
## §19 — Digest estructurado de tool-results (v1.1) — 2026-06-19

`build_compacted_messages` ahora renderiza un **digest determinista** para
mensajes `tool` cuyo contenido es JSON estructurado (objeto, array de objetos,
array de escalares), en lugar de un resumen NL con pérdida.

- Nuevo módulo puro `llm/application/tool_digest.rs` → `digest_tool_result(content) -> Option<String>`.
- Digest: esquema (columnas/campos) + N filas + muestra de filas + min/max de
  hasta 3 columnas numéricas; para objetos: inventario de campos, escalares
  inline, marcadores de anidados (`items[8]`, `customer{2}`) y drill-down en el
  array-de-objetos dominante.
- **Sin LLM, sin cache, sin migración, sin cambio de API pública** — se computa
  fresco cada load porque es determinista y barato. Resultados de tools en NL
  (no-JSON) caen al resumen semántico de v1, sin cambios.
- La línea cita `recall_history(turn=N)`; el resultado completo se recupera
  verbatim (recall lossless de v1).
- Motivación: que el modelo conserve la FORMA de los datos (qué columnas/campos
  existían) al envejecer el mensaje, evitando alucinación o recall a ciegas.
  Caso real: agente de datos/soporte que reusa campos específicos turnos después.

Tests: 9 unit en `tool_digest`, 1 de wiring en `history_compaction`, 1 E2E real.
```

- [ ] **Step 3: Document in the memory guide**

In `docs/developer_guide/15_memory_guide.md`, inside the existing section "🗜️ Compactación y recuperación de memoria", add a subsection (place it right after the per-role policy description):

```markdown
### Digest estructurado de tool-results (v1.1)

Cuando un resultado de tool **estructurado** (JSON: objeto, array de objetos, o
array de escalares) envejece y sale de la ventana reciente, en vez de un resumen
NL con pérdida se genera un **digest determinista** que conserva la FORMA:

- **Array de objetos** (p.ej. `sql_query`, listas de una API):
  `600 filas · cols: month, region, revenue, units · muestra: {month:2026-01, region:Norte, …}; {…} · revenue: min 12000 max 480000`
- **Objeto** (p.ej. detalle de un pedido):
  `objeto · campos: order_id, status, total, items[8], shipping_address · status=en transito, total=340 · items[8] cols: sku, qty`
- **Array de escalares:** `40 elementos · muestra: [0, 1, 2, …]`

El digest **no usa LLM, no se cachea y no toca la DB** (es determinista y barato,
se recalcula en cada load). La línea cita `recall_history(turn=N)`: el resultado
completo se recupera **verbatim** (recall lossless). Si el contenido del tool NO
es JSON estructurado (texto NL de una búsqueda web, etc.), cae al resumen
semántico normal.

**Por qué importa:** un resumen NL ("devolvió ventas mensuales por región") borra
las columnas; el modelo no sabe que existía `revenue` ni `margin`, así que alucina
o no sabe que puede recuperar. El digest preserva el esquema → el modelo decide
con precisión si responde del digest o hace `recall_history` del detalle.
```

- [ ] **Step 4: Update CLAUDE.md status**

In `CLAUDE.md`, under "## Current Status", add a bullet after the most recent dated entry:

```markdown
- **Tool-result structured digest (v1.1) shipped 2026-06-19** — resultados de
  tools estructurados (JSON object / array-of-objects / scalar array) ahora se
  compactan como un digest determinista (esquema + N filas + muestra + min/max)
  en vez de prosa NL, vía `llm/application/tool_digest.rs`. Sin LLM, sin cache,
  sin migración, sin cambio de API pública (solo el wire-format del bloque de
  resumen que ve el modelo) → ADP no afectado. Resultados de tools en NL caen al
  resumen semántico v1. Recall sigue lossless. Spec:
  [`docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md`](docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md);
  plan: [`docs/superpowers/plans/2026-06-19-tool-result-structured-digest-v1-1.md`](docs/superpowers/plans/2026-06-19-tool-result-structured-digest-v1-1.md).
```

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md docs/CHANGELOG_2026-06.md docs/developer_guide/15_memory_guide.md CLAUDE.md
git commit -m "$(cat <<'EOF'
docs(memory): document tool-result structured digest (v1.1)

Mark v1.1 shipped in the design spec, add CHANGELOG entry, document the
digest format + rationale in the memory guide, and update CLAUDE.md status.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: End-to-end verification with a real graph

Prove that, across a multi-turn conversation, a tool that returns a structured JSON table ages out of the recent window and is sent to the model as a **digest** (columns + filas), not NL prose — and that `recall_history` reconstructs the full table. Per project rules: real tools (no mocks), `--agent-session-id`, save SSE to `/tmp/colmena_e2e/`, friendly report.

**Files:**
- Create: `tests/graphs/agents/tool_digest_e2e.json`

- [ ] **Step 1: Create the test graph**

Create `tests/graphs/agents/tool_digest_e2e.json`. It uses a real `python_script` tool that returns a list of dicts (a structured table the LLM must reason over later). Confirm `python_script` is registered in `registry.rs` before running.

```json
{
  "nodes": [
    {
      "id": "agent",
      "node_type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "system_message": "Sos un analista de datos. Cuando el usuario pida datos de ventas, llamá la tool sales_table una vez y luego respondé en base a lo que devuelve. En turnos siguientes respondé sobre esos datos; si necesitás el detalle exacto de filas anteriores, usá recall_history(turn=N).",
        "lazy_tool_loading": false,
        "tool_configurations": {
          "sales_table": {
            "name": "sales_table",
            "node_type": "python_script",
            "description": "Devuelve la tabla de ventas por región (lista de filas con region, revenue, units, margin).",
            "node_schema": {
              "code": {
                "fixed": "output = [\n  {'region': 'Norte', 'revenue': 420000, 'units': 1200, 'margin': 18},\n  {'region': 'Sur', 'revenue': 310000, 'units': 980, 'margin': 22},\n  {'region': 'Este', 'revenue': 120000, 'units': 540, 'margin': 9},\n  {'region': 'Oeste', 'revenue': 275000, 'units': 860, 'margin': 14},\n  {'region': 'Centro', 'revenue': 198000, 'units': 700, 'margin': 11}\n]"
              }
            }
          }
        }
      }
    }
  ],
  "edges": []
}
```

- [ ] **Step 2: Run the multi-turn conversation against live Gemini + Postgres**

```bash
mkdir -p /tmp/colmena_e2e
set -a; source .env; set +a
SID=agent_tool_digest_e2e_001

# Turn 1 — agent calls sales_table; result is large + structured → stored full.
cargo run --bin dag_engine -- run tests/graphs/agents/tool_digest_e2e.json \
  --agent-session-id "$SID" \
  --answer "Traé las ventas por región." \
  > /tmp/colmena_e2e/tool_digest_t1.sse 2>&1

# Turns 2-4 — unrelated chatter to push the tool result out of the recent window.
for i in 2 3 4; do
  cargo run --bin dag_engine -- run tests/graphs/agents/tool_digest_e2e.json \
    --agent-session-id "$SID" \
    --answer "Contame algo breve sobre buenas prácticas de reporting (mensaje $i)." \
    > /tmp/colmena_e2e/tool_digest_t$i.sse 2>&1
done

# Turn 5 — force reasoning over a column that NL prose would have dropped.
cargo run --bin dag_engine -- run tests/graphs/agents/tool_digest_e2e.json \
  --agent-session-id "$SID" \
  --answer "De esa tabla de ventas, ¿qué región tuvo el peor margen y cuántas unidades vendió?" \
  --include-extra-info \
  > /tmp/colmena_e2e/tool_digest_t5.sse 2>&1
```

- [ ] **Step 3: Verify the digest reached the model and the answer is correct**

Inspect what was sent to the model on turn 5. The conversation-summary `system` block should contain the digest line for the aged tool result, e.g.:

```
[T2] TOOL: 5 filas · cols: region, revenue, units, margin · muestra: {region:Norte, revenue:420000, units:1200}; {region:Sur, revenue:310000, units:980} · revenue: min 120000 max 420000 · units: min 540 max 1200 · recall_history(turn=2) para el detalle
```

Checks:
- The turn-5 SSE shows the model answered **Este, 540 unidades** (lowest margin = 9). This is the data point an NL summary would have erased.
- The summary block contains `filas · cols:` and `margin`, NOT a prose sentence like "devolvió ventas por región".
- If the model needed exact rows it would have emitted a `recall_history(turn=2)` tool call; confirm the dump shows either a correct direct answer from the digest or a successful recall.

To see the assembled context, temporarily enable the existing context dump in `agent_service.rs` (search for the `DUMP`/eprintln block near the compaction call), run turn 5, then revert with `git checkout`. Grep the captured stderr:

```bash
grep -nE "filas · cols|recall_history\(turn=2\)|Este|540" /tmp/colmena_e2e/tool_digest_t5.sse
```

Expected: matches for the digest line and the correct answer.

- [ ] **Step 4: Write the friendly report**

Produce a short report (input prompts per turn, the digest line the model saw on turn 5, whether the model answered from the digest or via recall, token counts, and the final answer correctness). Save alongside the SSE:

```bash
# (Author the report from the captured SSE; do not paste full SSE into chat.)
```

- [ ] **Step 5: Commit the graph**

```bash
git add tests/graphs/agents/tool_digest_e2e.json
git commit -m "$(cat <<'EOF'
test(memory): add E2E graph for tool-result structured digest (v1.1)

Multi-turn graph where a python_script tool returns a structured sales table
that ages out of the recent window; a later turn asks about a column NL prose
would drop, verifying the digest preserves the schema and the model answers
correctly (or recalls losslessly).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Full pre-push verification

- [ ] **Step 1: Run the full verbose test suite (CI parity, no DATABASE_URL)**

Run: `cargo test -p colmena_dag_engine --verbose 2>&1 | tail -30`
Expected: all unit + integration + doctests pass (CI runs `--verbose`; `--lib` alone hides doctest/integration failures). Do NOT set `DATABASE_URL` for this run — two pre-existing `crdt_doc_tools` tests fail only when a live Postgres is connected (unrelated to this feature).

- [ ] **Step 2: Confirm clean format + lint across the crate**

Run: `cargo fmt --check && cargo clippy -p colmena_dag_engine --all-targets 2>&1 | tail -20`
Expected: no diff, no warnings.

- [ ] **Step 3: Push (only when the user asks)**

Per project policy, push only on explicit request. When asked:

```bash
git push -u origin docs/conversation-memory-summary
```

---

## Self-Review

**Spec coverage:** The spec's v1.1 line — "digest estructurado de tool-results (esquema + N filas + valores clave)" — is implemented: esquema (cols/fields, Task 1), N filas (count, Task 1), valores clave (sample rows + inline scalars + min/max aggregates, Tasks 1–2). Deterministic + no LLM (Task 1/3). NL fallback preserved (Task 3, `None` path). Recall remains lossless (unchanged from v1; digest cites `recall_history(turn=N)`). Docs updated (Task 4). E2E real graph (Task 5).

**Placeholder scan:** No TBD/TODO; every code step shows complete code; every command shows expected output.

**Type consistency:** `digest_tool_result(content: &str) -> Option<String>` is defined in Task 1 and called identically in Task 3 (`matches!(msg.role(), MessageRole::Tool).then(|| digest_tool_result(msg.content())).flatten()`). Helpers (`collect_columns`, `join_capped`, `sample_rows`, `scalar_str`, `cap`, `numeric_aggregates`, `fmt_num`) are defined before use. `numeric_aggregates(arr: &[Value], cols: &[String])` is called in both `digest_array` and `digest_object` with the same signature. No DB/repo signature changes (digests are never persisted), so no cross-task type drift with the v1 `summary` column.

**Scope check:** Single subsystem (the compaction layer's per-message line for tool results). Self-contained and independently testable. No migration, no public-API change → ADP sweep not required.
