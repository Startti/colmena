# LLM Temporal & Geographic Context — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Inject the current local date/time, the user's geographic location, AND the user's BCP 47 locale into every `llm_call`'s system message automatically. The graph author declares `timezone` (IANA), `location` (free-text), and `locale` (BCP 47) ONCE at the graph root; the engine threads them down and the LLM node renders the block. Datetime is rendered as ISO 8601 (canonical) with a human-readable echo. Defaults: `America/Bogota` / `Bogotá, Colombia` / `es-CO`.

**Architecture:** Three coordinated changes:

1. **Domain.** Three new optional fields on the `Graph` struct (`timezone`, `location`, `locale`, all `Option<String>`). Backward-compatible via `#[serde(default)]`.
2. **Engine.** In `run_use_case::execute_stream`, after the existing session-id injections (~line 415), inject `__colmena_timezone`, `__colmena_location`, and `__colmena_locale` into every node's inputs. Non-LLM nodes ignore unknown inputs.
3. **LLM node.** A new private helper `format_temporal_context_block(timezone, location, locale) -> String` that produces the canonical block: ISO 8601 first, human-readable in parentheses, separate lines for `Timezone`, `Location`, `Locale`. Invalid IANA falls back to `America/Bogota` AND rewrites the displayed timezone label for coherence. The block is prepended as the FIRST section of the system message inside the existing `if !history_exists` guard.

**Tech Stack:** Rust 1.95, `chrono` (already a dep), `chrono-tz` 0.9 (new), serde.

**Spec:** [docs/superpowers/specs/2026-05-12-llm-temporal-geographic-context-design.md](../specs/2026-05-12-llm-temporal-geographic-context-design.md) (revised 2026-05-18 to align with ISO 8601 + BCP 47 standards).

**Standards baseline (why this revision):**

- **Date format:** ISO 8601 (`2026-05-17T10:34:00-05:00`) is what Anthropic's own system prompts use and what production LLM applications recommend to avoid `M/D/Y` vs `D/M/Y` ambiguity. We also include a human-readable echo so the LLM can surface time naturally to users.
- **Timezone:** IANA TZDB strings (`America/Bogota`, `Europe/Madrid`) are the universal standard.
- **Locale:** BCP 47 IETF language tags (`es-CO`, `en-US`) are the formal standard for "language + region" identifiers, used across iOS, Android, browsers, and CLDR.

**Resolved against the spec while writing this plan:**

1. **Invalid IANA string:** falls back to `America/Bogota` AND rewrites the displayed label so the block stays internally coherent (label matches offset).
2. **Where the block is computed:** inside the `if !history_exists` guard, not at the top of `execute` — avoids wasted work on resume/multi-turn paths.
3. **Locale validation:** none. The string is taken verbatim and shown to the LLM; the model is the final arbiter of which language to use. Future strict validation can be added later if needed.

---

## File Structure

**Modified:**

```
src/libs/colmena/Cargo.toml
  └─ add chrono-tz = "0.9" to [dependencies]

src/libs/colmena/src/dag_engine/domain/graph.rs
  └─ add timezone, location, locale (all Option<String>) to Graph

src/libs/colmena/src/dag_engine/application/run_use_case.rs
  └─ inject __colmena_timezone, __colmena_location, __colmena_locale after session_id injections

src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
  ├─ private helper format_temporal_context_block(tz, loc, locale) -> String
  ├─ inline tests for the helper (#[cfg(test)] mod)
  └─ wire helper into the system_message assembly inside the !history_exists guard

docs/node_configurations.json
  └─ document timezone, location, locale as graph-root fields
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

Expected: `Finished dev profile`. Pulls down `chrono-tz` and its `phf` transitive dep. Zero warnings (deny-warnings active).

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/Cargo.toml Cargo.lock
git commit -m "deps(temporal-context): add chrono-tz for IANA timezone support"
```

> Note: workspace `Cargo.lock` is at the repo root, not under `src/libs/colmena/`.

---

## Task 2: Extend `Graph` struct with `timezone`, `location`, `locale`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/graph.rs`

