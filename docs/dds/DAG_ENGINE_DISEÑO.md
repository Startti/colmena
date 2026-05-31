# DAG Engine - Documento de Diseño

> ⚠️ Algunas secciones técnicas pueden estar desactualizadas — referenciar [`12_dag_engine_guide.md`](../developer_guide/12_dag_engine_guide.md) para el estado actual.
>
> Este documento mantiene su valor como **registro de diseño y rationale** (arquitectura hexagonal, precedencia `inputs > config`, edge resolution, modos de ejecución). Los detalles técnicos (trait `ExecutableNode`, campos de `NodeConfig`, inventario de nodos) fueron refrescados; el roadmap histórico se conserva con anotaciones.

## Resumen Ejecutivo

El DAG Engine es un motor de orquestación de workflows basado en grafos acíclicos dirigidos (DAG), implementado en Rust con arquitectura hexagonal. Permite definir workflows complejos mediante archivos JSON y ejecutarlos de forma eficiente y extensible.

## Objetivo

Proporcionar un sistema de ejecución de workflows que:
- Sea extensible mediante nodos personalizados
- Permita configuración dinámica en runtime
- Soporte múltiples modos de ejecución (local vs producción)
- Integre fácilmente con servicios externos (HTTP, LLMs, etc.)
- Mantenga una arquitectura limpia y testable

## Arquitectura

### Principios de Diseño

1. **Arquitectura Hexagonal (Puertos y Adaptadores)**
   - Dominio puro sin dependencias externas
   - Lógica de aplicación independiente de infraestructura
   - Adaptadores intercambiables para diferentes implementaciones

2. **Inversión de Dependencias**
   - El dominio define interfaces (traits)
   - La infraestructura implementa las interfaces
   - La aplicación depende de abstracciones, no de concreciones

3. **Extensibilidad**
   - Nuevos nodos se añaden sin modificar el core
   - Registry pattern para descubrimiento de nodos
   - Configuración dinámica mediante precedencia inputs > config

### Capas de Arquitectura

```
┌─────────────────────────────────────────┐
│         Infrastructure Layer            │
│  ┌───────────────────────────────────┐  │
│  │ Nodes (HTTP, LLM, Math, etc.)    │  │
│  │ Registry (HashMapNodeRegistry)   │  │
│  │ Main (CLI, Server)               │  │
│  └───────────────────────────────────┘  │
└─────────────────────────────────────────┘
              ↓ implements
┌─────────────────────────────────────────┐
│         Application Layer               │
│  ┌───────────────────────────────────┐  │
│  │ DagRunUseCase                    │  │
│  │ Topological Sort                 │  │
│  │ Edge Resolution                  │  │
│  └───────────────────────────────────┘  │
└─────────────────────────────────────────┘
              ↓ depends on
┌─────────────────────────────────────────┐
│           Domain Layer                  │
│  ┌───────────────────────────────────┐  │
│  │ Graph, Node, Edge (data)         │  │
│  │ ExecutableNode (trait)           │  │
│  │ NodeRegistryPort (trait)         │  │
│  │ DagError                         │  │
│  └───────────────────────────────────┘  │
└─────────────────────────────────────────┘
```

## Componentes Principales

### 1. Domain Layer

#### Graph
```rust
pub struct Graph {
    /// Map of node ID -> NodeConfig.
    pub nodes: HashMap<String, NodeConfig>,
    /// Connections between nodes.
    pub edges: Vec<Edge>,
    /// Optional IANA timezone (e.g. "Europe/Madrid") for temporal-context injection.
    pub timezone: Option<String>,
    /// Optional human-readable location (e.g. "Madrid, España") for geo-context injection.
    pub location: Option<String>,
    /// Optional BCP-47 locale tag (e.g. "es-ES") for localization features.
    pub locale: Option<String>,
}
```
Representa la estructura del DAG definida en JSON. Incluye `validate()` que rechaza node IDs con `/` (reservado para path qualifiers en subgraphs).

Definida en `src/libs/colmena/src/dag_engine/domain/graph.rs`.

