# SseMapper Unification + tool-input-start Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Añadir el evento `tool-input-start` al `SseMapper` para cumplir con el protocolo Vercel AI SDK, y migrar toda la lógica de streaming inline de `api.rs` para que use `SseMapper` como única fuente de verdad.

**Architecture:** El `SseMapper` es la única clase que traduce `DagExecutionEvent` a JSON SSE. Actualmente `main.rs` (modo `run`) lo usa correctamente, pero `api.rs` tiene dos bloques de lógica inline duplicada — uno en `run_dag()` (stdout) y otro en `handler_webhook()` (HTTP SSE) — que divergen del mapper. Se añade `tool-input-start` al mapper, se migran ambos bloques a usarlo, y se añade soporte SSE real a `handler_resume` que actualmente ignora el header `Accept: text/event-stream`.

**Tech Stack:** Rust, axum, `async-stream`, `futures::StreamExt`, `uuid`

---

## Mapa de archivos

| Archivo | Acción | Qué cambia |
|---|---|---|
| `src/libs/colmena/src/dag_engine/sse_mapper.rs` | Modificar | Añadir `seen_tool_ids: HashSet<String>`, emitir `tool-input-start` / `subgraph-tool-input-start` |
| `src/libs/colmena/src/dag_engine/api.rs` | Modificar | Reemplazar lógica inline en `run_dag()` y `handler_webhook()`, añadir SSE a `handler_resume()` |
| `docs/sse_events_reference.md` | Modificar | Documentar `tool-input-start` y `subgraph-tool-input-start` |

---

## Task 1: Añadir `tool-input-start` al `SseMapper`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/sse_mapper.rs`

### Contexto

El protocolo Vercel AI SDK exige esta secuencia completa para cada tool call:
```
tool-input-start      ← toolCallId + toolName (primer LlmToolCall del tool_id)
tool-input-delta × N  ← chunks de argumentos (cada LlmToolCall)
tool-input-available  ← argumentos completos (LlmToolCallStart)
tool-output-available ← resultado (LlmToolCallFinish)
```

Actualmente `SseMapper` emite todo excepto `tool-input-start`. Hay que añadir un `HashSet<String>` para rastrear qué `tool_id` ya recibió su `tool-input-start` y emitirlo una sola vez.

- [ ] **Step 1: Escribir test que falla — secuencia completa de tool call**

