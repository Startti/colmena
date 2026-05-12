# `secure_suspend_allowed` Flag for `llm_call` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a boolean `secure_suspend_allowed` flag to the `llm_call` node config that, when `true`, auto-registers `secure_suspend` as a tool named `ask_secret` with canonical description + `node_schema` — collapsing today's 4-line `tool_configurations` block to a single line.

**Architecture:** A pure helper in `secure_suspend.rs` inspects the in-memory `tool_configurations` map and injects a synthetic entry when the flag is on and there is no conflicting entry. The existing `apply_secure_suspend_tool_defaults` loop in `llm.rs:741-743` then fills the description and `node_schema` as it already does for explicit entries — so the LLM sees an identical contract whether the user uses the flag or declares the tool manually. No behavior change for existing graphs.

**Tech Stack:** Rust 1.95.0, `serde_json`, `HashMap<String, ToolConfiguration>`, no new deps.

---

## File Structure

- **Create:** none
- **Modify:**
  - `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs` — add 2 pure helpers (`synthetic_secure_suspend_tool`, `maybe_inject_secure_suspend_tool`) + unit tests
  - `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` — read flag from `config`/`inputs`, call the new helper before the existing `apply_secure_suspend_tool_defaults` loop
  - `docs/node_configurations.json` — add `secure_suspend_allowed` field to the `llm_call` config schema
  - `docs/developer_guide/13_security_strategy.md` — document the flag as the recommended way to expose `secure_suspend` to an LLM
- **Add fixture:**
  - `tests/graphs/advanced/llm_tool_suspend_flag_smoke.json` — twin of `llm_tool_suspend_smoke.json` but using the flag instead of `tool_configurations`

**Responsibilities:**
- `secure_suspend.rs` owns everything about its tool shape (description, schema, synthetic ToolConfiguration constructor, injection rule). Keeps llm.rs slim — it just calls the helper.
- `llm.rs` owns reading the flag and ordering the injection-then-defaults sequence.
- `node_configurations.json` is the canonical user-facing schema.
- `13_security_strategy.md` is where users actually find the recommendation.

---

## Task 1: Helper `synthetic_secure_suspend_tool` + injection rule (TDD)

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`

Add two pure functions:
1. `synthetic_secure_suspend_tool(name: &str) -> ToolConfiguration` — builds a minimal `ToolConfiguration` that the existing `apply_secure_suspend_tool_defaults` will then fill.
2. `maybe_inject_secure_suspend_tool(flag, map)` — opts a synthetic entry in if and only if `flag == true` AND no existing entry has `node_type == "secure_suspend"` AND the target key (`"ask_secret"`) is not already taken by another tool.

This makes the rule trivially unit-testable without touching async LLM execution.

- [ ] **Step 1: Add failing test — flag false is a no-op**

In `secure_suspend.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn maybe_inject_secure_suspend_tool_noop_when_flag_false() {
    let mut map: std::collections::HashMap<String, ToolConfiguration> =
        std::collections::HashMap::new();
    maybe_inject_secure_suspend_tool(false, &mut map);
    assert!(map.is_empty(), "flag=false must not inject anything");
}
```

- [ ] **Step 2: Run test — confirm it fails to compile**

Run: `cargo test -p colmena_dag_engine --lib secure_suspend::tests::maybe_inject_secure_suspend_tool_noop_when_flag_false`
Expected: FAIL with `cannot find function maybe_inject_secure_suspend_tool`.

- [ ] **Step 3: Add three more failing tests covering injection and conflicts**

```rust
#[test]
fn maybe_inject_secure_suspend_tool_inserts_when_flag_true_and_no_conflict() {
    let mut map: std::collections::HashMap<String, ToolConfiguration> =
        std::collections::HashMap::new();
    maybe_inject_secure_suspend_tool(true, &mut map);
    let entry = map.get("ask_secret").expect("ask_secret entry must be inserted");
    assert_eq!(entry.name, "ask_secret");
    assert_eq!(entry.node_type, "secure_suspend");
    // description and node_schema are left empty — apply_secure_suspend_tool_defaults
    // fills them downstream. Verify the contract: empty here.
    assert!(entry.description.is_empty());
    assert!(entry.node_schema.is_none());
}

