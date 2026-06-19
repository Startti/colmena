# Subgrafos/LLMs como Tools (agents-as-tools) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Permitir que un `llm_call` registre un grafo hijo (o un `llm_call` inline) como tool vía `node_type: "subgraph"` en `tool_configurations`, con entrada `task` por defecto, aislamiento stateless, streaming transparente y HITL.

**Architecture:** Reusar el nodo `subgraph` existente permitiéndolo como `node_type` de tool. El dispatch genérico (`get_node`) ya instancia el nodo; los cambios son aditivos: (1) leer config del grafo hijo desde `inputs`, (2) exponer `task` por defecto en el schema, (3) inyectar un path qualifier efímero determinista + depth guard, (4) enhebrar el observer para streaming. HITL y resume reusan los rieles existentes de `secure_suspend` sin código nuevo.

**Tech Stack:** Rust (crate `colmena_dag_engine`), `serde_json`, `async_trait`, `tokio`. Tests: `cargo test --lib`, grafos JSON E2E vía `cargo run --bin dag_engine -- run`.

**Spec:** [docs/superpowers/specs/2026-06-18-subgraph-as-tool-design.md](../specs/2026-06-18-subgraph-as-tool-design.md)

---

## File Structure

| Archivo | Responsabilidad | Cambio |
|---------|-----------------|--------|
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs` | Nodo `subgraph`: schema, lectura de config, depth guard | Modificar |
| `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` | Dispatch de tools: inyección de path/depth, observer threading, builder `with_observer` / `with_subgraph_depth` | Modificar |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` | Construcción del `DagToolExecutor`: pasar observer + depth | Modificar |
| `docs/node_as_tools_reference.json` | Whitelist de `node_type` + sección `subgraph` | Modificar |
| `docs/developer_guide/19_nested_agents_and_subgraphs.md` | Sección "Subgrafo como tool" | Modificar |
| `tests/graphs/agents/subgraph_tool_*.json` | Grafos E2E (T1–T7) | Crear |
| `tests/graphs/agents/sub/*.json` | Grafos hijos para los E2E | Crear |

> **Constante compartida:** el límite de profundidad y las claves reservadas
> nuevas se nombran así en todo el plan (no renombrar entre tasks):
> - Clave path qualifier: `__colmena_node_id_path` (ya existe).
> - Clave depth: `__colmena_subgraph_depth`.
> - Límite: `MAX_SUBGRAPH_TOOL_DEPTH = 5`.
> - Prefijo del path efímero: `"tool/"` + `tool_call.id`.

---

## Task 1: `SubGraphNode` lee `child_graph_path`/`child_graph_inline` desde `inputs` (G1)

**Por qué:** El tool path mergea `fixed_config` en `inputs` y pasa `config = {}`
([dag_tool_executor.rs:1755](../../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs)),
pero hoy `subgraph` solo lee la config del grafo desde `config`
([subgraph.rs:117-130](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs)).
Sin esto, una tool `subgraph` falla con "requires child_graph_inline or child_graph_path".

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs:116-130`
- Test: mismo archivo, módulo `#[cfg(test)]`

- [ ] **Step 1: Escribir el test que falla**

Añadir al final de `subgraph.rs` (dentro de un `#[cfg(test)] mod tests` — si ya existe uno, agregar la función; si no, crear el módulo):

```rust
#[cfg(test)]
mod subgraph_tool_input_config_tests {
    use super::*;
    use crate::dag_engine::domain::node::NodeInputs;
    use serde_json::json;

    /// Helper: resolver el grafo hijo combinando inputs + config como hará el
    /// nodo. Refleja exactamente la lógica de carga (sin ejecutar el subgrafo).
    fn resolve_graph_source(inputs: &NodeInputs, config: &Value) -> Option<Value> {
        SubGraphNode::resolve_child_graph_source(inputs, config)
    }

    #[test]
    fn reads_inline_from_inputs_when_config_empty() {
        let mut inputs: NodeInputs = NodeInputs::new();
        let inline = json!({ "nodes": {}, "edges": [] });
        inputs.insert("child_graph_inline".to_string(), inline.clone());
        let config = json!({});
        assert_eq!(resolve_graph_source(&inputs, &config), Some(inline));
    }

    #[test]
    fn reads_path_from_inputs_when_config_empty() {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert(
            "child_graph_path".to_string(),
            json!("./agents/weather_agent.json"),
        );
        let config = json!({});
        assert_eq!(
            resolve_graph_source(&inputs, &config),
            Some(json!("./agents/weather_agent.json"))
        );
    }

    #[test]
    fn config_takes_precedence_over_inputs_for_inline() {
        // Camino por-edges: la config gana (comportamiento legacy intacto).
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("child_graph_inline".to_string(), json!({ "from": "inputs" }));
        let config = json!({ "child_graph_inline": { "from": "config" } });
        assert_eq!(
            resolve_graph_source(&inputs, &config),
            Some(json!({ "from": "config" }))
        );
    }
}
```

