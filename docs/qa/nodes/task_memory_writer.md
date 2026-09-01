# QA — Nodo `task_memory_writer`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/task_memory_writer.rs`
Fuentes de doc revisadas:
- `docs/node_configurations.json` (esquema de config)
- `docs/agent_context/node_ports_reference.md` (puertos/outputs)
- `docs/developer_guide/20_orchestrator_architecture.md` (orquestación manual)
- `docs/developer_guide/12_dag_engine_guide.md` (guía DAG)

---

## 1) Config documentada NO soportada por el código

**Hallazgo 1.1 — Validación ausente en `add_tasks`:**
La documentación en `node_configurations.json` describe `add_tasks` como *"Each object must have 'task' (description string) and 'assigned_to' (agent name string)"*, sugiriendo validación obligatoria. Sin embargo, el código (líneas 54–62) usa fallbacks silenciosos: si `task` o `assigned_to` no existen, asigna la cadena literal `"Unknown"` en lugar de fallar. Impacto: un operador que omita estos campos no recibe error sino tareas con nombre genérico, ocultando errores de configuración.

**Hallazgo 1.2 — Silenciamiento de errores en `delete_tasks`:**
La documentación no menciona que los errores durante `delete_task` se ignoran. El código (línea 91) usa `let _ = repo.delete_task(id_str).await;`, descartando cualquier error de la operación (ID no encontrado, permisos, BD caída, etc.). Impacto: un operador creerá que un delete siempre tuvo éxito; si el ID no existe, el nodo continúa sin avisar.

---

## 2) Código NO documentado

**Hallazgo 2.1 — Dependencia de `session_id` en estado (`_state`):**
El código (líneas 30–34) lee `session_id` desde el mapa `_state`: `_state.get("session_id").and_then(|v| v.as_str()).unwrap_or("unknown_run")`. Ninguno de los documentos menciona que el nodo depende de este campo en estado. El fallback a `"unknown_run"` se invoca si `session_id` no está disponible, pero no hay advertencia en doc de que esto puede causar comportamiento inesperado (tareas sin session asociada o mixtura de sesiones).

**Hallazgo 2.2 — Fallbacks silenciosos en `add_tasks`:**
Las líneas 54–62 describen la construcción de objetos `DagTask` con fallbacks: `task_obj.get("task").and_then(|v| v.as_str()).unwrap_or("Unknown")` y análogo para `assigned_to`. Estos fallbacks no están documentados. Un operador que pase un `add_tasks` con estructuras anómalas (campos numéricos, null, etc.) recibirá tareas silenciosamente renombradas sin feedback.

**Hallazgo 2.3 — Formato de output NO wrappeado:**
Los documentos en `node_ports_reference.md` indican que el output no va envuelto en `{ output: ... }`, pero `node_configurations.json` no menciona explícitamente esto (la sección `output_ports` implícitamente describe `result` y `extra_info` como top-level, pero no es claro). El código (líneas 125–128) retorna directamente `{ "result": [...], "extra_info": {} }` sin wrapper.

**Hallazgo 2.4 — Campo `result` en output de suspend:**
En la ruta de suspend (líneas 116–122), el código retorna `"result": "Suspended by Critic Node"` como cadena literal. Mientras que `node_ports_reference.md` lo menciona, ni `node_configurations.json` ni el developer guide detalla que este string es fijo (no parametrizable).

**Hallazgo 2.5 — Campos no validados en delete_tasks:**
El código no valida que los elementos de `delete_tasks` sean strings UUID válidos. Líneas 88–93 simplemente iteran y llaman `repo.delete_task(id_str)` para cualquier string, incluyendo strings vacíos o malformados. El repositorio puede fallar internamente, pero el nodo no lo reporta (línea 91: `let _`).

---

## 3) Plan de pruebas QA

### Caso 3.1: Happy path — Actualizar resultado de tarea

**Objetivo:** Verificar que `task_id` + `result` actualizan correctamente una tarea existente en la base.

**Grafo mínimo:** (`test_task_update.json`)
```json
{
  "nodes": [
    {
      "id": "task_memory_writer",
      "node_type": "task_memory_writer",
      "config": {
        "task_id": "550e8400-e29b-41d4-a716-446655440000",
        "result": { "status": "completed", "output": "Flight booked" }
      }
    }
  ]
}
```

**Entrada:** (ninguna, todo en config)
**Resultado esperado:** Output `{ "result": [{...task objects...}], "extra_info": {} }` donde al menos una tarea tiene `id: "550e8400..."` y `result: { "status": "completed", ... }`.
**Verificación:** Consultar `dag_task` table en PostgreSQL; confirmar que la fila con ese UUID tiene el resultado guardado.

---

### Caso 3.2: Happy path — Agregar tareas (`add_tasks`)

**Objetivo:** Insertar nuevas tareas con `task` y `assigned_to`.

