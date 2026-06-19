# Tool-digest v1.2 — identifier drill (nominal map) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make the structured-digest drill-down show row IDENTITIES (e.g. `llmCall "Tool-using Agent"`) instead of only column names, by pulling identifying sub-fields (`name`/`title`/`label`/`type`/`kind`/`id`) from object-valued columns with a small depth budget (1 hop into object columns). Purely additive to the digest string; no schema/API/cache change.

**Architecture:** Add pure helpers to `llm/application/tool_digest.rs`: `find_identifier` (shallow priority+depth search), `row_label` (compact `<type> "<name>"`), `nominal_sample` (`[lbl0, lbl1, lbl2, … +N]`). Wire into (a) `digest_object`'s drill-down line and (b) `digest_array`/`sample_rows` object-column rendering. Builds on v1.1 (shipped, CHANGELOG §40).

**Tech Stack:** Rust, `serde_json` (`preserve_order` enabled → deterministic key order).

---

## Design decisions (locked)

- **Identifier categories:** name-ish = `["name","title","label"]`; type-ish = `["type","kind"]`. Plus `id` as a last-resort name-ish fallback is NOT included (ids are noisy) — fallback is the first scalar column instead.
- **Depth budget:** search the row + recurse 1 hop into object-valued columns (`IDENTITY_SEARCH_DEPTH = 1`). This reaches e.g. `data.label` but stops before deeper identifiers. Beyond that → not found → marker/recall (v1.1 behavior). Rationale: each level multiplies output; bounded + deterministic.
- **Row label format:** `<type> "<name>"` when both found; `"<name>"` if only name; `<type>` if only type; else first scalar column `col:value`; else omit the row.
- **Sample size:** first 3 rows (`IDENTITY_SAMPLE_ROWS = 3`), `, … +N` overflow.
- **Where:** drill-down line in `digest_object` gets `· muestra: [...]` appended (today it has NO sample). Top-level `sample_rows` renders object columns by their name-identifier instead of `{N}`.
- **Bounded:** the whole digest is still capped at `DIGEST_CEILING_CHARS` (400) in `digest_tool_result`.
- v1.1 already handles arbitrarily deep JSON safely (no recursion; markers). v1.2 only extends the useful-without-recall frontier one level for the common "records with a nested identifier" shape.

---

### Task 1: Implement the identifier drill

**Files:**
- Modify: `src/libs/colmena/src/llm/application/tool_digest.rs`

- [ ] **Step 1: Write the failing tests.** Add inside the existing `mod tests`:

```rust
    #[test]
    fn drill_down_shows_nominal_row_labels() {
        // Canvas-shaped: object whose dominant array has type + nested data.label.
        let content = r#"{
            "kind":"group",
            "nodes":[
                {"id":"a","type":"webSearch","data":{"label":"Web Search"}},
                {"id":"b","type":"llmCall","data":{"label":"Tool-using Agent"}},
                {"id":"c","type":"apiCall","data":{"label":"test_run_agent"}},
                {"id":"d","type":"apiCall","data":{"label":"resolve_built_agent"}}
            ]
        }"#;
        let d = digest_tool_result(content).expect("structured");
        assert!(d.contains("nodes[4] cols: id, type, data"), "drill cols: {d}");
        assert!(d.contains(r#"muestra: [webSearch "Web Search""#), "nominal sample missing: {d}");
        assert!(d.contains(r#"llmCall "Tool-using Agent""#), "type+label combo missing: {d}");
        assert!(d.contains("+1"), "overflow marker missing: {d}");
    }

    #[test]
    fn tabular_sample_renders_object_column_identifier() {
        let content = r#"[
            {"order_id":"8842","customer":{"name":"Ana Perez","tier":"pro"},"total":1284},
            {"order_id":"8843","customer":{"name":"Beto Ruiz","tier":"free"},"total":99}
        ]"#;
        let d = digest_tool_result(content).expect("structured");
        // Object column shows the name identifier, not {2}.
        assert!(d.contains("customer:Ana Perez"), "object identifier missing: {d}");
        assert!(!d.contains("customer:{2}"), "should not show opaque marker: {d}");
    }

    #[test]
    fn row_label_falls_back_to_first_scalar_when_no_identifier() {
        // No name/type keys anywhere → fall back to first scalar column.
        let content = r#"{"data":{"rows":[{"v":1,"w":2},{"v":3,"w":4}]}}"#;
        let d = digest_tool_result(content).expect("structured");
        assert!(d.contains("muestra: [v:1"), "scalar fallback missing: {d}");
    }

    #[test]
    fn identifier_search_respects_depth_budget() {
        // name is at depth 3 (row -> a -> b -> name): beyond budget 1 (one hop into
        // object columns) -> not found, so the row falls back to the first scalar column
        // rather than the deep name.
        let content = r#"{"items":[
            {"k":1,"a":{"b":{"name":"too deep"}}},
            {"k":2,"a":{"b":{"name":"also deep"}}}
        ]}"#;
        let d = digest_tool_result(content).expect("structured");
        assert!(!d.contains("too deep"), "should not reach depth-3 identifier: {d}");
        assert!(d.contains("muestra: [k:1"), "should fall back to first scalar: {d}");
    }
```