- [ ] **Step 1: Locate the struct**

```bash
grep -n "pub struct Graph" src/libs/colmena/src/dag_engine/domain/graph.rs
```

Expected: single hit. Struct has at least `nodes` and `edges` with `#[derive(Debug, Deserialize, Serialize, Clone)]`.

- [ ] **Step 2: Write failing tests**

Append at the bottom of the file:

```rust
#[cfg(test)]
mod temporal_context_tests {
    use super::*;

    #[test]
    fn graph_without_optional_fields_parses_with_none_defaults() {
        let json = r#"{"nodes": {}, "edges": []}"#;
        let g: Graph = serde_json::from_str(json).expect("must parse");
        assert!(g.timezone.is_none(), "timezone should be None when omitted");
        assert!(g.location.is_none(), "location should be None when omitted");
        assert!(g.locale.is_none(), "locale should be None when omitted");
    }

    #[test]
    fn graph_with_all_three_fields_parses_them() {
        let json = r#"{
            "nodes": {},
            "edges": [],
            "timezone": "Europe/Madrid",
            "location": "Madrid, España",
            "locale": "es-ES"
        }"#;
        let g: Graph = serde_json::from_str(json).expect("must parse");
        assert_eq!(g.timezone.as_deref(), Some("Europe/Madrid"));
        assert_eq!(g.location.as_deref(), Some("Madrid, España"));
        assert_eq!(g.locale.as_deref(), Some("es-ES"));
    }

    #[test]
    fn graph_with_partial_fields_parses() {
        let json = r#"{
            "nodes": {},
            "edges": [],
            "locale": "en-US"
        }"#;
        let g: Graph = serde_json::from_str(json).expect("must parse");
        assert!(g.timezone.is_none());
        assert!(g.location.is_none());
        assert_eq!(g.locale.as_deref(), Some("en-US"));
    }
}
```

- [ ] **Step 3: Run tests — they must FAIL**

```bash
cargo test -p colmena_dag_engine --lib temporal_context_tests
```

Expected: **FAIL with compile error** — `Graph` has no `timezone` / `location` / `locale` fields.

- [ ] **Step 4: Add the fields**

Find the `Graph` struct and add the three fields at the end (minimize diff churn):

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

> The `#[serde(default)]` makes the field deserialize to `None` when the key is absent in JSON.

- [ ] **Step 5: Run tests — must PASS**

```bash
cargo test -p colmena_dag_engine --lib temporal_context_tests
```

Expected: 3 passed.

- [ ] **Step 6: Full lib suite for regression**

```bash
cargo test -p colmena_dag_engine --lib
```

Expected: ~750+ passed, 0 failed. Existing graph tests pass because serde is backward-compatible.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/graph.rs
git commit -m "feat(graph): optional timezone, location, locale at graph root"
```

---

## Task 3: Pure helper `format_temporal_context_block` in `llm.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

This task adds the pure helper that does IANA parsing, time computation, and block formatting. TDD with 6 unit tests covering ISO 8601 shape, fallback coherence, BCP 47 passthrough, and half-hour offsets.

- [ ] **Step 1: Write the failing tests**

Append a new test module at the bottom of `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`:

```rust
#[cfg(test)]
mod temporal_context_helper_tests {
    use super::*;

    #[test]
    fn block_starts_with_canonical_header() {
        let out = format_temporal_context_block("America/Bogota", "Bogotá, Colombia", "es-CO");
        assert!(
            out.starts_with("## Temporal & Geographic Context"),
            "missing header: {}",
            out
        );
    }

    #[test]
    fn iso_8601_appears_as_primary_timestamp() {
        // ISO 8601 format: YYYY-MM-DDTHH:MM:SS±HH:MM
        // Loose regex-free check: must contain "20XX-" and "T" and a "+" or "-"
        // in the offset position (after the time).
        let out = format_temporal_context_block("America/Bogota", "Bogotá, Colombia", "es-CO");
        let body = out
            .lines()
            .find(|l| l.starts_with("Current date and time:"))
            .expect("missing 'Current date and time:' line");
        // ISO substring: anywhere a sequence like "20XX-XX-XXTXX:XX:XX" plus "-05:00"
        assert!(body.contains("T"), "expected 'T' separator in: {}", body);
        // The Bogota offset is -05:00
        assert!(
            body.contains("-05:00"),
            "expected Bogotá ISO offset -05:00 in: {}",
            body
        );
    }

    #[test]
    fn human_echo_appears_in_parens() {
        let out = format_temporal_context_block("America/Bogota", "Bogotá, Colombia", "es-CO");
        let body = out
            .lines()
            .find(|l| l.starts_with("Current date and time:"))
            .unwrap();
        // After the ISO timestamp, the human form is in parens. We check a
        // generic shape: open paren, weekday-like word, comma, year, AM/PM.
        assert!(body.contains("("), "missing opening paren in: {}", body);
        assert!(body.contains(")"), "missing closing paren in: {}", body);
        assert!(
            body.contains("AM") || body.contains("PM"),
            "missing AM/PM marker in: {}",
            body
        );
    }

    #[test]
    fn block_has_timezone_location_locale_lines() {
        let out = format_temporal_context_block("America/Bogota", "Bogotá, Colombia", "es-CO");
        assert!(out.contains("Timezone: America/Bogota (UTC-5)"), "tz line: {}", out);
        assert!(out.contains("Location: Bogotá, Colombia"), "loc line: {}", out);
        assert!(out.contains("Locale: es-CO"), "locale line: {}", out);
    }

    #[test]
    fn half_hour_offset_renders_correctly() {
        // Asia/Kolkata is UTC+5:30
        let out = format_temporal_context_block("Asia/Kolkata", "Mumbai, India", "hi-IN");
        assert!(out.contains("Timezone: Asia/Kolkata (UTC+5:30)"), "expected UTC+5:30 in: {}", out);
        assert!(out.contains("Locale: hi-IN"));
    }

    #[test]
    fn invalid_iana_falls_back_coherently() {
        let out = format_temporal_context_block("Mars/Olympus", "Mars Base", "en-US");
        // Fallback rewrites the timezone label so the offset matches:
        assert!(out.contains("Timezone: America/Bogota (UTC-5)"), "fallback tz: {}", out);
        // ISO 8601 line shows the Bogota offset:
        assert!(out.contains("-05:00"), "fallback ISO offset: {}", out);
        // Location and locale are preserved verbatim
        assert!(out.contains("Location: Mars Base"));
        assert!(out.contains("Locale: en-US"));
    }
}
```

- [ ] **Step 2: Run tests — they must FAIL**

```bash
cargo test -p colmena_dag_engine --lib temporal_context_helper_tests
```

Expected: **FAIL with compile error** — `cannot find function format_temporal_context_block`.

- [ ] **Step 3: Implement the helper**