En `src/libs/colmena/src/dag_engine/sse_mapper.rs`, añadir al final del archivo:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::domain::events::DagExecutionEvent;

    fn tool_call_sequence() -> Vec<DagExecutionEvent> {
        vec![
            DagExecutionEvent::LlmToolCall {
                node_id: "llm_1".into(),
                tool_id: "call_abc".into(),
                tool_name: "getWeather".into(),
                args_chunk: "{\"city\"".into(),
            },
            DagExecutionEvent::LlmToolCall {
                node_id: "llm_1".into(),
                tool_id: "call_abc".into(),
                tool_name: "getWeather".into(),
                args_chunk: ":\"SF\"}".into(),
            },
            DagExecutionEvent::LlmToolCallStart {
                node_id: "llm_1".into(),
                tool_id: "call_abc".into(),
                tool_name: "getWeather".into(),
                tool_args: "{\"city\":\"SF\"}".into(),
            },
            DagExecutionEvent::LlmToolCallFinish {
                node_id: "llm_1".into(),
                tool_id: "call_abc".into(),
                success: true,
                output: "{\"weather\":\"sunny\"}".into(),
            },
        ]
    }

    #[test]
    fn test_tool_input_start_emitted_once_before_first_delta() {
        let mut mapper = SseMapper::new();
        let events = tool_call_sequence();

        // First LlmToolCall → should emit tool-input-start THEN tool-input-delta
        let parts = mapper.map(&events[0]);
        assert_eq!(parts.len(), 2, "expected [tool-input-start, tool-input-delta]");
        assert_eq!(parts[0]["type"], "tool-input-start");
        assert_eq!(parts[0]["toolCallId"], "call_abc");
        assert_eq!(parts[0]["toolName"], "getWeather");
        assert_eq!(parts[1]["type"], "tool-input-delta");

        // Second LlmToolCall (same tool_id) → only tool-input-delta, no duplicate start
        let parts2 = mapper.map(&events[1]);
        assert_eq!(parts2.len(), 1, "expected only tool-input-delta on repeat");
        assert_eq!(parts2[0]["type"], "tool-input-delta");
    }

    #[test]
    fn test_tool_input_available_and_output() {
        let mut mapper = SseMapper::new();
        let events = tool_call_sequence();

        // Warm up seen_tool_ids with the first call
        mapper.map(&events[0]);

        // LlmToolCallStart → tool-input-available
        let parts = mapper.map(&events[2]);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "tool-input-available");
        assert_eq!(parts[0]["toolName"], "getWeather");

        // LlmToolCallFinish → tool-output-available
        let parts = mapper.map(&events[3]);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "tool-output-available");
    }

    #[test]
    fn test_subgraph_tool_input_start_emitted_once() {
        let mut mapper = SseMapper::new();
        let inner = DagExecutionEvent::LlmToolCall {
            node_id: "inner_llm".into(),
            tool_id: "call_xyz".into(),
            tool_name: "search".into(),
            args_chunk: "{\"q\"".into(),
        };
        let event = DagExecutionEvent::SubgraphWrapped {
            inner: Box::new(inner.clone()),
        };

        // First wrapped LlmToolCall → subgraph-tool-input-start + subgraph-tool-input-delta
        let parts = mapper.map(&event);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "subgraph-tool-input-start");
        assert_eq!(parts[0]["toolCallId"], "call_xyz");
        assert_eq!(parts[1]["type"], "subgraph-tool-input-delta");

        // Same tool_id again → only subgraph-tool-input-delta
        let parts2 = mapper.map(&event);
        assert_eq!(parts2.len(), 1);
        assert_eq!(parts2[0]["type"], "subgraph-tool-input-delta");
    }
}
```

- [ ] **Step 2: Ejecutar tests para verificar que fallan**

```bash
cargo test --lib sse_mapper -p colmena_dag_engine 2>&1 | tail -30
```

Esperado: FAIL — `test_tool_input_start_emitted_once_before_first_delta` falla porque `parts.len() == 1` (solo delta, no start).

- [ ] **Step 3: Añadir `seen_tool_ids` al struct `SseMapper`**

En `src/libs/colmena/src/dag_engine/sse_mapper.rs`, cambiar la línea del import y el struct:

```rust
// Línea 1 — cambiar de:
use std::collections::HashMap;
// a:
use std::collections::{HashMap, HashSet};
```

```rust
// El struct SseMapper — cambiar de:
pub struct SseMapper {
    text_block_ids: HashMap<String, String>,
    node_types: HashMap<String, String>,
    total_prompt_tokens: u64,
    total_completion_tokens: u64,
    total_thinking_tokens: u64,
    total_cache_read_tokens: u64,
    total_cache_write_tokens: u64,
}
// a:
pub struct SseMapper {
    text_block_ids: HashMap<String, String>,
    node_types: HashMap<String, String>,
    seen_tool_ids: HashSet<String>,
    total_prompt_tokens: u64,
    total_completion_tokens: u64,
    total_thinking_tokens: u64,
    total_cache_read_tokens: u64,
    total_cache_write_tokens: u64,
}
```

- [ ] **Step 4: Inicializar `seen_tool_ids` en `new()`**

```rust
// En impl SseMapper, fn new() — cambiar de:
pub fn new() -> Self {
    Self {
        text_block_ids: HashMap::new(),
        node_types: HashMap::new(),
        total_prompt_tokens: 0,
        total_completion_tokens: 0,
        total_thinking_tokens: 0,
        total_cache_read_tokens: 0,
        total_cache_write_tokens: 0,
    }
}
// a:
pub fn new() -> Self {
    Self {
        text_block_ids: HashMap::new(),
        node_types: HashMap::new(),
        seen_tool_ids: HashSet::new(),
        total_prompt_tokens: 0,
        total_completion_tokens: 0,
        total_thinking_tokens: 0,
        total_cache_read_tokens: 0,
        total_cache_write_tokens: 0,
    }
}
```

- [ ] **Step 5: Emitir `tool-input-start` en la Fase 1 (state management)**

En el `match event` de la Fase 1 (bloque que empieza en línea 48), añadir dos nuevos brazos **antes** del `_ => {}` final. El match de `SubgraphWrapped` ya existe — hay que añadir `LlmToolCall` dentro de él:

```rust
// Añadir después de DagExecutionEvent::LlmUsage { ... } => { ... }
// y ANTES del DagExecutionEvent::SubgraphWrapped existente:
DagExecutionEvent::LlmToolCall { tool_id, tool_name, .. } => {
    if !self.seen_tool_ids.contains(tool_id) {
        self.seen_tool_ids.insert(tool_id.clone());
        parts.push(json!({
            "type": "tool-input-start",
            "toolCallId": tool_id,
            "toolName": tool_name
        }));
    }
}
```

Luego, dentro del `DagExecutionEvent::SubgraphWrapped { inner } => match inner.as_ref()` existente, añadir antes del `_ => {}` final:

```rust
DagExecutionEvent::LlmToolCall { tool_id, tool_name, .. } => {
    if !self.seen_tool_ids.contains(tool_id) {
        self.seen_tool_ids.insert(tool_id.clone());
        parts.push(json!({
            "type": "subgraph-tool-input-start",
            "toolCallId": tool_id,
            "toolName": tool_name
        }));
    }
}
```

- [ ] **Step 6: Verificar que los tests pasan**

```bash
cargo test --lib sse_mapper -p colmena_dag_engine 2>&1 | tail -20
```

Esperado: todos los tests `ok`.

- [ ] **Step 7: Verificar que no hay regresiones en el resto del crate**

```bash
cargo check -p colmena_dag_engine 2>&1 | tail -20
```

Esperado: sin errores.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/dag_engine/sse_mapper.rs
git commit -m "feat(sse): add tool-input-start and subgraph-tool-input-start to SseMapper

Emits tool-input-start once per tool_id (on first LlmToolCall) to comply
with the Vercel AI SDK Data Stream Protocol before tool-input-delta chunks.
Tracks seen tool_ids in a HashSet to avoid duplicate starts on multi-chunk calls."
```

