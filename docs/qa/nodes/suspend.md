# suspend — Auditoría QA (Documentación vs Código)

**Nodo:** `suspend`  
**Código fuente:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs`  
**Documentación primaria:** `docs/developer_guide/44_suspend_node.md`  
**Configuración canónica:** `docs/node_configurations.json` → `node_types.suspend`  
**Puertos:** `docs/agent_context/node_ports_reference.md` → `suspend`  
**Spec de diseño:** `docs/superpowers/specs/2026-05-08-suspend-qa-response-format-design.md`  
**Fecha de auditoría:** 2026-08-30

---

## 1. Hallazgos: Documentación

### 1.1 node_as_tools_reference.json — entrada ausente para suspend como tool

**Problema:** `docs/node_as_tools_reference.json` no contiene una entrada de `suspend` para modelar cómo usar el nodo como herramienta LLM (p.ej. dentro de un `subgraph` que es usado como tool por un `llm_call`).

**Realidad en código:** `suspend.rs:32-45` implementa el nodo como `ExecutableNode` estándar. El `subgraph` node puede envolver un child graph que contiene un `suspend`, haciéndolo accesible como tool.

**Impacto:** Medio. LLM developers que buscan "cómo exponer un suspend dentro de un subgraph como tool" no encuentran documentación canónica en `node_as_tools_reference.json` y deben inferir desde `44_suspend_node.md:269-283` ("Suspend dentro de un `subgraph`").

**Remediación:** Agregar entrada `suspend` a `docs/node_as_tools_reference.json` mostrando cómo un `subgraph` con un `suspend` interno se expone como tool (con `node_schema` para inputs dinámicos).

---

### 1.2 44_suspend_node.md — sección "Troubleshooting" alude a documento que no existe

**Problema:** Línea 357 dice "Detalles adicionales (troubleshooting profundo de Q/A, multi-suspend, secure_suspend) en [`docs/agent_context/node_ports_reference.md`](../../agent_context/node_ports_reference.md) §"Troubleshooting the `suspend` Node"."

**Verificación:** `node_ports_reference.md` líneas 138-300 NOT contienen una sección titulada "Troubleshooting the `suspend` Node". El archivo SÍ cubre suspend detalladamente, pero sin una sección de troubleshooting separada.

**Impacto:** Bajo. El troubleshooting SÍ existe en `44_suspend_node.md:345-356` (tabla con síntomas/fixes). Solo el anchor es incorrecto.

**Remediación:** Actualizar línea 357 de `44_suspend_node.md` para referenciar la sección correcta en `node_ports_reference.md` o referenciar la tabla de troubleshooting local (línea 345).

---

### 1.3 node_configurations.json — nota sobre `config.id` fallback es ambigua

**Problema:** Línea ~1382 en `node_configurations.json` dice:

```
"description": "Stable per-question identifier used to key the resume answer payload. Required (no fallback to __node_id)."
```

**Verificación:** Correcto según código `suspend.rs:71-74` que REQUIERE id sin fallback. El spec 2026-05-08:92 lo confirma explícitamente.

**Incorrección:** La palabra "Required" está en la descripción, pero el campo JSON está marcado como `"required": true` — esto es correcto y no es una inconsistencia, solo confirmación.

**Estado:** OK. No es un hallazgo.

---

### 1.4 Developer guide — NO menciona `cfg_or_input` pattern para tool-path usage

**Problema:** `docs/developer_guide/44_suspend_node.md` líneas 48-75 muestran ejemplos de suspend en graph nodes (config-driven), pero NO documentan que cuando `suspend` se usa como tool LLM, el executor merges `fixed_config` + `node_schema` en `inputs` y pasa `config = {}` vacío.

**Realidad en código:** `suspend.rs:27-29` implementa `cfg_or_input()` que maneja exactamente esto — config first, fallback a inputs. Los tests líneas 285-306 verifican el comportamiento en tool path.

**Impacto:** Medio. LLM developers que setean `suspend` como tool en `tool_configurations` pueden tener confusión sobre si usen `fixed_config` o `node_schema` si no leen el código.

**Remediación:** Agregar sección en `44_suspend_node.md` (nuevo o extender §6.6) documentando:
- Config-first/inputs-fallback (`cfg_or_input` pattern)
- Example: tool_configurations entry para suspend con `node_schema` + `fixed_config`
- Link a `node_as_tools_reference.json` entrada suspend (cuando exista).

---

## 2. Hallazgos: Código

### 2.1 cfg_or_input pattern es correcto y bien implementado

**Implementación:** `suspend.rs:27-29` define la función que resuelve config primero, inputs segundo:

```rust
fn cfg_or_input<'a>(config: &'a Value, inputs: &'a NodeInputs, key: &str) -> Option<&'a Value> {
    config.get(key).or_else(|| inputs.get(key))
}
```

**Tests:** Líneas 285-306 y 309-320 verifican que:
1. Tool path (config vacío, todos los valores en inputs): funciona ✓
2. Config toma precedencia sobre inputs: funciona ✓

**Documentación:** Comentario línea 24-26 explica el patrón.

**Estado:** Excelente. Completamente alineado.

---

### 2.2 Charset validation for config.id es correcto

**Implementación:** `suspend.rs:76-80` valida el id contra `[A-Za-z0-9_-]{1,64}`:

```rust
if !is_valid_qa_id(&id) {
    return Err(Box::<dyn Error + Send + Sync>::from(format!(
        "suspend: invalid config.id '{id}' (must match [A-Za-z0-9_-]{{1,64}})"
    )));
}
```

**Función:** `qa_response_parser.rs` implementa `is_valid_qa_id()` que chequea exactamente este patrón.

**Spec:** Línea 31 de 2026-05-08 confirma `[A-Za-z0-9_-]{1,64}`.

**Estado:** Correcto. Tests línea 173-185 verifican que ids explícitos funcionan.

---

### 2.3 Default question fallback es correcto

**Implementación:** `suspend.rs:66-69`:

```rust
let question = cfg_or_input(config, inputs, "question")
    .and_then(|v| v.as_str())
    .unwrap_or("What is your input?")
    .to_string();