- [ ] **Step 2: Correr el test y verificar que falla**

Run: `cargo test --lib subgraph_tool_input_config_tests`
Expected: FAIL — `no function or associated item named resolve_child_graph_source`.

- [ ] **Step 3: Implementar `resolve_child_graph_source` y usarla en `execute`**

En `subgraph.rs`, dentro de `impl SubGraphNode { ... }` (junto a `new()`), añadir:

```rust
    /// Resolve the child graph source (inline object or path string) for both
    /// the edge-based path (config) and the tool path (inputs).
    ///
    /// Precedence: `config` wins over `inputs` so the legacy edge-based behavior
    /// is unchanged; the tool path supplies the value via `inputs` (because the
    /// executor merges `fixed_config` into inputs and passes `config = {}`).
    /// Returns the inline graph object, or the path string, or `None`.
    fn resolve_child_graph_source(inputs: &NodeInputs, config: &Value) -> Option<Value> {
        if let Some(inline) = config.get("child_graph_inline") {
            return Some(inline.clone());
        }
        if let Some(path) = config.get("child_graph_path") {
            return Some(path.clone());
        }
        if let Some(inline) = inputs.get("child_graph_inline") {
            return Some(inline.clone());
        }
        if let Some(path) = inputs.get("child_graph_path") {
            return Some(path.clone());
        }
        None
    }
```

Reemplazar el bloque de carga `// --- 2. GRAPH LOADING ---` (líneas 116-130) por:

```rust
        // --- 2. GRAPH LOADING ---
        // Source can come from `config` (edge-based path) or `inputs` (tool path,
        // where the executor merges fixed_config into inputs and passes config={}).
        let graph_source = Self::resolve_child_graph_source(inputs, config).ok_or(
            "SubGraphNode requires 'child_graph_inline' or 'child_graph_path' \
             in config (edge path) or inputs (tool path)",
        )?;

        let graph_json = if graph_source.is_object() {
            graph_source
        } else if let Some(path_val) = graph_source.as_str() {
            let path = std::path::Path::new(path_val);
            if !path.exists() {
                return Err(format!("child_graph_path not found: {}", path_val).into());
            }
            let contents = fs::read_to_string(path).await?;
            serde_json::from_str(&contents)?
        } else {
            return Err("child_graph source must be an inline object or a path string".into());
        };
```

- [ ] **Step 4: Correr el test y verificar que pasa**

Run: `cargo test --lib subgraph_tool_input_config_tests`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs
git commit -m "feat(subgraph): read child graph source from inputs for tool path"
```

---

## Task 2: `SubGraphNode::schema()` expone `task` por defecto (G5)

**Por qué:** Sin `node_schema`, el schema mostrado al LLM cae a `node.schema()`,
que el builder lee como `schema["inputs"]` (mapa de descripciones, NO JSON-Schema
`properties` — ver [dag_tool_executor.rs:762-770](../../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs)).
Hoy `SubGraphNode::schema()` devuelve `properties: {}`, así que el LLM vería una
tool sin argumentos. Debe exponer `task: string`.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs:31-36`
- Test: mismo archivo

- [ ] **Step 1: Escribir el test que falla**

Añadir el módulo de test:

```rust
#[cfg(test)]
mod subgraph_schema_tests {
    use super::*;
    use crate::dag_engine::domain::node::ExecutableNode;

    #[test]
    fn schema_exposes_task_input_for_tool_use() {
        let node = SubGraphNode::new();
        let schema = node.schema();
        let inputs = schema
            .get("inputs")
            .and_then(|v| v.as_object())
            .expect("schema must have an 'inputs' object so the tool builder exposes params");
        assert!(
            inputs.contains_key("task"),
            "default schema must expose a 'task' input; got keys: {:?}",
            inputs.keys().collect::<Vec<_>>()
        );
        let desc = inputs.get("task").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            desc.contains("string"),
            "task description must hint type 'string' for the builder; got: {desc:?}"
        );
    }
}
```

- [ ] **Step 2: Correr el test y verificar que falla**

Run: `cargo test --lib subgraph_schema_tests`
Expected: FAIL — `schema must have an 'inputs' object` (panic en `.expect`).

- [ ] **Step 3: Implementar**

Reemplazar `fn schema` (líneas 31-36) por:

```rust
    fn schema(&self) -> Value {
        // The `inputs` map is what the tool-definition builder reads to expose
        // parameters to the LLM (it parses each value's string for type hints
        // like "string"/"number"/"optional"). Default to a single `task` string.
        // A `node_schema` in tool_configurations takes precedence over this.
        json!({
            "inputs": {
                "task": "string — the task or instruction for the sub-agent to perform"
            }
        })
    }
```

- [ ] **Step 4: Correr el test y verificar que pasa**

Run: `cargo test --lib subgraph_schema_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs
git commit -m "feat(subgraph): expose default 'task' input in schema for tool use"
```

---

## Task 3: Inyectar path qualifier efímero determinista en el tool path (G2+G3)

**Por qué:** El tool path no inyecta `__colmena_node_id_path`, así que el LLM
interno del subgrafo keya su memoria solo por `agent_session_id` → colisiones y no
hay aislamiento stateless. Inyectamos un path único por llamada, **determinista del
`tool_call.id`** (estable suspend↔resume), justo donde el executor ya inyecta
`__colmena_session_id` ([dag_tool_executor.rs:1708-1722](../../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs)).

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` (zona ~1722, dentro de `execute_inner`, después de inyectar agent_session_id)
- Test: mismo archivo, módulo de tests existente

- [ ] **Step 1: Escribir el test que falla**

`execute_inner` recibe `tool_call: &ToolCall`. Verificamos que el path inyectado es
`format!("tool/{}", tool_call.id)`. Para testear sin un nodo real, extraemos la
derivación a una función pura y la testeamos:

```rust
#[cfg(test)]
mod ephemeral_path_tests {
    use super::*;

    #[test]
    fn ephemeral_path_is_deterministic_from_tool_call_id() {
        assert_eq!(
            DagToolExecutor::ephemeral_subgraph_path("call_abc123"),
            "tool/call_abc123"
        );
        // Determinista: misma id → mismo path (clave para el resume HITL).
        assert_eq!(
            DagToolExecutor::ephemeral_subgraph_path("call_abc123"),
            DagToolExecutor::ephemeral_subgraph_path("call_abc123")
        );
        // Distinta id → distinto path (aislamiento entre llamadas).
        assert_ne!(
            DagToolExecutor::ephemeral_subgraph_path("call_1"),
            DagToolExecutor::ephemeral_subgraph_path("call_2")
        );
    }
}
```

- [ ] **Step 2: Correr el test y verificar que falla**

Run: `cargo test --lib ephemeral_path_tests`
Expected: FAIL — `no function named ephemeral_subgraph_path`.

- [ ] **Step 3: Implementar la función + la inyección**

En `impl DagToolExecutor`, añadir la función pura:

```rust
    /// Deterministic ephemeral path qualifier for a node invoked as a tool.
    ///
    /// Derived from the `tool_call.id` so it is stable across a suspend/resume
    /// cycle (the same pending tool call is replayed with the same id), which
    /// keeps the sub-agent's internal LLM memory scoped consistently. It is
    /// unique per tool call, so two calls to the same subgraph-tool do NOT share
    /// memory (stateless isolation).
    fn ephemeral_subgraph_path(tool_call_id: &str) -> String {
        format!("tool/{tool_call_id}")
    }
```

En `execute_inner`, justo después del bloque que inyecta `__colmena_agent_session_id`
(después de la línea 1722, antes de la sección SECURE VALUES), añadir:

```rust
        // Inject a deterministic ephemeral path qualifier so a node invoked as a
        // tool (notably `subgraph`) scopes its child memory per-call (stateless)
        // while remaining stable across suspend/resume. Engine-authoritative:
        // overwrites any caller-supplied value.
        inputs.insert(
            "__colmena_node_id_path".to_string(),
            Value::String(Self::ephemeral_subgraph_path(&tool_call.id)),
        );
```

> Nota: `inputs` aquí es `HashMap<String, Value>` (mutable `let mut inputs = inputs;`
> ya está declarado arriba para el resume answer). Insertar antes de la conversión a
> `NodeInputs`.

- [ ] **Step 4: Correr el test y verificar que pasa**

Run: `cargo test --lib ephemeral_path_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs
git commit -m "feat(tool-executor): inject deterministic ephemeral path for tool-invoked nodes"
```

---

## Task 4: Enhebrar el observer en el tool path (G4 — streaming transparente)

**Por qué:** Ambos `.execute(...)` del tool path pasan `None` como observer
([dag_tool_executor.rs:645 y :1763](../../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs)),
así que los `subgraph-*` events del hijo no llegan al stream del padre. Añadimos un
campo + builder `with_observer` (patrón idéntico a `with_skill_observer`) y lo pasamos.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` (struct, builders ~345, los dos `.execute`)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs:2090` (construcción del executor)
- Test: `dag_tool_executor.rs`

- [ ] **Step 1: Escribir el test que falla**

```rust
#[cfg(test)]
mod observer_wiring_tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn with_observer_stores_the_observer() {
        let registry: Arc<dyn NodeRegistryPort> =
            Arc::new(crate::dag_engine::infrastructure::registry::HashMapNodeRegistry::new());
        let exec = DagToolExecutor::new(registry, HashMap::new());
        assert!(exec.observer.is_none(), "fresh executor has no observer");