---

## Task 2: Migrar `run_dag()` stdout path en `api.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/api.rs` (líneas 57–409)

### Contexto

`run_dag()` tiene un bloque `if is_stream { ... }` con ~330 líneas de lógica inline duplicada. Se reemplaza todo ese bloque con `SseMapper`. El valor de retorno `final_output` sigue capturándose via `GraphFinish` antes de pasarlo al mapper.

- [ ] **Step 1: Reemplazar el bloque `if is_stream` en `run_dag()`**

Localizar la línea `if is_stream {` (aprox línea 57) y reemplazar **todo** el bloque hasta el `} else {` que llama a `engine.run_dag` (aprox línea 404), dejando el `else` intacto:

```rust
if is_stream {
    use crate::dag_engine::domain::events::DagExecutionEvent;
    use crate::dag_engine::sse_mapper::SseMapper;
    use futures::StreamExt;

    let mut mapper = SseMapper::new();
    let mut final_output: Value = Value::Null;

    // Global START marker (compatible with Vercel AI SDK)
    println!(
        "data: {}\n",
        serde_json::json!({
            "type": "start",
            "messageId": format!("msg_{}", uuid::Uuid::new_v4())
        })
    );

    let internal_stream = engine.execute_stream(
        graph,
        resume_id.clone(),
        resume_answer.clone(),
        include_extra_info,
    );
    tokio::pin!(internal_stream);

    while let Some(result) = internal_stream.next().await {
        let event = match result {
            Ok(ev) => ev,
            Err(e) => {
                println!(
                    "data: {}\n",
                    serde_json::json!({ "type": "error", "errorText": e.to_string() })
                );
                continue;
            }
        };

        // Capture final output for return value (before mapper consumes the event)
        if let DagExecutionEvent::GraphFinish { output } = &event {
            final_output = output.clone();
        }

        for part in mapper.map(&event) {
            println!("data: {}\n", part);
        }
    }

    println!("data: [DONE]\n");
    Ok(final_output)
```

Los `use` statements que estaban al inicio del bloque original (`use crate::dag_engine::domain::events::DagExecutionEvent;`) se pueden eliminar del bloque anterior si ya no se usan fuera — pero son safe de dejar con `use` local.

- [ ] **Step 2: Verificar que compila**

```bash
cargo check -p colmena_dag_engine 2>&1 | tail -20
```

Esperado: sin errores. Si hay `unused import` warnings en `api.rs`, eliminar los imports que ya no se usan (los `HashMap`, `HashSet`, `DagExecutionEvent` que estaban en el bloque inline).