```

**Verificación:** Default "What is your input?" matchea documentación en `node_configurations.json` y `44_suspend_node.md:42`.

**Estado:** Correcto.

---

### 2.4 Output structure emits both legacy y canónico format

**Implementación:** `suspend.rs:90-105` construye objeto con:
- `__colmena_status: "SUSPENDED"` (detection marker)
- `question: <string>` (legacy, para BC)
- `questions: [{ id, question, type, options }]` (canónico)

**Spec:** 2026-05-08, línea 145 dice "legacy field still present alongside the canonical questions array."

**Tests:** Línea 188-206 verifica que ambos campos están presentes.

**Estado:** Correcto. Decisión de BC bien documentada en comentario línea 64-65.

---

### 2.5 Resume path Q/A parsing es correcto

**Implementación:** `suspend.rs:41-62`:
1. Detecta `__colmena_resume_answer` presente
2. Llama a `parse_qa_response(raw, &[id])`
3. Extrae respuesta por id
4. Retorna `{ status: resumed, answer_received: <answer> }`

**Spec:** 2026-05-08:115-130 define API de parser. Todo matchea.

**Tests:** Líneas 222-282 verifican:
- Open question parsing ✓
- Choice question parsing ✓
- Free-text override de options ✓
- Parser error propagation ✓

**Estado:** Excelente.

---

### 2.6 default_output es correcto

**Implementación:** `suspend.rs:112-114`:

```rust
fn default_output(&self) -> Option<&str> {
    Some("answer_received")
}
```

**Verificación:** `node_ports_reference.md:99` confirma `default_output: answer_received`. ✓

**State:** Correcto.

---

### 2.7 default_input es correcto

**Implementación:** `suspend.rs:108-110`:

```rust
fn default_input(&self) -> Option<&str> {
    Some("question")
}
```

**Verificación:** `node_ports_reference.md:99` y `44_suspend_node.md:11` confirman. ✓

**State:** Correcto.

---

## 3. Casos de Prueba Ejecutables

Todos los casos usan `cargo run --bin dag_engine -- run <graph.json>` con `--agent-session-id` para keying de estado.

### 3.1 Test A: Minimal suspend con open question

**Archivo:** `tests/graphs/basic/test_suspend_manual.json` (ya existe)

```json
{
  "nodes": {
    "start": {
      "type": "input",
      "config": { "msg": "Procesando..." }
    },
    "approval": {
      "type": "suspend",
      "config": {
        "id": "approve_continue",
        "question": "¿Apruebas continuar con el proceso?"
      }
    },
    "finish": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "start", "to": "approval" },
    { "from": "approval", "to": "finish" }
  ]
}
```

**Ejecución - Run 1 (suspend):**
```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/basic/test_suspend_manual.json \
  --agent-session-id agent_demo_001