#### NodeConfig
```rust
pub struct NodeConfig {
    /// Tipo de nodo (ej. "llm_call", "http_request", "loop_controller").
    #[serde(rename = "type")]
    pub node_type: String,

    /// Configuración estática del nodo (forma libre por tipo).
    #[serde(default)]
    pub config: Value,

    /// Condición opcional que decide si el nodo debe correr en base al
    /// `__colmena_loop_status` global. Ej: "FINISHED_PHASE", "NEXT_TURN", "FINISHED".
    #[serde(default)]
    pub trigger_on: Option<String>,

    /// Máximo de veces que el nodo puede ejecutarse durante un DAG run.
    #[serde(default)]
    pub max_total_calls: Option<u32>,

    /// Máximo de ejecuciones, desglosado por el ID del nodo llamador.
    /// Útil para limitar reentradas desde un loop_controller específico.
    #[serde(default)]
    pub max_calls_from: Option<HashMap<String, u32>>,
}
```
Configuración de cada nodo en el grafo. Los campos `trigger_on`, `max_total_calls` y `max_calls_from` habilitan loops controlados y fan-in determinístico.

#### Edge
```rust
pub struct Edge {
    pub from: String,  // "source_node.output.field"
    pub to: String,    // "target_node.input.field"

    /// Si `true`, el edge se trata como cíclico (backward edge):
    /// no bloquea la ejecución inicial del nodo destino durante el
    /// ordenamiento topológico. Indispensable para loops controlados
    /// por `loop_controller`.
    #[serde(default)]
    pub cyclic: Option<bool>,
}
```
Define las conexiones entre nodos y el flujo de datos. Los edges marcados `cyclic: true` permiten construir ciclos sin romper el topological sort.

#### ExecutableNode Trait
```rust
#[async_trait::async_trait]
pub trait ExecutableNode: Send + Sync {
    /// Lógica principal del nodo.
    /// - `inputs`: outputs ya resueltos de los nodos antecesores (vía edges).
    /// - `config`: bloque `config` estático del NodeConfig.
    /// - `state`: estado global mutable del DAG run.
    /// - `observer`: notificador opcional de eventos de ejecución.
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        state: &mut Value,
        observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>>;

    /// JSON Schema que describe config, inputs y outputs del nodo (consumido por el frontend).
    fn schema(&self) -> Value;

    /// Descripción legible para humanos y LLMs (usada cuando el nodo actúa como tool).
    fn description(&self) -> Option<&str> { None }

    /// Texto adicional derivado de `fixed_config`, anexado al bloque de contexto
    /// cuando el nodo se usa como tool. Debe ser una función pura (sin I/O).
    fn tool_description_supplement(&self, _fixed_config: &Value) -> Option<String> { None }

    /// Nombre del puerto de input por defecto cuando un edge entrante no especifica campo.
    fn default_input(&self) -> Option<&str> { None }

    /// Nombre del puerto de output por defecto cuando un edge saliente no especifica campo.
    fn default_output(&self) -> Option<&str> { None }
}
```
Definido en `src/libs/colmena/src/dag_engine/domain/node.rs`. Notas relevantes:

- El bound de error es `Box<dyn StdError + Send + Sync>` (no solo `StdError`) — requerido para propagar errores entre threads en async.
- `observer` permite emitir eventos estructurados (start / progress / finish / error) hacia consumidores externos sin acoplar el nodo a la infraestructura.
- `default_input` / `default_output` evitan repetir el campo en cada edge cuando el nodo tiene un puerto canónico.

Existen además dos traits complementarios en el mismo módulo `domain/`:

- **`ToolkitNode`** (`toolkit_node.rs`): nodos que también exponen sub-herramientas dispatcheables por un LLM (ej. `tavily_client`, `api_explorer`, `echo_toolkit`).
- **`InitializableNode`** (`initializable_node.rs`): hook de inicialización para nodos que necesitan setup antes del primer `execute`.

### 2. Application Layer

#### DagRunUseCase
Orquesta la ejecución del grafo:

1. **Validación**: Verifica que el grafo no tenga ciclos
2. ** Ordenamiento Topológico**: Determina el orden de ejecución
3. **Resolución de Inputs**: Construye inputs para cada nodo desde edges
4. **Ejecución**: Ejecuta nodos en orden, pasando datos entre ellos
5. **Gestión de Estado**: Mantiene outputs de cada nodo