- [ ] **Step 3: Smoke test en modo stdout**

```bash
source .env && cargo run --bin dag_engine -- run tests/graphs/agents/llm_call.json 2>&1 | grep '"type"' | head -20
```

Esperado: ver `node-start`, `text-start`, `text-delta`, `text-end`, `node-end`, `usage-summary`, `finish`.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/api.rs
git commit -m "refactor(api): use SseMapper in run_dag() stdout streaming path

Removes ~300 lines of duplicated inline event mapping. SseMapper is now
the single source of truth for DagExecutionEvent → SSE JSON translation."
```

---

## Task 3: Migrar `handler_webhook()` HTTP SSE path en `api.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/api.rs` (bloque `if is_sse` en `handler_webhook`, aprox líneas 548–980)

### Contexto

`handler_webhook()` tiene un bloque `if is_sse` con el stream Axum que replica toda la lógica del mapper inline. Se reemplaza con `SseMapper`. El loop multi-turno y los marcadores de turno se mantienen exactamente igual — solo la parte de traducción de eventos cambia. Se crea un `SseMapper::new()` por turno (el mapper es stateful y debe resetearse entre turnos).

- [ ] **Step 1: Reemplazar el cuerpo del `async_stream::stream!` en `handler_webhook`**

Localizar el `let protocol_stream = async_stream::stream! {` (aprox línea 556) y reemplazar **todo el cuerpo** del stream hasta el `};` que lo cierra (aprox línea 968), manteniendo las líneas de `Sse::new(protocol_stream)` y headers intactas:

```rust
let protocol_stream = async_stream::stream! {
    use crate::dag_engine::domain::events::DagExecutionEvent;
    use crate::dag_engine::sse_mapper::SseMapper;

    let is_loop = params.get("loop").map(|v| v == "true").unwrap_or(false);
    let mut turn_count = 1;
    let mut current_graph = graph_instance;

    loop {
        // Fresh mapper per turn — resets all stateful tracking (text blocks, tool ids, tokens)
        let mut mapper = SseMapper::new();
        let mut final_output_value: Option<Value> = None;

        if is_loop {
            yield Ok::<Event, std::io::Error>(
                Event::default()
                    .json_data(serde_json::json!({
                        "type": "text-delta",
                        "id": format!("txt_sys_{}", uuid::Uuid::new_v4()),
                        "delta": format!("\n\n*--- Starting Turn {} ---*\n\n", turn_count)
                    }))
                    .expect("json_data"),
            );
        }

        let internal_stream = engine.execute_stream(current_graph.clone(), None, None, false);
        tokio::pin!(internal_stream);

        while let Some(result) = internal_stream.next().await {
            let event = match result {
                Ok(ev) => ev,
                Err(e) => {
                    yield Ok(
                        Event::default()
                            .json_data(serde_json::json!({
                                "type": "error",
                                "errorText": e.to_string()
                            }))
                            .expect("json_data"),
                    );
                    continue;
                }
            };

            // Capture final output before mapper consumes the event (needed for loop control)
            if let DagExecutionEvent::GraphFinish { output } = &event {
                final_output_value = Some(output.clone());
            }

            for part in mapper.map(&event) {
                yield Ok(Event::default().json_data(part).expect("json_data"));
            }
        }

        // --- Loop control (unchanged logic) ---
        let mut should_stop_loop = !is_loop;

        if let Some(out) = final_output_value.as_ref() {
            if let Some(obj) = out.as_object() {
                let find_status = |o: &serde_json::Map<String, serde_json::Value>, key: &str| -> Option<String> {
                    if let Some(v) = o.get(key) {
                        return v.as_str().map(|s| s.to_string());
                    }
                    for (_, val) in o {
                        if let Some(child_obj) = val.as_object() {
                            if let Some(v) = child_obj.get(key) {
                                return v.as_str().map(|s| s.to_string());
                            }
                        }
                    }
                    None
                };

                let find_bool = |o: &serde_json::Map<String, serde_json::Value>, key: &str| -> bool {
                    if let Some(v) = o.get(key).and_then(|v| v.as_bool()) {
                        if v { return true; }
                    }
                    for (_, val) in o {
                        if let Some(child_obj) = val.as_object() {
                            if let Some(v) = child_obj.get(key).and_then(|v| v.as_bool()) {
                                if v { return true; }
                            }
                        }
                    }
                    false
                };

                if let Some(status) = find_status(obj, "__colmena_status") {
                    if status == "SUSPENDED" {
                        should_stop_loop = true;
                    }
                }
                if let Some(loop_status) = find_status(obj, "__colmena_loop_status") {
                    if loop_status == "FINISHED" {
                        should_stop_loop = true;
                    }
                }
                if find_bool(obj, "__colmena_is_output_node") {
                    should_stop_loop = true;
                }
            }

            if !should_stop_loop {
                for (_, node) in current_graph.nodes.iter_mut() {
                    if node.node_type == "trigger_webhook" || node.node_type == "input" {
                        if node.config.is_null() {
                            node.config = serde_json::json!({});
                        }
                        node.config["__payload__"] = out.clone();
                    }
                }
                turn_count += 1;
            }
        } else {
            // No output = graph crashed or finished empty
            should_stop_loop = true;
        }

        if should_stop_loop {
            break;
        }
    }

    yield Ok(Event::default().data("[DONE]"));
};
```

Después del `};`, las líneas de `Sse::new(...)`, `keep_alive`, `into_response()`, y el header `x-vercel-ai-ui-message-stream` **permanecen exactamente igual**.

- [ ] **Step 2: Eliminar imports que ya no se usan en handler_webhook**

Dentro del bloque `if is_sse` en `handler_webhook`, los imports que estaban al inicio ya no se necesitan individualmente. Verificar que el `use crate::dag_engine::domain::events::DagExecutionEvent;` y `use axum::response::sse::{Event, KeepAlive, Sse};` siguen presentes (los necesita el nuevo código).

- [ ] **Step 3: Verificar que compila**

```bash
cargo check -p colmena_dag_engine 2>&1 | tail -20
```

Esperado: sin errores. Eliminar cualquier `unused variable` / `unused import` que aparezca.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/api.rs
git commit -m "refactor(api): use SseMapper in handler_webhook() HTTP SSE stream

Removes ~350 lines of duplicated inline event mapping in the webhook handler.
SseMapper is instantiated fresh per loop turn to reset stateful tracking.
Fixes: finish event now includes output, subgraph-text-delta uses correct
'id' field, subgraph events now include reasoning/skill/usage-summary."
```

