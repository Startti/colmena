# Plan de Diseño: Integración de Memoria y Tools (Agentes) en Colmena

Este documento detalla la estrategia de arquitectura para transformar el `LlmNode` actual (ejecución lineal) en un Agente autónomo capaz de mantener estado (Memoria) y ejecutar acciones (Tools), respetando la Arquitectura Hexagonal y el diseño del DAG Engine existente.

---

## 1. Arquitectura General: El "LlmNode" como Mini-Orquestador

El cambio fundamental es que el `LlmNode` dejará de ser un simple paso "input -> output". Ahora encapsulará un bucle de ejecución (**ReAct Loop**) que le permite iterar entre pensar y actuar antes de devolver un resultado final al DAG.

### Estructura de Integración
El módulo `llm` definirá las abstracciones (Puertos), mientras que el `dag_engine` proveerá las implementaciones de herramientas basadas en nodos existentes.

---

## Parte 1: Gestión de Memoria (Conversation History)

El objetivo es persistir el contexto de la conversación. Siguiendo la arquitectura hexagonal, definimos esto mediante Puertos y Adaptadores.

### 1.1 Capa de Dominio (`src/llm/domain/memory.rs`)

Definimos las estructuras agnósticas a la base de datos.

```rust
use crate::llm::domain::{LlmMessage, LlmError};
use async_trait::async_trait;

// Value Object para identificar hilos de conversación
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThreadId(pub String);

// Entidad que agrupa el historial
#[derive(Debug, Clone)]
pub struct Conversation {
    pub thread_id: ThreadId,
    pub messages: Vec<LlmMessage>,
}

// PUERTO: Contrato para el almacenamiento
#[async_trait]
pub trait ConversationRepository: Send + Sync {
    /// Recupera el historial completo
    async fn get_by_id(&self, id: &ThreadId) -> Result<Conversation, LlmError>;
    
    /// Agrega un nuevo mensaje al historial
    async fn add_message(&self, id: &ThreadId, message: LlmMessage) -> Result<(), LlmError>;
    
    /// Limpia el historial (opcional)
    async fn delete(&self, id: &ThreadId) -> Result<(), LlmError>;
}
```

### 1.2 Capa de Infraestructura (Adaptadores)

Aquí implementarás los vendors específicos.

  * **`PostgresConversationRepository`**: Utilizará `sqlx` (ya presente en tu `Cargo.toml`). Se recomienda una tabla simple `chat_messages` con columnas `(thread_id, role, content, created_at)`.
  * **`FirebaseConversationRepository`**: Implementación futura para persistencia NoSQL en la nube.
  * **`InMemoryConversationRepository`**: Para testing local y desarrollo rápido.

### 1.3 Integración en `LlmNode`

El nodo debe actualizarse para manejar el `thread_id` desde los inputs.

**Flujo de Ejecución con Memoria:**

1.  **Input Resolution:** El nodo busca `thread_id` en `inputs` o `config`.
2.  **Load:** Si existe `thread_id`, llama a `repository.get_by_id()`.
3.  **Merge:** Concatena `System Message` + `Historial Recuperado` + `Nuevo Prompt`.
4.  **Execute:** Llama al proveedor de LLM (OpenAI/Anthropic).
5.  **Save:** Guarda asíncronamente el `User Message` (nuevo prompt) y el `Assistant Message` (respuesta) usando `repository.add_message()`.

-----

## Parte 2: Tools (Function Calling & Node Bridging)

Queremos aprovechar que el DAG Engine ya tiene nodos ejecutables (`ExecutableNode`) para usarlos como herramientas del LLM.

### 2.1 Abstracción en Dominio LLM (`src/llm/domain/tools.rs`)

El módulo LLM no debe conocer el DAG, solo debe saber de definiciones de herramientas.

```rust
use serde_json::Value;

// Definición que se envía a la API (OpenAI/Anthropic)
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value, // JSON Schema
}

// Representa la intención del LLM de ejecutar algo
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,        // ID de llamada (necesario para OpenAI)
    pub name: String,      // Nombre de la función
    pub arguments: String, // JSON String con los argumentos
}

// Actualización al Request
pub struct LlmRequest {
    // ... campos existentes ...
    pub tools: Option<Vec<ToolDefinition>>,
}
```

### 2.2 El "Registry Bridge" (Adaptador DAG -\> LLM)

Necesitamos convertir tus nodos existentes (`MathNode`, `HttpNode`, etc.) en `ToolDefinition`.

**Lógica de Conversión:**
Cada `ExecutableNode` ya implementa `fn schema(&self) -> Value`.
Crearemos un adaptador que tome ese schema y lo formatee para OpenAI:

```rust
fn node_to_tool_definition(node_name: &str, node: &Arc<dyn ExecutableNode>) -> ToolDefinition {
    let schema = node.schema();
    // Extraer descripción y parámetros del schema del nodo
    // y mapearlos a la estructura que espera OpenAI.
    ToolDefinition {
        name: node_name.to_string(),
        description: schema["description"].as_str().unwrap_or("").to_string(),
        parameters: schema["inputs"].clone(), // Asumiendo que inputs define los parámetros requeridos
    }
}
```

### 2.3 El Bucle de Ejecución (The Loop)

El método `execute` del `LlmNode` cambiará drásticamente. Ahora debe manejar la recursividad.

**Pseudocódigo Rust para `LlmNode::execute`:**

```rust
async fn execute(&self, inputs: &NodeInputs, config: &Value, state: &mut Value) -> Result<Value, Error> {
    // 1. Setup: Resolver Tools permitidas
    let allowed_tools = resolve_allowed_tools(config); // e.g., ["google_search", "calculator"]
    let registry = self.registry.clone(); // Acceso al registry global
    
    let mut tool_defs = Vec::new();
    let mut executable_nodes = HashMap::new();

    // 2. Preparar definiciones
    for tool_name in allowed_tools {
        if let Some(node) = registry.get_node(&tool_name) {
            tool_defs.push(node_to_tool_definition(&tool_name, &node));
            executable_nodes.insert(tool_name, node);
        }
    }

    let mut messages = build_initial_messages(inputs, config)?;
    
    // 3. THE LOOP (ReAct Pattern)
    loop {
        // A. Llamada al LLM
        let response = llm_use_case.call(messages.clone(), &tool_defs).await?;
        
        // B. Verificar si el LLM quiere usar una herramienta
        if let Some(tool_calls) = response.tool_calls {
            // Agregar mensaje del asistente con la intención de llamada
            messages.push(LlmMessage::assistant_tool_call(&tool_calls));

            for call in tool_calls {
                // C. Ejecutar el Nodo correspondiente
                if let Some(node) = executable_nodes.get(&call.name) {
                    let tool_inputs: NodeInputs = serde_json::from_str(&call.arguments)?;
                    
                    // Ejecución real del nodo (Reusando lógica del DAG)
                    let result = node.execute(&tool_inputs, &json!({}), state).await?;
                    
                    // D. Agregar resultado al historial
                    messages.push(LlmMessage::tool_result(call.id, result));
                }
            }
            // E. Loop continúa: El LLM recibirá el resultado y decidirá si finalizar
        } else {
            // F. Caso Base: Respuesta final de texto
            return Ok(json!({
                "output": {
                    "content": response.content,
                    "usage": response.usage
                }
            }));
        }
    }
}
```

-----

## 3\. Plan de Implementación por Fases

Para mantener el control y testabilidad, se sugiere el siguiente orden:

### Fase 1: Memoria (Persistencia)

  * [ ] Definir trait `ConversationRepository` en `llm/domain`.
  * [ ] Implementar `PostgresConversationRepository` en `llm/infrastructure`.
  * [ ] Crear tabla SQL (migración).
  * [ ] Modificar `LlmNode` para leer/escribir historial si `thread_id` está presente.

### Fase 2: Definición de Tools (Estructura)

  * [ ] Actualizar `LlmRequest` y `LlmResponse` en `llm/domain` para incluir `tools` y `tool_calls`.
  * [ ] Actualizar `OpenAiAdapter` para serializar tools y deserializar llamadas.
  * [ ] (Opcional) Agregar soporte stub para Gemini/Anthropic (lanzar error "Not Implemented" por ahora).

### Fase 3: Ejecución Recursiva (Agente)

  * [ ] Crear la lógica `node_to_tool_definition`.
  * [ ] Refactorizar `LlmNode::execute` para incluir el bucle `loop`.
  * [ ] **Test de Integración:** Crear un grafo JSON donde un nodo LLM tenga acceso a un nodo "Calculator" simple y verificar que obtenga el resultado matemático correcto.

-----

## 4\. Ventajas de este Diseño

1.  **Agnóstico al Proveedor:** La lógica de bucle y memoria está en el `LlmNode` (Aplicación/Infra del DAG), no acoplada a OpenAI. Si Gemini mejora sus tools, solo actualizas el adaptador.
2.  **Reutilización Masiva:** Cualquier nodo que crees para el DAG (ej: `SendEmailNode`, `QueryDatabaseNode`) se convierte automáticamente en una herramienta disponible para tus agentes de IA.
3.  **Escalabilidad Hexagonal:** Puedes cambiar Postgres por Redis o Firebase solo cambiando la implementación del repositorio, sin tocar la lógica del agente.
