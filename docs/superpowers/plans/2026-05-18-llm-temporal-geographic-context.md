# LLM Temporal & Geographic Context — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Inject the current local date/time and the user's geographic location into every `llm_call`'s system message automatically. The graph author declares `timezone` (IANA string) and `location` (free-text) ONCE at the graph root; the engine threads them down and the LLM node formats the runtime block. Default: `America/Bogota` / `Bogotá, Colombia`.

**Architecture:** Three coordinated changes:

1. **Domain.** Two new optional fields on the `Graph` struct (`timezone: Option<String>`, `location: Option<String>`). Backward-compatible via `#[serde(default)]`.
2. **Engine.** In `run_use_case::execute_stream`, after the existing session-id injections (~line 415), inject `__colmena_timezone` and `__colmena_location` into every node's inputs. Non-LLM nodes ignore unknown inputs.
3. **LLM node.** A new private helper `format_temporal_context_block(timezone_str, location_str) -> String` parses the IANA string (with fallback to `America/Bogota` on invalid input), computes `Utc::now().with_timezone(&tz)`, and produces the formatted block. The block is prepended as the FIRST section of the system message inside the existing `if !history_exists` guard.

**Tech Stack:** Rust 1.95, `chrono` (already a dep), `chrono-tz` 0.9 (new — provides the IANA database at compile time, no runtime files), serde.

**Spec:** [docs/superpowers/specs/2026-05-12-llm-temporal-geographic-context-design.md](../specs/2026-05-12-llm-temporal-geographic-context-design.md)

**Design choices resolved against the spec while writing this plan:**

1. **Invalid IANA string handling.** Spec §7 says "Falls back to `America/Bogota`; no error". When the fallback triggers, this plan **also rewrites the displayed timezone name** to `"America/Bogota"` so the rendered block stays internally coherent (the displayed timezone matches the offset). Without this, a user typo like `"Mars/Olympus"` would render `Current date and time: … (Mars/Olympus, UTC-5)` which mixes a garbage label with the Bogotá offset.
2. **Where the block is computed.** Spec §4 shows the computation at the top of `execute`. Per spec §5, the block is only used inside the `if !history_exists` guard (first turn of a conversation). This plan **moves the computation INSIDE the guard** so multi-turn / resume paths don't recompute a value they will not use. Behavior is identical for first-turn flows.

---

## File Structure

**Modified:**

```
src/libs/colmena/Cargo.toml
  └─ add chrono-tz = "0.9" to [dependencies]

src/libs/colmena/src/dag_engine/domain/graph.rs
  └─ add `timezone: Option<String>` and `location: Option<String>` to Graph

src/libs/colmena/src/dag_engine/application/run_use_case.rs
  └─ inject __colmena_timezone and __colmena_location after the session_id injections

src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
  ├─ private helper format_temporal_context_block(tz, loc) -> String
  ├─ inline tests for the helper (#[cfg(test)] mod)
  └─ wire helper into the system_message assembly inside the `!history_exists` guard

docs/node_configurations.json
  └─ document `timezone` and `location` as graph-root fields
```

**New:**

```
tests/graphs/agents/llm_temporal_context_test.json    # smoke graph
```

---

## Task 1: Add `chrono-tz` dependency

**Files:**
- Modify: `src/libs/colmena/Cargo.toml`

- [ ] **Step 1: Add the dep**

Open `src/libs/colmena/Cargo.toml`. Locate the `[dependencies]` table. Find the existing `chrono` line. Add directly below it:

```toml
chrono-tz = "0.9"
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo build -p colmena_dag_engine
```

Expected: `Finished dev profile`. Pulls down `chrono-tz` and its `phf` (perfect hash function) transitive dep. Zero warnings (deny-warnings active).

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/Cargo.toml src/libs/colmena/Cargo.lock
# Note: the workspace Cargo.lock lives at repo root, not under src/libs/colmena/.
git add Cargo.lock
git commit -m "deps(temporal-context): add chrono-tz for IANA timezone support"
```

---

## Task 2: Extend `Graph` struct with `timezone` and `location`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/graph.rs`

- [ ] **Step 1: Read the current struct to find the right spot**