---

## Task 4: Añadir SSE real a `handler_resume()`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/api.rs` (función `handler_resume`, aprox líneas 1144–1190)

### Contexto

`handler_resume()` detecta el header `Accept: text/event-stream` pero tiene un comentario `// SSE not fully supported yet on /resume, falling back to JSON`. Se implementa el path SSE usando `SseMapper` igual que en `handler_webhook`, sin loop (una sola ejecución de reanudación).

- [ ] **Step 1: Reemplazar el bloque `if is_sse` en `handler_resume`**

Localizar el bloque (aprox líneas 1161–1166):
```rust
if is_sse {
    // ... We could duplicate the SSE stream runner here...
    eprintln!("⚠️ SSE not fully supported yet on /resume, falling back to JSON");
}
```

Reemplazarlo con:

```rust
if is_sse {
    use crate::dag_engine::domain::events::DagExecutionEvent;
    use crate::dag_engine::sse_mapper::SseMapper;
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures::StreamExt;

    let engine = state.engine.clone();
    let session_id = payload.session_id.clone();
    let answer = payload.answer.clone();

    let protocol_stream = async_stream::stream! {
        let mut mapper = SseMapper::new();

        let internal_stream = engine.execute_stream(
            graph_instance,
            Some(session_id),
            Some(answer),
            false,
        );
        tokio::pin!(internal_stream);

        while let Some(result) = internal_stream.next().await {
            let event = match result {
                Ok(ev) => ev,
                Err(e) => {
                    yield Ok::<Event, std::io::Error>(
                        Event::default()
                            .json_data(serde_json::json!({
                                "type": "error",
                                "errorText": e.to_string()
                            }))
                            .expect("json_data"),
                    );
                    continue;
                }
            };

            for part in mapper.map(&event) {
                yield Ok(Event::default().json_data(part).expect("json_data"));
            }
        }

        yield Ok(Event::default().data("[DONE]"));
    };

    let mut response = Sse::new(protocol_stream)
        .keep_alive(KeepAlive::default())
        .into_response();

    response.headers_mut().insert(
        "x-vercel-ai-ui-message-stream",
        axum::http::HeaderValue::from_static("v1"),
    );

    return response;
}
```