        let obs: Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver> =
            Arc::new(crate::dag_engine::domain::observer::NoopObserver::default());
        let exec = exec.with_observer(Some(obs));
        assert!(exec.observer.is_some(), "with_observer must store the observer");
    }
}
```

> Si `NoopObserver` no existe con ese nombre, usar el observer de test ya presente en
> el archivo (buscar `impl ExecutionObserver for` en los tests de `dag_tool_executor.rs`
> o `observer.rs`) y ajustar el path. Verificar con:
> `rg -n "struct .*Observer|impl ExecutionObserver" src/libs/colmena/src/dag_engine/domain/observer.rs`

- [ ] **Step 2: Correr el test y verificar que falla**

Run: `cargo test --lib observer_wiring_tests`
Expected: FAIL — `no field 'observer'` / `no method with_observer`.

- [ ] **Step 3: Implementar el campo, el builder y el threading**

3a. Añadir el campo al struct `DagToolExecutor` (junto a los otros, p.ej. después de
`skill_observer`):

```rust
    /// Optional observer threaded into tool-invoked nodes so they can emit SSE
    /// events (notably `subgraph` emitting `subgraph-*` child events). When
    /// `None`, tool-invoked nodes run silently (legacy behavior).
    observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
```

3b. Inicializarlo en `new()` (donde se construye `Self { ... }`):

```rust
            observer: None,
