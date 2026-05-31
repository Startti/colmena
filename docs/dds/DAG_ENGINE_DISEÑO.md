# DAG Engine - Documento de Diseño

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
    pub nodes: HashMap<String, NodeConfig>,
    pub edges: Vec<Edge>,
}
```
Representa la estructura del DAG definida en JSON.

#### NodeConfig
```rust
pub struct NodeConfig {
    pub node_type: String,
    pub config: Value,
}
```
Configuración de cada nodo en el grafo.

#### Edge
```rust
pub struct Edge {
    pub from: String,  // "source_node.output.field"
    pub to: String,    // "target_node.input.field"
}
```
Define las conexiones entre nodos y el flujo de datos.

#### ExecutableNode Trait
```rust
#[async_trait]
pub trait ExecutableNode: Send + Sync {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        state: &mut Value,
    ) -> Result<Value, Box<dyn StdError>>;
    
    fn schema(&self) -> Value;
}
```
Interfaz que todos los nodos deben implementar.

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
- Multi-provider (OpenAI, Gemini, Anthropic)
- Configuración dinámica completa
- Retorna usage statistics
- Integración con módulo LLM existente

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
   // src/dag_engine/infrastructure/nodes/my_node.rs
   pub struct MyNode;
   
   #[async_trait::async_trait]
   impl ExecutableNode for MyNode {
       async fn execute(...) -> Result<Value, Box<dyn StdError>> {
           //  implementación
       }
       
       fn schema(&self) -> Value {
           json!({
               "type": "my_node",
               "config": {...},
               "inputs": {...},
               "outputs": {...}
           })
       }
   }
   ```

2. **Exportar módulo**:
   ```rust
   // src/dag_engine/infrastructure/nodes/mod.rs
   pub mod my_node;
   ```

3. **Registrar nodo**:
   ```rust
   // src/dag_engine/infrastructure/registry.rs
   use crate::infrastructure::nodes::my_node::*;
   
   impl HashMapNodeRegistry {
       pub fn new() -> Self {
           let mut nodes = HashMap::new();
           // ...
           nodes.insert("my_node".to_string(), Arc::new(MyNode));
           Self { nodes }
       }
   }
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
        let node = HttpNode;
        let inputs = HashMap::new();
        let config = json!({
            "base_url": "https://api.example.com",
            "endpoint": "/data",
            "method": "GET"
        });
        let mut state = json!({});
        
        let result = node.execute(&inputs, &config, &mut state).await;
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

### Corto Plazo
- [x] HttpNode
- [x] LlmNode
- [x] test_payload para testing local
- [x] Configuración dinámica
- [ ] Error handling mejorado
- [ ] Logging estructurado

### Medio Plazo
- [ ] Ejecución paralela de nodos independientes
- [ ] Conditional nodes (if/else)
- [ ] Loop nodes (for/while)
- [ ] Retry logic con backoff
- [ ] Circuit breaker pattern

### Largo Plazo
- [ ] DAG composition (sub-graphs)
- [ ] Dynamic graph modification
- [ ] Distributed execution
- [ ] Persistent state management
- [ ] Observability (traces, metrics)

## Referencias

- [Developer Guide](../developer_guide/12_dag_engine_guide.md)
- [Usage Examples](../examples/USAGE_EXAMPLES.md)
- [LLM Module Design](MODULO_LLM_DISEÑO.md)
- [Hexagonal Architecture Guide](ARQUITECTURA_HEXAGONAL_GUIA.md)