Pseudocódigo:
```
function execute(graph):
    validate_no_cycles(graph)
    execution_order = topological_sort(graph)
    outputs = {}
    
    for node_id in execution_order:
        inputs = build_inputs_from_edges(node_id, graph.edges, outputs)
        node = registry.get_node(node_id.type)
        result = node.execute(inputs, node.config, state)
        outputs[node_id] = result
    
    return final_output
```

### 3. Infrastructure Layer

#### Node Implementations

##### HttpNode
```rust
pub struct HttpNode;

impl ExecutableNode for HttpNode {
    async fn execute(...) -> Result<Value, Box<dyn StdError>> {
        // 1. Resolve config (inputs > config)
        let base_url = inputs.get("base_url")
            .or(config.get("base_url"));
        
        // 2. Make HTTP request
        let response = client.request(method, url).send().await?;
        
        // 3. Return result
        Ok(json!({
            "output": {
                "status": response.status(),
                "body": response.json().await?
            }
        }))
    }
}
```

**Características**:
- HTTP/1.1 forzado para compatibilidad
- User-Agent por defecto
- Configuración dinámica de endpoint, método, headers
- Soporte GET, POST, PUT, DELETE

##### LlmNode
```rust
pub struct LlmNode;

impl ExecutableNode for LlmNode {
    async fn execute(...) -> Result<Value, Box<dyn StdError>> {
        // 1. Resolve provider dynamically
        let provider_kind = resolve_provider(inputs, config)?;
        
        // 2. Create LLM config
        let llm_config = LlmConfig::new(provider)
            .with_temperature(temp)?
            .with_max_tokens(tokens)?;
        
        // 3. Execute LLM call
        let repository = LlmProviderFactory::create(provider_kind);
        let use_case = LlmCallUseCase::new(repository);
        let response = use_case.execute(messages, llm_config).await?;
        
        // 4. Return result
        Ok(json!({
            "output": {
                "content": response.content(),
                "usage": response.usage()
            }
        }))
    }
}
```

**Características**:
- Multi-provider via `ProviderKind` { `OpenAi`, `Google`, `Anthropic`, `Mock`, `Generated` } — `GEMINI_API_KEY` mapea a `Google`.
- Configuración dinámica completa (precedencia `inputs > config`).
- Retorna usage statistics.
- Integración con módulo LLM existente; soporta carga de skills via `skills_path` / `skills_paths` y la tool sintética `load_skill({name, reference?})`.

##### TriggerWebhookNode
```rust
pub struct TriggerWebhookNode;

impl ExecutableNode for TriggerWebhookNode {
    async fn execute(...) -> Result<Value, Box<dyn StdError>> {
        // Priority: __payload__ > test_payload > inputs
        let payload = config.get("__payload__")
            .or(config.get("test_payload"))
            .or(serde_json::to_value(inputs)?);
        
        Ok(json!({ "output": payload }))
    }
}
```

**Características**:
- Soporte para modo `serve` (`__payload__`)
- Soporte para modo `run` (`test_payload`)
- Permite testing sin servidor

#### Inventario de Nodos Registrados

Los tres ejemplos anteriores (`HttpNode`, `LlmNode`, `TriggerWebhookNode`) ilustran patrones; el registry real (`src/libs/colmena/src/dag_engine/infrastructure/registry.rs`) registra una superficie mucho mayor. Algunos requieren dependencias opcionales (storage, secure-value service) y solo se registran si están disponibles.