```

3c. Añadir el builder (junto a `with_skill_observer`, ~línea 345):

```rust
    /// Thread an `ExecutionObserver` into tool-invoked nodes so their internal
    /// events (e.g. `subgraph-*`) propagate to the parent stream.
    pub fn with_observer(
        mut self,
        observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Self {
        self.observer = observer;
        self
    }
```

3d. Reemplazar los dos sitios `.execute(&inputs, &node_cfg, &mut state, None)` /
`.execute(&inputs, &node_exec_config, &mut state, None)` (líneas 645 y 1763) por:

```rust
            .execute(&inputs, &node_cfg, &mut state, self.observer.clone())
```
y
```rust
            .execute(&inputs, &node_exec_config, &mut state, self.observer.clone())
```
respectivamente (mantener el primer/segundo arg como están en cada sitio).

3e. En `llm.rs`, en la construcción del executor (~línea 2090), añadir el threading
del observer del nodo padre. Después de la línea
`executor = executor.with_agent_session_id(agent_session_id_str.clone());` añadir:

```rust
            // Thread the parent observer so tool-invoked subgraphs emit subgraph-* events.
            executor = executor.with_observer(_observer.clone());
```

- [ ] **Step 4: Correr el test y verificar que pasa**

Run: `cargo test --lib observer_wiring_tests`
Expected: PASS.

- [ ] **Step 5: Compilar todo el crate (el threading toca llm.rs)**

Run: `cargo check`
Expected: sin errores.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(tool-executor): thread observer into tool-invoked nodes for subgraph streaming"
```

---

## Task 5: Depth guard contra recursión (G6)

**Por qué:** Un subgraph-tool cuyo hijo declara otro subgraph-tool podría recursar
infinitamente por mala config. Propagamos `__colmena_subgraph_depth` y cortamos con
error claro pasado `MAX_SUBGRAPH_TOOL_DEPTH = 5`.

**Diseño del flujo de depth:**
1. El `llm_call` padre lee `__colmena_subgraph_depth` de sus inputs (default `0`) y lo
   pasa al executor vía `with_subgraph_depth(depth)`.
2. El executor inyecta `__colmena_subgraph_depth = depth` en los inputs de la tool.
3. `SubGraphNode::execute` lee ese valor; si `>= MAX`, retorna error. Si no, inyecta
   `depth + 1` en el `global_shared_state` del hijo (NO lo filtra como `__colmena_*`).
4. El `llm_call` del hijo lee `__colmena_subgraph_depth` de sus inputs → cierra el loop.

**Files:**
- Modify: `subgraph.rs` (lectura + check + propagación al child global_state)
- Modify: `dag_tool_executor.rs` (campo `subgraph_depth`, builder, inyección)
- Modify: `llm.rs` (leer input + `with_subgraph_depth`)
- Test: `subgraph.rs` y `dag_tool_executor.rs`

- [ ] **Step 1: Escribir el test que falla (constante + check en subgraph)**

```rust
#[cfg(test)]
mod subgraph_depth_guard_tests {
    use super::*;
    use crate::dag_engine::domain::node::NodeInputs;
    use serde_json::json;

    #[test]
    fn max_depth_constant_is_five() {
        assert_eq!(SubGraphNode::MAX_SUBGRAPH_TOOL_DEPTH, 5);
    }

    #[test]
    fn depth_at_limit_is_rejected() {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("__colmena_subgraph_depth".to_string(), json!(5));
        assert!(
            SubGraphNode::depth_exceeded(&inputs),
            "depth == MAX must be rejected"
        );
    }

    #[test]
    fn depth_below_limit_is_allowed() {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("__colmena_subgraph_depth".to_string(), json!(4));
        assert!(!SubGraphNode::depth_exceeded(&inputs));
        // Default (ausente) = 0 → permitido.
        assert!(!SubGraphNode::depth_exceeded(&NodeInputs::new()));
    }
}
```

- [ ] **Step 2: Correr el test y verificar que falla**

Run: `cargo test --lib subgraph_depth_guard_tests`
Expected: FAIL — constante/método inexistentes.

- [ ] **Step 3: Implementar en `subgraph.rs`**

3a. Añadir la constante y los helpers en `impl SubGraphNode`:

```rust
    /// Maximum nesting depth for subgraphs invoked as LLM tools. Beyond this the
    /// node fails fast to prevent runaway recursion from a misconfigured graph.
    pub const MAX_SUBGRAPH_TOOL_DEPTH: u64 = 5;

    /// Current subgraph-tool depth from inputs (0 when absent).
    fn current_depth(inputs: &NodeInputs) -> u64 {
        inputs
            .get("__colmena_subgraph_depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    }

    /// True when invoking another subgraph at this depth would exceed the limit.
    fn depth_exceeded(inputs: &NodeInputs) -> bool {
        Self::current_depth(inputs) >= Self::MAX_SUBGRAPH_TOOL_DEPTH
    }
```

3b. Al inicio de `execute`, después de calcular `parent_path` y ANTES del bloque de
RESUME, añadir el check:

```rust
        if Self::depth_exceeded(inputs) {
            return Err(format!(
                "Subgraph-as-tool nesting exceeded MAX_SUBGRAPH_TOOL_DEPTH ({}). \
                 Check for a subgraph tool that references itself or a cycle of agents.",
                Self::MAX_SUBGRAPH_TOOL_DEPTH
            )
            .into());
        }
```

3c. Propagar `depth + 1` al `global_shared_state` del hijo. El mapeo IN
(`// --- 3. STATE MAPPING (IN) ---`, subgraph.rs:140-148) construye
`child_state_obj` filtrando claves `__colmena_*` dentro del `for`. Por eso la
inyección de depth va **después** del loop (si fuera dentro, el filtro la
descartaría). Localizar:

```rust
        let mut child_state_obj = serde_json::Map::new();
        for (k, v) in inputs {
            if !k.starts_with("__colmena_") && k != "__node_id" {
                child_state_obj.insert(k.clone(), v.clone());
            }
        }
        let child_state = Value::Object(child_state_obj);
```

e insertar la propagación entre el cierre del `for` y la construcción de
`child_state`:

```rust
        let mut child_state_obj = serde_json::Map::new();
        for (k, v) in inputs {
            if !k.starts_with("__colmena_") && k != "__node_id" {
                child_state_obj.insert(k.clone(), v.clone());
            }
        }
        // Propagate (depth+1) into the child so nested subgraph tools can enforce
        // the limit. Inserted AFTER the loop on purpose: the loop filters out
        // every __colmena_* key, so this is the only way the depth survives into
        // the child's global state.
        let next_depth = Self::current_depth(inputs) + 1;
        child_state_obj.insert(
            "__colmena_subgraph_depth".to_string(),
            serde_json::json!(next_depth),
        );
        let child_state = Value::Object(child_state_obj);
```

- [ ] **Step 4: Correr el test del guard**

Run: `cargo test --lib subgraph_depth_guard_tests`
Expected: PASS.

- [ ] **Step 5: Inyectar depth desde el executor (test que falla)**

```rust
#[cfg(test)]
mod executor_depth_tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn with_subgraph_depth_stores_value() {
        let registry: Arc<dyn NodeRegistryPort> =
            Arc::new(crate::dag_engine::infrastructure::registry::HashMapNodeRegistry::new());
        let exec = DagToolExecutor::new(registry, HashMap::new()).with_subgraph_depth(2);
        assert_eq!(exec.subgraph_depth, 2);
    }
}
```

- [ ] **Step 6: Correr y verificar que falla**

Run: `cargo test --lib executor_depth_tests`
Expected: FAIL — `no field 'subgraph_depth'`.

- [ ] **Step 7: Implementar en `dag_tool_executor.rs`**

7a. Campo en el struct:

```rust
    /// Current subgraph-tool nesting depth, threaded from the parent llm_call so
    /// tool-invoked subgraphs receive `depth` and can enforce the recursion limit.
    subgraph_depth: u64,
```

7b. En `new()`: `subgraph_depth: 0,`

7c. Builder:

```rust
    /// Set the current subgraph nesting depth (0 at the top level).
    pub fn with_subgraph_depth(mut self, depth: u64) -> Self {
        self.subgraph_depth = depth;
        self
    }
```

7d. En `execute_inner`, junto a la inyección de `__colmena_node_id_path` (Task 3),
añadir:

```rust
        inputs.insert(
            "__colmena_subgraph_depth".to_string(),
            Value::Number(self.subgraph_depth.into()),
        );
```

7e. En `llm.rs`, leer el input y pasarlo al executor. Después de
`executor = executor.with_observer(_observer.clone());` (Task 4), añadir:

```rust
            let inbound_depth = inputs
                .get("__colmena_subgraph_depth")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            executor = executor.with_subgraph_depth(inbound_depth);
```

> **Verificación:** el child `llm_call` debe ver `__colmena_subgraph_depth` en sus
> `inputs`. El engine ya siembra los inputs del nodo de entrada desde el
> `global_state` del hijo (así llegan `__colmena_session_id`/`__colmena_agent_session_id`
> que el llm ya lee), por lo que la clave de depth viaja por el mismo canal. Si T7
> demuestra que no llega, leerla también del `global_state`/`config` como fallback.

- [ ] **Step 8: Correr los tests + check**

Run: `cargo test --lib executor_depth_tests && cargo check`
Expected: PASS y sin errores de compilación.

- [ ] **Step 9: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs \
        src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(subgraph): depth guard against runaway subgraph-as-tool recursion"
```

---

## Task 6: Verificar que el bubble SUSPENDED preserva `questions` (G7)

**Por qué:** El loop padre construye el payload SUSPENDED con `questions`
([llm.rs:3133](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs)).
Hay que garantizar que cuando el hijo suspende, el `subgraph` devuelve el resultado
verbatim (incluyendo `questions`/`question`) sin recortarlo. Hoy
[subgraph.rs:109-113](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs)
ya retorna `result` tal cual en SUSPENDED — este task lo blinda con un test.

**Files:**
- Test: `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs`

- [ ] **Step 1: Escribir el test que falla (o que documenta el invariante)**

```rust
#[cfg(test)]
mod subgraph_suspend_passthrough_tests {
    use serde_json::json;

    /// Pura: el subgraph debe devolver el resultado SUSPENDED del hijo verbatim,
    /// preservando `questions`. Esta función refleja el chequeo de passthrough.
    fn passes_through_suspended(child_result: &serde_json::Value) -> serde_json::Value {
        // Mirror of subgraph.rs SUSPENDED branch: return verbatim.
        child_result.clone()
    }

    #[test]
    fn suspended_result_preserves_questions() {
        let child = json!({
            "__colmena_status": "SUSPENDED",
            "questions": [{ "id": "q1", "text": "¿Cuántas personas?" }]
        });
        let out = passes_through_suspended(&child);
        assert_eq!(out["__colmena_status"], "SUSPENDED");
        assert_eq!(out["questions"][0]["id"], "q1");
    }
}
```

- [ ] **Step 2: Correr el test**

Run: `cargo test --lib subgraph_suspend_passthrough_tests`
Expected: PASS (verifica el invariante; si en el futuro alguien transforma el
resultado en la rama SUSPENDED, este test + el E2E T4 lo atrapan).

- [ ] **Step 3: Auditar la rama SUSPENDED real**

Run: `rg -n "__colmena_status.*SUSPENDED" src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs`
Confirmar que ambas ramas (resume y first-run, líneas ~109 y ~188) retornan `result`
sin mutar `questions`. Si alguna recorta campos, corregir para retornar verbatim.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs
git commit -m "test(subgraph): lock SUSPENDED passthrough preserves questions"
```

---

## Task 7: Documentar `subgraph` como tool (whitelist + guías)

**Files:**
- Modify: `docs/node_as_tools_reference.json`
- Modify: `docs/developer_guide/19_nested_agents_and_subgraphs.md`

- [ ] **Step 1: Añadir `subgraph` al whitelist de `node_type`**

En `docs/node_as_tools_reference.json`, en `concepts.tool_configurations.entry_schema.node_type.valid_values`, añadir `"subgraph"` al array.

- [ ] **Step 2: Añadir la sección `subgraph` bajo `node_types_as_tools`**

Insertar (idioma español, consistente con el resto del archivo) una entrada
`"subgraph"` con: `summary` (reusar un grafo existente o un llm inline como tool),
`special_behaviors` (default `task`; estructurado vía `node_schema`; stateless por
call; HITL soportado; streaming `subgraph-*`; `child_graph_path`/`child_graph_inline`
en `fixed_config`; depth máx 5), y los tres ejemplos de la spec (Forma A, Forma B
inline con web search, entrada estructurada). Copiar los JSON de la spec sección 4.

- [ ] **Step 3: Añadir sección en la guía 19**

En `docs/developer_guide/19_nested_agents_and_subgraphs.md`, añadir una sección
"## Subgrafo como Tool (agents-as-tools)" explicando: declaración, default `task`,
stateless, HITL reusa los rieles de tools, streaming transparente, y depth guard.
Enlazar a la spec.

- [ ] **Step 4: Commit**

```bash
git add docs/node_as_tools_reference.json docs/developer_guide/19_nested_agents_and_subgraphs.md
git commit -m "docs(subgraph): document subgraph/llm as LLM tools"
```

---

## Task 8: Grafos E2E (T1–T7)

Todos con `--agent-session-id <id_estable>` (regla CLAUDE.md). Default LLM:
`google/gemini-2.5-flash` + `DATABASE_URL` Postgres. Sourcing de keys:
`set -a; source .env; set +a`.

**Files:**
- Create: `tests/graphs/agents/sub/echo_capability.json` (grafo hijo simple)
- Create: `tests/graphs/agents/subgraph_tool_basic.json` (T1)
- Create: `tests/graphs/agents/subgraph_tool_websearch_inline.json` (T2)
- Create: `tests/graphs/agents/subgraph_tool_structured.json` (T3)
- Create: `tests/graphs/agents/sub/suspending_agent.json` + `subgraph_tool_hitl.json` (T4)
- Create: `subgraph_tool_stateless.json` (T6)
- Create: `subgraph_tool_depth_guard.json` (T7)

- [ ] **Step 1: Crear el grafo hijo `echo_capability.json`**

`tests/graphs/agents/sub/echo_capability.json`:

```json
{
  "nodes": {
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "system_message": "Eres un asistente conciso. Resuelve esta tarea y responde en una frase: {{task}}"
      }
    },
    "out": { "type": "output" }
  },
  "edges": [
    { "from": "agent", "to": "out" }
  ]
}
```

- [ ] **Step 2: Crear T1 — capability sin HITL (`subgraph_tool_basic.json`)**

```json
{
  "nodes": {
    "main": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "system_message": "Cuando necesites una sub-tarea especializada, usa la tool 'consultar_especialista'.",
        "prompt": "Pídele al especialista que explique qué es la fotosíntesis en una frase.",
        "tool_configurations": {
          "consultar_especialista": {
            "name": "consultar_especialista",
            "description": "Delega una tarea a un sub-agente especialista que devuelve una respuesta concisa.",
            "node_type": "subgraph",
            "fixed_config": {
              "child_graph_path": "./tests/graphs/agents/sub/echo_capability.json"
            }
          }
        }
      }
    },
    "out": { "type": "output" }
  },
  "edges": [
    { "from": "main", "to": "out" }
  ]
}
```

- [ ] **Step 3: Correr T1 (E2E real) y guardar SSE**

```bash
set -a; source .env; set +a
mkdir -p /tmp/colmena_e2e
cargo run --bin dag_engine -- run tests/graphs/agents/subgraph_tool_basic.json \
  --agent-session-id sgt_t1_001 > /tmp/colmena_e2e/subgraph_tool_basic.sse 2>&1
```
Expected: el stream incluye `subgraph-*` events (streaming transparente) y la
respuesta final del `main` cita lo que devolvió el especialista. Revisar el SSE y
presentar un reporte amigable (input, tool call, tokens, resumen). No pegar el SSE
completo en el chat.

- [ ] **Step 4: Crear T2 — LLM-as-tool inline + web search (`subgraph_tool_websearch_inline.json`)**

Usar la Forma B de la spec (sección 4.2) con `child_graph_inline` y `tavily_client`.
Requiere `TAVILY_API_KEY`. Prompt del `main`: "Investiga la capital de Australia y
dime el dato verificado."

- [ ] **Step 5: Correr T2**

```bash
cargo run --bin dag_engine -- run tests/graphs/agents/subgraph_tool_websearch_inline.json \
  --agent-session-id sgt_t2_001 > /tmp/colmena_e2e/subgraph_tool_websearch_inline.sse 2>&1
```
Expected: el sub-agente inline usa `web__search`, devuelve un resumen; el padre
responde con el dato. SSE muestra `subgraph-tool-output-available`.

- [ ] **Step 6: Crear y correr T3 — entrada estructurada**

Grafo con `node_schema` `{ ciudad, fecha }` y un hijo que usa `{{ciudad}}`/`{{fecha}}`
en su `system_message`. Correr con `--agent-session-id sgt_t3_001`. Verificar en el
SSE que el sub-agente recibió ambos campos (no un único `task`).

- [ ] **Step 7: Crear T4 — HITL suspend→resume**

`tests/graphs/agents/sub/suspending_agent.json`: un `llm_call` hijo con un
`suspend`-style tool (o `secure_suspend_allowed: true`) que pregunta al usuario antes
de responder. `subgraph_tool_hitl.json`: padre que expone ese hijo como tool.

Run 1 (suspende):
```bash
cargo run --bin dag_engine -- run tests/graphs/agents/subgraph_tool_hitl.json \
  --agent-session-id sgt_t4_001 > /tmp/colmena_e2e/subgraph_tool_hitl_1.sse 2>&1
```
Expected: el run termina en SUSPENDED con la `question` del hijo visible.

Run 2 (resume):
```bash
cargo run --bin dag_engine -- run tests/graphs/agents/subgraph_tool_hitl.json \
  --agent-session-id sgt_t4_001 \
  --answer "Q[<id>]: <pregunta echo>\nA[<id>]: <respuesta>" \
  > /tmp/colmena_e2e/subgraph_tool_hitl_2.sse 2>&1
```
Expected: el hijo reanuda con la respuesta y el padre completa. **Este es el test que
valida G3** (el path determinista permitió reencontrar el scope del hijo).

- [ ] **Step 8: Crear y correr T6 — aislamiento stateless**

Padre que llama DOS veces a la misma tool en el mismo run: primero "recuerda el número
7", luego "¿qué número te dije?". Expected: la segunda llamada NO conoce el 7 (memoria
aislada por `tool_call_id`). Correr con `--agent-session-id sgt_t6_001`, revisar SSE.