- [ ] **Step 2: Verificar que compila**

```bash
cargo check -p colmena_dag_engine 2>&1 | tail -20
```

Esperado: sin errores.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/dag_engine/api.rs
git commit -m "feat(api): add SSE streaming support to handler_resume()

Previously /resume ignored Accept: text/event-stream and fell back to JSON.
Now uses SseMapper to stream events identically to handler_webhook()."
```

---

## Task 5: Actualizar documentación SSE

**Files:**
- Modify: `docs/sse_events_reference.md`

- [ ] **Step 1: Añadir `tool-input-start` y `subgraph-tool-input-start` a la sección de herramientas**

En `docs/sse_events_reference.md`, localizar la sección `## Eventos de herramientas — nodo \`llm\` con tool calling` y reemplazarla con:

```markdown
## Eventos de herramientas — nodo `llm` con tool calling

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `tool-input-start` | `toolCallId`, `toolName` | Primer chunk de argumentos del tool (una vez por tool_id) |
| `tool-input-delta` | `toolCallId`, `inputTextDelta` | Chunk de argumentos del tool (streaming) |
| `tool-input-available` | `toolCallId`, `toolName`, `input` | Argumentos del tool completos |
| `tool-output-available` | `toolCallId`, `output` | El tool terminó de ejecutarse |

La secuencia completa para un tool call:

```
tool-input-start      { toolCallId: "call_abc", toolName: "getWeather" }
tool-input-delta      { toolCallId: "call_abc", inputTextDelta: "{\"city\"" }
tool-input-delta      { toolCallId: "call_abc", inputTextDelta: ":\"SF\"}" }
tool-input-available  { toolCallId: "call_abc", toolName: "getWeather", input: {"city":"SF"} }
tool-output-available { toolCallId: "call_abc", output: {"weather":"sunny"} }
```
```

Hacer lo mismo en la sección `### Herramientas` dentro de `## Eventos de subgrafo`:

```markdown
### Herramientas

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `subgraph-tool-input-start` | `toolCallId`, `toolName` | Primer chunk de argumentos de tool interno |
| `subgraph-tool-input-delta` | `toolCallId`, `inputTextDelta` | Chunk de argumentos de tool interno |
| `subgraph-tool-input-available` | `toolCallId`, `toolName`, `input` | Argumentos del tool interno completos |
| `subgraph-tool-output-available` | `toolCallId`, `output` | Tool interno terminó |
```

- [ ] **Step 2: Actualizar el ejemplo de flujo con tool calling**

En la sección `## Flujo completo de ejemplo`, actualizar el ejemplo de LLM con tools:

```
node-start              { node_id: "llm_agent", node_type: "llm" }
  text-start            { id: "txt_abc123" }
  text-delta            { id: "txt_abc123", delta: "Voy a " }
  text-delta            { id: "txt_abc123", delta: "buscar..." }
  tool-input-start      { toolCallId: "tc_1", toolName: "search" }
  tool-input-delta      { toolCallId: "tc_1", inputTextDelta: '{"q"' }
  tool-input-delta      { toolCallId: "tc_1", inputTextDelta: '":"..."}' }
  tool-input-available  { toolCallId: "tc_1", toolName: "search", input: { q: "..." } }
  tool-output-available { toolCallId: "tc_1", output: { results: [...] } }
  text-end              { id: "txt_abc123" }
node-end                { node_id: "llm_agent" }
```

- [ ] **Step 3: Commit**

```bash
git add docs/sse_events_reference.md
git commit -m "docs(sse): document tool-input-start and subgraph-tool-input-start events"
```

---

## Verificación final

- [ ] **Compilación limpia completa**

```bash
cargo build -p colmena_dag_engine 2>&1 | tail -10
```

Esperado: `Finished` sin errores ni warnings relevantes.

- [ ] **Todos los tests pasan**

```bash
cargo test --lib -p colmena_dag_engine 2>&1 | tail -20
```

Esperado: todos `ok`.

- [ ] **Smoke test modo `run` con tool calling**

```bash
source .env && cargo run --bin dag_engine -- run tests/graphs/agents/llm_call.json 2>&1 | grep -E '"type"' | head -30
```

Esperado: en la secuencia de tool calls aparece `tool-input-start` antes del primer `tool-input-delta` para cada `toolCallId`.
