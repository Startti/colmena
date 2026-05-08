# Diseño: `llm_call` propaga `SUSPENDED` desde un tool y reanuda

**Estado:** propuesta aprobada
**Fecha:** 2026-05-08
**Autor:** Daniel García (Startti)

## Contexto

Validamos en pruebas reales que cuando un tool LLM (p. ej. `ask_secret` apuntando a `secure_suspend`) retorna `__colmena_status: "SUSPENDED"`, el `agent_service` lo trata como un tool result normal: lo serializa como string, lo añade a la conversación, y continúa el loop. El LLM recibe un objeto JSON raro y termina rindiéndose.

```
data: {"type":"tool-output-available","output":{"__colmena_status":"SUSPENDED","questions":[...]}}
data: {"type":"node-end","output":{"result":"...necesito tus credenciales..."}}  // ← LLM se rinde
```

El motor del DAG no se entera. No hay pausa, no hay forma de reanudar.

Para que `secure_suspend` (y cualquier futura tool suspendible) funcione invocada desde un agente LLM, necesitamos que:

1. **Detección + propagación:** el tool result con `__colmena_status: "SUSPENDED"` interrumpe el loop del agente y propaga el estado SUSPENDED hacia arriba — `llm_call` lo emite como output, el motor del DAG pausa.
2. **Resume:** cuando el motor reanuda con `__colmena_resume_answer`, `llm_call` reconstruye el contexto desde memoria de conversación, ejecuta el tool pendiente con la respuesta del usuario, agrega el resultado a la historia, y continúa el loop normal del agente.

## Decisiones tomadas en brainstorming

- **Estrategia de resume:** **Opción A — replay desde memoria de conversación** (decidido por el usuario). Reusa la persistencia existente (`connection_url` con `conversation_repository`).
- **Pre-condición:** todos los grafos relevantes ya tienen `connection_url` configurado.

## Diseño

### 1. Extender `LlmResponse` con un campo de suspend

`src/libs/colmena/src/llm/domain/response.rs` (o donde viva `LlmResponse`):

```rust
pub struct LlmResponse {
    // ... campos existentes ...
    suspend: Option<SuspendInfo>,
}

pub struct SuspendInfo {
    pub tool_call_id: String,
    pub questions: Value,    // el array `questions` que vino del tool
    pub raw_output: String,  // el output crudo del tool (para auditoría)
}

impl LlmResponse {
    pub fn suspend(&self) -> Option<&SuspendInfo> { self.suspend.as_ref() }
    pub fn suspended(tool_call_id: String, questions: Value, raw_output: String) -> Self { ... }
}
```

`SuspendInfo` se serializa fácilmente por si llega al log; pero en práctica se intercepta antes.

### 2. `agent_service.rs` — detectar SUSPENDED en tool result

En `agent_service.rs:257` (justo después de `tool_executor.execute(tool_call)`):

```rust
let result = match tool_executor.execute(tool_call).await {
    Ok(res) => res,
    Err(e) => ToolResult { /* ... existente ... */ },
};

// NUEVO: detectar suspend ANTES de añadir el resultado a la conversación.
if let Ok(parsed) = serde_json::from_str::<Value>(&result.output) {
    if parsed.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED") {
        // Persistir el assistant-message con el tool_call (no su result aún)
        // para que en resume sepamos qué tool quedó pendiente.
        if let Some(asst_msg) = response.assistant_message_with_tool_calls() {
            self.conversation_repository
                .add_message(session_id, asst_msg)
                .await?;
        }
        let questions = parsed.get("questions").cloned().unwrap_or(Value::Null);
        return Ok(LlmResponse::suspended(
            tool_call.id.clone(),
            questions,
            result.output,
        ));
    }
}

// Resto del flujo existente: añadir tool_message, continuar.
```

Detalles:
- **Solo se persiste el assistant-message** (con tool_call). No se persiste tool_message porque no tenemos uno todavía.
- En resume, identificamos el último assistant-message con tool_call sin tool_result correspondiente — ese es el suspendido.
- Si el LLM hizo múltiples tool_calls en paralelo y uno suspendió, los demás aún no se ejecutaron en el loop serial actual. Solo el primero suspendido para. Aceptable.

### 3. `llm.rs` — emitir SUSPENDED al DAG

Después de `agent_service.run()` (~línea 1152):

```rust
let response = agent_service.run(params).await?;

if let Some(suspend) = response.suspend() {
    return Ok(json!({
        "__colmena_status": "SUSPENDED",
        "questions": suspend.questions.clone(),
        "_pending_tool_call_id": suspend.tool_call_id.clone(),
        "_conversation_key": conversation_key.clone(),
    }));
}

// Resto del flujo normal (extra_info, write_to_memory, etc.).
```

