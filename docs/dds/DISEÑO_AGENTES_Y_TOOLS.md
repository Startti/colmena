# Plan de Diseño: Integración de Memoria y Tools (Agentes) en Colmena

Este documento detalla la estrategia de arquitectura para transformar el `LlmNode` actual (ejecución lineal) en un Agente autónomo capaz de mantener estado (Memoria) y ejecutar acciones (Tools), respetando la Arquitectura Hexagonal y el diseño del DAG Engine existente.

---

## 1. Arquitectura General: Desacoplamiento del DAG

El `dag_engine` debe permanecer como un orquestador de ejecución "tonto". La lógica compleja de Agentes (ReAct, manejo de herramientas, bucles) se moverá al módulo `llm` (capa de Aplicación/Dominio).

### Estructura de Integración

*   **`llm::application::AgentService`**: Nuevo servicio que encapsula el bucle de razonamiento (ReAct Loop). Recibe un prompt, historial y herramientas, y devuelve una respuesta final.
*   **`dag_engine::nodes::LlmNode`**: Actúa solo como un adaptador/puente.
    1.  Recopila inputs y configuración.
    2.  Instancia o llama al `AgentService`.
    3.  Devuelve el resultado final al grafo.

---

## Parte 2: Tools (Function Calling & Node Bridging)

### 2.1 Abstracción en Dominio LLM (`src/llm/domain/tools.rs`)

Definimos las estructuras para herramientas de forma agnóstica.

```rust
// ... (ToolDefinition, ToolCall structs igual que antes) ...
```

### 2.2 El "Registry Bridge" (Adaptador DAG -> LLM)

El `dag_engine` sigue siendo responsable de *proveer* las herramientas (porque tiene los nodos), pero no de *ejecutar* la lógica de decisión.

---

## Parte 3: Lógica de Agente (ReAct) en el Módulo LLM

En lugar de poner el `loop` en el nodo, creamos un caso de uso en `src/llm/application/agent.rs`.

```rust
pub struct AgentService {
    llm_repository: Arc<dyn LlmRepository>,
    conversation_repository: Arc<dyn ConversationRepository>,
}

impl AgentService {
    pub async fn run(
        &self, 
        thread_id: &ThreadId, 
        prompt: String, 
        tools: Vec<ToolDefinition>,
        tool_executor: &dyn ToolExecutor // Trait para ejecutar tools (callback)
    ) -> Result<String, LlmError> {
        
        // 1. Cargar historial y agregar prompt usuario
        // ...
        
        // 2. THE LOOP (ReAct Pattern)
        loop {
            // A. Llamada al LLM
            let response = self.llm_repository.call(messages, tools).await?;
            
            // B. Verificar Tool Calls
            if let Some(calls) = response.tool_calls {
                for call in calls {
                    // C. Ejecutar Tool (Delegado al caller/dag_engine via trait)
                    let result = tool_executor.execute(&call.name, &call.arguments).await?;
                    
                    // D. Agregar resultado al historial
                    // ...
                }
            } else {
                // E. Respuesta Final
                return Ok(response.content);
            }
        }
    }
}
```

### Trait `ToolExecutor`
Definido en `llm::domain`, implementado por `dag_engine`. Esto permite que el `AgentService` pida ejecutar una herramienta sin conocer qué es un "Nodo".

```rust
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, tool_name: &str, args: &str) -> Result<String, LlmError>;
}
```

---

## Parte 4: El Nuevo `LlmNode`

El nodo se simplifica drásticamente:

```rust
impl ExecutableNode for LlmNode {
    async fn execute(&self, inputs, config, ...) {
        // 1. Preparar dependencias
        let agent = AgentService::new(self.llm_repo.clone(), self.mem_repo.clone());
        
        // 2. Adaptador de Tools (El nodo mismo o un helper actúa como ToolExecutor)
        let tool_executor = DagToolExecutor::new(self.registry.clone());
        
        // 3. Delegar ejecución
        let response = agent.run(thread_id, prompt, tools, &tool_executor).await?;
        
        Ok(json!({ "output": response }))
    }
}
```

-----

## 3\. Plan de Implementación por Fases

Para mantener el control y testabilidad, se sugiere el siguiente orden:

### Fase 1: Memoria (Persistencia) ✅ COMPLETADO

  * [x] Definir trait `ConversationRepository` en `llm/domain`.
  * [x] Implementar `PostgresConversationRepository` en `llm/infrastructure`.
  * [x] Implementar `SqliteConversationRepository` para soporte local.
  * [x] Crear tabla SQL (migración) para Postgres y SQLite.
  * [x] Modificar `LlmNode` para leer/escribir historial si `thread_id` está presente.
  * [x] Implementar `MockAdapter` para testing sin consumo de API.

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