```bash
grep -n "pub struct Graph" src/libs/colmena/src/dag_engine/domain/graph.rs
```

Expected: a single hit. The struct currently has at least `nodes` and `edges` fields with a `#[derive(Debug, Deserialize, Serialize, Clone)]` annotation.

- [ ] **Step 2: Write a failing test**

Append a `#[cfg(test)]` test (or add to an existing one) at the bottom of the file:

```rust
#[cfg(test)]
mod temporal_context_tests {
    use super::*;

    #[test]
    fn graph_without_timezone_location_parses_with_none_defaults() {
        let json = r#"{
            "nodes": {},
            "edges": []
        }"#;
        let g: Graph = serde_json::from_str(json).expect("must parse");
        assert!(g.timezone.is_none(), "expected timezone None when omitted");
        assert!(g.location.is_none(), "expected location None when omitted");
    }

    #[test]
    fn graph_with_timezone_and_location_parses_them() {
        let json = r#"{
            "nodes": {},
            "edges": [],
            "timezone": "Europe/Madrid",
            "location": "Madrid, España"
        }"#;
        let g: Graph = serde_json::from_str(json).expect("must parse");
        assert_eq!(g.timezone.as_deref(), Some("Europe/Madrid"));
        assert_eq!(g.location.as_deref(), Some("Madrid, España"));
    }
}
```

- [ ] **Step 3: Run the tests — they must FAIL**

```bash
cargo test -p colmena_dag_engine --lib temporal_context_tests
```

Expected: **FAIL with a compile error** — `Graph` does not have `timezone` / `location` fields.

- [ ] **Step 4: Add the fields**

Find the `Graph` struct. It looks roughly like:

```rust
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Graph {
    pub nodes: HashMap<String, NodeConfig>,
    pub edges: Vec<Edge>,
}
```

Add the two fields:

```rust
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Graph {
    pub nodes: HashMap<String, NodeConfig>,
    pub edges: Vec<Edge>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
}
```

> If the existing struct already has other fields (e.g. `_comment` for documentation), add timezone/location at the END of the field list to minimize diff churn. The `#[serde(default)]` makes them parse from JSON that omits them (= `None`).

- [ ] **Step 5: Run tests — must PASS**

```bash
cargo test -p colmena_dag_engine --lib temporal_context_tests
```

Expected: 2 passed.

- [ ] **Step 6: Full lib suite for regression check**

```bash
cargo test -p colmena_dag_engine --lib
```

Expected: ~750+ passed, 0 failed. Existing graph tests still pass because serde is backward-compatible.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/graph.rs
git commit -m "feat(graph): optional timezone and location at graph root"
```

---

## Task 3: Private helper `format_temporal_context_block` in `llm.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

This task adds the pure helper function that does the parsing + formatting. It's tested in isolation with TDD before being wired into `execute` in Task 5.

- [ ] **Step 1: Write the failing tests**

Append a new test module at the bottom of `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (after any existing `#[cfg(test)]` blocks):

