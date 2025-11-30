# 📋 Tareas Pendientes: Integración de Agentes

Este documento rastrea el trabajo restante para completar la transformación del `dag_engine` en un sistema de Agentes Autónomos.

## 🚧 Fase 2: Definición de Tools (Estructura)

El objetivo de esta fase es permitir que el LLM entienda y solicite la ejecución de herramientas.

- [ ] **Actualizar Modelos de Dominio (`src/llm/domain`)**
    - [ ] Modificar `LlmRequest` para incluir campo `tools: Option<Vec<ToolDefinition>>`.
    - [ ] Modificar `LlmResponse` para incluir campo `tool_calls: Option<Vec<ToolCall>>`.
    - [ ] Definir structs `ToolDefinition` y `ToolCall`.

- [ ] **Actualizar Adaptadores (`src/llm/infrastructure`)**
    - [ ] Actualizar `OpenAiAdapter` para serializar `tools` en el request JSON.
    - [ ] Actualizar `OpenAiAdapter` para parsear `tool_calls` del response JSON.
    - [ ] (Opcional) Stub para otros proveedores (Gemini/Anthropic).

## 🤖 Fase 3: Ejecución Recursiva (Agente)

El objetivo es implementar el bucle de ejecución "ReAct" dentro del módulo `llm`, manteniendo el `dag_engine` limpio.

- [ ] **Abstracción de Ejecución de Tools**
    - [ ] Definir trait `ToolExecutor` en `llm/domain`.
    - [ ] Implementar `DagToolExecutor` en `dag_engine` (adaptador que llama a `ExecutableNode`).

- [ ] **Servicio de Agente (`llm/application`)**
    - [ ] Crear `AgentService` (o `AgentUseCase`).
    - [ ] Implementar el bucle ReAct dentro de `AgentService::run`.
    - [ ] Manejar persistencia de historial dentro del servicio.

- [ ] **Integración en `LlmNode`**
    - [ ] Actualizar `LlmNode` para instanciar `AgentService`.
    - [ ] Delegar la ejecución al servicio, pasando el `DagToolExecutor`.

- [ ] **Testing y Validación**
    - [ ] Crear DAG de prueba "Agente Matemático".
    - [ ] Verificar que el Agente resuelve problemas usando nodos del DAG.