Add this private function near other module-scope helpers in `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (look for `generate_one_summary` from prior work — that's a good neighborhood):

```rust
/// Format the temporal & geographic context block that goes at the top of
/// the LLM system message.
///
/// - `timezone_str`: IANA timezone identifier (e.g. "America/Bogota"). Invalid
///   inputs fall back to `America/Bogota` and the displayed label is rewritten
///   to match the fallback so the rendered block stays internally coherent.
/// - `location_str`: free-text geographic description. No validation; taken
///   verbatim.
/// - `locale_str`: BCP 47 language+region tag (e.g. "es-CO"). No validation;
///   taken verbatim — the LLM is the final arbiter of which language to use.
///
/// The block renders ISO 8601 as the primary timestamp (canonical, locale-
/// neutral, machine-friendly for time reasoning) with a human-readable echo
/// in parentheses so the model can surface time naturally in its replies.
fn format_temporal_context_block(timezone_str: &str, location_str: &str, locale_str: &str) -> String {
    use chrono::Utc;
    use chrono_tz::Tz;

    // Parse IANA. On failure, fall back to Bogotá AND rewrite the displayed
    // label so the user sees a coherent (label, offset) pair.
    let (tz, tz_display) = match timezone_str.parse::<Tz>() {
        Ok(tz) => (tz, timezone_str.to_string()),
        Err(_) => (chrono_tz::America::Bogota, "America/Bogota".to_string()),
    };

    let local_dt = Utc::now().with_timezone(&tz);

    // ISO 8601 timestamp.
    let iso_8601 = local_dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string();

    // Human-readable echo.
    let human = local_dt.format("%A, %B %-d, %Y, %-I:%M %p").to_string();

    // UTC offset: "UTC-5" / "UTC+5:30" (drop ":00" minutes; drop leading
    // zero from hour count).
    let raw_offset = local_dt.format("%:z").to_string();
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

    format!(
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
    )
}
```

> Notes:
> - `%-d` and `%-I` use the chrono "no leading zero" flag (chrono provides this on all platforms regardless of libc).
> - `chrono_tz::America::Bogota` is a const-like value from `chrono-tz`'s generated module. If the build path complains, fall back to `"America/Bogota".parse::<Tz>().expect("hardcoded literal must parse")`.

- [ ] **Step 4: Run tests — they must PASS**

```bash
cargo test -p colmena_dag_engine --lib temporal_context_helper_tests
```

Expected: 6 passed.

- [ ] **Step 5: Full lib suite for regression**

```bash
cargo test -p colmena_dag_engine --lib
```

Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(temporal-context): pure helper rendering ISO 8601 + BCP 47 block"
```

---

## Task 4: Inject `__colmena_timezone` / `__colmena_location` / `__colmena_locale` in `run_use_case`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`

- [ ] **Step 1: Locate the session-id injection**

```bash
grep -n "__colmena_agent_session_id\|__colmena_node_id_path" src/libs/colmena/src/dag_engine/application/run_use_case.rs | head -5
```

You'll see the block (~line 410-420) inserting `__node_id`, `__colmena_node_id_path`, and `__colmena_agent_session_id` into the per-node `inputs` map.

- [ ] **Step 2: Add the three new inserts**

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
        if let Some(lc) = graph.locale.as_deref() {
            inputs.insert(
                "__colmena_locale".to_string(),
                Value::String(lc.to_string()),
            );
        }
```

> `graph` is the borrowed `Graph` value already in scope at this point. If the binding name differs in the actual code (e.g. `g`, `dag_graph`), adapt — read the surrounding lines for context. All three fields are `Option<String>` (from Task 2).

- [ ] **Step 3: Build & full lib suite**

```bash
cargo build -p colmena_dag_engine
cargo test -p colmena_dag_engine --lib
```

Expected: clean build, full suite still green. No behavioral change yet — non-LLM nodes ignore the new inputs, and `llm.rs` doesn't read them until Task 5.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/application/run_use_case.rs
git commit -m "feat(engine): inject timezone/location/locale into node inputs"
```

---

## Task 5: Wire the helper into `LlmNode::execute`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

This is the user-visible integration. Helper (Task 3) + injected inputs (Task 4) finally meet here.

- [ ] **Step 1: Locate the system message assembly**

```bash
grep -n "history_exists\|sections.*Vec<String>" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs | head -10
```

Find the `if !history_exists` (or equivalent guard) block where the existing `system_message`, attachments prelude, etc., are assembled into a `sections: Vec<String>`.

- [ ] **Step 2: Read the three injected inputs and prepend the context block**

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
            let locale_str = inputs
                .get("__colmena_locale")
                .and_then(|v| v.as_str())
                .unwrap_or("es-CO");
            let context_block = format_temporal_context_block(tz_str, loc_str, locale_str);
            sections.push(context_block);
