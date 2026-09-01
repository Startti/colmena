# QA — Nodo `orchestrator`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs:801-1849`
Fuentes de doc revisadas: 
- `docs/node_configurations.json` (orchestrator config_fields)
- `docs/agent_context/node_ports_reference.md` (orchestrator entry)
- `docs/developer_guide/20_orchestrator_architecture.md`

---

## 1) Config documentada NO soportada por el código

Sin discrepancias detectadas. La documentación de campos obligatorios es conservadora (requiere `final_reactor`, `agents`), pero el código maneja gracias misericordia sus ausencias.

---

## 2) Código NO documentado

### 2.1 — Campo `temperature` en planner, critic, phase_reactor, final_reactor

**Dónde:** orchestrator.rs líneas 666-671 (final_reactor), similar patrón para planner/reactor

```rust
if let Some(temp) = final_reactor_cfg
    .get("temperature")
    .and_then(|v| v.as_f64())
{
    llm_config = llm_config.with_temperature(temp as f32)?;
}
```

**Hallazgo:** El código acepta y aplica el campo `temperature` en todas las componentes LLM internas (planner, critic, phase_reactor, final_reactor), pero la documentación `node_configurations.json` no incluye este campo en las sub_fields de ninguna componente. 

**Impacto:** Los usuarios no saben que pueden controlar la temperatura en estos componentes. El parámetro llm_shared_fields (global) no lo documenta; solo se documenta en `llm_call` individualmente.

### 2.2 — Campo `thinking_budget` en final_reactor

**Dónde:** orchestrator.rs líneas 672-677

```rust
if let Some(budget) = final_reactor_cfg
    .get("thinking_budget")
    .and_then(|v| v.as_u64())
{
    llm_config = llm_config.with_thinking_budget(budget as u32);
}
```

**Hallazgo:** El código soporta `thinking_budget` (razonamiento extendido para Anthropic/modelos que lo soporten) en final_reactor, pero NO está documentado en node_configurations.json ni en ningún lugar. Solo aparece en `llm_call` en algunas guías.

**Impacto:** No es posible activar razonamiento extendido en la síntesis final sin leer el código Rust.

### 2.3 — Campo `allow_suspend` en agentes (agent sub_fields)

**Dónde:** orchestrator.rs línea 1609 (`allow_suspend_for(&subgraph_cfg)` aplicado a agente)

**Hallazgo:** El código verifica la presencia de `allow_suspend` en la configuración de cada agente (subgraph), imprimiendo un log de advertencia si está a `false` pero el agente suspende igualmente. Sin embargo, `allow_suspend` no está documentada como un sub_field válido bajo `agents` en node_configurations.json. 

Está documentada en planner, critic, phase_reactor, final_reactor, pero NO en agents.

**Impacto:** Operadores pueden intentar agregar `allow_suspend: false` a un agente esperando que bloquee la suspensión, pero el código lo ignora (propaga igual). El log avisa, pero la expectativa de control no está alineada con la doc.

### 2.4 — Validación permisiva de `agents.description`

**Dónde:** orchestrator.rs línea 941 (planner setup)

```rust
let desc = props
    .get("description")
    .and_then(|v| v.as_str())
    .unwrap_or("No description provided");
agent_descriptions.push_str(&format!("- {}: {}\n", name, desc));
```

**Hallazgo:** La documentación marca `description` como `"required": true` en las sub_fields de agents. El código, sin embargo, lo trata como opcional y provee un default "No description provided" en lugar de fallar.

**Impacto:** Grafos pueden funcionar sin descripciones de agentes (el planner verá "No description provided"), contradiciendo la doc. Fail-closed esperado (error en validación) no ocurre.

---

## 3) Plan de pruebas QA

### T1 — Configuración mínima: solo final_reactor (sin planner)

**Objetivo:** Validar que orchestrator falla con error claro si final_reactor está ausente.

**Grafo JSON mínimo:**
```json
{
  "nodes": {
    "in": { "type": "input", "config": {} },
    "orch": {
      "type": "orchestrator",
      "config": {
        "final_reactor": {
          "provider": "google",
          "api_key": "${GEMINI_API_KEY}",
          "model": "gemini-2.5-flash"
        }
      }
    },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "in", "to": "orch" },
    { "from": "orch", "to": "out" }
  ]
}
```

**Comando:** `cargo run --bin dag_engine -- run <graph.json> --agent-session-id test_minimal_001`

**Entrada:** prompt = "Hola"

**Resultado esperado:** Error fallido `'final_reactor' is required in config` (orchestrator.rs:608).

**Verificación:** Capturar stderr/stdout, assert error message contains "final_reactor" + "required".

---

### T2 — Final reactor con campos temperature y thinking_budget

