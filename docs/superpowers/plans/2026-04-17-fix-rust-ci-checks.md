# Fix Rust CI Checks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the three Rust CI checks pass: `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` (doc-tests).

**Architecture:** All changes are mechanical linting/formatting fixes — no functional behavior changes. Each fix addresses a specific compiler or linter diagnostic.

**Tech Stack:** Rust, cargo fmt, cargo clippy

---

## File Map

| File | Action | What Changes |
|------|--------|-------------|
| All Rust source files | Auto-format | `cargo fmt` fixes whitespace/formatting |
| `src/libs/colmena/src/dag_engine/application/run_use_case.rs` | Modify line 749 | Remove redundant field name |
| `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs` | Modify lines 9-10 | Fix doc comment indentation |
| `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` | Modify line 68 | Remove unnecessary `.to_string()` |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/input.rs` | Modify lines 15-30 | Rewrite `loop` as `while let` |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` | Modify lines 108-121 | Collapse nested `if let` patterns |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs` | Modify lines 15-19 | Add `Default` impl |
| `src/libs/colmena/src/llm/domain/tools.rs` | Modify lines 17-21 | Add missing `pattern` field in doc-test |

---

### Task 1: Run `cargo fmt`

**Files:**
- Modify: all `.rs` files (automatic)

- [ ] **Step 1: Run cargo fmt**

```bash
cargo fmt
```

- [ ] **Step 2: Verify formatting passes**

```bash
cargo fmt -- --check
```

Expected: exits 0, no diff output.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "style: apply cargo fmt formatting"
```

---

### Task 2: Fix redundant field name in `run_use_case.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/run_use_case.rs:749`

Clippy error: `redundant field names in struct initialization`

- [ ] **Step 1: Apply fix**

Change line 749 from:

```rust
                graph_json: graph_json,
```

to:

```rust
                graph_json,
```

- [ ] **Step 2: Verify clippy passes for this error**

```bash
cargo clippy -- -D clippy::redundant-field-names 2>&1 | grep "redundant"
```

Expected: no output (no more redundant field errors).

---

### Task 3: Fix doc comment indentation in `tool_configuration.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs:9-10`

Clippy error: `doc list item without indentation` (x2)

- [ ] **Step 1: Apply fix**

Lines 9-10 are continuation lines of list items in a doc comment. They need to be indented to align with the list item text above. Change:

```rust
//!    - `fixed`: hidden from the LLM, always applied as-is.
//!    - LLM-visible: typed, optionally required, with description and pattern constraints.
//!    Container fields (e.g. `body`, `query_params`) support nested `properties`, allowing
//!    mixed fixed/dynamic sub-fields. Use this for all non-trivial tool configurations.
```

to:

```rust
//!    - `fixed`: hidden from the LLM, always applied as-is.
//!    - LLM-visible: typed, optionally required, with description and pattern constraints.
//!      Container fields (e.g. `body`, `query_params`) support nested `properties`, allowing
//!      mixed fixed/dynamic sub-fields. Use this for all non-trivial tool configurations.
```

The continuation lines (lines 9-10) need 6 spaces of indentation (`//!      `) to align under the list item content.

- [ ] **Step 2: Verify clippy passes for this error**

```bash
cargo clippy -- -D clippy::doc-lazy-continuation 2>&1 | grep "doc list"
```

Expected: no output.

---

### Task 4: Fix unnecessary `.to_string()` in `dag_tool_executor.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs:68`

Clippy error: `unnecessary use of to_string`

- [ ] **Step 1: Apply fix**

The issue is on line 68 inside the `replace_all` closure. The `unwrap_or` receives a `&String` from `caps[0].to_string()` but the outer `.to_string()` is redundant since `as_str()` already returns `&str` and the closure must return an owned `String`. The real fix is to use `into_owned()` on the `Cow` result or restructure. However clippy specifically flags line 68's `.to_string()` as redundant because the value from `unwrap_or` is already a `&str` that gets `.to_string()` called on it.

Change the closure from:

```rust
        re.replace_all(template, |caps: &regex::Captures| {
            let key = &caps[1];
            inputs
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or(&caps[0].to_string())
                .to_string()
        })
        .to_string()
```

to:

```rust
        re.replace_all(template, |caps: &regex::Captures| {
            let key = &caps[1];
            match inputs.get(key).and_then(|v| v.as_str()) {
                Some(resolved) => resolved.to_string(),
                None => caps[0].to_string(),
            }
        })
        .to_string()
```

This avoids the temporary `String` from `caps[0].to_string()` being passed by reference to `unwrap_or`, which clippy flags as unnecessary.

- [ ] **Step 2: Verify clippy passes for this error**

```bash
cargo clippy -- -D clippy::unnecessary-to-owned 2>&1 | grep "unnecessary"
```

Expected: no output.

---

### Task 5: Rewrite `loop` as `while let` in `input.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/input.rs:15`

Clippy error: `this loop could be written as a while let loop`

- [ ] **Step 1: Apply fix**

Change lines 15-30 from:

```rust
            let mut result = s.clone();
            let mut search_from = 0;
            loop {
                let Some(start) = result[search_from..].find("{{") else { break };
                let abs_start = search_from + start;
                let Some(end) = result[abs_start..].find("}}") else { break };
                let abs_end = abs_start + end;
                let key = result[abs_start + 2..abs_end].trim();
                let replacement = state
                    .get(key)
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                result.replace_range(abs_start..abs_end + 2, &replacement);
                search_from = abs_start + replacement.len();
            }
```