```

> If the existing assembly initializes `sections` as `Vec::new()` and immediately pushes `system_message`, insert the context-block push BEFORE the `system_message` push so the temporal context is `sections[0]`. If the existing assembly is `let mut sections = vec![system_message.to_string()]`, refactor minimally so the context block goes first.

- [ ] **Step 3: Build & full lib suite for regression**

```bash
cargo build -p colmena_dag_engine
cargo test -p colmena_dag_engine --lib
```

Expected: clean build, full suite green. The helper unit tests (Task 3) still pass.

> **Heads-up:** if any pre-existing test snapshots the full rendered system message with an exact-string match, it will fail because the temporal context block is now `sections[0]` and contains a live ISO 8601 timestamp. Search for `expect_system_message`, `assert.*system_message`, or similar — switch those to substring assertions or rewrite the snapshot to use a regex / partial match. The full lib suite output will surface any such failure.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(llm_call): prepend temporal+geographic+locale context to system message"
```

---

## Task 6: Smoke test graph

**Files:**
- Create: `tests/graphs/agents/llm_temporal_context_test.json`

- [ ] **Step 1: Write the graph**

Create `tests/graphs/agents/llm_temporal_context_test.json`:

```json
{
  "_comment": "Smoke test for LLM temporal/geographic/locale context injection. Run with `source .env && cargo run --bin dag_engine -- run tests/graphs/agents/llm_temporal_context_test.json`. Expected: the assistant response references the current date/time in ISO 8601, the Bogotá location, the America/Bogota timezone (UTC-5), and answers in Spanish (per es-CO locale).",
  "timezone": "America/Bogota",
  "location": "Bogotá, Colombia",
  "locale": "es-CO",
  "nodes": {
    "ask": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "system_message": "You are a helpful local assistant. Answer using the contextual information you have. Respond in the user's locale language.",
        "prompt": "¿Qué fecha y hora es ahora? ¿Dónde estoy ubicado? ¿En qué idioma debo responder?"
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

- [ ] **Step 3: Optional end-to-end run (requires `.env` with GEMINI_API_KEY)**

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/llm_temporal_context_test.json
```

Expected: assistant response in Spanish that mentions the current date/time and Bogotá.

- [ ] **Step 4: Commit**

```bash
git add tests/graphs/agents/llm_temporal_context_test.json
git commit -m "test(temporal-context): smoke graph with locale es-CO"
```

---

## Task 7: Document graph-root fields

**Files:**
- Modify: `docs/node_configurations.json`
- Modify: `docs/DEVELOPER_GUIDE.md` (optional — add index entry)

- [ ] **Step 1: Add `timezone`, `location`, `locale` to `node_configurations.json`**

Open `docs/node_configurations.json` and locate the top of the file. If a `"graph_root_fields"` section already exists, append the three new fields to its `fields` map. If not, create the section at the top.

Skeleton if creating:

```json
  "graph_root_fields": {
    "name": "Graph Root Fields",
    "description": "Optional fields declared at the top level of the graph JSON, alongside `nodes` and `edges`. The engine propagates them to every node via the `__colmena_*` namespace.",
    "fields": {
      "timezone": {
        "type": "string",
        "required": false,
        "default": "America/Bogota",
        "description": "IANA timezone identifier (e.g. `America/Bogota`, `Europe/Madrid`). Used by `llm_call` to render the temporal context block at the top of the system message. Invalid IANA strings silently fall back to `America/Bogota`, and the displayed timezone label is rewritten to match the fallback so the rendered block stays internally coherent.",
        "example": "Europe/Madrid"
      },
      "location": {
        "type": "string",
        "required": false,
        "default": "Bogotá, Colombia",
        "description": "Free-text geographic location shown to the LLM in the temporal context block. No validation; used verbatim. Use whatever level of detail you want the LLM to see (city + country, country only, region, address, etc.).",
        "example": "Madrid, España"
      },
      "locale": {
        "type": "string",
        "required": false,
        "default": "es-CO",
        "description": "BCP 47 language+region tag (e.g. `es-CO`, `en-US`, `pt-BR`). Tells the LLM what language to respond in independently of the `location` string — a user in `Bogotá, Colombia` might still want English replies (`en-CO`). No validation; passed verbatim to the model.",
        "example": "en-US"
      }
    }
  },
```

> Read the file before editing to match the actual JSON structure conventions. Validate after with `python3 -c "import json; json.load(open('docs/node_configurations.json'))"`.