#[test]
fn maybe_inject_secure_suspend_tool_noop_when_user_already_declared_secure_suspend() {
    // User explicitly declared secure_suspend under a different alias — flag must NOT
    // duplicate it (would expose the same tool twice to the LLM).
    let mut map: std::collections::HashMap<String, ToolConfiguration> =
        std::collections::HashMap::new();
    map.insert(
        "ask_credentials".to_string(),
        synthetic_secure_suspend_tool("ask_credentials"),
    );
    maybe_inject_secure_suspend_tool(true, &mut map);
    assert_eq!(map.len(), 1, "must not inject when user already declared secure_suspend");
    assert!(map.contains_key("ask_credentials"));
    assert!(!map.contains_key("ask_secret"));
}

#[test]
fn maybe_inject_secure_suspend_tool_noop_when_ask_secret_key_taken_by_other_tool() {
    // Edge case: user has a tool named "ask_secret" but pointing to a different node.
    // Do not clobber — explicit wins.
    let mut map: std::collections::HashMap<String, ToolConfiguration> =
        std::collections::HashMap::new();
    let mut other = synthetic_secure_suspend_tool("ask_secret");
    other.node_type = "log".to_string();
    map.insert("ask_secret".to_string(), other);
    maybe_inject_secure_suspend_tool(true, &mut map);
    assert_eq!(map.len(), 1);
    assert_eq!(map.get("ask_secret").unwrap().node_type, "log", "must not clobber existing key");
}
```

- [ ] **Step 4: Run all four tests — confirm they fail**

Run: `cargo test -p colmena_dag_engine --lib secure_suspend::tests::maybe_inject`
Expected: 4 failures (compilation error — `synthetic_secure_suspend_tool` and `maybe_inject_secure_suspend_tool` not defined).

- [ ] **Step 5: Implement the two helpers**

In `secure_suspend.rs` (after `secure_suspend_tool_node_schema`, before the `Lazy<Regex>`):

```rust
/// Build a minimal synthetic `ToolConfiguration` for `secure_suspend`. The
/// `description` and `node_schema` fields are intentionally left empty —
/// callers are expected to run [`apply_secure_suspend_tool_defaults`] on the
/// resulting entry (the LLM node already does this for every entry in
/// `tool_configurations`, so injecting then deferring is the cheapest path).
pub fn synthetic_secure_suspend_tool(name: &str) -> ToolConfiguration {
    #[allow(deprecated)]
    ToolConfiguration {
        name: name.to_string(),
        description: String::new(),
        node_type: "secure_suspend".to_string(),
        fixed_config: std::collections::HashMap::new(),
        exposed_inputs: None,
        parameters: None,
        mergeable_fields: None,
        field_mapping: None,
        node_schema: None,
        node_config: None,
        expose_sub_tools: None,
        summary: None,
        eager: false,
    }
}

/// Inject a synthetic `ask_secret` tool into the given `tool_configurations`
/// map iff `flag` is true AND no existing entry already wires `secure_suspend`
/// AND the target key is free. Idempotent and conflict-safe — explicit user
/// declarations always win.
pub fn maybe_inject_secure_suspend_tool(
    flag: bool,
    tool_configurations: &mut std::collections::HashMap<String, ToolConfiguration>,
) {
    if !flag {
        return;
    }
    let already_declared = tool_configurations
        .values()
        .any(|tc| tc.node_type == "secure_suspend");
    if already_declared {
        return;
    }
    const INJECTED_KEY: &str = "ask_secret";
    if tool_configurations.contains_key(INJECTED_KEY) {
        return;
    }
    tool_configurations.insert(
        INJECTED_KEY.to_string(),
        synthetic_secure_suspend_tool(INJECTED_KEY),
    );
}
```

- [ ] **Step 6: Run all four tests — confirm they pass**

Run: `cargo test -p colmena_dag_engine --lib secure_suspend::tests::maybe_inject`
Expected: 4 passed, 0 failed.

- [ ] **Step 7: Run the full `secure_suspend` module test suite to catch regressions**

Run: `cargo test -p colmena_dag_engine --lib secure_suspend`
Expected: all existing tests still pass, plus the 4 new ones.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs
git commit -m "feat(secure_suspend): synthetic tool injection helpers

Adds maybe_inject_secure_suspend_tool + synthetic_secure_suspend_tool.
Pure injection rule: opt-in when flag is true and no explicit secure_suspend
entry collides. Wired into llm_call in the next commit."
```