```rust
#[cfg(test)]
mod temporal_context_helper_tests {
    use super::*;

    #[test]
    fn formats_block_for_bogota() {
        let out = format_temporal_context_block("America/Bogota", "Bogotá, Colombia");
        assert!(out.starts_with("## Temporal & Geographic Context"), "missing header: {}", out);
        assert!(out.contains("America/Bogota"), "missing tz name: {}", out);
        assert!(out.contains("UTC-5"), "missing UTC-5 offset: {}", out);
        assert!(out.contains("User location: Bogotá, Colombia"), "missing location: {}", out);
    }

    #[test]
    fn formats_block_for_madrid() {
        let out = format_temporal_context_block("Europe/Madrid", "Madrid, España");
        assert!(out.contains("Europe/Madrid"), "missing tz: {}", out);
        // Madrid is UTC+1 in winter, UTC+2 in summer — accept either
        assert!(out.contains("UTC+1") || out.contains("UTC+2"), "missing UTC+1/+2: {}", out);
        assert!(out.contains("User location: Madrid, España"), "missing location: {}", out);
    }

    #[test]
    fn formats_block_for_half_hour_offset() {
        // India Standard Time is UTC+5:30
        let out = format_temporal_context_block("Asia/Kolkata", "Mumbai, India");
        assert!(out.contains("UTC+5:30"), "expected UTC+5:30 in: {}", out);
    }

    #[test]
    fn invalid_iana_falls_back_to_bogota_coherently() {
        let out = format_temporal_context_block("Mars/Olympus", "Mars Base");
        // Fallback rewrites the timezone label so the offset matches:
        assert!(out.contains("America/Bogota"), "fallback tz label missing: {}", out);
        assert!(out.contains("UTC-5"), "fallback offset (Bogota) missing: {}", out);
        // Caller-supplied location is preserved verbatim
        assert!(out.contains("User location: Mars Base"), "location lost: {}", out);
    }

    #[test]
    fn output_contains_day_of_week_and_full_month() {
        // Loose smoke test: not asserting the actual date, just that the format
        // string produced something with a comma after a weekday-like token.
        let out = format_temporal_context_block("UTC", "London, UK");
        // %A produces full weekday name; %B produces full month name. Both
        // English by chrono default. We assert one comma after weekday at minimum.
        let body = out.lines().nth(1).unwrap_or(""); // "Current date and time: ..."
        let comma_count = body.matches(',').count();
        assert!(
            comma_count >= 2,
            "expected at least 2 commas in '{}', got {}",
            body,
            comma_count
        );
    }
}
```

- [ ] **Step 2: Run tests — they must FAIL (function does not exist)**

```bash
cargo test -p colmena_dag_engine --lib temporal_context_helper_tests
```

Expected: **FAIL with compile error** — `cannot find function format_temporal_context_block`.

- [ ] **Step 3: Implement the helper**

Add this private function near the top of `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, alongside other module-scope helpers (e.g., `generate_one_summary` from earlier work). If there is a clear "helpers" section, group it there:

```rust
/// Format the temporal & geographic context block that goes at the top of
/// the LLM system message. Parses `timezone_str` as an IANA name; on invalid
/// input, falls back to `America/Bogota` AND rewrites the displayed timezone
/// label so the block stays internally coherent (label matches offset).
/// `location_str` is taken verbatim — no validation, no fallback.
fn format_temporal_context_block(timezone_str: &str, location_str: &str) -> String {
    use chrono::Utc;
    use chrono_tz::Tz;

    // Parse IANA. On failure, fall back to Bogotá AND rewrite the displayed
    // label so the user sees a coherent (label, offset) pair.
    let (tz, tz_display) = match timezone_str.parse::<Tz>() {
        Ok(tz) => (tz, timezone_str.to_string()),
        Err(_) => (chrono_tz::America::Bogota, "America/Bogota".to_string()),
    };

    let local_dt = Utc::now().with_timezone(&tz);

    // Format offset as "UTC-5" / "UTC+5:30" (drop ":00" minutes; drop leading
    // zero from hour count).
    let raw_offset = local_dt.format("%:z").to_string(); // "-05:00", "+05:30", "+00:00", ...
    let sign = if raw_offset.starts_with('-') { "-" } else { "+" };
    let trimmed = raw_offset.trim_start_matches(['+', '-']);
    let parts: Vec<&str> = trimmed.split(':').collect();
    let hours: i32 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let mins: i32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let offset_display = if mins == 0 {
        format!("UTC{}{}", sign, hours)
    } else {
        format!("UTC{}{}:{:02}", sign, hours, mins)
    };

    let formatted = local_dt.format("%A, %B %-d, %Y, %-I:%M %p").to_string();
    format!(
        "## Temporal & Geographic Context\nCurrent date and time: {} ({}, {})\nUser location: {}",
        formatted, tz_display, offset_display, location_str
    )
}
```

> Notes:
> - `%-d` and `%-I` use the GNU/glibc-style "no leading zero" flag. `chrono` supports this on all platforms regardless of the underlying libc.
> - `chrono_tz::America::Bogota` is a const-like type provided by `chrono-tz`'s generated module — no runtime parsing of "America/Bogota" on the fallback path.

- [ ] **Step 4: Run tests — they must PASS**

```bash
cargo test -p colmena_dag_engine --lib temporal_context_helper_tests
```

Expected: 5 passed.

- [ ] **Step 5: Full lib suite for regression**

```bash
cargo test -p colmena_dag_engine --lib
```

Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(temporal-context): pure helper to format the IANA context block"
```