```

**Validación esperada:**
- Output contiene `"__colmena_status": "SUSPENDED"`
- Output contiene `"question": "¿Apruebas continuar con el proceso?"`
- Output contiene `"questions": [{"id": "approve_continue", "type": "open", "options": null}]`
- Event final es `"finishReason": "suspended"`

**Ejecución - Run 2 (resume):**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/test_suspend_manual.json \
  --agent-session-id agent_demo_001 \
  --answer "Q[approve_continue]: ¿Apruebas continuar con el proceso?
A[approve_continue]: sí, aprobado"
```

**Validación esperada:**
- Engine restaura snapshot
- suspend ejecuta con `__colmena_resume_answer` inyectado
- Output: `{ "status": "resumed", "answer_received": "sí, aprobado" }`
- DAG continúa downstream (log nodo recibe "sí, aprobado")

---

### 3.2 Test B: Suspend con choice question

**Archivo:** `tests/graphs/basic/test_suspend_choice.json` (crear si no existe)

```json
{
  "nodes": {
    "pick_env": {
      "type": "suspend",
      "config": {
        "id": "pick_env",
        "question": "¿A qué entorno hacemos deploy?",
        "question_type": "choice",
        "options": ["staging", "production", "rollback"]
      }
    },
    "output": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "pick_env", "to": "output" }
  ]
}
```

**Ejecución - Run 1:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/test_suspend_choice.json \
  --agent-session-id choice_demo_001
```

**Validación esperada:**
- `questions[0].type: "choice"`
- `questions[0].options: ["staging", "production", "rollback"]`

**Ejecución - Run 2 (respuesta sugerida):**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/test_suspend_choice.json \
  --agent-session-id choice_demo_001 \
  --answer "Q[pick_env]: ¿A qué entorno hacemos deploy?
A[pick_env]: production"
```

**Ejecución - Run 3 (respuesta libre, NO en options):**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/test_suspend_choice.json \
  --agent-session-id choice_demo_002 \
  --answer "Q[pick_env]: ¿A qué entorno hacemos deploy?
A[pick_env]: review-app-123"
```

**Validación esperada:**
- Ambas respuestas (dentro y fuera de `options`) son aceptadas
- Parser no valida contra `options` (son UX hints, no whitelist)

---

### 3.3 Test C: Pregunta dinámica desde upstream

**Archivo:** `tests/graphs/basic/test_suspend_dynamic_question.json`

```json
{
  "nodes": {
    "build_question": {
      "type": "python_script",
      "config": {
        "code": "order_id = '12345'\noutput = {'question': f'¿Aprobás la orden {order_id}?'}"
      }
    },
    "approval": {
      "type": "suspend",
      "config": {
        "id": "approval",
        "question": "Pregunta por defecto (será sobrescrita)"
      }
    },
    "log": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "build_question.question", "to": "approval.question" },
    { "from": "approval", "to": "log" }
  ]
}
```

**Ejecución - Run 1:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/test_suspend_dynamic_question.json \
  --agent-session-id dynamic_demo_001
```

