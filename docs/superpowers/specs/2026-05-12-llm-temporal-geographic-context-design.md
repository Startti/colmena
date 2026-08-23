---
title: LLM Temporal & Geographic Context Injection
date: 2026-05-12
revised: 2026-05-18
status: approved
---

# LLM Temporal & Geographic Context Injection

## Problem

LLM nodes have no awareness of the current date, time, user location, or user language. This causes agents to give stale, geographically incorrect, or wrong-language responses when asked time-sensitive, location-sensitive, or language-sensitive questions. The model cannot know "what day is it today", "what time is it in the user's city", or "should I answer in Spanish or English" without this context being injected at runtime.

## Goals

- Every `llm_call` node automatically receives the current local date and time at execution.
- Timezone, location, and locale are declared once at the graph root level — not repeated per node.
- The config only stores IANA / BCP 47 strings; the engine computes the actual time at runtime from the server clock.
- The rendered datetime uses **ISO 8601** as the primary format (machine-friendly, no locale ambiguity) with a human-readable echo in parentheses so the model can echo it back naturally.
- The rendered locale is **BCP 47** (`es-CO`, `en-US`, …) so the LLM can pick the right response language.
- Defaults: `America/Bogota` / `Bogotá, Colombia` / `es-CO`.

## Non-Goals

- Per-node timezone / locale override via node config.
- Dynamic timezone / locale override via input port from upstream nodes.
- Exposing timezone / location / locale as node output ports.
- Automatic locale-aware formatting of dates inside the rendered block (ISO 8601 is locale-neutral by design).

## Standards baseline

This revision was added on 2026-05-18 after an audit of industry practice (see [docs/CHANGELOG_2026-05.md](../../CHANGELOG_2026-05.md) Gap #2 follow-up). Key sources:

- **Date/time format:** ISO 8601 (`YYYY-MM-DDTHH:MM:SS±HH:MM`). This is what Anthropic Claude (web/mobile) injects in its own system prompt, and what production LLM applications use to avoid `M/D/Y` vs `D/M/Y` ambiguity (Damián Galarza, *How to Fix LLM Date and Time Issues in Production*, 2026-01).
- **Timezone:** IANA timezone database identifiers (`America/Bogota`). Universal standard, used by everything from Linux to JavaScript `Intl`.
- **Locale:** BCP 47 IETF language tags (`es-CO`, `en-US`, `pt-BR`). The formal standard for "language + region" identifiers, used by iOS, Android, browsers, Microsoft, and the Unicode CLDR ecosystem.

## Design

### 1. Graph JSON Schema

Three new optional fields at the graph root, alongside `_comment`, `_test_instructions`, `nodes`, and `edges`:

```json
{
  "_comment": "...",
  "timezone": "America/Bogota",
  "location": "Bogotá, Colombia",
  "locale": "es-CO",
  "nodes": { ... },
  "edges": [ ... ]
}
```

- `timezone`: IANA timezone string (e.g. `"America/New_York"`, `"Europe/Madrid"`). Optional. Default: `"America/Bogota"`.
- `location`: free-text geographic description shown to the LLM (e.g. `"Bogotá, Colombia"`). Optional. Default: `"Bogotá, Colombia"`.
- `locale`: BCP 47 language+region tag (e.g. `"es-CO"`, `"en-US"`, `"pt-BR"`). Optional. Default: `"es-CO"`. Tells the LLM what language to respond in independently of the location string.

All three fields are optional. If omitted, the defaults apply automatically in the LLM node — no engine-level default injection needed.

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
    #[serde(default)]
    pub locale: Option<String>,
}
```

Serde ignores these fields for all existing graphs without them (backward compatible).

### 3. Engine Injection (`application/run_use_case.rs`)

In `execute_stream`, immediately after the existing `__colmena_session_id` / `__colmena_agent_session_id` injections (~line 409), inject three new special inputs into every node's input map:

```rust
if let Some(tz) = &graph.timezone {
    inputs.insert("__colmena_timezone".to_string(), Value::String(tz.clone()));
}
if let Some(loc) = &graph.location {
    inputs.insert("__colmena_location".to_string(), Value::String(loc.clone()));
}
if let Some(lc) = &graph.locale {
    inputs.insert("__colmena_locale".to_string(), Value::String(lc.clone()));
}
```

These are injected into ALL nodes (not just `llm_call`) — non-LLM nodes simply ignore unknown inputs, so there is no impact.

### 4. LLM Node Runtime (`infrastructure/nodes/llm.rs`)

Inside the `if !history_exists` guard (so the block is computed only when it's actually consumed), resolve the three injected inputs:

```rust
let timezone_str = inputs
    .get("__colmena_timezone")
    .and_then(|v| v.as_str())
    .unwrap_or("America/Bogota");