**Grafo mínimo:** (`test_add_tasks.json`)
```json
{
  "nodes": [
    {
      "id": "task_memory_writer",
      "node_type": "task_memory_writer",
      "config": {
        "add_tasks": [
          { "task": "Search hotels in Shibuya", "assigned_to": "hotels_agent" },
          { "task": "Check weather forecast", "assigned_to": "weather_agent" }
        ]
      }
    }
  ]
}
```

**Entrada:** (ninguna)
**Resultado esperado:** Output contiene 2 nuevos objetos en `result` array con `task_name: "Search hotels..."`, `assigned_to: "hotels_agent"`, `completed: false`.
**Verificación:** Confirmar que `dag_task` table tiene 2 filas nuevas con esos nombres y fase 1.

---

### Caso 3.3: Happy path — Borrar tareas (`delete_tasks`)

**Objetivo:** Remover tareas por ID.

**Grafo mínimo:** (`test_delete_tasks.json`)
```json
{
  "nodes": [
    {
      "id": "task_memory_writer",
      "node_type": "task_memory_writer",
      "config": {
        "delete_tasks": ["550e8400-e29b-41d4-a716-446655440000"]
      }
    }
  ]
}
```

**Entrada:** (ninguna)
**Resultado esperado:** Output contiene array sin la tarea con ese ID; `extra_info: {}`.
**Verificación:** Consultar `dag_task` table; confirmar que ese UUID ya no existe.
**Nota QA:** Probar también con ID inexistente; el nodo NO debe fallar (error silenciado), solo devolver lista actualizada sin él.

---

### Caso 3.4: Suspend path — `suspend: true`

**Objetivo:** Verificar que `suspend: true` retorna estado `SUSPENDED` y pausa el DAG.

**Grafo mínimo:** (`test_suspend.json`)
```json
{
  "nodes": [
    {
      "id": "task_memory_writer",
      "node_type": "task_memory_writer",
      "config": {
        "task_id": "550e8400-e29b-41d4-a716-446655440000",
        "result": { "info": "Waiting for review" },
        "suspend": true
      }
    }
  ]
}
```

**Entrada:** (ninguna)
**Resultado esperado:** Output `{ "result": "Suspended by Critic Node", "extra_info": { "__colmena_status": "SUSPENDED", "all_tasks": [...] } }`.
**Verificación:** Confirmar que SSE contiene un evento que indica suspensión; el DAG no debe continuar a siguiente nodo.

---

### Caso 3.5: Error path — Sin repositorio

**Objetivo:** Verificar que nodo falla si `task_memory_repo` no se pasa.

**Grafo mínimo:** (`test_no_repo.json`) — igual que caso 3.1, pero ejecutado SIN DATABASE_URL.

**Entrada:** (ninguna)
**Resultado esperado:** Error `"TaskMemoryWriterNode requires a Task Memory Repository"`.
**Verificación:** Comando: `cargo run --bin dag_engine -- run test_no_repo.json` sin DATABASE_URL; confirmar error en SSE o stdout.

---

### Caso 3.6: Fallback en `add_tasks` — Campos faltantes

**Objetivo:** Verificar que tareas sin `task` o `assigned_to` se crean con `"Unknown"`.

**Grafo mínimo:** (`test_add_fallback.json`)
```json
{
  "nodes": [
    {
      "id": "task_memory_writer",
      "node_type": "task_memory_writer",
      "config": {
        "add_tasks": [
          { "task": "Normal task", "assigned_to": "agent_a" },
          { "assigned_to": "agent_b" },
          { "task": "Task without agent" },
          {}
        ]
      }
    }
  ]
}
```

**Entrada:** (ninguna)
**Resultado esperado:** Output contiene 4 tareas:
  - 1: `task_name: "Normal task"`, `assigned_to: "agent_a"`
  - 2: `task_name: "Unknown"`, `assigned_to: "agent_b"` (falta task)
  - 3: `task_name: "Task without agent"`, `assigned_to: "Unknown"` (falta assigned_to)
  - 4: `task_name: "Unknown"`, `assigned_to: "Unknown"` (ambos faltan)
**Verificación:** Confirmar en `dag_task` que todos aparecen con esos nombres; NO hay error.
**QA Note:** Este es un comportamiento silencioso que pasa controles; operador no sabe que se perdieron datos.

---

### Caso 3.7: Input override — Config fallback a inputs

**Objetivo:** Verificar que inputs sobrescriben config (línea 38: `inputs.get(...).or_else(|| config.get(...))`).

**Grafo mínimo:** (`test_input_override.json`)
```json
{
  "nodes": [
    {
      "id": "input_node",
      "node_type": "input",
      "config": {
        "prompt": "Enter task ID and result"
      }
    },
    {
      "id": "task_memory_writer",
      "node_type": "task_memory_writer",
      "config": {
        "task_id": "config-id-fallback"
      },
      "inputs": {
        "task_id": { "type": "input", "from_node": "input_node", "key": "task_id" }
      }
    }
  ]
}
```