---

## Task 4: Inject `__colmena_timezone` / `__colmena_location` in `run_use_case`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`

- [ ] **Step 1: Locate the existing session-id injection**

```bash
grep -n "__colmena_agent_session_id\|__colmena_node_id_path" src/libs/colmena/src/dag_engine/application/run_use_case.rs | head -5
```

You should find the block (~line 410-420) that does:

```rust
inputs.insert("__node_id".to_string(), Value::String(node_id.clone()));
inputs.insert(
    "__colmena_node_id_path".to_string(),
    Value::String(node_id_path.clone()),
);
if let Some(a) = active_agent_session_id.as_deref() {
    inputs.insert(
        "__colmena_agent_session_id".to_string(),
        Value::String(a.to_string()),
    );
}
```

This is the per-node input assembly inside `execute_stream`. We add two more inserts immediately after.

- [ ] **Step 2: Add the timezone/location inserts**

Right after the `if let Some(a) = active_agent_session_id ...` block (before the GLOBAL SHARED STATE comment / loop), insert:

```rust
        if let Some(tz) = graph.timezone.as_deref() {
            inputs.insert(
                "__colmena_timezone".to_string(),
                Value::String(tz.to_string()),
            );
        }
        if let Some(loc) = graph.location.as_deref() {
            inputs.insert(
                "__colmena_location".to_string(),
                Value::String(loc.to_string()),
            );
        }
```

> Note: `graph` is the borrowed `Graph` value already in scope at this point in `execute_stream`. If the variable is named differently in the actual code (e.g. `g`, `dag_graph`), adjust accordingly — read the surrounding lines for context. The two new fields you added in Task 2 are `Option<String>`.

- [ ] **Step 3: Add an integration-style test**

Write a small test that constructs a Graph with `timezone` + `location` set, runs the engine for one trivial node (e.g. a `log` node), and confirms that the node sees the injected keys in its inputs.

Realistically the cleanest place is a `#[cfg(test)]` block at the end of `run_use_case.rs`. If the file already has one with helpers (look for `mod tests`), extend it. Otherwise create one. The exact test setup mirrors any existing engine-level test — search for `execute_stream` in test contexts:

```bash
grep -n "execute_stream\|engine.execute" src/libs/colmena/src/dag_engine/application/run_use_case.rs | head -10
```

If the existing tests in this file don't drive `execute_stream` end-to-end, **skip this step** and rely on the unit tests from Task 3 plus the smoke graph from Task 6 for coverage. Add a TODO comment in the new injection block referencing this:

```rust
        // TODO: cover via integration test when execute_stream gains a test harness.
        if let Some(tz) = graph.timezone.as_deref() {
```

- [ ] **Step 4: Build & full lib suite**

```bash
cargo build -p colmena_dag_engine
cargo test -p colmena_dag_engine --lib
```