let location_str = inputs
    .get("__colmena_location")
    .and_then(|v| v.as_str())
    .unwrap_or("Bogotá, Colombia");

let locale_str = inputs
    .get("__colmena_locale")
    .and_then(|v| v.as_str())
    .unwrap_or("es-CO");
```

Compute local time using `chrono` + `chrono-tz` and render BOTH ISO 8601 (primary) and a human-readable form (parenthesised echo):

```rust
use chrono::Utc;
use chrono_tz::Tz;

// Parse IANA; on invalid input, fall back to Bogotá AND rewrite the
// displayed label so (label, offset) stay coherent.
let (tz, tz_display) = match timezone_str.parse::<Tz>() {
    Ok(tz) => (tz, timezone_str.to_string()),
    Err(_) => (chrono_tz::America::Bogota, "America/Bogota".to_string()),
};

let local_dt = Utc::now().with_timezone(&tz);

// ISO 8601 (canonical, machine-friendly, locale-neutral):
//   "2026-05-17T10:34:00-05:00"
let iso_8601 = local_dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string();

// Human-readable echo for the LLM to naturally surface in its replies:
//   "Tuesday, May 17, 2026, 10:34 AM"
let human = local_dt.format("%A, %B %-d, %Y, %-I:%M %p").to_string();

// Offset display: "UTC-5" / "UTC+5:30" (drop ":00" minutes; drop leading
// zero from hour count).
let raw_offset = local_dt.format("%:z").to_string();
let sign = if raw_offset.starts_with('-') { "-" } else { "+" };
let parts: Vec<&str> = raw_offset.trim_start_matches(['+', '-']).split(':').collect();
let hours: i32 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
let mins: i32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
let offset_display = if mins == 0 {
    format!("UTC{}{}", sign, hours)
} else {
    format!("UTC{}{}:{:02}", sign, hours, mins)
};

let context_block = format!(
    "## Temporal & Geographic Context\n\
     Current date and time: {iso} ({human})\n\
     Timezone: {tz_display} ({offset})\n\
     Location: {location}\n\
     Locale: {locale}",
    iso = iso_8601,
    human = human,
    tz_display = tz_display,
    offset = offset_display,
    location = location_str,
    locale = locale_str,
);
```

### 5. System Message Assembly

The `context_block` is prepended as the **first section** in the system message assembly block (currently at [llm.rs:1108-1151](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs#L1108)):

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
Current date and time: 2026-05-17T10:34:00-05:00 (Tuesday, May 17, 2026, 10:34 AM)
Timezone: America/Bogota (UTC-5)
Location: Bogotá, Colombia
Locale: es-CO

---

You are a travel expert specializing in Latin American destinations.
```

The ISO 8601 string is the canonical source of truth for the model when reasoning about time (date math, "is X in the past", etc.). The parenthesised human-readable form lets the model echo the time back to the user in a natural way without needing to reformat. The `Locale` line tells the model which language + region conventions to use in its response.

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
| `locale` omitted from graph | `"es-CO"` used |
| `timezone` present but invalid IANA string | Falls back to `America/Bogota`; displayed label is rewritten to the fallback so the rendered block stays coherent; no error |
| `locale` present but malformed | Taken verbatim (no validation). BCP 47 parsing is intentionally lenient — the LLM is the final arbiter of language. |
| Graph has no LLM nodes | Fields are ignored silently |

### 8. Backward Compatibility

- Existing graphs without `timezone`/`location`/`locale` fields: no change in behavior (serde `#[serde(default)]` = `None`, defaults apply in the LLM node).
- No breaking changes to the node API, input ports, or output ports.
- Tests that snapshot the rendered system message will need to update their fixtures — the temporal context block is now the first section in the assembled system message and includes an ISO 8601 timestamp that varies per run.

## Files to Modify

| File | Change |
|------|--------|
| `src/libs/colmena/src/dag_engine/domain/graph.rs` | Add `timezone`, `location`, and `locale` fields to `Graph` |
| `src/libs/colmena/src/dag_engine/application/run_use_case.rs` | Inject `__colmena_timezone` / `__colmena_location` / `__colmena_locale` after session_id injections |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` | Read injected values, compute local time, prepend context block to system message |
| `src/libs/colmena/Cargo.toml` | Add `chrono-tz = "0.9"` |
| `docs/node_configurations.json` | Document `timezone`, `location`, and `locale` as graph-root fields |

## Test Graph

A new graph at `tests/graphs/agents/llm_temporal_context_test.json` with:
- `"timezone": "America/Bogota"` at root
- `"location": "Bogotá, Colombia"` at root
- `"locale": "es-CO"` at root
- One `llm_call` node with `prompt: "What day and time is it? Where am I located? In what language should you respond?"`
- Expected: LLM response reflects the correct local date/time (ISO 8601 + human-readable), Bogotá location, and answers in Spanish (per `es-CO`).