- [ ] **Step 2: Validate JSON**

```bash
python3 -c "import json; json.load(open('docs/node_configurations.json')); print('valid')"
```

- [ ] **Step 3: Optionally add an entry to `DEVELOPER_GUIDE.md`**

If the developer guide has a "Graph-root fields" entry, do nothing (the new fields auto-appear in `node_configurations.json` discovery). If not, append a brief paragraph at an appropriate spot:

```markdown
35. [**Temporal & Geographic Context**](./superpowers/specs/2026-05-12-llm-temporal-geographic-context-design.md): Campos opcionales `timezone` (IANA), `location` (texto libre) y `locale` (BCP 47) al root del graph JSON. El motor los inyecta en cada `llm_call` y el nodo prepende un bloque `## Temporal & Geographic Context` con fecha/hora en ISO 8601 (más echo human-readable), offset UTC y locale al inicio del system message. Defaults: `America/Bogota` / `Bogotá, Colombia` / `es-CO`. IANA inválido cae a Bogotá silenciosamente. La spec linkeada documenta el diseño completo.
```

- [ ] **Step 4: Commit**

```bash
git add docs/node_configurations.json docs/DEVELOPER_GUIDE.md
git commit -m "docs(temporal-context): document timezone/location/locale graph-root fields"
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

- [ ] **Step 2: Confirm commit chain**

```bash
git log --oneline | head -10
```

Expected in some order:

```
docs(temporal-context): document timezone/location/locale graph-root fields
test(temporal-context): smoke graph with locale es-CO
feat(llm_call): prepend temporal+geographic+locale context to system message
feat(engine): inject timezone/location/locale into node inputs
feat(temporal-context): pure helper rendering ISO 8601 + BCP 47 block
feat(graph): optional timezone, location, locale at graph root
deps(temporal-context): add chrono-tz for IANA timezone support
```

- [ ] **Step 3: Optional end-to-end**

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/llm_temporal_context_test.json
```

Confirm response in Spanish referencing local date, time, and Bogotá.

---

## Open caveats for the implementer

- **`chrono_tz::America::Bogota` import path.** `chrono-tz` exposes timezones via auto-generated constants under `chrono_tz::<Region>::<City>`. If the build path complains, fall back to `"America/Bogota".parse::<Tz>().expect("hardcoded literal must parse")`. Slightly slower (parsing at every fallback) but works regardless of the crate's macro internals.

- **`%-d` / `%-I` portability.** GNU-specific flags in C strftime but chrono provides them on all platforms. If they render as literal `%-d`, switch to `%d` / `%I` (will show `01`, `07`, etc. — acceptable, less polished).

- **ISO 8601 vs RFC 3339.** The format `%Y-%m-%dT%H:%M:%S%:z` produces RFC 3339 / ISO 8601 strings like `2026-05-17T10:34:00-05:00`. This is the universally-accepted form for LLM consumption.

- **BCP 47 validation.** Intentionally NONE. The plan accepts anything as the locale string and trusts the LLM to interpret. If you later want strict validation, the `language-tags` crate handles BCP 47 parsing properly — but that's a future concern.

- **Existing tests with system-message snapshots.** Any test that does an exact-string match on the full rendered system message will fail because (a) the temporal context block is now `sections[0]` and (b) the ISO 8601 timestamp is non-deterministic. Search for `assert_eq!(.*system.*message`, `expect_system_message`, etc. — switch to substring / regex assertions.

- **Helper tests + time.** The 6 unit tests in Task 3 use `Utc::now()` which is non-deterministic. They only assert presence of substrings (`"-05:00"`, `"AM"` / `"PM"`, format shape) — never exact times. Don't mock the clock unless a future test explicitly needs determinism.

- **`%A` / `%B` locale.** chrono produces English weekday and month names by default in the human-readable echo. The ISO 8601 line is locale-neutral by construction. If the team later wants Spanish weekday names in the parenthesised echo, switch to `chrono`'s locale-aware formatters (requires the `unstable-locales` feature). Out of scope for v1.