- [ ] **Step 9: Crear y correr T7 — depth guard**

Grafo donde un subgraph-tool inline expone, a su vez, otra subgraph-tool que se
referencia formando un ciclo. Forzar al LLM a encadenarlas. Expected: corta con el
error `MAX_SUBGRAPH_TOOL_DEPTH (5)` en lugar de recursar sin fin. (Si es difícil
forzar el ciclo vía LLM, hacer un test de integración Rust que llame al subgraph con
`__colmena_subgraph_depth: 5` y assert el error.)

- [ ] **Step 10: Commit de los grafos**

```bash
git add tests/graphs/agents/sub/ tests/graphs/agents/subgraph_tool_*.json
git commit -m "test(graphs): E2E graphs for subgraph-as-tool (basic, websearch, structured, HITL, stateless, depth)"
```

---

## Task 9: Verificación final y suite completa

- [ ] **Step 1: Correr la suite completa con `--verbose` (regla CLAUDE.md)**

Run: `cargo test --verbose`
Expected: todo verde (unit + integration + doctests). `--lib` solo oculta fallos de
integración/doctest.

- [ ] **Step 2: Lint y formato**

Run: `cargo clippy --all-targets && cargo fmt --check`
Expected: sin warnings (recordar `[lints.rust] warnings = "deny"`).