**Objetivo:** Verificar que `temperature` y `thinking_budget` se aceptan y aplican correctamente.

**Grafo JSON (mínimo con un agent):**
```json
{
  "nodes": {
    "in": {
      "type": "input",
      "config": { "user_message": "¿Cuánto es 2+2?" }
    },
    "orch": {
      "type": "orchestrator",
      "config": {
        "planner": {
          "provider": "google",
          "api_key": "${GEMINI_API_KEY}",
          "model": "gemini-2.5-flash"
        },
        "agents": {
          "calculator": {
            "description": "Calcula números",
            "child_graph_inline": {
              "nodes": {
                "in": { "type": "input", "config": {} },
                "out": { "type": "output", "config": {} }
              },
              "edges": [{ "from": "in", "to": "out" }]
            }
          }
        },
        "final_reactor": {
          "provider": "google",
          "api_key": "${GEMINI_API_KEY}",
          "model": "gemini-2.5-flash",
          "temperature": 0.2,
          "thinking_budget": 5000
        }
      }
    },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "in", "to": "orch" },
    { "from": "orch", "to": "out" }
  ]
}
```

**Comando:** `cargo run --bin dag_engine -- run <graph.json> --agent-session-id test_temp_001`

**Resultado esperado:** Ejecución completada sin error; final_response contiene respuesta (idealmente numérica o corta porque temperature=0.2).

**Verificación:** 
- No hay error en stderr sobre temperature/thinking_budget inválido.
- final_response existe y es no-vacía.
- SSE no tiene errores de parser para LLM config.

---

### T3 — Agent sin campo `description`

**Objetivo:** Validar que agentes sin descripción se procesan correctamente (con default).

**Grafo JSON:**
```json
{
  "nodes": {
    "in": {
      "type": "input",
      "config": { "user_message": "Hola" }
    },
    "orch": {
      "type": "orchestrator",
      "config": {
        "planner": {
          "provider": "google",
          "api_key": "${GEMINI_API_KEY}",
          "model": "gemini-2.5-flash"
        },
        "agents": {
          "missing_desc_agent": {
            "child_graph_inline": {
              "nodes": {
                "in": { "type": "input", "config": {} },
                "out": { "type": "output", "config": {} }
              },
              "edges": [{ "from": "in", "to": "out" }]
            }
          }
        },
        "final_reactor": {
          "provider": "google",
          "api_key": "${GEMINI_API_KEY}",
          "model": "gemini-2.5-flash"
        }
      }
    },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "in", "to": "orch" },
    { "from": "orch", "to": "out" }
  ]
}
```

**Comando:** `cargo run --bin dag_engine -- run <graph.json> --agent-session-id test_no_desc_001`

**Resultado esperado:** Ejecución completada sin error (descripción reemplazada por "No description provided" en logs).

**Verificación:** 
- No hay error de validación.
- Logs contienen "No description provided" o similar cuando se lista agent_descriptions para el planner (orchestrator.rs:943).

---

### T4 — Agent con allow_suspend: false (pero suspensión ocurre en subgraph)

**Objetivo:** Verificar que `allow_suspend: false` en agent NO bloquea la propagación de suspend (comportamiento actual).

**Grafo JSON:**
```json
{
  "nodes": {
    "in": {
      "type": "input",
      "config": { "user_message": "Ejecuta tarea" }
    },
    "orch": {
      "type": "orchestrator",
      "config": {
        "planner": {
          "provider": "google",
          "api_key": "${GEMINI_API_KEY}",
          "model": "gemini-2.5-flash"
        },
        "agents": {
          "suspending_agent": {
            "description": "Agente que se suspende",
            "allow_suspend": false,
            "child_graph_inline": {
              "nodes": {
                "in": { "type": "input", "config": {} },
                "susp": {
                  "type": "suspend",
                  "config": { "id": "test_q", "question": "¿Continuar?" }
                },
                "out": { "type": "output", "config": {} }
              },
              "edges": [
                { "from": "in", "to": "susp" },
                { "from": "susp", "to": "out" }
              ]
            }
          }
        },
        "final_reactor": {
          "provider": "google",
          "api_key": "${GEMINI_API_KEY}",
          "model": "gemini-2.5-flash"
        }
      }
    },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "in", "to": "orch" },
    { "from": "orch", "to": "out" }
  ]
}
```

**Comando (Run 1):** `cargo run --bin dag_engine -- run <graph.json> --agent-session-id test_allow_false_001`

**Resultado esperado en Run 1:** Orchestrator emite SUSPENDED con la pregunta del suspend node.

**Verificación:**
- __colmena_loop_status = "SUSPENDED"
- La pregunta "¿Continuar?" está en extra_info.
- Logs contienen advertencia "has allow_suspend=false, but its subgraph already suspended. Flag has no safe effect..."

