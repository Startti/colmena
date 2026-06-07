# Spec — Fix de ruteo de `__colmena_resume_answer` (Enfoque B)

**Fecha:** 2026-06-05
**Autor:** equipo Colmena (a partir de bug report ADP 2026-06-04)
**Estado:** implementado en branch `claude/affectionate-agnesi-488291` (commits `f9f7242`..`189c3f7`)
**Tipo:** bugfix de motor + defense-in-depth en `llm_call`

---

## 1. Resumen ejecutivo

Hoy, durante un **resume** de DAG (reanudación tras un `suspend`), el engine inyecta el input mágico `__colmena_resume_answer` en **todos los nodos** del run, no solo en el nodo que estaba suspendido. Cualquier nodo aguas abajo del `suspend` que también lea esa key entra a su rama de "reanudar" cuando debería correr fresh.

El caso más visible — reportado por ADP el 2026-06-04 — es `suspend → llm_call`: el `llm_call` entra a su rama de "reanudar tool call pendiente", no encuentra ninguno en su historial, y aborta con:

```
llm_call resume: no pending tool call found in conversation history
```

El mismo defecto rompe también `suspend → suspend` (cascada), porque el segundo `suspend` recibe la key, intenta parsear un Q/A con un id que no le corresponde, y falla con `missing answer`.

Este documento especifica el fix:

1. **Engine:** restringir la inyección de `__colmena_resume_answer` exclusivamente a los nodos que estaban en estado SUSPENDED en el snapshot persistido.
2. **`llm_call`:** agregar un guard defensivo — si el branch de resume no encuentra un tool call pendiente, caer a fresh run en lugar de errorear.

No se modifica el contrato externo del nodo `suspend`, ni el formato Q/A, ni el comportamiento del nodo `llm_call` cuando suspende por su propio tool call (que sigue funcionando idéntico).

---

## 2. Contexto y root cause

### 2.1 Repro mínima (de ADP)

```
input → suspend(id=ek9d...) → llm_call(memoria=on, sin tools) → output_sink
```

Run inicial pausa en `suspend`. Resume con `--answer "Q[ek9d...]: ¿Cuál es tu nombre?\nA[ek9d...]: Julian"` propaga `answer_received: "Julian"` al `llm_call`, pero éste explota antes de hablar con el LLM.

### 2.2 Cadena causal en el código actual

Tres puntos del código en juego:

1. **Inyección masiva del input mágico** — [`run_use_case.rs:377-379`](../../../src/libs/colmena/src/dag_engine/application/run_use_case.rs#L377):

   ```rust
   if let Some(ans) = &resume_answer {
       inputs.insert("__colmena_resume_answer".to_string(), Value::String(ans.clone()));
   }
   ```

   Este bloque corre dentro del loop principal `while let Some(node_id) = active_queue.pop_front()`, una vez por cada nodo. No discrimina si el nodo era el suspendido o no.

2. **`llm_call` detecta la key y entra al branch de resume** — [`llm.rs:820-823`](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs#L820):

   ```rust
   let resume_answer: Option<String> = inputs
       .get("__colmena_resume_answer")
       .and_then(|v| v.as_str())
       .map(|s| s.to_string());
   ```

3. **Branch de resume busca tool call inexistente y aborta** — [`llm.rs:1799-1802`](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs#L1799):

   ```rust
   if let Some(answer) = resume_answer.as_deref() {
       let conversation = conversation_repo.get_by_id(&conversation_key).await?;
       let pending = find_pending_tool_call(&conversation.messages)
           .ok_or("llm_call resume: no pending tool call found in conversation history")?;
       ...
   ```

   El `llm_call` corre por primera vez; su `conversation_key` no tiene un assistant message con tool_use pendiente → `find_pending_tool_call` devuelve `None` → `.ok_or(...)` lanza error.

### 2.3 Daño colateral en `suspend → suspend`

[`suspend.rs:34-47`](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs#L34) lee la misma key:

```rust
if let Some(answer_val) = inputs.get("__colmena_resume_answer") {
    let id = config.get("id").and_then(|v| v.as_str()).ok_or(...)?;
    let mut parsed = parse_qa_response(raw, &[id])
        .map_err(|e| Box::<dyn Error + Send + Sync>::from(format!("suspend: {e}")))?;
```

Si el resume_answer trae `Q[id1]:/A[id1]:` y el segundo `suspend` tiene `id2`, el parser falla con `missing answer for id2`. Misma raíz: la key no debería llegar al segundo `suspend` en su primer run.

### 2.4 Por qué los nodos componibles no se rompen hoy

Tanto [`subgraph.rs:71-101`](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs#L71) como [`orchestrator.rs:1432`](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs#L1432) implementan **manualmente** la disciplina correcta: cuando bubble-up de `SUSPENDED` los pone como nodo "actual" en el restore, ellos thread `__colmena_resume_answer` solo al hijo que estaba suspendido (en orchestrator: `task_inputs.remove("__colmena_resume_answer")` evita el cascadeo). Eso confirma que la convención correcta ya existe — está implementada por-nodo. Lo que falta es subirla un nivel: el **engine** mismo debe respetarla.

### 2.5 Por qué este fix es correcto y no introduce regresiones

- El nodo `suspend` que efectivamente se suspendió **es** el que tiene su output con `__colmena_status: "SUSPENDED"` persistido en `DagRunState.all_outputs`. Esa es la fuente de verdad inequívoca.
- En un cascade orchestrator/subgraph, el nodo wrapper también persiste su propio output bubble-up con `__colmena_status: "SUSPENDED"`. El wrapper mismo entra al set "resuming"; su lógica interna sigue threading hacia el hijo correcto. Cero cambio para esos consumers.
- El caso "LLM-as-parent-with-suspend-tool" (el `llm_call` se suspende a sí mismo dentro de su loop porque uno de sus tools devolvió SUSPENDED): el `llm_call` mismo persiste su output como SUSPENDED y queda en `resuming_node_ids` en el próximo run → recibe el answer → resume su tool pendiente → continúa. Idéntico al comportamiento actual.

---

## 3. Contrato nuevo del engine

### 3.1 Regla formal

> Durante un resume de DAG, el engine inyecta `__colmena_resume_answer` en los `inputs` de un nodo **si y solo si** ese nodo estaba en estado SUSPENDED en el snapshot persistido que se restauró al iniciar el run.

"Estar en estado SUSPENDED en el snapshot" se define como: `state.all_outputs[node_id]` contiene la clave `__colmena_status` con valor `"SUSPENDED"` (búsqueda recursiva via `Self::find_status_by_key`, la misma helper que ya usa el engine para detectar suspensions).

### 3.2 Lo que NO cambia

- El input key sigue siendo `__colmena_resume_answer` (string). No se cambia el formato.
- Los nodos consumidores (`suspend`, `secure_suspend`, `subgraph`, `orchestrator`, `llm_call`) NO cambian su API ni sus chequeos internos de la key. Siguen leyéndola del mismo lugar.
- El formato Q/A `Q[<id>]: ... A[<id>]: ...` se mantiene idéntico.
- El multi-resume en paralelo sigue soportado: si dos `suspend` hermanos quedaron suspendidos en la misma capa, ambos están en `all_outputs` con SUSPENDED, ambos reciben el answer, el parser bindea por id.

---

## 4. Cambios por archivo

### 4.1 `src/libs/colmena/src/dag_engine/application/run_use_case.rs`

#### 4.1.1 Computar `resuming_node_ids` una vez al restaurar el estado

Hoy hay dos puntos donde se restaura `all_outputs` desde el repositorio:

- Branch 1 (resume directo por session_id) — línea 190: `all_outputs = state.all_outputs;`
- Branch 2 (resolve por agent_session_id) — línea 216: `all_outputs = state.all_outputs;`

Después del bloque `match` de restauración (después de la línea 240, donde la lifecycle decision ya cerró), agregar:

```rust
// Build the resuming set BEFORE the main loop. The loop's `all_outputs.remove(&node_id)`
// at line 343 destroys the SUSPENDED marker once a node re-executes, so we have to
// snapshot the set up front.
//
// A node is "resuming" iff its persisted output has `__colmena_status: "SUSPENDED"`.
// This set is computed once and never mutated — it represents the resume topology
// at the moment the snapshot was taken.
let resuming_node_ids: std::collections::HashSet<String> = if resume_answer.is_some() {
    all_outputs
        .iter()
        .filter_map(|(nid, out)| {
            if Self::find_status_by_key(out, "__colmena_status")
                == Some("SUSPENDED".to_string())
            {
                Some(nid.clone())
            } else {
                None
            }
        })
        .collect()
} else {
    std::collections::HashSet::new()
};
```

Notas:
- Si `resume_answer` es `None` (fresh run), el set queda vacío y el gate de la sección 4.1.2 es no-op.
- `find_status_by_key` ya hace búsqueda recursiva en arrays/objetos, lo que cubre outputs anidados (caso orchestrator/subgraph que envuelven el SUSPENDED de su hijo en su propia estructura).

#### 4.1.2 Gatear la inyección en el loop

Reemplazar el bloque actual de líneas 377-379:

```rust
// ANTES
if let Some(ans) = &resume_answer {
    inputs.insert("__colmena_resume_answer".to_string(), Value::String(ans.clone()));
}
```

por:

```rust
// DESPUÉS
if let Some(ans) = &resume_answer {
    if resuming_node_ids.contains(&node_id) {
        inputs.insert("__colmena_resume_answer".to_string(), Value::String(ans.clone()));
    } else {
        // Defensive trace — useful when debugging why a node ran "fresh"
        // during a resume run. Removed in production via the `tracing` filter.
        tracing::trace!(
            target: "colmena::dag_engine",
            node_id = %node_id,
            "resume_answer present but node was not in SUSPENDED set; skipping injection"
        );
    }
}
```

### 4.2 `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

#### 4.2.1 Guard defensivo en el branch de resume

Reemplazar el bloque actual de líneas 1799-1810:

```rust
// ANTES
if let Some(answer) = resume_answer.as_deref() {
    let conversation = conversation_repo.get_by_id(&conversation_key).await?;
    let pending = find_pending_tool_call(&conversation.messages)
        .ok_or("llm_call resume: no pending tool call found in conversation history")?;

    tracing::info!(
        target: "colmena::llm_node",
        "llm_call: resume — replaying pending tool with user answer"
    );
    let result = tool_executor
        .execute_with_resume_answer(&pending, answer)
        .await?;
    ...
}
```

por:

```rust
// DESPUÉS
if let Some(answer) = resume_answer.as_deref() {
    let conversation = conversation_repo.get_by_id(&conversation_key).await?;
    match find_pending_tool_call(&conversation.messages) {
        Some(pending) => {
            tracing::info!(
                target: "colmena::llm_node",
                "llm_call: resume — replaying pending tool with user answer"
            );
            let result = tool_executor
                .execute_with_resume_answer(&pending, answer)
                .await?;
            // ... (resto del bloque actual sin cambios)
        }
        None => {
            // Defense-in-depth: if the engine's per-node gating is broken and we
            // received __colmena_resume_answer despite having no pending tool call,
            // fall through to the fresh run path instead of aborting the DAG.
            //
            // This branch should be unreachable in normal operation after the engine
            // fix in run_use_case.rs §4.1. We log a warning so regressions surface
            // in tracing/observability.
            tracing::warn!(
                target: "colmena::llm_node",
                node_id = inputs
                    .get("__node_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unknown)"),
                "llm_call: resume_answer present but no pending tool call in history; \
                 falling through to fresh run (engine routing may be broken)"
            );
            // Intentional fallthrough — control continues to the standard
            // agent_service.run path below.
        }
    }
}
```

Notas:
- El `warn!` es importante: es lo que nos avisará si en el futuro alguien rompe la regla de §3.1 sin actualizar este guard.
- El fallthrough preserva el resto de la lógica del nodo. El `prompt` puede ser vacío (si el upstream era `suspend` y `answer_received` no se conectó al puerto `prompt`), pero los chequeos preexistentes de líneas 838-851 ya manejan ese caso (skip silencioso).
- En el repro de ADP el `prompt` no será vacío: el edge `suspend → llm_call` propaga `answer_received` al puerto default `prompt` del `llm_call`. Se ejecuta normalmente.

### 4.3 Archivos NO modificados (validación negativa)

- `suspend.rs` — sin cambios. Una vez aplicado §4.1, un `suspend` que NO estaba suspendido nunca recibe la key.
- `secure_suspend.rs` — sin cambios. Mismo razonamiento.
- `subgraph.rs`, `orchestrator.rs` — sin cambios. Su propio output bubble-up sigue marcado SUSPENDED, siguen entrando al `resuming_node_ids`, su lógica interna existente continúa funcionando.
- `http.rs`, `router/node.rs`, `dag_tool_executor.rs` — sin cambios. Las únicas referencias a la key son writes (cuando el nodo mismo inyecta hacia un sub-call), no reads. Auditadas en §6.

---

## 5. Matriz de casos cubiertos

| # | Escenario | Comportamiento esperado | Cubierto por |
|---|---|---|---|
| 1 | `input → suspend → llm_call → output` (repro ADP, llm_call con memoria, sin tools) | Resume: suspend emite answer; llm_call corre fresh con prompt = answer; LLM responde normal | §4.1 (gating engine) — basta sin §4.2 |
| 2 | `suspend1(id=a) → suspend2(id=b) → log` (cascada) | Resume #1 con `Q[a]/A[a]`: suspend1 emite, suspend2 corre fresh y pausa. Resume #2 con `Q[b]/A[b]`: suspend2 emite, log corre. | §4.1 (gating engine) |
| 3 | `suspend1 ∥ suspend2 → join` (multi-suspend paralelo en misma capa) | Run pausa con ambos suspends en `all_outputs` marcados SUSPENDED. Resume con `Q[a]/A[a]\nQ[b]/A[b]`: ambos reciben la key, cada uno bindea su id, ambos emiten en paralelo. | §4.1 (gating engine) |
| 4 | `orchestrator { ... → suspend → ... }` (suspend dentro de orchestrator) | El orchestrator bubble-up SUSPENDED, está en `resuming_node_ids`, recibe la key, su lógica interna (líneas 793-872) la thread al child suspendido. | Sin cambio (orchestrator preserva comportamiento) |
| 5 | `subgraph { ... → suspend → ... }` (suspend dentro de subgraph) | Igual a #4 con `subgraph.rs:71-101`. | Sin cambio |
| 6 | `llm_call` con tool que suspende (caso LLM-with-suspend-tool actual) | El `llm_call` mismo persiste SUSPENDED, está en `resuming_node_ids`, recibe la key, su branch de resume encuentra el pending tool, lo ejecuta con la answer, continúa. | Sin cambio funcional (idéntico a hoy) |
| 7 | Fresh run sin resume (`resume_answer = None`) | `resuming_node_ids` vacío, la key nunca se inyecta. | §4.1 (gating engine) |
| 8 | Resume con session_id inválido / sin estado | `all_outputs` queda vacío, `resuming_node_ids` vacío, comportamiento legacy preservado. | §4.1 |
| 9 | Defensa: engine rompe la regla en el futuro y un `llm_call` fresh recibe la key | Guard de §4.2 atrapa, warning, fallthrough a fresh run. | §4.2 (defense-in-depth) |

---

## 6. Audit de consumers de `__colmena_resume_answer`

Resultados del grep `grep -rn "__colmena_resume_answer" src/libs/colmena/src --include="*.rs"`:

| Archivo:línea | Read/Write | Comportamiento con el fix | Acción |
|---|---|---|---|
| `nodes/suspend.rs:34` | read | Solo activa cuando el nodo estaba SUSPENDED → comportamiento correcto | Sin cambio |
| `nodes/suspend.rs:221, 239, 262, 284` | write (tests) | Tests internos del nodo, no run real | Sin cambio |
| `nodes/secure_suspend.rs:234, 240` | read | Solo activa cuando el nodo estaba SUSPENDED → correcto | Sin cambio |
| `nodes/secure_suspend.rs:847, 867, 931, 966, 997, 1026, 1058, 1081, 1196` | write (tests) | Tests internos | Sin cambio |
| `nodes/llm.rs:820-823` | read | Detección | Sin cambio |
| `nodes/llm.rs:1799-1810` | read | Branch de resume | **§4.2: agregar guard** |
| `nodes/orchestrator.rs:796-797, 1432` | read + remove | Lee para threading interno; remove evita cascade hacia hijos no suspendidos | Sin cambio |
| `nodes/subgraph.rs:74-101` | read | Thread hacia child interno | Sin cambio |
| `nodes/http.rs:866` | write | Inyección del nodo http a un sub-call (suspend nested) | Sin cambio |
| `nodes/router/node.rs:146` | write | Inyección del router a una rama | Sin cambio |
| `infrastructure/dag_tool_executor.rs:551-572, 906-908` | write | Inyección al ejecutar un tool durante resume del LLM | Sin cambio (es la API explícita `execute_with_resume_answer`, no se ve afectada) |
| `application/run_use_case.rs:377-378` | write | **§4.1: gatear** | Cambio |

Total: 2 cambios funcionales, 0 cambios en API pública, 0 cambios en consumers externos.

---

## 7. Plan de tests

### 7.1 Tests unit nuevos (Rust)

En `src/libs/colmena/src/dag_engine/application/run_use_case.rs` (módulo `tests` inline si existe; si no, en `tests/` integration):

1. **`resuming_node_ids_includes_suspended_nodes`** — restaurar un `DagRunState` con dos nodos: uno con output SUSPENDED, otro con output normal. Asegurar que solo el primero entra al set.
2. **`resuming_node_ids_is_empty_for_fresh_run`** — sin `resume_answer`, el set queda vacío incluso si `all_outputs` tiene un SUSPENDED histórico.
3. **`resuming_node_ids_includes_nested_suspended_outputs`** — output del estilo `{ "result": { "__colmena_status": "SUSPENDED" } }` (caso orchestrator wrap) entra al set vía `find_status_by_key` recursivo.

**Cobertura del guard de `llm.rs` §4.2.1 — nota de pragmatismo:** un unit test directo del fallthrough requeriría construir `LlmNode` con un `ConversationRepositoryFactory` y un `Weak<dyn NodeRegistryPort>` mockeados, y luego forzar una situación que el engine fix de §4.1 vuelve inalcanzable en operación normal. La inversión no se justifica. La cobertura efectiva del guard es:
- **Code review** del bloque (revisión humana del PR).
- **Observabilidad**: el `tracing::warn!` con `target: "colmena::llm_node"` aparece en Cloud Logging si alguna vez se dispara → alerta accionable inmediata.
- **Test indirecto**: si en el futuro alguien rompe el engine gate (§4.1) y olvida actualizar este guard, el integration test §7.2 ítem 5 vuelve a fallar con la misma firma original. El guard mismo no se testea aislado.

### 7.2 Integration tests con grafos JSON

Nuevos grafos en `tests/graphs/basic/`:

5. **`suspend_then_llm_resume.json`** — exactamente el repro de ADP:
   ```
   input → suspend(id=ask_name, "¿Cuál es tu nombre?") → llm_call(google/gemini-2.5-flash, system="Hacele un poema corto al usuario") → log
   ```
   Pasos del test:
   - Run 1 con `--agent-session-id agent_t_55`: asserts `finishReason: "suspended"`, `questions[0].id: "ask_name"`.
   - Run 2 con `--agent-session-id agent_t_55 --answer "Q[ask_name]: ¿Cuál es tu nombre?\nA[ask_name]: Julian"`: asserts `node-end llm_call` emite un completion no vacío, NO emite error.

6. **`suspend_cascade.json`** — `suspend1 → suspend2 → log` con dos ids distintos. Test asegura que el segundo resume corre fresh el suspend2 (no falla con `missing answer`).

7. **`suspend_in_subgraph.json`** — ya existe; agregar assertion en su ejecución que el comportamiento se mantiene (regression guard para el wrap caso #5 de la matriz).

### 7.3 Comandos de verificación

```bash
# Unit + integration (Rust)
source .env
cargo test --lib resuming_node_ids
cargo test --lib resume_branch_falls_through
cargo test --verbose  # cobertura completa CI-equivalente

# Integration end-to-end con grafos
cargo run --bin dag_engine -- run \
  tests/graphs/basic/suspend_then_llm_resume.json \
  --agent-session-id agent_e2e_b55

cargo run --bin dag_engine -- run \
  tests/graphs/basic/suspend_then_llm_resume.json \
  --agent-session-id agent_e2e_b55 \
  --answer "Q[ask_name]: ¿Cuál es tu nombre?
A[ask_name]: Julian"

# Save + report según convención del usuario:
# guardar SSE en /tmp/colmena_e2e/<name>.sse + reportar friendly
```

### 7.4 Test de no-regresión: LLM-as-parent-with-suspend-tool

Reutilizar `tests/graphs/agents/llm_tool_suspend_smoke.json` (ya existe). Validar que el flujo completo:
- LLM llama un tool que suspende.
- Run pausa.
- Resume con el answer del tool.
- LLM recibe el tool result, continúa, completa.

Comportamiento debe ser idéntico al actual. Es el caso #6 de la matriz.

---

## 8. Sweep ADP (CLAUDE.md breaking-change discipline)

Per la regla de breaking-change discipline (CLAUDE.md "breaking changes discipline"):

> anything that changes colmena's public API (`EngineConfig`, `ColmenaEngine`, exported trait signatures) must be swept against the ADP worker (`apps/service/ia/platform/{worker,api}/src/` in the adp repo) BEFORE pushing to colmena develop

Este fix **no cambia ninguna API pública**:
- No toca `EngineConfig`, `ColmenaEngine`, ni traits exportadas.
- No cambia la firma de `execute` ni de los métodos del `DagRunUseCase`.
- No cambia el formato del SSE event stream.
- No cambia el formato Q/A de `--answer`.
- No cambia el comportamiento observable para runs que ya funcionaban.

Cambia solamente:
- Comportamiento observable para runs que ANTES fallaban (ahora funcionan).
- Comportamiento observable para usuarios que estuviera (incorrectamente) leyendo `__colmena_resume_answer` desde un nodo no-suspendido y dependiendo de ese error como signal. Esto sería un anti-patrón; no hay reportes de uso en ADP.

**Sweep check antes de mergear:** `grep -rn "__colmena_resume_answer\|colmena_resume_answer" /Users/danielgarcia/startti/adp/apps/service/ia/platform/{worker,api}/src/` debería devolver 0 resultados — la key es interna del motor y ADP no debería leerla nunca.

---

## 9. Backout

El cambio es additive (gate más restrictivo + guard defensivo). Si surge una regresión:

```bash
git revert <commit-sha-engine-fix>
git revert <commit-sha-llm-guard>
```

Ambos commits son independientes y revertibles por separado. El cambio del engine (§4.1) es el único que altera comportamiento observable; el guard del `llm_call` (§4.2) es no-op cuando el engine respeta la regla.

---

## 10. Riesgos y mitigaciones

| Riesgo | Probabilidad | Impacto | Mitigación |
|---|---|---|---|
| Algún consumer no documentado lee `__colmena_resume_answer` desde un nodo no suspendido y dependía del comportamiento previo | Muy baja | Medio | Audit completo de §6; ADP sweep §8; warning log en `llm.rs` §4.2 captura regresiones silenciosas |
| `find_status_by_key` recursivo es más caro de lo necesario | Muy baja | Bajo | Solo corre una vez al inicio del resume; `all_outputs` típicamente <50 entries |
| Race entre el cómputo de `resuming_node_ids` y un mutation de `all_outputs` | Imposible | — | `resuming_node_ids` se computa antes del loop, `all_outputs` solo se muta dentro del loop |
| El branch de resume del `llm_call` tenía side-effects que ahora se saltan en el caso fallthrough | Muy baja | Bajo | Releída la rama de líneas 1799-1840: el único side-effect es persistir un tool message si encontró pending. En el caso fallthrough no había pending, no hay nada que persistir |
| Multi-resume parcial: usuario manda respuesta solo para algunos de varios suspends paralelos | Existente, no nuevo | — | El parser ya rechaza payloads incompletos. Sin cambio de comportamiento |

---

## 11. Referencias

- Bug report ADP 2026-06-04 (en el chat de brainstorm que originó este spec).
- [`docs/developer_guide/44_suspend_node.md`](../../developer_guide/44_suspend_node.md) — comportamiento canónico del nodo `suspend`.
- [`docs/superpowers/specs/2026-05-08-suspend-qa-response-format-design.md`](2026-05-08-suspend-qa-response-format-design.md) — formato Q/A.
- [`docs/developer_guide/19_nested_agents_and_subgraphs.md`](../../developer_guide/19_nested_agents_and_subgraphs.md) — propagación HITL.
- [`docs/developer_guide/20_orchestrator_architecture.md`](../../developer_guide/20_orchestrator_architecture.md) — orchestrator + HITL.
- Source Rust: [`run_use_case.rs`](../../../src/libs/colmena/src/dag_engine/application/run_use_case.rs), [`llm.rs`](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs), [`suspend.rs`](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs).
