# SocketIoNode Transport-Error Visibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a `socketio_request` operation fails, the LLM sees the transport-level errors that occurred during that operation (plus actionable advice) inside the error envelope; after the node finishes, leaked background connections can no longer spam `EngineIO Error` lines into the worker logs.

**Architecture:** All changes live in one file (`socketio.rs`). Two shared handles are threaded into the connection's lifecycle handlers: an `Arc<AtomicBool>` (`active`) that gates every handler so a zombie connection goes silent once the node is done, and an `Arc<Mutex<Vec<String>>>` (`transport_errors`) that captures error events fired during the execution window. On op failure the captured errors are aggregated (`"EngineIO Error (x4)"`) and attached to the failure envelope as `transport_errors` + `advice` — the envelope already travels to the LLM (tool message) and over SSE (`tool-output-available`), so no engine/executor changes are needed. On success the buffer is discarded.

**Tech Stack:** Rust, `rust_socketio 0.6` (async), `tokio`, `serde_json`. Tests: inline `#[cfg(test)]` unit tests (pure helpers) + live E2E with a local `python-socketio` server.

## Global Constraints

- Crate name is `colmena_dag_engine` — test with `cargo test --lib socketio`, NEVER `cargo test -p colmena`.
- `[lints.rust] warnings = "deny"` — any rustc warning fails the build.
- Conventional Commits enforced by CI: only `feat/fix/docs/style/refactor/perf/test/build/ci/chore/revert` prefixes.
- Before push: `cargo fmt`, `cargo clippy`, and full `cargo test --verbose` (CI parity — `--lib` alone hides doctest/integration failures).
- Additive change only: the envelope gains optional fields; no public API change (ADP unaffected).
- Docs language: Spanish in `docs/`, English in code comments and LLM-facing strings.
- E2E runs: save SSE output to `/tmp/colmena_e2e/<name>.sse`; unset `COLMENA_LOCAL` before `dag_engine run`.
- Python venv for the test server lives in the MAIN repo checkout: `/Users/danielgarcia/startti/colmena/.venv` (`python-socketio` + `aiohttp` already installed there on 2026-07-05).

## Design decisions (locked)