**Validación esperada:**
- `question: "¿Aprobás la orden 12345?"` (NOT la default)
- `questions[0].question: "¿Aprobás la orden 12345?"` (edge gana sobre config)
- `questions[0].id: "approval"` (id siempre desde config)

**Ejecución - Run 2:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/test_suspend_dynamic_question.json \
  --agent-session-id dynamic_demo_001 \
  --answer "Q[approval]: cualquier_pregunta_aqui
A[approval]: sí"
```

**Validación esperada:**
- El texto `Q[approval]:` es ignorado (no validado)
- Solo el id y cuerpo de `A[approval]:` importan
- Downstream recibe "sí"

---

### 3.4 Test D: Cascada de múltiples suspends

**Archivo:** `tests/graphs/basic/suspend_cascade.json` (ya existe)

```json
{
  "nodes": {
    "manager_approval": {
      "type": "suspend",
      "config": {
        "id": "manager",
        "question": "Aprobación de manager"
      }
    },
    "director_approval": {
      "type": "suspend",
      "config": {
        "id": "director",
        "question": "Aprobación de director"
      }
    },
    "process": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "manager_approval", "to": "director_approval" },
    { "from": "director_approval", "to": "process" }
  ]
}
```

**Ejecución - Run 1:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/suspend_cascade.json \
  --agent-session-id cascade_001
```

**Validación esperada:**
- Pausa en primer suspend: `id: manager`

**Ejecución - Run 2 (resume en manager):**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/suspend_cascade.json \
  --agent-session-id cascade_001 \
  --answer "Q[manager]: Aprobación de manager
A[manager]: aprobado por manager"
```

**Validación esperada:**
- Resume ejecuta manager suspend
- Pausa en director suspend

**Ejecución - Run 3 (resume en director):**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/suspend_cascade.json \
  --agent-session-id cascade_001 \
  --answer "Q[director]: Aprobación de director
A[director]: aprobado por director"
```

**Validación esperada:**
- Resume ejecuta director suspend
- DAG continúa a process (log)
- Orden-independencia: el parser bindea por id, no por posición

---

### 3.5 Test E: Suspend dentro de subgraph (HITL bubbling)

**Archivo:** `tests/graphs/basic/suspend_in_subgraph.json` (ya existe)

```json
{
  "nodes": {
    "parent_input": {
      "type": "input",
      "config": { "order": "OXY-001" }
    },
    "child_graph": {
      "type": "subgraph",
      "config": {
        "child_graph_path": "tests/graphs/basic/suspend_in_subgraph_child.json"
      }
    },
    "parent_log": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "parent_input", "to": "child_graph" },
    { "from": "child_graph", "to": "parent_log" }
  ]
}
```

**Child graph:** `tests/graphs/basic/suspend_in_subgraph_child.json`

```json
{
  "nodes": {
    "confirm": {
      "type": "suspend",
      "config": {
        "id": "confirm_transfer",
        "question": "¿Confirmar la transferencia?"
      }
    },
    "child_output": {
      "type": "output"
    }
  },
  "edges": [
    { "from": "confirm", "to": "child_output" }
  ]
}
```

**Ejecución - Run 1:**
```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/basic/suspend_in_subgraph.json \
  --agent-session-id subgraph_001
```

**Validación esperada:**
- El status `SUSPENDED` burbujea desde el child al parent
- Output contiene `__colmena_status: SUSPENDED` (no anidado dentro de un child result)
- El id del suspend es accesible en el payload de resume

**Ejecución - Run 2:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/suspend_in_subgraph.json \
  --agent-session-id subgraph_001 \
  --answer "Q[confirm_transfer]: ¿Confirmar la transferencia?