| `type` en `graph.json` | Implementación (`infrastructure/nodes/...`) | Notas |
| --- | --- | --- |
| `mock_input` | `debug.rs::MockInputNode` | Inputs sintéticos para tests. |
| `log` | `debug.rs::LogNode` | Imprime su input. |
| `output` | `output.rs::OutputNode` | Marca el output final del DAG. |
| `add`, `subtract`, `multiply`, `divide`, `exponential` | `math.rs` | Aritmética básica. |
| `current_time` | `current_time.rs` | Devuelve la hora actual (timezone-aware vía `Graph.timezone`). |
| `trigger_webhook` | `trigger.rs::TriggerWebhookNode` | Entry point en modo `serve`. |
| `input` | `input.rs::InputNode` | Entrada parametrizada para CLI / API. |
| `http_request` | `http.rs::HttpNode` | Cliente HTTP; resuelve `$attachment:<id>` si hay storage. |
| `socketio_request` | `socketio.rs::SocketIoNode` | Cliente Socket.IO. |
| `sql_query` | `sql.rs::SqlNode` | Requiere `SqlPortFactory` + pool registry. |
| `llm_call` | `llm.rs::LlmNode` | Multi-provider, tools, skills, secure values. |
| `python_script` | `python_node.rs::PythonNode` | Ejecuta script Python embebido. |
| `information_extraction` | `extraction.rs::ExtractionNode` | Extracción estructurada vía LLM. |
| `suspend` | `suspend.rs::SuspendNode` | Pausa el DAG hasta resume externo. |
| `secure_suspend` | `secure_suspend.rs::SecureSuspendNode` | Variante con `SecureValueService` (TTL 24h). Solo si hay servicio. |
| `loop_controller` | `loop_controller.rs::LoopControllerNode` | Driver de loops; combina con `Edge.cyclic` y `NodeConfig.trigger_on`. |
| `orchestrator` | `orchestrator.rs::OrchestratorNode` | Config anidada `{ planner, critic, phase_reactor, final_reactor }`. Ver `tests/graphs/advanced/trip_planner_v2.json`. |
| `task_memory_writer` | `task_memory_writer.rs` | Persiste memoria de tareas. |
| `planner` | `planner.rs::PlannerNode` | Planificación de tareas. |
| `critic` | `critic.rs::CriticNode` | Crítica de outputs; prompts en inglés (`=== PREVIOUS ATTEMPT — WHY IT FAILED ===`). |
| `reactor` | `reactor.rs::ReactorNode` | Reactor de fase / final. Requiere `task_memory_repo`. |
| `tavily_client` | `tavily_client.rs::TavilyClientNode` | Búsqueda web; también `ToolkitNode`. Secure-values opcional. |
| `document_create`, `document_edit`, `document_read` | `document_nodes.rs` | Documentos (incluye `ArtifactKind::Html` desde PR #79). |
| `api_explorer` | `api_explorer.rs::ApiExplorerNode` | Exploración dinámica de APIs; también `ToolkitNode`. |
| `image_generation` | `image_generation.rs::ImageGenerationNode` | Solo si hay `AssetStore`. |
| `image_edit` | `image_edit.rs::ImageEditNode` | Solo si hay `AssetStore`. |
| `tts` | `tts.rs::TtsNode` | Solo si hay `AssetStore`. |
| `subgraph` | `subgraph.rs::SubGraphNode` | Composición de DAGs; los node IDs hijos se cualifican como `outer/inner`. |

Existen además helpers en `infrastructure/nodes/`: `qa_response_parser.rs`, `echo_toolkit.rs`, `llm_synthetic_tools/`, `prompts/`, `util/`. La lista canónica vive en `HashMapNodeRegistry::new_with_secure_values`.

#### HashMapNodeRegistry
```rust
pub struct HashMapNodeRegistry {
    nodes: HashMap<String, Arc<dyn ExecutableNode>>,
}

impl NodeRegistryPort for HashMapNodeRegistry {
    fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
        self.nodes.get(node_type).cloned()
    }
}
```

Registry simple que mapea strings a implementaciones de nodos.

## Configuración Dinámica

### Precedencia de Configuración

El sistema implementa una precedencia `inputs > config` que permite:
1. Valores base en `config` (estáticos)
2. Override runtime mediante `inputs` (dinámicos)

```rust
let endpoint = inputs.get("endpoint").and_then(|v| v.as_str())
    .or_else(|| config.get("endpoint").and_then(|v| v.as_str()))
    .unwrap_or("");
```

### Beneficios

1. **Reutilización**: Grafos genéricos con valores dinámicos
2. **Flexibilidad**: Cambiar comportamiento sin modificar grafo
3. **Testing**: Facilita diferentes escenarios de prueba

## Flujo de Datos

### Edge Resolution

Los edges usan sintaxis JSON-pointer para extraer campos específicos:

```
"from": "http_call.output.body.data"
"to": "llm.prompt"
```

El motor:
1. Busca el output de `http_call`
2. Navega por `output.body.data`
3. Asigna el valor a `llm` bajo key `prompt`

### Data Flow Example

```json
{
  "nodes": {
    "webhook": {
      "type": "trigger_webhook",
      "config": {
        "test_payload": {"endpoint": "/joke"}
      }
    },
    "fetch": {
      "type": "http_request",
      "config": {
        "base_url": "https://api.example.com"
      }
    }
  },
  "edges": [
    {
      "from": "webhook.output.endpoint",
      "to": "fetch.endpoint"
    }
  ]
}
```

Flujo:
1. `webhook` ejecuta → output: `{"endpoint": "/joke"}`
2. Edge resuelve `"/joke"`
3. `fetch` recibe inputs: `{"endpoint": "/joke"}`
4. `fetch` combina con config → URL final: `https://api.example.com/joke`

## Modos de Ejecución

### Run Mode (Testing Local)

```bash
cargo run --bin dag_engine -- run graph.json
```

**Características**:
- Usa `test_payload` del grafo
- No levanta servidor
- Output a stdout
- Rápido para desarrollo

**Flujo**:
```
main.rs
  ↓
Load graph.json
  ↓
DagRunUseCase.execute()
  ↓
Print output
```

### Serve Mode (Producción)

```bash
cargo run --bin dag_engine -- serve graph.json --port 3000
```

**Características**:
- Levanta servidor HTTP con Axum
- Registra rutas de `trigger_webhook` nodes
- Inyecta payload HTTP en `__payload__`
- Ejecuta grafo por petición

**Flujo**:
```
main.rs
  ↓
Load graph.json
  ↓
For each trigger_webhook:
  Register HTTP route
  ↓
Start Axum server
  ↓
On HTTP request:
  Clone graph
  Inject payload to __payload__
  DagRunUseCase.execute()
  Return JSON response
```

## Seguridad

### API Keys

**Problema**: Los grafos contienen API keys sensibles

**Soluciones**:
1. **Variables de entorno** (recomendado):
   ```rust
   "api_key": "${OPENAI_API_KEY}"
   ```
   
2. **Secrets management** (futuro):
   - Integración con HashiCorp Vault
   - AWS Secrets Manager
   - Azure Key Vault

3. **Runtime injection**:
   ```rust
   curl -X POST /endpoint \
     -H "X-API-Key: sk-..." \
     -d '{"message": "..."}'
   ```

### Rate Limiting

**Implementación futura**:
```rust
pub struct RateLimitedNode {
    inner: Arc<dyn ExecutableNode>,
    limiter: RateLimiter,
}
```

## Performance

### Ejecución Secuencial

Actualmente, los nodos se ejecutan secuencialmente en orden topológico.

**Ventajas**:
- Simple de implementar y debugear
- Predecible
- Suficiente para muchos casos de uso

**Limitaciones**:
- No aprovecha concurrencia potencial
- Puede ser lento con muchos nodos independientes

### Optimización Futura: Ejecución Paralela

```rust
// Identificar nodos independientes en cada "nivel"
let levels = compute_execution_levels(graph);

for level in levels {
    // Ejecutar nodos del mismo nivel en paralelo
    let futures: Vec<_> = level.iter()
        .map(|node_id| execute_node(node_id))
        .collect();
    
    join_all(futures).await?;
}
```

## Extensibilidad

### Añadir un Nuevo Nodo

**Pasos**:

1. **Crear implementación**:
   ```rust
   // src/libs/colmena/src/dag_engine/infrastructure/nodes/my_node.rs
   pub struct MyNode;

   #[async_trait::async_trait]
   impl ExecutableNode for MyNode {
       async fn execute(
           &self,
           inputs: &NodeInputs,
           config: &Value,
           state: &mut Value,
           observer: Option<Arc<dyn ExecutionObserver>>,
       ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
           // implementación
           Ok(serde_json::json!({ "output": "..." }))
       }

       fn schema(&self) -> Value {
           json!({
               "type": "my_node",
               "config": {...},
               "inputs": {...},
               "outputs": {...}
           })
       }

       fn default_input(&self) -> Option<&str> { Some("input") }
       fn default_output(&self) -> Option<&str> { Some("output") }
   }
   ```

2. **Exportar módulo**:
   ```rust
   // src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs
   pub mod my_node;
   ```

3. **Registrar nodo**:
   ```rust
   // src/libs/colmena/src/dag_engine/infrastructure/registry.rs
   use crate::dag_engine::infrastructure::nodes::my_node::MyNode;

   // dentro de HashMapNodeRegistry::new_with_secure_values(...)
   nodes.insert("my_node".to_string(), Arc::new(MyNode));
   ```

4. **Usar en grafo**:
   ```json
   {
     "nodes": {
       "my_step": {
         "type": "my_node",
         "config": {...}
       }
     }
   }
   ```

### Best Practices para Nodos

1. **Configuración dinámica**: Siempre implementar precedencia `inputs > config`
2. **Error handling**: Usar `Result` y errores descriptivos
3. **Schema**: Documentar config, inputs, outputs
4. **Testing**: Unit tests para cada nodo
5. **Idempotencia**: Si es posible, hacer nodos idempotentes

## Testing

### Unit Tests

Cada nodo debe tener tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_http_node_get() {
        let node = HttpNode::new();
        let inputs = HashMap::new();
        let config = json!({
            "base_url": "https://api.example.com",
            "endpoint": "/data",
            "method": "GET"
        });
        let mut state = json!({});

        // Firma actual: execute(inputs, config, state, observer)
        let result = node.execute(&inputs, &config, &mut state, None).await;
        assert!(result.is_ok());
    }
}
```

### Integration Tests

Tests de grafos completos:

```rust
#[tokio::test]
async fn test_http_to_llm_pipeline() {
    let graph_json = include_str!("../tests/http_llm.json");
    let graph: Graph = serde_json::from_str(graph_json).unwrap();
    
    let registry = Arc::new(HashMapNodeRegistry::new());
    let use_case = DagRunUseCase::new(registry);
    
    let result = use_case.execute(graph).await;
    assert!(result.is_ok());
}
```

## Roadmap

> Estado actualizado: varios ítems originales del roadmap ya están en producción. Mantenemos la lista histórica con anotaciones.

### Completado
- [x] HttpNode, LlmNode, TriggerWebhookNode + ~30 nodos adicionales (ver inventario arriba)
- [x] `test_payload` para testing local
- [x] Precedencia `inputs > config` (configuración dinámica)
- [x] Logging estructurado + observer (`ExecutionObserver`)
- [x] Conditional execution (vía `NodeConfig.trigger_on`)
- [x] Loops (`loop_controller` + `Edge.cyclic` + `max_total_calls` / `max_calls_from`)
- [x] DAG composition (`subgraph` node con node IDs path-qualified `outer/inner`)
- [x] Multimedia (`image_generation`, `image_edit`, `tts`) — PR multimedia, May 2026
- [x] HTML artifacts (`ArtifactKind::Html`, `AssetStore` port) — PR #79
- [x] Recursive skill references (depth 5, cycle detection) — PR #80

### Pendiente / Exploratorio
- [ ] Ejecución paralela de nodos independientes (hoy es secuencial topo-sort)
- [ ] Retry logic con backoff (caso por caso, no genérico)
- [ ] Circuit breaker pattern
- [ ] Dynamic graph modification en runtime
- [ ] Distributed execution
- [ ] Persistent state management cross-run
- [ ] Observability completa (traces OpenTelemetry, métricas Prometheus)

## Referencias

- [Developer Guide](../developer_guide/12_dag_engine_guide.md)
- [Usage Examples](../examples/USAGE_EXAMPLES.md)
- [LLM Module Design](MODULO_LLM_DISEÑO.md)
- [Hexagonal Architecture Guide](ARQUITECTURA_HEXAGONAL_GUIA.md)