| Decision | Choice |
|---|---|
| Which handlers capture into the buffer | `error` and `connect_error` only |
| Which handlers are muted post-completion | ALL lifecycle/debug handlers: `error`, `connect_error`, `disconnect`, `exception`, `on_any` (wait_event handlers are already self-cleaning via `WaitSlots`) |
| Buffer cap | 10 raw entries (`MAX_TRANSPORT_ERRORS`), silently drop beyond |
| Aggregation | duplicates collapsed preserving first-seen order: `["E","E","F"]` → `["E (x2)","F"]` |
| Envelope fields | `transport_errors: [string]` + `advice: string`, added ONLY to failure envelopes and ONLY when the buffer is non-empty |
| Success path | buffer discarded, envelope unchanged |
| Connect-failure path (node returns `Err`) | append captured errors to the error string: `... (transport errors during connect: E (x2))` |
| `active` flip timing | `active.store(false)` immediately BEFORE each `client.disconnect()` call (mutes noise triggered by the disconnect itself; the node's own `⚠ disconnect failed` log is not gated) |

---

### Task 1: Branch setup

**Files:** none (git only)

- [ ] **Step 1: Create feature branch from fresh develop**

```bash
cd /Users/danielgarcia/startti/colmena/.claude/worktrees/gallant-rhodes-02ee34
git fetch origin develop
git checkout -b feat/socketio-transport-error-visibility origin/develop
```

- [ ] **Step 2: Verify clean state**

Run: `git status --short`
Expected: empty output. Also verify the PR #145 changes are present: `grep -n 'unwrap_or("websocket")' src/libs/colmena/src/dag_engine/infrastructure/nodes/socketio.rs` → one hit.

---

### Task 2: Pure helpers with unit tests (TDD)

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/socketio.rs` (impl block + `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces (used by Task 3):
  - `const MAX_TRANSPORT_ERRORS: usize = 10;` (module scope)
  - `const TRANSPORT_ERROR_ADVICE: &str` (module scope)
  - `SocketIoNode::payload_to_compact_string(payload: Payload) -> String`
  - `SocketIoNode::summarize_transport_errors(raw: &[String]) -> Vec<String>`
  - `SocketIoNode::attach_transport_context(envelope: &mut Value, raw_errors: &[String])`

- [ ] **Step 1: Write the failing tests** — append inside the existing `mod tests` in `socketio.rs`:

```rust
    #[test]
    fn summarize_transport_errors_empty() {
        let out = SocketIoNode::summarize_transport_errors(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn summarize_transport_errors_single() {
        let raw = vec!["EngineIO Error".to_string()];
        let out = SocketIoNode::summarize_transport_errors(&raw);
        assert_eq!(out, vec!["EngineIO Error".to_string()]);
    }

    #[test]
    fn summarize_transport_errors_aggregates_preserving_order() {
        let raw = vec![
            "EngineIO Error".to_string(),
            "EngineIO Error".to_string(),
            "Connection reset".to_string(),
            "EngineIO Error".to_string(),
        ];
        let out = SocketIoNode::summarize_transport_errors(&raw);
        assert_eq!(
            out,
            vec![
                "EngineIO Error (x3)".to_string(),
                "Connection reset".to_string()
            ]
        );
    }

    #[test]
    fn attach_transport_context_noop_when_empty() {
        let mut env = json!({ "success": false, "event": "ping", "error": "Timeout" });
        SocketIoNode::attach_transport_context(&mut env, &[]);
        assert!(env.get("transport_errors").is_none());
        assert!(env.get("advice").is_none());
    }

    #[test]
    fn attach_transport_context_adds_fields() {
        let mut env = json!({ "success": false, "event": "ping", "error": "Timeout" });
        let raw = vec!["EngineIO Error".to_string(), "EngineIO Error".to_string()];
        SocketIoNode::attach_transport_context(&mut env, &raw);
        assert_eq!(
            env["transport_errors"],
            json!(["EngineIO Error (x2)"])
        );
        assert_eq!(env["advice"], json!(TRANSPORT_ERROR_ADVICE));
        // Pre-existing fields untouched
        assert_eq!(env["error"], json!("Timeout"));
    }

    #[test]
    fn payload_to_compact_string_unwraps_single_text() {
        let p = Payload::Text(vec![Value::String("EngineIO Error".to_string())]);
        assert_eq!(
            SocketIoNode::payload_to_compact_string(p),
            "EngineIO Error"
        );
    }

    #[test]
    fn payload_to_compact_string_serializes_object() {
        let p = Payload::Text(vec![json!({ "code": 1 })]);
        assert_eq!(
            SocketIoNode::payload_to_compact_string(p),
            "{\"code\":1}"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib socketio 2>&1 | tail -20`
Expected: compilation FAILURE (`summarize_transport_errors` etc. not found) — that is the failing state for compiled languages.

- [ ] **Step 3: Write the implementation** — module-scope constants right after `type WaitSlots = ...;`:

```rust
/// Max raw transport-error messages captured per execution (drop beyond).
const MAX_TRANSPORT_ERRORS: usize = 10;

/// LLM-facing advice attached to failure envelopes that carry transport errors.
const TRANSPORT_ERROR_ADVICE: &str = "Transport-level errors occurred during this operation: \
    the connection to the server is unstable or the server dropped the session. Retrying the \
    same call is unlikely to help while these errors persist — if the problem continues, \
    inform the user that the realtime backend appears to be unreachable.";
```

And three helpers inside `impl SocketIoNode` (right after `payload_to_value`):

```rust
    /// Render a payload as a compact single-line string for logs/envelopes.
    fn payload_to_compact_string(payload: Payload) -> String {
        match Self::payload_to_value(payload) {
            Value::String(s) => s,
            other => other.to_string(),
        }
    }

    /// Collapse duplicate transport-error messages preserving first-seen
    /// order: `["E", "E", "F", "E"]` → `["E (x3)", "F"]`.
    fn summarize_transport_errors(raw: &[String]) -> Vec<String> {
        let mut order: Vec<&String> = Vec::new();
        let mut counts: HashMap<&String, usize> = HashMap::new();
        for msg in raw {
            if !counts.contains_key(msg) {
                order.push(msg);
            }
            *counts.entry(msg).or_insert(0) += 1;
        }
        order
            .into_iter()
            .map(|msg| {
                let n = counts[msg];
                if n > 1 {
                    format!("{} (x{})", msg, n)
                } else {
                    msg.clone()
                }
            })
            .collect()
    }

    /// Attach `transport_errors` + `advice` to a failure envelope. No-op when
    /// no transport errors were captured — success envelopes never call this.
    fn attach_transport_context(envelope: &mut Value, raw_errors: &[String]) {
        if raw_errors.is_empty() {
            return;
        }
        envelope["transport_errors"] = json!(Self::summarize_transport_errors(raw_errors));
        envelope["advice"] = json!(TRANSPORT_ERROR_ADVICE);
    }
```

`HashMap` is already imported at the top of the file (`use std::collections::HashMap;`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib socketio 2>&1 | tail -5`
Expected: all socketio tests PASS (10 pre-existing + 7 new = 17 passed). NOTE: the helpers are not yet called from `execute()`, so deny-warnings will flag them as dead code — if `cargo test` fails on `dead_code`, proceed immediately to Task 3 and commit both tasks together after Task 3 Step 4 instead. Try first; if it compiles (test cfg references count as uses), commit now.

- [ ] **Step 5: Commit (if Step 4 compiled clean)**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/socketio.rs
git commit -m "feat(dag_engine): socketio transport-error helpers (summarize + envelope attach)"
```

---

### Task 3: Wire buffer + active flag into `execute()`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/socketio.rs` (`execute()`, sections 4–12; imports)

**Interfaces:**
- Consumes: `MAX_TRANSPORT_ERRORS`, `TRANSPORT_ERROR_ADVICE`, `payload_to_compact_string`, `attach_transport_context` from Task 2.
- Produces: failure envelopes with optional `transport_errors` + `advice`; all lifecycle handlers gated on `active`.

- [ ] **Step 1: Add the atomic import** — extend the existing `use std::sync::Arc;` line:

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
```

- [ ] **Step 2: Create the shared handles** — in `execute()`, immediately after the `builder` is created and headers/cookies are applied (before the "Lifecycle/debug handlers" comment):

```rust
        // ---- 4b. Execution-window gate + transport-error capture ----
        // `active` is true only while this execution owns the connection.
        // rust_socketio's background task can outlive disconnect() (it keeps
        // polling and surfacing "EngineIO Error" events); gating every handler
        // on `active` makes those zombie connections silent. `transport_errors`
        // collects error events fired DURING the window so op failures can
        // tell the LLM WHY (unstable connection vs. slow server).
        let active = Arc::new(AtomicBool::new(true));
        let transport_errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
```

- [ ] **Step 3: Replace the `error` and `connect_error` handlers** — the current handlers:

```rust
        // Lifecycle/debug handlers
        builder = builder.on("error", |payload, _client| {
            async move {
                println!("[SocketIoNode] ⚠ server error event: {:?}", payload);
            }
            .boxed()
        });
        builder = builder.on("connect_error", |payload, _client| {
            async move {
                println!("[SocketIoNode] ⚠ connect_error event: {:?}", payload);
            }
            .boxed()
        });
```

become capture-and-gate versions:

```rust
        // Lifecycle/debug handlers — all gated on `active` (see 4b).
        for evt in ["error", "connect_error"] {
            let active = active.clone();
            let errors = transport_errors.clone();
            builder = builder.on(evt, move |payload, _client| {
                let active = active.clone();
                let errors = errors.clone();
                async move {
                    if !active.load(Ordering::Relaxed) {
                        return; // stale connection from a finished execution
                    }
                    let msg = Self::payload_to_compact_string(payload);
                    println!("[SocketIoNode] ⚠ transport error event: {}", msg);
                    let mut buf = errors.lock().await;
                    if buf.len() < MAX_TRANSPORT_ERRORS {
                        buf.push(msg);
                    }
                }
                .boxed()
            });
        }
```

NOTE: `ClientBuilder::on` takes `Into<Event>`; `"error"`/`"connect_error"` map to the reserved `Event::Error`/custom events exactly as before (same strings the current code registers).

- [ ] **Step 4: Gate the remaining debug handlers** — `disconnect`, `exception`, and `on_any` each get the same guard. Replace:

```rust
        builder = builder.on("disconnect", |payload, _client| {
            async move {
                println!("[SocketIoNode] ⚠ disconnect event: {:?}", payload);
            }
            .boxed()
        });
```

with:

```rust
        {
            let active = active.clone();
            builder = builder.on("disconnect", move |payload, _client| {
                let active = active.clone();
                async move {
                    if active.load(Ordering::Relaxed) {
                        println!("[SocketIoNode] ⚠ disconnect event: {:?}", payload);
                    }
                }
                .boxed()
            });
        }
```

In the existing `exception` handler, add as the FIRST lines of the async block (the handler already clones `exc_tx`; add an `active` clone in the same closure prologue):

```rust
                    if !active.load(Ordering::Relaxed) {
                        return;
                    }
```

In the existing `on_any` handler, same first-line guard (add an `active` clone to its closure prologue too).

- [ ] **Step 5: Flip `active` off before each disconnect** — two call sites.

Pre-event failure path — replace:

```rust
                Err(msg) => {
                    println!("[SocketIoNode] ✗ pre_event '{}' failed: {}", pe.event, msg);
                    if let Err(e) = client.disconnect().await {
                        println!("[SocketIoNode] ⚠ disconnect failed: {}", e);
                    }
                    return Ok(json!({
                        "success": false,
                        "event": event_name,
                        "failed_pre_event": pe.event,
                        "error": msg,
                        "pre_responses": pre_responses,
                    }));
                }
```

with:

```rust
                Err(msg) => {
                    println!("[SocketIoNode] ✗ pre_event '{}' failed: {}", pe.event, msg);
                    active.store(false, Ordering::Relaxed);
                    if let Err(e) = client.disconnect().await {
                        println!("[SocketIoNode] ⚠ disconnect failed: {}", e);
                    }
                    let mut out = json!({
                        "success": false,
                        "event": event_name,
                        "failed_pre_event": pe.event,
                        "error": msg,
                        "pre_responses": pre_responses,
                    });
                    Self::attach_transport_context(&mut out, &transport_errors.lock().await);
                    return Ok(out);
                }
```

Main path — replace:

```rust
        // ---- 11. Disconnect (always) ----
        // A failed disconnect leaves the underlying engine.io task half-open
        // (it keeps polling/pinging and surfaces recurring "EngineIO Error"
        // events) — log it so the leak is visible in worker logs.
        if let Err(e) = client.disconnect().await {
            println!("[SocketIoNode] ⚠ disconnect failed: {}", e);
        }
```

with:

```rust
        // ---- 11. Disconnect (always) ----
        // Flip `active` first so a leaked background task (rust_socketio can
        // keep polling after an incomplete disconnect) goes silent instead of
        // spamming logs. The disconnect result itself is still logged.
        active.store(false, Ordering::Relaxed);
        if let Err(e) = client.disconnect().await {
            println!("[SocketIoNode] ⚠ disconnect failed: {}", e);
        }
```

- [ ] **Step 6: Attach transport context to the main failure envelope** — in section 12, replace the `Err(msg)` arm:

```rust
            Err(msg) => {
                println!("[SocketIoNode] ← error: {}", msg);
                let mut out = json!({
                    "success": false,
                    "event": event_name,
                    "error": msg,
                });
                if let Some(pre) = pre_responses_val {
                    out["pre_responses"] = pre;
                }
                Ok(out)
            }
```

with:

```rust
            Err(msg) => {
                println!("[SocketIoNode] ← error: {}", msg);
                let mut out = json!({
                    "success": false,
                    "event": event_name,
                    "error": msg,
                });
                if let Some(pre) = pre_responses_val {
                    out["pre_responses"] = pre;
                }
                Self::attach_transport_context(&mut out, &transport_errors.lock().await);
                Ok(out)
            }
```

- [ ] **Step 7: Enrich the connect-failure error** — replace:

```rust
        let client = builder.connect().await.map_err(|e| {
            format!(
                "socketio_request: failed to connect to {} (namespace {}): {}",
                url, namespace, e
            )
        })?;
```

with:

```rust
        let client = match builder.connect().await {
            Ok(c) => c,
            Err(e) => {
                active.store(false, Ordering::Relaxed);
                let captured = transport_errors.lock().await;
                let extra = if captured.is_empty() {
                    String::new()
                } else {
                    format!(
                        " (transport errors during connect: {})",
                        Self::summarize_transport_errors(&captured).join("; ")
                    )
                };
                return Err(format!(
                    "socketio_request: failed to connect to {} (namespace {}): {}{}",
                    url, namespace, e, extra
                )
                .into());
            }
        };
```

- [ ] **Step 8: Update `schema()` outputs** — in the `outputs` object of `schema()`, after the `"error"` line add:

```rust
                "transport_errors": "array<string> (only on failure, when transport-level errors occurred during the operation — aggregated, e.g. \"EngineIO Error (x4)\")",
                "advice": "string (only present alongside transport_errors — actionable guidance for the caller/LLM)"
```

And in the module doc `//! ## Outputs` section, extend the failure line:

```rust
//! On failure the envelope may also include `transport_errors` (aggregated
//! transport-level errors captured during the operation) and `advice`.
```

- [ ] **Step 9: Compile, lint, unit tests**

```bash
cargo check 2>&1 | tail -3
cargo clippy 2>&1 | tail -3
cargo fmt
cargo test --lib socketio 2>&1 | tail -5
```

Expected: clean check/clippy (no warnings — deny-warnings is on), 17 tests PASS.

- [ ] **Step 10: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/socketio.rs
git commit -m "feat(dag_engine): socketio failure envelopes carry transport_errors + advice; mute zombie connections"
```

(If Task 2 Step 5 was skipped due to dead-code deny, this commit carries both tasks — use the same message.)

---

### Task 4: Documentation sweep

**Files:**
- Modify: `docs/node_configurations.json` (socketio outputs, if an outputs block exists for the node — check first; if the file only documents config fields, skip it)
- Modify: `docs/node_as_tools_reference.json` (socketio `special_behaviors` error-envelope bullet)
- Modify: `docs/developer_guide/21_socketio_node.md` (outputs table + troubleshooting section)
- Modify: `docs/CHANGELOG_2026-07.md` (new §3)

- [ ] **Step 1: node_as_tools_reference.json** — the bullet that currently reads:

```
"Errors are returned as an envelope, not exceptions: { success: false, event, error, exception? }. This lets downstream nodes branch on success."
```

becomes:

```
"Errors are returned as an envelope, not exceptions: { success: false, event, error, transport_errors?, advice? }. transport_errors (aggregated, e.g. 'EngineIO Error (x4)') and advice appear when transport-level errors occurred during the failed operation — they tell the LLM the connection itself is unstable so it can stop retrying and inform the user. This lets downstream nodes branch on success."
```

Validate JSON after editing: `python3 -c "import json; json.load(open('docs/node_as_tools_reference.json'))"`.

- [ ] **Step 2: dev guide 21** — two edits:

(a) In the Output Ports / outputs documentation section (search for the `success` / `error` output rows), add two rows:

```markdown
| `transport_errors` | array | Only on failure — aggregated transport-level errors captured during the operation (e.g. `"EngineIO Error (x4)"`) |
| `advice` | string | Only alongside `transport_errors` — actionable guidance for the caller/LLM |
```

(b) In the "Recurring `EngineIO Error`" troubleshooting section, append:

```markdown
Since 2026-07-05 the node also: (1) attaches `transport_errors` + `advice` to the
failure envelope when transport errors occur DURING an operation, so the LLM can
distinguish "server slow" from "connection broken" and inform the user; and
(2) silences all handler logging for a connection once its execution finishes —
zombie connections leaked by an incomplete disconnect no longer spam the logs.
```

- [ ] **Step 3: node_configurations.json** — check whether the socketio entry documents outputs (`grep -n '"outputs"' docs/node_configurations.json | head`). If the socketio node block has an outputs section, add `transport_errors` / `advice` entries mirroring the schema() strings from Task 3 Step 8; if outputs are not part of that file's schema, skip. Validate JSON after any edit.

- [ ] **Step 4: CHANGELOG_2026-07.md §3** — append:

```markdown
---

## 3. `socketio_request` — visibilidad de errores de transporte para el LLM + mute de conexiones zombi

**Qué cambió.** (1) Cuando una operación falla, el envelope de error ahora incluye `transport_errors` (errores de transporte capturados durante ESA operación, agregados — p.ej. `"EngineIO Error (x4)"`) y `advice` (guía accionable para el LLM: la conexión está inestable, reintentar no ayuda, informar al usuario). El envelope ya viajaba al LLM (tool message) y por SSE (`tool-output-available`), así que el modelo ahora puede distinguir "server lento" de "conexión rota" sin ningún cambio en el executor ni en ADP. (2) Todos los handlers de la conexión se gatean con un flag `active` que se apaga al desconectar: las conexiones zombi que `rust_socketio 0.6` filtra tras un disconnect incompleto (su task de fondo sigue vivo consumiendo el stream) ya no pueden imprimir `EngineIO Error` infinitos en los logs del worker.

**Por qué importa.** Follow-up del incidente ADP 2026-07-04 (PR #145): websocket-only eliminó la causa polling/stickiness, pero los logs del worker (revision 00083) mostraron que el ruido residual viene del task de fondo del crate que no muere tras `disconnect()`. En éxito el buffer se descarta (no se alarma al modelo por ruido irrelevante).

**Documentación de referencia.**
- Plan: [`docs/superpowers/plans/2026-07-05-socketio-transport-error-visibility.md`](superpowers/plans/2026-07-05-socketio-transport-error-visibility.md)
- Dev guide: [`docs/developer_guide/21_socketio_node.md`](developer_guide/21_socketio_node.md)
- Tools reference: [`docs/node_as_tools_reference.json`](node_as_tools_reference.json)

**Estado.** done.
```

- [ ] **Step 5: Commit**

```bash
git add docs/
git commit -m "docs: socketio transport_errors + advice envelope fields, zombie mute"
```

---

### Task 5: Live E2E verification + PR

**Files:**
- Create (scratchpad, not committed): `<scratchpad>/sio_server.py` (already exists from the 2026-07-05 session — reuse; it answers `ping` via ack), `<scratchpad>/sio_noack_server.py`, `<scratchpad>/sio_default_transport.json` (exists), `<scratchpad>/sio_timeout_test.json`

- [ ] **Step 1: Happy path E2E (no regression, no envelope pollution)**

```bash
SCRATCH=<scratchpad dir>
/Users/danielgarcia/startti/colmena/.venv/bin/python $SCRATCH/sio_server.py > $SCRATCH/sio_server.log 2>&1 &
echo $! > $SCRATCH/sio_server.pid
# wait for port 8899, then:
unset COLMENA_LOCAL
cargo run --bin dag_engine -- run $SCRATCH/sio_default_transport.json 2>&1 | tee /tmp/colmena_e2e/socketio_transport_ctx_happy.sse | grep -E '"success"|transport_errors'
kill $(cat $SCRATCH/sio_server.pid)
```

Expected: `"success":true` envelope; NO `transport_errors` key anywhere in the SSE.

- [ ] **Step 2: Failure path E2E (timeout + mid-op transport errors)**

Create `$SCRATCH/sio_noack_server.py` — same as `sio_server.py` but the `ping` handler never returns (so the ack never fires):

```python
import asyncio
import socketio
from aiohttp import web

sio = socketio.AsyncServer(async_mode="aiohttp", cors_allowed_origins="*")
app = web.Application()
sio.attach(app)

@sio.event
async def connect(sid, environ):
    print(f"[server] connect sid={sid}", flush=True)

@sio.event
async def ping(sid, data):
    print(f"[server] ping received — never acking", flush=True)
    await asyncio.sleep(3600)  # never ack

if __name__ == "__main__":
    web.run_app(app, host="127.0.0.1", port=8899, print=None)
```

Create `$SCRATCH/sio_timeout_test.json` — same graph as `sio_default_transport.json` but `"timeout_ms": 15000`.

Run the graph in background, kill the server 3 s in (this breaks the websocket mid-wait → the client surfaces transport errors → then the ack timeout fires):

```bash
/Users/danielgarcia/startti/colmena/.venv/bin/python $SCRATCH/sio_noack_server.py > $SCRATCH/sio_noack.log 2>&1 &
echo $! > $SCRATCH/sio_server.pid
# wait for port, then:
(cargo run --bin dag_engine -- run $SCRATCH/sio_timeout_test.json > /tmp/colmena_e2e/socketio_transport_ctx_fail.sse 2>&1 &)
sleep 3 && kill $(cat $SCRATCH/sio_server.pid)
# wait ~20s for the run to finish, then:
grep -E 'transport_errors|advice|"success":false' /tmp/colmena_e2e/socketio_transport_ctx_fail.sse
```

Expected: failure envelope contains `"success":false`, an `error` (timeout or server-exception), `transport_errors` with at least one aggregated entry, and the `advice` string. Also verify the mute: after the final `node-end` frame there must be NO further `⚠ transport error event` lines in the output.

NOTE: exact error strings from a killed socket vary (`EngineIO Error`, connection reset, etc.) — assert presence of the KEYS, not specific messages. If the kill lands before any error event fires (timing), re-run; 3 s into a 15 s wait is reliable.

- [ ] **Step 3: Full suite (CI parity)**

Run: `cargo test --verbose 2>&1 | tail -5`
Expected: exit 0, all tests pass.

- [ ] **Step 4: Push + PR**

```bash
git push -u origin feat/socketio-transport-error-visibility
gh pr create --base develop \
  --title "feat(dag_engine): socketio transport-error visibility + zombie connection mute" \
  --body "<summary: what/why, envelope example, E2E evidence, ADP unaffected (additive envelope fields), follow-up to PR #145. End with the Claude Code attribution line.>"
```

Expected: PR URL printed; CI (7 Python versions + conventional commits) green.

- [ ] **Step 5: Report**

Present to the user: PR link, the failure-envelope JSON captured in the E2E, and the reminder that the Cloud Run log verification (zombie noise gone) happens after the next worker deploy.

---

## Self-Review (done at plan-writing time)

- **Spec coverage:** capture-during-window ✔ (Task 3 Steps 2–4), envelope enrichment on all three failure paths — main op ✔ (Step 6), pre_event ✔ (Step 5), connect ✔ (Step 7) —, success path untouched ✔ (only `Err` arms edited), zombie mute ✔ (Steps 4–5), LLM/SSE delivery needs no engine change ✔ (verified in session: `DagToolExecutor` → `ToolResult.output` → `tool-output-available`).
- **Placeholder scan:** all code steps carry full code; PR body is intentionally summarized (content depends on E2E evidence gathered in Task 5).
- **Type consistency:** `attach_transport_context(&mut Value, &[String])` — call sites pass `&transport_errors.lock().await` which derefs `MutexGuard<Vec<String>>` → `&[String]` via deref coercion ✔; `summarize_transport_errors(&[String]) -> Vec<String>` used in Task 2 tests and Task 3 Step 7 ✔; constants referenced in tests are module-scope (visible to `mod tests` via `use super::*`) ✔.