to:

```rust
            let mut result = s.clone();
            let mut search_from = 0;
            while let Some(start) = result[search_from..].find("{{") {
                let abs_start = search_from + start;
                let Some(end) = result[abs_start..].find("}}") else { break };
                let abs_end = abs_start + end;
                let key = result[abs_start + 2..abs_end].trim();
                let replacement = state
                    .get(key)
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                result.replace_range(abs_start..abs_end + 2, &replacement);
                search_from = abs_start + replacement.len();
            }
```

The first `let Some(...) else { break }` becomes the `while let` condition. The second one stays as `else { break }` inside the loop body.

- [ ] **Step 2: Verify clippy passes for this error**

```bash
cargo clippy -- -D clippy::while-let-loop 2>&1 | grep "while let"
```

Expected: no output.

---

### Task 6: Collapse nested `if let` in `llm.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs:108-121`

Clippy error: `this if let can be collapsed into the outer if let` (x2)

- [ ] **Step 1: Apply fix**

Change lines 107-123 from:

```rust
        for field in schema.values_mut() {
            // Resolve fixed value if it's a string
            if let Some(fixed) = field.fixed.as_mut() {
                if let Value::String(s) = fixed {
                    *s = Self::resolve_context_vars(s, inputs);
                }
            }

            // Recursively resolve in nested properties
            if let Some(properties) = field.properties.as_mut() {
                for nested_field in properties.values_mut() {
                    if let Some(fixed) = nested_field.fixed.as_mut() {
                        if let Value::String(s) = fixed {
                            *s = Self::resolve_context_vars(s, inputs);
                        }
                    }
                }
            }
        }
```

to:

```rust
        for field in schema.values_mut() {
            // Resolve fixed value if it's a string
            if let Some(Value::String(s)) = field.fixed.as_mut() {
                *s = Self::resolve_context_vars(s, inputs);
            }

            // Recursively resolve in nested properties
            if let Some(properties) = field.properties.as_mut() {
                for nested_field in properties.values_mut() {
                    if let Some(Value::String(s)) = nested_field.fixed.as_mut() {
                        *s = Self::resolve_context_vars(s, inputs);
                    }
                }
            }
        }
```

Both nested `if let` chains collapse into a single `if let Some(Value::String(s))` pattern.

- [ ] **Step 2: Verify clippy passes for this error**

```bash
cargo clippy -- -D clippy::collapsible-match 2>&1 | grep "collapsed"
```

Expected: no output.

---

### Task 7: Add `Default` impl for `SubGraphNode`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs:15`

Clippy error: `you should consider adding a Default implementation for SubGraphNode`

- [ ] **Step 1: Apply fix**

Add a `Default` impl before the existing `impl SubGraphNode` block. Insert after line 13 (closing brace of struct):

```rust
impl Default for SubGraphNode {
    fn default() -> Self {
        Self::new()
    }
}
```

The full code at lines 11-19 becomes:

```rust
pub struct SubGraphNode {
    pub executor: Arc<OnceLock<Arc<dyn SubGraphExecutorPort>>>,
}

impl Default for SubGraphNode {
    fn default() -> Self {
        Self::new()
    }
}

impl SubGraphNode {
    pub fn new() -> Self {
        Self { executor: Arc::new(OnceLock::new()) }
    }
}
```

- [ ] **Step 2: Verify clippy passes for this error**

```bash
cargo clippy -- -D clippy::new-without-default 2>&1 | grep "Default"
```

Expected: no output.

---

### Task 8: Fix doc-test in `tools.rs`

**Files:**
- Modify: `src/libs/colmena/src/llm/domain/tools.rs:17-21`

Error: `missing field 'pattern' in initializer of ParameterProperty`

- [ ] **Step 1: Apply fix**

The doc-test example creates a `ParameterProperty` struct literal but is missing the `pattern` field added later. Add `pattern: None` to the example.

Change the doc example from:

```rust
///     ParameterProperty {
///         property_type: "number".to_string(),
///         description: "First number".to_string(),
///         enum_values: None,
///     }
```

to:

```rust
///     ParameterProperty {
///         property_type: "number".to_string(),
///         description: "First number".to_string(),
///         enum_values: None,
///         pattern: None,
///     }
```

- [ ] **Step 2: Verify doc-test passes**

```bash
cargo test --doc 2>&1 | tail -5
```

Expected: `test result: ok. 1 passed; 0 failed; 2 ignored`

---

### Task 9: Full CI verification

- [ ] **Step 1: Run all three checks**

```bash
cargo fmt -- --check && echo "FMT OK" && cargo clippy -- -D warnings && echo "CLIPPY OK" && cargo test --verbose 2>&1 | tail -5
```

Expected: all three pass, 111+ tests pass, 0 failures.

- [ ] **Step 2: Run cargo fmt to ensure no formatting drift from code changes**

```bash
cargo fmt && cargo fmt -- --check
```

Expected: exits 0.

- [ ] **Step 3: Final commit**

```bash
git add -A
git commit -m "fix: resolve all clippy warnings and doc-test failure for CI"
```