---

### T5 — Final reactor con missing provider/api_key

**Objetivo:** Validar fail-closed en final_reactor: provider y api_key son realmente obligatorios.

**Grafo JSON (final_reactor sin provider):**
```json
{
  "nodes": {
    "in": { "type": "input", "config": { "user_message": "Hola" } },
    "orch": {
      "type": "orchestrator",
      "config": {
        "planner": {
          "provider": "google",
          "api_key": "${GEMINI_API_KEY}",
          "model": "gemini-2.5-flash"
        },
        "agents": {
          "dummy": {
            "description": "dummy",
            "child_graph_inline": {
              "nodes": {
                "in": { "type": "input", "config": {} },
                "out": { "type": "output", "config": {} }
              },
              "edges": [{ "from": "in", "to": "out" }]
            }
          }
        },
        "final_reactor": {
          "api_key": "${GEMINI_API_KEY}",
          "model": "gemini-2.5-flash"
        }
      }
    },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "in", "to": "orch" },
    { "from": "orch", "to": "out" }
  ]
}
```

**Comando:** `cargo run --bin dag_engine -- run <graph.json> --agent-session-id test_final_no_provider_001`

**Resultado esperado:** Error con mensaje "final_reactor: missing 'provider'" (orchestrator.rs:616).

**Verificación:** Assert stderr contains "final_reactor: missing 'provider'".

---

### T6 — Output ports: final_response, all_tasks, extra_info.__colmena_loop_status

**Objetivo:** Verificar que los outputs documentados en node_ports_reference están presentes en el resultado.

**Grafo JSON:** (usar T4 pero simple happy path sin suspend)

```json
{
  "nodes": {
    "in": {
      "type": "input",
      "config": { "user_message": "Test output ports" }
    },
    "orch": {
      "type": "orchestrator",
      "config": {
        "planner": {
          "provider": "google",
          "api_key": "${GEMINI_API_KEY}",
          "model": "gemini-2.5-flash"
        },
        "agents": {
          "echo": {
            "description": "Echo agent",
            "child_graph_inline": {
              "nodes": {
                "in": { "type": "input", "config": {} },
                "out": { "type": "output", "config": {} }
              },
              "edges": [{ "from": "in", "to": "out" }]
            }
          }
        },
        "final_reactor": {
          "provider": "google",
          "api_key": "${GEMINI_API_KEY}",
          "model": "gemini-2.5-flash"
        }
      }
    },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "in", "to": "orch" },
    { "from": "orch", "to": "out" }
  ]
}
```

**Comando:** `cargo run --bin dag_engine -- run <graph.json> --agent-session-id test_outputs_001`

**Resultado esperado:** El output del orchestrator contiene:
- `final_response` (string, no vacío)
- `all_tasks` (array, puede estar vacío o con tareas)
- `extra_info.__colmena_loop_status` = "FINISHED"
- `extra_info.phase_summaries` (array)

**Verificación:**
- Parse resultado JSON como object.
- Assert has keys: final_response, all_tasks, extra_info.
- Assert extra_info has __colmena_loop_status = "FINISHED".
- Assert final_response es string no-vacío.

---

### T7 — Default output port (`final_response`) y explicit edge fallback

**Objetivo:** Verificar que `default_output="final_response"` (orchestrator.rs:1869) se respeta en edges implícitas.

**Grafo JSON (edge implícita desde orchestrator):**
```json
{
  "nodes": {
    "in": {
      "type": "input",
      "config": { "user_message": "Qué es 2+2" }
    },
    "orch": {
      "type": "orchestrator",
      "config": {
        "planner": { "provider": "google", "api_key": "${GEMINI_API_KEY}", "model": "gemini-2.5-flash" },
        "agents": {
          "math": {
            "description": "math",
            "child_graph_inline": {
              "nodes": {
                "in": { "type": "input", "config": {} },
                "out": { "type": "output", "config": {} }
              },
              "edges": [{ "from": "in", "to": "out" }]
            }
          }
        },
        "final_reactor": { "provider": "google", "api_key": "${GEMINI_API_KEY}", "model": "gemini-2.5-flash" }
      }
    },
    "verifier": {
      "type": "python_script",
      "config": { "code": "output = {'received': response_text}" }
    },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "in", "to": "orch" },
    { "from": "orch", "to": "verifier.response_text" },
    { "from": "verifier", "to": "out" }
  ]
}
```

**Comando:** `cargo run --bin dag_engine -- run <graph.json> --agent-session-id test_default_output_001`

**Resultado esperado:** El python_script recibe `response_text` = final_response del orchestrator (la síntesis final).

**Verificación:** 
- Output no-vacío: verifier.output.received contiene texto no-vacío.
- No hay error de resolución de edge (el default_output se aplicó correctamente).