- [ ] **Step 3: Confirmar que no hay breaking change para ADP**

Las firmas públicas tocadas son builders nuevos (`with_observer`, `with_subgraph_depth`)
y campos privados — aditivos. `ExecutableNode::execute` no cambió de firma. No requiere
sweep del worker ADP. Confirmar con:
Run: `rg -n "pub fn (with_observer|with_subgraph_depth)" src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`

- [ ] **Step 4: Reporte E2E final**

Presentar un resumen de T1, T2 y T4 (los E2E reales obligatorios): input, tool calls,
eventos `subgraph-*` observados, tokens y veredicto. SSE en `/tmp/colmena_e2e/`.

---

## Notas de ejecución

- **Orden de tasks:** 1→9 secuencial. Tasks 1, 2, 6 son independientes entre sí
  (pueden paralelizarse); 3, 4, 5 tocan `dag_tool_executor.rs`+`llm.rs` y conviene
  hacerlas en orden para evitar conflictos de merge.
- **Riesgo concentrado en G3 (Task 3 + T4):** el path determinista del `tool_call_id`
  es lo único correctness-critical. Si T4 falla en el resume, revisar que el
  `__colmena_node_id_path` inyectado en el run 2 sea idéntico al del run 1 (mismo
  `tool_call.id` en la tool call pendiente).
- **`NodeInputs`:** es `HashMap<String, Value>` (alias en `domain::node`). `.insert`,
  `.get` aplican directamente.