**Entrada/Input prompt:** `{ "task_id": "550e8400-e29b-41d4-a716-446655440000", "result": "..." }`
**Resultado esperado:** El nodo usa `task_id` del input, no del config. Tarea actualizada con ese ID, no "config-id-fallback".
**Verificación:** Consultar `dag_task`; confirmar que la tarea con el ID del input fue modificada, no buscó "config-id-fallback".

---

### Caso 3.8: Múltiples operaciones en un run

**Objetivo:** Actualizar + agregar + borrar en un solo nodo en una sesión.

**Grafo mínimo:** (`test_combined_ops.json`)
```json
{
  "nodes": [
    {
      "id": "task_memory_writer",
      "node_type": "task_memory_writer",
      "config": {
        "task_id": "550e8400-e29b-41d4-a716-446655440000",
        "result": { "status": "done" },
        "add_tasks": [
          { "task": "New subtask", "assigned_to": "sub_agent" }
        ],
        "delete_tasks": ["old-id-to-remove"]
      }
    }
  ]
}
```

**Entrada:** (ninguna)
**Resultado esperado:** Output array incluye:
  - Tarea con UUID originario, actualizada con `result`
  - Nueva tarea con nombre "New subtask"
  - El "old-id-to-remove" NO aparece
**Verificación:** Base de datos refleja todos los cambios de una sola ejecución (transacción).

---

### Caso 3.9: Session ID persistencia entre runs

**Objetivo:** Verificar que el nodo usa `session_id` de estado para agrupar tareas.

**Ejecución 1:** `cargo run --bin dag_engine -- run test_combined_ops.json --agent-session-id agent_demo_001`
**Ejecución 2:** `cargo run --bin dag_engine -- run test_add_tasks.json --agent-session-id agent_demo_001` (segunda invocación, mismo agent_session_id)

**Resultado esperado:** Ambas ejecuciones escriben tareas bajo la misma `session_id` en la base; output de ejecución 2 incluye tareas de ejecución 1 + nuevas de ejecución 2.
**Verificación:** Consultar `dag_task` con `WHERE session_id = 'agent_demo_001'`; confirmar que hay tareas de ambas ejecuciones bajo el mismo session.

---

### Caso 3.10: Variación — `task_id` sin `result`

**Objetivo:** Pasar `task_id` pero SIN `result`; confirmar que el nodo NO actualiza (solo si ambos presentes).

**Grafo mínimo:** (`test_task_id_only.json`)
```json
{
  "nodes": [
    {
      "id": "task_memory_writer",
      "node_type": "task_memory_writer",
      "config": {
        "task_id": "550e8400-e29b-41d4-a716-446655440000"
      }
    }
  ]
}
```

**Entrada:** (ninguna)
**Resultado esperado:** El nodo retorna la lista de tareas SIN modificar la tarea con ese ID (porque `result` es ausente/null).
**Verificación:** Consultar `dag_task`; confirmar que esa tarea no cambió desde el estado anterior.

---

### Caso 3.11: Variación — `result` sin `task_id`

**Objetivo:** Pasar `result` pero NO `task_id`; confirmar que no se escribe (solo si ambos presentes).

**Grafo mínimo:** (`test_result_only.json`)
```json
{
  "nodes": [
    {
      "id": "task_memory_writer",
      "node_type": "task_memory_writer",
      "config": {
        "result": { "info": "orphan result" }
      }
    }
  ]
}
```

**Entrada:** (ninguna)
**Resultado esperado:** El nodo retorna lista de tareas sin escribir nada (porque no hay `task_id`).
**Verificación:** Confirmar en base que ninguna tarea fue modificada.

---

### Caso 3.12: Output structure — Siempre two-level

**Objetivo:** Verificar que output es SIEMPRE `{ "result": ..., "extra_info": ... }`, nunca wrappeado en `{ output: { result: ..., extra_info: ... } }`.

**Grafo de check:** Cualquiera de los anteriores; inspeccionear SSE o stdout.

**Resultado esperado:** Top-level keys son `result` y `extra_info`; NUNCA existe una key `output`.
**Verificación:** Parsear JSON de output; confirmar schema `{ result: <any>, extra_info: <object> }`.

---

### Caso 3.13: `delete_tasks` con ID inexistente (no falla)

**Objetivo:** Pasar un UUID que no existe en `delete_tasks`; confirmar que nodo NO falla.

**Grafo mínimo:** (`test_delete_nonexistent.json`)
```json
{
  "nodes": [
    {
      "id": "task_memory_writer",
      "node_type": "task_memory_writer",
      "config": {
        "delete_tasks": ["00000000-0000-0000-0000-000000000000"]
      }
    }
  ]
}
```

**Entrada:** (ninguna)
**Resultado esperado:** Nodo completa exitosamente (sin error); retorna lista actual de tareas. El error de delete es silenciado (línea 91: `let _`).
**Verificación:** SSE no contiene un event de error; output es un array válido.
**QA Note:** Este es un comportamiento poco intuitivo: el operador cree que borró algo pero nada sucedió. No hay feedback.