- [ ] **Step 2: Run to verify they fail.** `cargo test -p colmena_dag_engine --lib tool_digest` — expect the 4 new tests to fail.

- [ ] **Step 3: Implement.** Add constants near the other consts:

```rust
const IDENTITY_KEYS_NAME: &[&str] = &["name", "title", "label"];
const IDENTITY_KEYS_TYPE: &[&str] = &["type", "kind"];
const IDENTITY_SEARCH_DEPTH: usize = 1;
const IDENTITY_SAMPLE_ROWS: usize = 3;
```

Add helpers (place after `scalar_str`):

```rust
/// Shallow priority search for the first SCALAR value whose key is in `keys`,
/// checking the requested keys at this level first, then recursing into
/// object-valued fields up to `depth`. Deterministic (serde_json preserves
/// key order). Returns the scalar rendered via `scalar_str`.
fn find_identifier(
    map: &serde_json::Map<String, Value>,
    keys: &[&str],
    depth: usize,
) -> Option<String> {
    for k in keys {
        if let Some(v) = map.get(*k) {
            if !v.is_object() && !v.is_array() {
                return Some(scalar_str(v));
            }
        }
    }
    if depth == 0 {
        return None;
    }
    for (_k, v) in map.iter() {
        if let Value::Object(child) = v {
            if let Some(found) = find_identifier(child, keys, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

/// Compact nominal label for a row object: `<type> "<name>"` when both are
/// found (shallow), else whichever is present, else `col:value` for the first
/// scalar column, else None.
fn row_label(row: &Value, cols: &[String]) -> Option<String> {
    let map = row.as_object()?;
    let name = find_identifier(map, IDENTITY_KEYS_NAME, IDENTITY_SEARCH_DEPTH);
    let kind = find_identifier(map, IDENTITY_KEYS_TYPE, IDENTITY_SEARCH_DEPTH);
    match (kind, name) {
        (Some(k), Some(n)) => Some(format!("{k} \"{n}\"")),
        (Some(k), None) => Some(k),
        (None, Some(n)) => Some(format!("\"{n}\"")),
        (None, None) => cols.iter().find_map(|c| {
            map.get(c.as_str()).and_then(|v| {
                (!v.is_object() && !v.is_array()).then(|| format!("{c}:{}", scalar_str(v)))
            })
        }),
    }
}

/// Nominal preview of the first rows: `[lbl0, lbl1, lbl2, … +N]`. None if no row
/// yields a label.
fn nominal_sample(arr: &[Value], cols: &[String]) -> Option<String> {
    let labels: Vec<String> = arr
        .iter()
        .take(IDENTITY_SAMPLE_ROWS)
        .filter_map(|r| row_label(r, cols))
        .collect();
    if labels.is_empty() {
        return None;
    }
    let extra = arr.len().saturating_sub(labels.len());
    let suffix = if extra > 0 {
        format!(", … +{extra}")
    } else {
        String::new()
    };
    Some(format!("[{}{}]", labels.join(", "), suffix))
}
```

Wire into `digest_object`'s drill block — after the aggregates loop, before the block closes:

```rust
    if let Some((k, a)) = drill {
        let cols = collect_columns(a);
        s.push_str(&format!(" · {k}[{}] cols: {}", a.len(), join_capped(&cols)));
        for agg in numeric_aggregates(a, &cols) {
            s.push_str(&format!(" · {agg}"));
        }
        if let Some(sample) = nominal_sample(a, &cols) {
            s.push_str(&format!(" · muestra: {sample}"));
        }
    }
```