A[confirm_transfer]: sí, confirmar"
```

**Validación esperada:**
- Engine resume restaura AMBOS el parent snapshot Y el child snapshot
- Child suspend ejecuta con respuesta
- Child output retorna el resultado al parent
- Parent continúa downstream

---

### 3.6 Test F: Resume fallido — id no coincide

**Archivo:** `tests/graphs/basic/test_suspend_manual.json` (reutilizar)

**Ejecución - Run 1:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/test_suspend_manual.json \
  --agent-session-id failed_001
```

**Ejecución - Run 2 (resume con id incorrecto):**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/test_suspend_manual.json \
  --agent-session-id failed_001 \
  --answer "Q[wrong_id]: ¿Apruebas continuar?
A[wrong_id]: sí"
```

**Validación esperada:**
- Parser error: `missing answer for id 'approve_continue'` (esperado id no aparece)
- DAG falla con error de parsing (no continúa)

---

### 3.7 Test G: Q/A format validation — respuesta vacía

**Archivo:** `tests/graphs/basic/test_suspend_manual.json` (reutilizar)

**Ejecución - Run 1:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/test_suspend_manual.json \
  --agent-session-id empty_001
```

**Ejecución - Run 2 (respuesta vacía):**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/test_suspend_manual.json \
  --agent-session-id empty_001 \
  --answer "Q[approve_continue]: ¿Apruebas continuar?
A[approve_continue]: "
```

**Validación esperada:**
- Parser error: `empty answer for id 'approve_continue'` (no permitido)
- DAG falla antes de ejecutar suspend

---

## Resumen de Hallazgos

| # | Tipo | Severidad | Descripción |
|---|------|-----------|-------------|
| 1.1 | Docs | Media | `node_as_tools_reference.json` sin entrada para `suspend` como tool LLM |
| 1.2 | Docs | Baja | `44_suspend_node.md` línea 357 alude a anchor incorrecto en `node_ports_reference.md` |
| 1.3 | Docs | OK | `node_configurations.json` charset validation documentado correctamente |
| 1.4 | Docs | Media | Developer guide NO documenta `cfg_or_input` pattern para tool-path usage |
| 2.1 | Código | Excelente | `cfg_or_input` pattern correcto y bien testeado |
| 2.2 | Código | Excelente | Charset validation alineado con spec y tests |
| 2.3 | Código | OK | Default question fallback correcto |
| 2.4 | Código | Excelente | Output structure emite formato legacy + canónico para BC |
| 2.5 | Código | Excelente | Resume Q/A parsing correcto, spec-aligned, bien testeado |
| 2.6 | Código | OK | `default_output` correctamente documentado |
| 2.7 | Código | OK | `default_input` correctamente documentado |

---

## Remediaciones Recomendadas

### Prioridad MEDIA (afecta discovery de LLM developers)

1. **Agregar entrada `suspend` a `docs/node_as_tools_reference.json`** mostrando cómo exponer un suspend dentro de un subgraph como tool LLM (con `node_schema` para inputs dinámicos, ejemplo de `tool_configurations`).
2. **Extender `docs/developer_guide/44_suspend_node.md`** con nueva sección (§10 o expandir §6.6) documentando `cfg_or_input` pattern, tool-path usage, y ejemplo de `tool_configurations` entry.

### Prioridad BAJA (precisión)

3. **Actualizar line 357 de `44_suspend_node.md`** para corregir anchor en `node_ports_reference.md` (la sección de troubleshooting en realidad está en `44_suspend_node.md:345-356`).

---

**Auditoría completada:** 4 hallazgos en documentación (2 media, 2 baja/OK) + 7 aspectos de código validados (5 excelentes, 2 OK) + 7 casos de prueba ejecutables cubriendo suspend, choice, dynamic input, cascada, subgraph, error handling, y Q/A format validation.