---

## Task 2: Wire `secure_suspend_allowed` flag into `llm_call`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` around lines 718-743

Read the flag from `inputs` (override) and `config` (default), call the helper before the existing `apply_secure_suspend_tool_defaults` loop so the injected synthetic entry gets its description/`node_schema` filled by the same code path that fills user-provided entries.

- [ ] **Step 1: Locate the parsing block**

Read `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` lines 714-745. The block parses `tool_configurations` (lines 718-735) and then runs `apply_secure_suspend_tool_defaults` over every entry (lines 741-743). The new flag must be read between those two steps.

- [ ] **Step 2: Insert the flag read + injection call**

Replace this exact block (lines 737-743 in the current code):

```rust
        // Auto-fill canonical tool defaults for node types that ship them.
        // Currently only `secure_suspend` opts in — keeps `tool_configurations`
        // minimal (just `name` + `node_type`) and avoids forcing users to
        // duplicate the contract in their system_message.
        for tool_cfg in tool_configurations.values_mut() {
            crate::dag_engine::infrastructure::nodes::secure_suspend::apply_secure_suspend_tool_defaults(tool_cfg);
        }
```

with:

```rust
        // Opt-in shorthand: `config.secure_suspend_allowed: true` auto-registers
        // a tool named `ask_secret` backed by `secure_suspend`. No-op when the
        // flag is absent/false or when the user already wired `secure_suspend`
        // through `tool_configurations` (explicit always wins).
        let secure_suspend_allowed = inputs
            .get("secure_suspend_allowed")
            .or_else(|| config.get("secure_suspend_allowed"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        crate::dag_engine::infrastructure::nodes::secure_suspend::maybe_inject_secure_suspend_tool(
            secure_suspend_allowed,
            &mut tool_configurations,
        );

        // Auto-fill canonical tool defaults for node types that ship them.
        // Currently only `secure_suspend` opts in — keeps `tool_configurations`
        // minimal (just `name` + `node_type`) and fills defaults for any entry
        // injected by the `secure_suspend_allowed` shorthand above.
        for tool_cfg in tool_configurations.values_mut() {
            crate::dag_engine::infrastructure::nodes::secure_suspend::apply_secure_suspend_tool_defaults(tool_cfg);
        }
```

- [ ] **Step 3: Build to confirm no compile error**

Run: `cargo build -p colmena_dag_engine`
Expected: clean build (warnings-as-errors gate is on per `Cargo.toml [lints.rust]`).

- [ ] **Step 4: Run `llm` and `secure_suspend` unit tests**

Run: `cargo test -p colmena_dag_engine --lib llm && cargo test -p colmena_dag_engine --lib secure_suspend`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(llm_call): secure_suspend_allowed flag auto-registers ask_secret