`_pending_tool_call_id` y `_conversation_key` son metadatos que solo `llm_call` consume al reanudar — el motor del DAG los pasa transparentemente.

### 4. `llm.rs` — resume path

En la entrada de `LlmNode::execute`, antes del flujo normal, detectar resume:

```rust
async fn execute(&self, inputs: &NodeInputs, config: &Value, ...) -> Result<...> {
    if let Some(resume_answer) = inputs.get("__colmena_resume_answer").and_then(|v| v.as_str()) {
        return self.resume_from_suspended_tool(resume_answer, inputs, config, ...).await;
    }
    // ... flujo normal ...
}
```

`resume_from_suspended_tool` hace:

1. Resolver `conversation_key` (igual que el flujo normal: `session_id` desde config + sufijo).
2. Cargar historial: `let messages = self.conversation_repository.load(&conversation_key).await?;`
3. Identificar el último assistant-message con tool_call sin tool_result subsiguiente.
   - Si no hay → error: "no pending tool call to resume" (caso defensivo, no debería pasar).
4. Re-ejecutar ese tool inyectando el `__colmena_resume_answer` en sus inputs:
   ```rust
   let result = tool_executor
       .execute_with_resume_answer(&pending_tool_call, resume_answer)
       .await?;
   ```
5. Inspeccionar `result.output` por si TAMBIÉN trae `__colmena_status: SUSPENDED` (multi-suspend). Si sí, propagar de nuevo.
6. Persistir el `tool_message`:
   ```rust
   let tool_msg = LlmMessage::tool(pending_tool_call.id.clone(), result.output.clone())?;
   self.conversation_repository.add_message(&conversation_key, tool_msg.clone()).await?;
   messages.push(tool_msg);
   ```
7. Llamar `agent_service.continue_with_messages(...)` (nueva variante) o construir un `AgentRunParams` con `messages` poblado y un prompt vacío/stub. Procesa la siguiente respuesta del LLM normalmente — puede ser texto final o más tool_calls.
8. Retornar el output normal `{ result, extra_info }`.

### 5. `agent_service.rs` — modo continue

Hoy `agent_service.run` siempre añade el `prompt` como nuevo `user` message. En resume, no queremos añadir un prompt nuevo — queremos continuar la conversación.

Dos opciones:

- **a)** Hacer el `prompt` opcional en `AgentRunParams` (`Option<String>`). Si es `None`, no se añade un user message.
- **b)** Nuevo método `agent_service.continue_run(params_without_prompt)`.

Recomendación: **(a)**, cambio chico. Si `prompt` es `Some`, comportamiento actual; si es `None`, asume que `messages` ya tiene todo lo necesario y arranca el loop directamente.

### 6. `DagToolExecutor` — `execute_with_resume_answer`

`src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`:

```rust
pub async fn execute_with_resume_answer(
    &self,
    tool_call: &ToolCall,
    resume_answer: &str,
) -> Result<ToolResult, LlmError> {
    // Misma lógica que `execute`, pero antes de pasar `inputs` al nodo,
    // inserta `__colmena_resume_answer` en el HashMap.
    // ...
    inputs.insert(
        "__colmena_resume_answer".to_string(),
        Value::String(resume_answer.to_string()),
    );
    // ... continúa con node.execute(&inputs, ...) ...
}
```

Refactor: extraer la lógica común de `execute` a un helper privado y que ambas variantes (con/sin resume) llamen a ese helper.

## Flujo end-to-end (Amadeus simplificado)

```
1. [User] cargo run -- run agent.json --session-id S1
2. [LLM] tool_call: ask_secret(secrets:[{question:"client_id?",name:"a"},{question:"client_secret?",name:"b"}])
3. [secure_suspend] emite {__colmena_status:"SUSPENDED", questions:[Q1,Q2]}
4. [agent_service] detecta SUSPENDED, persiste assistant-msg con tool_call, retorna LlmResponse::suspended
5. [llm_call] retorna {__colmena_status:"SUSPENDED", questions:[Q1,Q2], _pending_tool_call_id:"call_xxx"}
6. [DAG engine] propaga SUSPENDED hacia el cliente. Pausa.
7. [User] cargo run -- run agent.json --session-id S1 --answer "client_id?\nABC\nclient_secret?\nXYZ"
8. [DAG engine] re-invoca llm_call con __colmena_resume_answer en inputs.
9. [llm_call] detecta resume. Carga messages. Encuentra pending tool_call.
10. [tool_executor] execute_with_resume_answer → inputs incluye __colmena_resume_answer="..."
11. [secure_suspend] resume-path: persiste handles, retorna {status:"resumed", handles:{a:"<sv_a>",b:"<sv_b>"}}
12. [llm_call] persiste tool_msg con handles. Llama agent_service.continue_run con messages.
13. [LLM] siguiente respuesta — usa los handles para construir el body del próximo tool call (dummy_login).
14. [tool_executor] dummy_login con body conteniendo handles → inject_secrets reemplaza por valores reales → HTTP enviado al server real.
15. [Server] recibe valores reales, responde.
16. [LLM] presenta resumen al usuario.
```