Enhance `sample_rows` so object-valued columns show their name-identifier:

```rust
fn sample_rows(arr: &[Value], cols: &[String]) -> Vec<String> {
    arr.iter()
        .take(SAMPLE_ROWS)
        .filter_map(|row| {
            row.as_object().map(|map| {
                let fields: Vec<String> = cols
                    .iter()
                    .take(MAX_INLINE_FIELDS)
                    .filter_map(|c| {
                        map.get(c.as_str()).map(|v| {
                            let rendered = match v {
                                Value::Object(o) => find_identifier(
                                    o,
                                    IDENTITY_KEYS_NAME,
                                    IDENTITY_SEARCH_DEPTH,
                                )
                                .unwrap_or_else(|| scalar_str(v)),
                                _ => scalar_str(v),
                            };
                            format!("{c}:{rendered}")
                        })
                    })
                    .collect();
                format!("{{{}}}", fields.join(", "))
            })
        })
        .collect()
}
```

- [ ] **Step 4: Run.** `cargo test -p colmena_dag_engine --lib tool_digest` — expect all pass (existing + 4 new). Existing tests must stay green (the drill-down `muestra` is appended after the asserted `cols`/aggregates substrings; scalar-only samples unchanged).

- [ ] **Step 5: fmt + clippy.** `cargo fmt && cargo clippy -p colmena_dag_engine --lib 2>&1 | tail -20` — no warnings.

- [ ] **Step 6: Commit.**

```bash
git add src/libs/colmena/src/llm/application/tool_digest.rs
git commit -m "$(cat <<'EOF'
feat(memory): identifier drill in tool-result digest (v1.2)

The digest drill-down now lists row identities (`<type> "<name>"`) pulled from
nested name/type fields (depth <= 2) instead of only column names, and tabular
sample rows render object columns by their identifier. Makes the canvas/list
digest a nominal map so the model knows which record is which without recall.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Docs + migrate from backlog

**Files:** `docs/CHANGELOG_2026-06.md`, `docs/developer_guide/15_memory_guide.md`, `docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md`, `docs/BACKLOG.md`.

- [ ] Add CHANGELOG `## 41. Digest v1.2 — drill de identificadores (mapa nominal) — 2026-06-19` (append after §40, file convention newest-at-bottom). Describe the nominal sample, depth budget, additive/no-DB/no-API.
- [ ] In `15_memory_guide.md`, extend the digest subsection with a v1.2 example (`nodes[17] cols: … · muestra: [llmCall "Tool-using Agent", …]`).
- [ ] In the spec's enhancements section, mark v1.2 shipped (link the §41 / this plan).
- [ ] In `BACKLOG.md`, in the "digest enhancements (v1.2, v2)" section, mark the v1.2 subsection `✅ SHIPPED 2026-06-19 (§41)` and KEEP the v2 subsection active.
- [ ] Commit `docs(memory): document tool-digest v1.2 (identifier drill)`.

---

### Task 3: Verify + PR

- [ ] Run the digest on a canvas-shaped JSON (temporary test, like the v1.1 demo) to confirm the real output is `nodes[N]: [llmCall "Tool-using Agent", …]`; revert the temp test (`git checkout`).
- [ ] `cargo test -p colmena_dag_engine --verbose` (no `DATABASE_URL`) → exit 0; `cargo fmt --check`; `cargo clippy --all-targets` clean.
- [ ] Push; open PR against `develop`.

---

## Self-Review

- **Determinism:** `find_identifier` iterates `serde_json::Map` in insertion order (preserve_order) and checks priority keys before descending — deterministic.
- **Bounded:** depth 1 hop, 3 sample rows, per-value `scalar_str` cap, whole-digest 400-char ceiling. No unbounded recursion (depth decrements; arrays are not descended into by `find_identifier`).
- **No regressions:** drill-down `muestra` is appended after existing substrings; `sample_rows` only changes object-column rendering (scalar columns identical → `region:R0` etc. unaffected).
- **Type consistency:** `find_identifier(&Map, &[&str], usize)`, `row_label(&Value, &[String])`, `nominal_sample(&[Value], &[String])` used consistently in `digest_object` and `sample_rows`.
- **Scope:** single module, content-only; no cache/DB/API/hot-path change → ADP unaffected.