When config.secure_suspend_allowed=true, the llm_call node injects an
ask_secret tool backed by secure_suspend. Explicit tool_configurations
entries take precedence (no override, no duplication)."
```

---

## Task 3: Update `docs/node_configurations.json` schema

**Files:**
- Modify: `docs/node_configurations.json` — insert `secure_suspend_allowed` next to `lazy_tool_loading` in the `llm_call` config schema (line 1207-1214 region)

- [ ] **Step 1: Add the field to the schema**

Open `docs/node_configurations.json`, find the `llm_call` config schema (search for `"lazy_tool_loading"`, around line 1207). Insert immediately AFTER the closing `}` of `lazy_tool_loading` (before the line `"tool_configurations": {`):

```json
        "secure_suspend_allowed": {
          "type": "boolean",
          "required": false,
          "default": false,
          "description": "When true, auto-registers a tool named `ask_secret` backed by the `secure_suspend` node. The LLM sees the canonical description and node_schema (a `secrets: [{question, name}]` array) without any entry in `tool_configurations`. Use this as the simplest way to let an agent ask the user for credentials. Explicit `tool_configurations` entries with `node_type: \"secure_suspend\"` take precedence — the flag is a no-op if the user already declared the tool. See developer_guide/13_security_strategy.md.",
          "example": true
        },
```

- [ ] **Step 2: Verify JSON is still valid**

Run: `jq -e '.' docs/node_configurations.json > /dev/null && echo OK`
Expected: `OK` (jq prints nothing on success; the `&& echo OK` confirms validity).

- [ ] **Step 3: Commit**

```bash
git add docs/node_configurations.json
git commit -m "docs(nodes): document secure_suspend_allowed flag in llm_call schema"
```

---

## Task 4: Add demonstration graph + end-to-end smoke

**Files:**
- Create: `tests/graphs/advanced/llm_tool_suspend_flag_smoke.json`

Mirrors `tests/graphs/advanced/llm_tool_suspend_smoke.json` but proves the flag works as a drop-in replacement for the explicit `tool_configurations` form. Same input prompt; expected behavior: the LLM calls `ask_secret`, the graph suspends asking for `usuario`/`password`, the resume reaches `log_result`.

- [ ] **Step 1: Create the file**

Write `tests/graphs/advanced/llm_tool_suspend_flag_smoke.json`:

```json
{
  "comment": "Twin of llm_tool_suspend_smoke.json that uses the `secure_suspend_allowed: true` shorthand instead of an explicit tool_configurations entry. The LLM should call `ask_secret` with the credentials needed for the requested login.",
  "metadata": {
    "category": "advanced",
    "features": [
      "llm_call",
      "secure_suspend",
      "secure_suspend_allowed"
    ],
    "requires_env": [
      "DATABASE_URL",
      "GEMINI_API_KEY"
    ]
  },
  "nodes": {
    "user_input": {
      "type": "input",
      "config": {
        "default": "Set up a connection — I need username and password to log in."
      }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "session_id": "llm_tool_suspend_flag_smoke",
        "connection_url": "${DATABASE_URL}",
        "temperature": 0.0,
        "stream": false,
        "secure_suspend_allowed": true,
        "system_message": "You collect credentials from the user. When the user asks you to set up any login or connection that needs secrets, call `ask_secret` exactly once with a list of `{question, name}` pairs covering ALL credentials you need. Do not chat about the secrets — just call the tool."
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "user_input", "to": "agent" },
    { "from": "agent.result", "to": "log_result" }
  ]
}
```

- [ ] **Step 2: Validate JSON syntax**

Run: `jq -e '.' tests/graphs/advanced/llm_tool_suspend_flag_smoke.json > /dev/null && echo OK`
Expected: `OK`.

- [ ] **Step 3: Live smoke run against Google Gemini**

Source the env first (so `GEMINI_API_KEY` and `DATABASE_URL` are set), then invoke:

```bash
set -a && source .env && set +a
cargo run --bin dag_engine -- run tests/graphs/advanced/llm_tool_suspend_flag_smoke.json --agent-session-id cmox2c4ba000n01s66tygjo3d
```

Expected: graph SUSPENDS waiting for the user; the suspend prompt enumerates two questions (one for user, one for password) with `Q[<id>]:` markers. The LLM emitted a `tool_call` for `ask_secret` with `secrets: [{question, name}, {question, name}]`.

- [ ] **Step 4: Resume with credentials, confirm the run completes**

Resume using the canonical Q/A format (the two `<name>`s come from what the LLM picked at Step 3 — read them from the suspend prompt printed by the CLI):

```bash
cargo run --bin dag_engine -- run tests/graphs/advanced/llm_tool_suspend_flag_smoke.json \
  --agent-session-id cmox2c4ba000n01s66tygjo3d \
  --answer "Q[<name1>]: <q1>
A[<name1>]: demo_user_value
Q[<name2>]: <q2>
A[<name2>]: demo_password_value"
```

Expected: graph completes; `log_result` shows the agent's final response. No real secret value appears in any output — only the opaque `<sv_<name>_<random>>` handles.

- [ ] **Step 5: Commit**

```bash
git add tests/graphs/advanced/llm_tool_suspend_flag_smoke.json
git commit -m "test(graphs): smoke for secure_suspend_allowed shorthand on llm_call"
```

---

## Task 5: Document the flag in the security guide

**Files:**
- Modify: `docs/developer_guide/13_security_strategy.md`

Add a short subsection presenting the flag as the recommended path; keep the explicit `tool_configurations` form documented for the rename / custom-alias case.

- [ ] **Step 1: Find the right anchor**

Run: `grep -n "secure_suspend\|tool_configurations" docs/developer_guide/13_security_strategy.md | head -20`

Identify the section where `secure_suspend` as a tool is described (it exists today — Task 5 only adds the flag as the recommended shorthand alongside it).

- [ ] **Step 2: Insert the recommendation**

Add a subsection (the exact heading depth should match the surrounding doc — bump or shrink as needed):

````markdown
### Exposing `secure_suspend` to an LLM — the `secure_suspend_allowed` shorthand

Set `secure_suspend_allowed: true` in the `llm_call` config. The engine
auto-registers a tool named `ask_secret` with the canonical description and
`node_schema` (`secrets: [{question, name}]` array). The LLM sees an identical
contract to declaring the tool by hand.

```json
"agent": {
  "type": "llm_call",
  "config": {
    "provider": "google",
    "model": "gemini-2.5-flash",
    "api_key": "${GEMINI_API_KEY}",
    "connection_url": "${DATABASE_URL}",
    "secure_suspend_allowed": true,
    "system_message": "..."
  }
}
```

**When to use the explicit form instead.** If you need to rename the tool
(`ask_credentials`, `request_secrets`, …) or co-locate it with extra
`tool_configurations` overrides, declare it the old way:

```json
"tool_configurations": {
  "ask_credentials": { "name": "ask_credentials", "node_type": "secure_suspend" }
}
```

Both paths converge on the same canonical defaults — `apply_secure_suspend_tool_defaults`
fills `description` and `node_schema` for whichever entry exists in the map.

**Precedence.** If `tool_configurations` already contains an entry with
`node_type: "secure_suspend"`, the flag is a no-op (no duplication).
````

- [ ] **Step 3: Commit**

```bash
git add docs/developer_guide/13_security_strategy.md
git commit -m "docs(security): document secure_suspend_allowed shorthand on llm_call"
```

---

## Task 6: Final integration sweep

- [ ] **Step 1: Run the full lib test suite**

Run: `cargo test -p colmena_dag_engine --lib`
Expected: all green, no new failures vs. the baseline before this branch.

- [ ] **Step 2: Run clippy on the touched files**

Run: `cargo clippy -p colmena_dag_engine --lib -- -D warnings 2>&1 | tail -40`
Expected: no NEW warnings in `secure_suspend.rs` or `llm.rs`. Pre-existing warnings in untouched files are out of scope (documented in CLAUDE.md as known noise on this branch).

- [ ] **Step 3: Run `cargo fmt` on touched files**

Run: `cargo fmt -- src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
Expected: no diff, or a clean fmt diff if rust-fmt re-indented anything.

- [ ] **Step 4: If fmt produced changes, commit them**

```bash
git diff --quiet || (git add -u && git commit -m "style: cargo fmt drift from secure_suspend_allowed wiring")
```

---

## Out of Scope (Explicit Non-Goals)

- Extending the flag to other LLM-backed nodes (`critic`, `planner`, `reactor`, `orchestrator`, `extraction`). Only `llm_call` for now; a follow-up plan can copy the same 3-line shim to each of those if usage justifies it.
- Configurable tool name via flag. The flag injects exactly `ask_secret`. Renaming requires the explicit `tool_configurations` form.
- Validation that `connection_url` is set when the flag is true. `secure_suspend` already errors at runtime if Postgres is unreachable — no need to duplicate the check at parse time.
- Python / Node bindings. Neither surface changes (`secure_suspend_allowed` lives in graph JSON, not in any binding API).