## Modos de falla

- **Pending tool no encontrado en historial:** error explícito `"resume: no pending tool call in conversation history"`. Probablemente significa que el historial no se persistió correctamente o que llamamos resume sin un suspend previo.
- **Tool re-ejecutado vuelve a suspender:** detectar el patrón y propagar SUSPENDED de nuevo. El motor del DAG pausa otra vez. Nesting natural.
- **`connection_url` no configurado:** sin memoria persistente no podemos hacer resume. En la entrada del resume path, validar que `conversation_repository` tiene un backing real (no in-memory). Si no, fallar con mensaje claro: `"resume requires connection_url for conversation memory"`. Documentar como pre-condición de uso.
- **Múltiples tool_calls en paralelo, uno suspende:** el loop serial actual de agent_service ejecuta uno a uno. El primer SUSPENDED aborta. Los demás se ejecutan al resume después de que el LLM continúe (porque el LLM ve el tool_result del que suspendió, decide si los demás siguen siendo necesarios, y los re-pide o no). Aceptable para v1.
- **Suspend sin `_pending_tool_call_id` al resume:** el `llm_call` puede inferirlo del historial (último assistant-msg con tool_call sin tool_result). El metadato es defensivo.

## Plan de testing

### Tests unitarios

1. `agent_service::detects_suspended_tool_result_and_short_circuits` — mock `ToolExecutor::execute` retornando `output` con `__colmena_status:"SUSPENDED"`. Verificar:
   - El loop NO continúa.
   - Retorna `LlmResponse::suspended(...)` con tool_call_id correcto y questions extraídas.
   - El `assistant_message_with_tool_calls` se persistió.
   - El `tool_message` NO se persistió.
2. `llm_node::propagates_suspended_to_dag_output` — usa el agent_service real con tool_executor mock que retorna SUSPENDED. Verifica que el output del nodo es `{__colmena_status:"SUSPENDED", questions:..., _pending_tool_call_id:..., _conversation_key:...}`.
3. `llm_node::resume_with_answer_executes_pending_tool_and_continues` — pre-popular conversation con assistant-msg suspendido. Re-invocar con `__colmena_resume_answer`. Verificar que:
   - Se llama al tool con `__colmena_resume_answer` en inputs.
   - Se persiste el tool_message resultante.
   - Se llama agent_service en modo continue (sin prompt nuevo).
4. `dag_tool_executor::execute_with_resume_answer_injects_into_inputs` — verifica que el método nuevo añade `__colmena_resume_answer` a los inputs antes de `node.execute`.

### Test de integración

Un grafo nuevo `tests/graphs/advanced/llm_secure_suspend_resume.json` que use el flujo Amadeus simplificado (LLM agent → ask_secret tool → http_request tool). Con DATABASE_URL real:

1. Run 1: LLM llama ask_secret, suspend.
2. Run 2: resume con --answer, verificar que el HTTP final llega con valores reales.

Marcado `#[ignore]` por dependencia de DATABASE_URL + GEMINI_API_KEY.

## Pre-requisitos / fuera de alcance

**Pre-requisitos:**
- Gap 1 (inject_secrets en config) — ya cerrado en commit `37f93f3`.
- Connection_url debe estar configurado para flujos LLM con suspend de tools (documentar).

**Fuera de alcance:**
- Suspend de múltiples tool_calls en paralelo en una sola iteración del loop. v1 maneja serialmente.
- Cambios al protocolo del DAG engine — los metadatos `_pending_tool_call_id` y `_conversation_key` viajan dentro del output del nodo, transparente al motor.
- UI/UX — el ADP frontend ya sabe renderizar `questions[]` para suspend (no cambia).

## Cambios concretos al repo

| Archivo | Acción |
|---|---|
| `src/libs/colmena/src/llm/domain/response.rs` (o donde viva LlmResponse) | Añadir `suspend: Option<SuspendInfo>` + `SuspendInfo` struct + métodos. |
| `src/libs/colmena/src/llm/application/agent_service.rs` | Detectar SUSPENDED en tool result, persistir assistant-msg, retornar early. Hacer `prompt` opcional en `AgentRunParams`. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` | Resume path: detectar `__colmena_resume_answer`, cargar historial, ejecutar pending tool, persistir tool_msg, continuar. |
| `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` | Nuevo método `execute_with_resume_answer`. |
| `tests/graphs/advanced/llm_secure_suspend_resume.json` | Grafo de integración. |
| `src/libs/colmena/tests/llm_tool_suspend_integration.rs` | Test de integración nuevo (ignored). |

Estimado total: ~150-200 LoC + tests.