Expected: clean build, full suite still green (no behavioral change yet — non-LLM nodes ignore the new inputs, and `llm.rs` isn't reading them yet either).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/application/run_use_case.rs
git commit -m "feat(engine): inject __colmena_timezone and __colmena_location into node inputs"
```

---

## Task 5: Wire the helper into `LlmNode::execute`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

This is the user-visible integration. The helper from Task 3 + the inputs from Task 4 finally meet here.

- [ ] **Step 1: Locate the system message assembly**

```bash
grep -n "history_exists\|sections.*Vec<String>" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs | head -10
```

Find the `if !history_exists` (or equivalent guard) block where the existing `system_message`, attachments prelude, etc., are assembled into a `sections: Vec<String>`. That's the spot.

- [ ] **Step 2: Read the two injected inputs and prepend the context block**

Inside the `if !history_exists` block, at the very top (before any other `sections.push(...)`), add:

```rust
            // Temporal & geographic context — always the first section.
            let tz_str = inputs
                .get("__colmena_timezone")
                .and_then(|v| v.as_str())
                .unwrap_or("America/Bogota");
            let loc_str = inputs
                .get("__colmena_location")
                .and_then(|v| v.as_str())
                .unwrap_or("Bogotá, Colombia");
            let context_block = format_temporal_context_block(tz_str, loc_str);
            sections.push(context_block);
```

> If the existing assembly initializes `sections` as `let mut sections: Vec<String> = Vec::new();` and immediately pushes `system_message`, insert the context block push BEFORE the `system_message` push so the temporal context is sections[0].
>
> If the existing assembly is `let mut sections = vec![system_message.to_string()]` (sections[0] = system_message), refactor minimally so the context block goes first.

- [ ] **Step 3: Build & full lib suite for regression**

```bash
cargo build -p colmena_dag_engine
cargo test -p colmena_dag_engine --lib
```

Expected: clean build, full suite green. The helper unit tests from Task 3 still pass; nothing else regresses.

- [ ] **Step 4: Optional sanity check on system message rendering**

If `llm.rs` has any integration test that captures the assembled system message (or `agent_service` mock test), add an assertion that the rendered system message starts with `"## Temporal & Geographic Context"`. If no such test exists, defer to the smoke graph in Task 6.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(llm_call): prepend temporal & geographic context to system message"
```

---

## Task 6: Smoke test graph

**Files:**
- Create: `tests/graphs/agents/llm_temporal_context_test.json`

- [ ] **Step 1: Write the graph**

Create `tests/graphs/agents/llm_temporal_context_test.json`:

```json
{
  "_comment": "Smoke test for LLM temporal & geographic context injection. Run with `source .env && cargo run --bin dag_engine -- run tests/graphs/agents/llm_temporal_context_test.json`. Expected: the assistant message references the current date, time, timezone (America/Bogota / UTC-5) and the user location (Bogotá, Colombia).",
  "timezone": "America/Bogota",
  "location": "Bogotá, Colombia",
  "nodes": {
    "ask": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "system_message": "You are a helpful local assistant. Answer using the contextual information you have.",
        "prompt": "¿Qué fecha y hora es ahora? ¿Dónde estoy ubicado? Responde brevemente."
      }
    },
    "out": { "type": "log" }
  },
  "edges": [
    { "from": "ask", "to": "out" }
  ]
}
```

- [ ] **Step 2: Validate JSON**

```bash
python3 -c "import json; json.load(open('tests/graphs/agents/llm_temporal_context_test.json')); print('valid')"
```

Expected: `valid`.

- [ ] **Step 3: Optional — run end-to-end (requires `.env` with GEMINI_API_KEY)**

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/llm_temporal_context_test.json
```

Expected: the assistant's response mentions the current date/time and Bogotá. (If you skip this step and only commit the graph file, the next person to run it will exercise the path.)

- [ ] **Step 4: Commit**

```bash
git add tests/graphs/agents/llm_temporal_context_test.json
git commit -m "test(temporal-context): smoke graph (Gemini Flash + Bogotá)"
```

---

## Task 7: Document graph-root fields + index entry

**Files:**
- Modify: `docs/node_configurations.json`
- Modify: `docs/DEVELOPER_GUIDE.md`

- [ ] **Step 1: Add `timezone` and `location` to `node_configurations.json`**

The file documents node-level config today. Graph-root fields are NOT under any specific node — they go in a top-level section. Open `docs/node_configurations.json` and locate the top of the file (look for `"graph_root_fields"` or, if it doesn't exist, the first node entry). If a `"graph_root_fields"` section exists, append the two new fields there. If not, create the section at the top.

Skeleton (if creating the section):

```json
  "graph_root_fields": {
    "name": "Graph Root Fields",
    "description": "Optional fields declared at the top level of the graph JSON, alongside `nodes` and `edges`. These propagate to every node via the engine input layer (under the `__colmena_*` namespace).",
    "fields": {
      "timezone": {
        "type": "string",
        "required": false,
        "default": "America/Bogota",
        "description": "IANA timezone string used by llm_call to render the temporal context block at the top of the system message. Invalid IANA strings silently fall back to America/Bogota.",
        "example": "Europe/Madrid"
      },
      "location": {
        "type": "string",
        "required": false,
        "default": "Bogotá, Colombia",
        "description": "Free-text geographic location shown to the LLM in the temporal context block. No validation, no fallback (used verbatim).",
        "example": "Madrid, España"
      }
    }
  },
```

> Read the file first to match the actual JSON structure conventions (some sections might use different key names). If `graph_root_fields` already exists, just merge the two new fields into the existing `fields` map.

- [ ] **Step 2: Validate JSON**

```bash
python3 -c "import json; json.load(open('docs/node_configurations.json')); print('valid')"
```

Expected: `valid`.

- [ ] **Step 3: Add a brief note to `docs/DEVELOPER_GUIDE.md` if relevant**

Open `docs/DEVELOPER_GUIDE.md`. If there is already an "LLM Node" entry (entry #16, "Deep Dive: Nodo LLM"), do NOT modify it — the temporal context is automatic and graph-author-facing rather than node-implementation-facing.

If there is no graph-root-fields entry, append a brief paragraph at the appropriate spot (after entry #34 or similar):

```markdown
35. [**Temporal & Geographic Context**](./superpowers/specs/2026-05-12-llm-temporal-geographic-context-design.md): Campos opcionales `timezone` (IANA string) y `location` (texto libre) al root del graph JSON. El motor los inyecta en cada `llm_call` y el nodo prepende un bloque `## Temporal & Geographic Context` con la fecha/hora local, offset UTC y la ubicación al inicio del system message. Defaults: `America/Bogota` / `Bogotá, Colombia`. IANA inválido cae a Bogotá silenciosamente. La spec linkeada documenta el diseño completo.
```

> If the developer guide has a different numbering or structure than expected, just append at the bottom — consistency over precision.

- [ ] **Step 4: Commit**

```bash
git add docs/node_configurations.json docs/DEVELOPER_GUIDE.md
git commit -m "docs(temporal-context): document timezone/location graph-root fields"
```

---

## Final verification

- [ ] **Step 1: Full test sweep**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p colmena_dag_engine --verbose
```

Expected: all green (including doctests).

- [ ] **Step 2: Confirm the chain of commits**

```bash
git log --oneline | head -10
```

Expected (in some order):

```
docs(temporal-context): document timezone/location graph-root fields
test(temporal-context): smoke graph (Gemini Flash + Bogotá)
feat(llm_call): prepend temporal & geographic context to system message
feat(engine): inject __colmena_timezone and __colmena_location into node inputs
feat(temporal-context): pure helper to format the IANA context block
feat(graph): optional timezone and location at graph root
deps(temporal-context): add chrono-tz for IANA timezone support
```

- [ ] **Step 3: Optional — run the smoke graph end-to-end**

If GEMINI_API_KEY is available:

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/llm_temporal_context_test.json
```

Confirm the assistant's response references the local date, time, and Bogotá location.

---

## Open caveats for the implementer

- **`chrono_tz::America::Bogota` import path.** `chrono-tz` exposes timezones via auto-generated constants under `chrono_tz::<Region>::<City>`. If the build complains about the path, fall back to parsing the literal string: `"America/Bogota".parse::<Tz>().expect("hardcoded literal must parse")`. Slightly slower (parsing at every fallback) but works regardless of the crate's macro internals.
- **`%-d` / `%-I` portability.** These format flags are GNU-specific in C strftime but `chrono` provides them on all platforms via its own impl. If you ever see them rendering as `%-d` literal, switch to `%d` / `%I` (will show `01`, `07`, etc. — acceptable, less polished).
- **Existing tests with hardcoded system_message snapshots.** Any test that does an exact-string match on the rendered system message will fail because the temporal context block is now sections[0]. Search for `expect_system_message`, `assert_eq!(.*system`, etc. before Task 5 — adjust those tests to use `contains` or to skip the temporal block prefix.
- **Mock LLM tests + time.** The smoke graph in Task 6 hits the real API. Unit tests for the helper (Task 3) use `Utc::now()` which is non-deterministic — that's fine because we only assert presence of substrings, never exact times. Do NOT mock the clock unless a future test explicitly needs determinism.
- **`%A` / `%B` locale.** `chrono` produces English weekday and month names by default. The user-visible language is English in the temporal block; if Spanish is needed later, add a `locale: Option<String>` graph-root field and switch via `chrono`'s locale features. Out of scope for v1.
