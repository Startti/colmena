---
title: LLM Temporal & Geographic Context Injection
date: 2026-05-12
status: approved
---

# LLM Temporal & Geographic Context Injection

## Problem

LLM nodes have no awareness of the current date, time, or user location. This causes agents to give stale or geographically incorrect responses when asked time-sensitive or location-sensitive questions. The model cannot know "what day is it today" or "what time is it in the user's city" without this context being injected at runtime.

## Goals

- Every `llm_call` node automatically receives the current local date and time at execution.
- Timezone and location are declared once at the graph root level — not repeated per node.
- The config only stores the IANA timezone string; the engine computes the actual time at runtime from the server clock.
- Default: `America/Bogota` / `Bogotá, Colombia`.

## Non-Goals

- Per-node timezone override via node config.
- Dynamic timezone override via input port from upstream nodes.
- Exposing timezone/location as node output ports.

## Design

### 1. Graph JSON Schema

Two new optional fields at the graph root, alongside `_comment`, `_test_instructions`, `nodes`, and `edges`:

```json
{
  "_comment": "...",
  "timezone": "America/Bogota",
  "location": "Bogotá, Colombia",
  "nodes": { ... },
  "edges": [ ... ]
}
```

- `timezone`: IANA timezone string (e.g. `"America/New_York"`, `"Europe/Madrid"`). Optional. Default: `"America/Bogota"`.
- `location`: free-text geographic description shown to the LLM (e.g. `"Bogotá, Colombia"`). Optional. Default: `"Bogotá, Colombia"`.

Both fields are optional. If omitted, the defaults apply automatically in the LLM node — no engine-level default injection needed.

### 2. Graph Struct (`domain/graph.rs`)

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

Serde ignores these fields for all existing graphs without them (backward compatible).

### 3. Engine Injection (`application/run_use_case.rs`)

In `execute_stream`, immediately after the existing `__colmena_session_id` / `__colmena_agent_session_id` injections (~line 409), inject two new special inputs into every node's input map:

```rust
if let Some(tz) = &graph.timezone {
    inputs.insert("__colmena_timezone".to_string(), Value::String(tz.clone()));
}
if let Some(loc) = &graph.location {
    inputs.insert("__colmena_location".to_string(), Value::String(loc.clone()));
}
```

These are injected into ALL nodes (not just `llm_call`) — non-LLM nodes simply ignore unknown inputs, so there is no impact.

### 4. LLM Node Runtime (`infrastructure/nodes/llm.rs`)

At the start of `execute`, after the existing field reads, resolve timezone and location:

```rust
let timezone_str = inputs
    .get("__colmena_timezone")
    .and_then(|v| v.as_str())
    .unwrap_or("America/Bogota");

let location_str = inputs
    .get("__colmena_location")
    .and_then(|v| v.as_str())
    .unwrap_or("Bogotá, Colombia");
```

Compute local time using `chrono` + `chrono-tz`:

```rust
use chrono::Utc;
use chrono_tz::Tz;

let tz: Tz = timezone_str
    .parse()
    .unwrap_or(chrono_tz::America::Bogota);  // fallback on invalid IANA string

let local_dt = Utc::now().with_timezone(&tz);
// Format offset "-05:00" → "UTC-5" (drop minutes if zero, drop leading zero from hour)
let raw_offset = local_dt.format("%:z").to_string(); // e.g. "-05:00" or "+05:30"
let offset_display = {
    let sign = if raw_offset.starts_with('-') { "-" } else { "+" };
    let parts: Vec<&str> = raw_offset.trim_start_matches(['+', '-']).split(':').collect();
    let hours: i32 = parts[0].parse().unwrap_or(0);
    let mins: i32 = parts.get(1).and_then(|m| m.parse().ok()).unwrap_or(0);
    if mins == 0 {
        format!("UTC{}{}", sign, hours)
    } else {
        format!("UTC{}{}:{:02}", sign, hours, mins)
    }
}; // e.g. "UTC-5" or "UTC+5:30"

let formatted = local_dt.format("%A, %B %-d, %Y, %-I:%M %p").to_string();
let context_block = format!(
    "## Temporal & Geographic Context\nCurrent date and time: {} ({}, {})\nUser location: {}",
    formatted, timezone_str, offset_display, location_str
);
```

### 5. System Message Assembly

The `context_block` is prepended as the **first section** in the system message assembly block (currently at [llm.rs:1108-1151](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs#L1108)):

```rust
// In the sections Vec<String> assembly:
let mut sections: Vec<String> = vec![context_block]; // always first
if let Some(sys_msg) = system_message {
    sections.push(sys_msg.to_string());
}
// ... documents prelude, tool instructions ...
```

The block is injected inside the existing `if !history_exists` guard — consistent with how all system message sections work. This means the temporal context reflects the time of the **first message** in a conversation; subsequent turns in a multi-turn conversation do not re-inject an updated timestamp. This is acceptable for v1: conversations are short-lived and the LLM can reason forward from the initial timestamp.

Sections are joined with `\n\n---\n` (existing convention).

**Example rendered system message prefix:**

```
## Temporal & Geographic Context
Current date and time: Tuesday, May 12, 2026, 10:34 AM (America/Bogota, UTC-5)
User location: Bogotá, Colombia

---

You are a travel expert specializing in Latin American destinations.
```

### 6. New Dependency

Add `chrono-tz` to `Cargo.toml`:

```toml
chrono-tz = "0.9"
```

`chrono` is already a direct dependency. `chrono-tz` provides the IANA timezone database at compile time — no network calls, no runtime files.

### 7. Fallback Behavior

| Condition | Result |
|-----------|--------|
| `timezone` omitted from graph | `"America/Bogota"` used |
| `location` omitted from graph | `"Bogotá, Colombia"` used |
| `timezone` present but invalid IANA string | Falls back to `America/Bogota`; no error |
| Graph has no LLM nodes | Fields are ignored silently |

### 8. Backward Compatibility

- Existing graphs without `timezone`/`location` fields: no change in behavior (serde `#[serde(default)]` = `None`, defaults apply in the LLM node).
- No breaking changes to the node API, input ports, or output ports.

## Files to Modify

| File | Change |
|------|--------|
| `src/libs/colmena/src/dag_engine/domain/graph.rs` | Add `timezone` and `location` fields to `Graph` |
| `src/libs/colmena/src/dag_engine/application/run_use_case.rs` | Inject `__colmena_timezone` / `__colmena_location` after session_id injections |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` | Read injected values, compute local time, prepend context block to system message |
| `src/libs/colmena/Cargo.toml` | Add `chrono-tz = "0.9"` |
| `docs/node_configurations.json` | Document `timezone` and `location` as graph-root fields |

## Test Graph

A new graph at `tests/graphs/agents/llm_temporal_context_test.json` with:
- `"timezone": "America/Bogota"` at root
- `"location": "Bogotá, Colombia"` at root
- One `llm_call` node with `prompt: "What day and time is it? Where am I located?"`
- Expected: LLM response reflects the correct local date/time and Bogotá location
