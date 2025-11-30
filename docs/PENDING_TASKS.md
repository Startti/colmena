# 📋 Tareas Pendientes: Integración de Agentes

Este documento rastrea el trabajo restante para completar la transformación del `dag_engine` en un sistema de Agentes Autónomos.

**📄 Plan Detallado**: Ver [TOOL_CALLING_IMPLEMENTATION_PLAN.md](./TOOL_CALLING_IMPLEMENTATION_PLAN.md) para detalles completos de implementación.

**Timeline Estimado**: 24 días divididos en 7 fases

---

## ✅ Fase 1: Memoria (Persistencia) - COMPLETADO

- [x] Definir trait `ConversationRepository` en `llm/domain`
- [x] Implementar `PostgresConversationRepository` en `llm/infrastructure`
- [x] Implementar `SqliteConversationRepository` para soporte local
- [x] Crear tabla SQL (migración) para Postgres y SQLite
- [x] Modificar `LlmNode` para leer/escribir historial si `thread_id` está presente
- [x] Implementar `MockAdapter` para testing sin consumo de API

---

## 🔬 Fase 2: Planificación e Investigación (2 días)

### 2.1 Investigación de APIs de Proveedores

- [ ] **OpenAI Function Calling API**
    - [ ] Documentar formato del parámetro `tools` (JSON Schema)
    - [ ] Documentar formato de respuesta `tool_calls`
    - [ ] Estudiar opciones de `tool_choice` parameter

- [ ] **Anthropic Tool Use API**
    - [ ] Documentar formato de tool/function calling de Claude
    - [ ] Identificar diferencias con formato OpenAI
    - [ ] Documentar estructura de respuesta

- [ ] **Gemini Function Calling API**
    - [ ] Documentar formato de function declaration de Google
    - [ ] Estudiar estructura de respuesta para tool calls
    - [ ] Identificar diferencias con otros proveedores

- [ ] **Crear Matriz de Compatibilidad**
    - [ ] Documento comparativo de formatos
    - [ ] Estrategia de conversión entre formatos
    - [ ] Crear `docs/research/PROVIDER_TOOL_FORMATS.md`

### 2.2 Diseño de Modelo de Dominio

- [ ] Diseñar struct `ToolDefinition` (basado en JSON Schema)
- [ ] Diseñar struct `ToolCall` (nombre + argumentos)
- [ ] Diseñar struct `ToolResult` (éxito/error + output)
- [ ] Diseñar trait `ToolExecutor` (abstracción)
- [ ] Revisar compatibilidad con trait `ExecutableNode` existente
- [ ] Crear diagramas UML actualizados

---

## 🏗️ Fase 3: Capa de Dominio - Abstracciones de Tools (3 días)

### 3.1 Crear Modelos de Dominio de Tools

**Archivo**: `src/llm/domain/tools.rs`

- [ ] Implementar struct `ToolDefinition`
    - [ ] Campos: name, description, parameters
    - [ ] Métodos builder para construcción ergonómica
    - [ ] Validación de JSON Schema

- [ ] Implementar struct `ToolParameters`
    - [ ] JSON Schema completo (type, properties, required)
    - [ ] Serialización/deserialización correcta

- [ ] Implementar struct `ParameterProperty`
    - [ ] Tipo, descripción, enum values opcionales

- [ ] Implementar struct `ToolCall`
    - [ ] ID, tipo, función
    - [ ] Parsing de argumentos JSON

- [ ] Implementar struct `FunctionCall`
    - [ ] Nombre y argumentos

- [ ] Implementar struct `ToolResult`
    - [ ] tool_call_id, success, output, error opcional

- [ ] Tests unitarios completos para todos los structs
- [ ] Agregar a exports de `src/llm/domain/mod.rs`

### 3.2 Actualizar LlmRequest y LlmResponse

**Archivos**:
- `src/llm/domain/llm_request.rs`
- `src/llm/domain/llm_response.rs`

- [ ] **LlmRequest**: Agregar campo `tools: Option<Vec<ToolDefinition>>`
- [ ] **LlmRequest**: Agregar campo `tool_choice: Option<String>`
- [ ] **LlmRequest**: Implementar método `with_tools()`
- [ ] **LlmRequest**: Implementar método `with_tool_choice()`
- [ ] **LlmRequest**: Agregar getters para tools

- [ ] **LlmResponse**: Agregar campo `tool_calls: Option<Vec<ToolCall>>`
- [ ] **LlmResponse**: Implementar método `with_tool_calls()`
- [ ] **LlmResponse**: Implementar método `has_tool_calls()`
- [ ] **LlmResponse**: Agregar getter para tool_calls

- [ ] Actualizar tests existentes
- [ ] Agregar tests para funcionalidad de tools
- [ ] Tests de serialización/deserialización

### 3.3 Crear Trait ToolExecutor

**Archivo**: `src/llm/domain/tool_executor.rs`

- [ ] Definir trait `ToolExecutor`
    - [ ] Método `async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, LlmError>`
    - [ ] Método `async fn available_tools(&self) -> Vec<ToolDefinition>`
    - [ ] Documentación con ejemplos de uso

- [ ] Agregar a exports de `mod.rs`
- [ ] Documentar casos de uso

### 3.4 Actualizar LlmMessage para Tool Messages

**Archivo**: `src/llm/domain/llm_message.rs`

- [ ] Agregar variante `Tool` al enum `MessageRole`
- [ ] Agregar campo `tool_call_id: Option<String>` a `LlmMessage`
- [ ] Implementar método `LlmMessage::tool()`
- [ ] Actualizar serialización para mensajes de tool
- [ ] Tests para creación de mensajes de tool

### 3.5 Agregar Nuevos Tipos de Error

**Archivo**: `src/llm/domain/llm_error.rs`

- [ ] Agregar variante `ToolExecutionFailed`
- [ ] Agregar variante `MaxIterationsReached`
- [ ] Agregar variante `InvalidToolCall`
- [ ] Agregar variante `ToolNotFound`
- [ ] Tests de error handling

---

## 🔌 Fase 4: Capa de Infraestructura - Adaptadores de Proveedores (5 días)

### 4.1 Actualizar OpenAI Adapter

**Archivo**: `src/llm/infrastructure/openai_adapter.rs`

- [ ] Actualizar `build_request_body()` para incluir tools
    - [ ] Serializar `tools` en formato OpenAI
    - [ ] Agregar `tool_choice` si está presente
    - [ ] Formato: `{"type": "function", "function": {...}}`

- [ ] Actualizar parsing de respuesta para `tool_calls`
    - [ ] Extraer tool_calls del JSON de respuesta
    - [ ] Deserializar a structs `ToolCall`
    - [ ] Manejar respuestas sin tool_calls

- [ ] Soportar mensajes con rol "tool"
- [ ] Tests con ejemplos de OpenAI
- [ ] Testing con API real de OpenAI (gpt-4, gpt-3.5-turbo)

### 4.2 Actualizar Anthropic Adapter

**Archivo**: `src/llm/infrastructure/anthropic_adapter.rs`

- [ ] Estudiar formato de tools de Anthropic (diferente a OpenAI)
- [ ] Implementar `convert_tools_to_anthropic()`
    - [ ] Formato: `{"name": ..., "description": ..., "input_schema": ...}`
    - [ ] Conversión desde `ToolDefinition`

- [ ] Actualizar `build_request_body()` para incluir tools
- [ ] Parsear bloques de `tool_use` de Anthropic
- [ ] Manejar content blocks de tool_use
- [ ] Tests con ejemplos de Claude
- [ ] Testing con API real de Claude (claude-3-opus, claude-3-sonnet)

### 4.3 Actualizar Gemini Adapter

**Archivo**: `src/llm/infrastructure/gemini_adapter.rs`

- [ ] Estudiar formato de function calling de Gemini
- [ ] Implementar `convert_tools_to_gemini()`
    - [ ] Formato: `{"function_declarations": [...]}`
    - [ ] Conversión desde `ToolDefinition`

- [ ] Actualizar construcción de request
- [ ] Parsear respuestas de function call
- [ ] Manejar estructura específica de Gemini
- [ ] Tests con ejemplos de Gemini
- [ ] Testing con API real (gemini-pro, gemini-1.5-pro)

### 4.4 Actualizar Mock Adapter

**Archivo**: `src/llm/infrastructure/mock_adapter.rs`

- [ ] Agregar simulación de tool calls
- [ ] Retornar tool calls predefinidos para testing
- [ ] Soportar comportamientos configurables
- [ ] Tests para escenarios de tool calling

---

## 🎯 Fase 5: Capa de Aplicación - Servicio de Agente (4 días)

### 5.1 Crear Agent Service

**Archivo**: `src/llm/application/agent_service.rs`

- [ ] Crear struct `AgentService`
    - [ ] Campos: llm_repository, conversation_repository

- [ ] Implementar método `run()`
    - [ ] Cargar historial de conversación
    - [ ] Agregar prompt del usuario
    - [ ] Obtener herramientas disponibles

- [ ] Implementar bucle ReAct
    - [ ] Llamar LLM con tools
    - [ ] Verificar si hay tool_calls
    - [ ] Ejecutar cada tool call via ToolExecutor
    - [ ] Agregar resultados al historial
    - [ ] Loop hasta respuesta final

- [ ] Implementar límite de iteraciones máximas
    - [ ] Parámetro configurable (default: 10)
    - [ ] Error si se alcanza el límite

- [ ] Manejo robusto de errores
- [ ] Logging para debugging
- [ ] Tests unitarios con mocks
- [ ] Tests de integración

---

## 🌉 Fase 6: Integración con DAG Engine (4 días)

### 6.1 Crear DagToolExecutor

**Archivo**: `src/dag_engine/infrastructure/tool_executor.rs`

- [ ] Implementar struct `DagToolExecutor`
    - [ ] Campo: registry (Arc<dyn NodeRegistryPort>)

- [ ] Implementar método `node_schema_to_tool()`
    - [ ] Convertir schema de nodo a `ToolDefinition`
    - [ ] Extraer descripción del schema
    - [ ] Convertir inputs a ToolParameters
    - [ ] Generar lista de campos required

- [ ] Implementar método `extract_properties()`
    - [ ] Parsear schema de inputs
    - [ ] Convertir a HashMap de ParameterProperty

- [ ] Implementar trait `ToolExecutor`
    - [ ] `execute()`: Ejecutar nodo desde tool_call
    - [ ] Obtener nodo del registry
    - [ ] Parsear argumentos JSON
    - [ ] Ejecutar nodo
    - [ ] Retornar ToolResult

- [ ] Implementar `get_tools(tool_names: &[String])`
    - [ ] Recibe lista de nombres de tools desde config
    - [ ] Filtra solo los tools solicitados
    - [ ] Convierte schemas a ToolDefinitions

- [ ] Implementar `get_all_available_tools()`
    - [ ] Retorna todos los tools disponibles
    - [ ] Usado cuando `enabled_tools` contiene "*"
    - [ ] Lista hardcodeada inicialmente, dinámica después

- [ ] Tests unitarios
- [ ] Tests de integración con nodos reales

### 6.2 Actualizar LlmNode para Usar AgentService

**Archivo**: `src/dag_engine/infrastructure/nodes/llm.rs`

- [ ] Agregar parsing de `enabled_tools` config (array de strings)
    - [ ] Soportar lista específica: `["add", "multiply", "http_request"]`
    - [ ] Soportar wildcard `["*"]` para todos los tools disponibles

- [ ] Agregar opción de configuración `max_iterations` (default: 10)

- [ ] Implementar lógica de filtrado de tools
    - [ ] Si `enabled_tools` es `["*"]`: usar `tool_executor.get_all_available_tools()`
    - [ ] Si es lista específica: usar `tool_executor.get_tools(&tool_names)`
    - [ ] Si no hay `enabled_tools`: comportamiento normal (sin tools)

- [ ] Instanciar `AgentService` cuando tools están habilitados
- [ ] Crear instancia de `DagToolExecutor` con registry
- [ ] Pasar lista filtrada de tools al agent service
- [ ] Mantener retrocompatibilidad (sin tools = llamada LLM normal)

- [ ] Agregar validación de nombres de tools
    - [ ] Verificar que tools existen en registry
    - [ ] Error claro si tool no existe

- [ ] Actualizar schema para documentar:
    - [ ] `enabled_tools`: array de strings o ["*"]
    - [ ] `max_iterations`: número opcional
    - [ ] Ejemplos de configuración

- [ ] Tests con varias configuraciones
    - [ ] Test con lista específica de tools
    - [ ] Test con wildcard "*"
    - [ ] Test sin tools (backward compatibility)
    - [ ] Test con tool inexistente (error handling)

- [ ] Crear archivos JSON de ejemplo de DAGs
    - [ ] `math_agent.json` - tools específicos
    - [ ] `research_agent.json` - HTTP request tool
    - [ ] `general_agent.json` - wildcard "*"

### 6.3 Actualizar Schemas de Nodos

- [ ] Revisar todos los node schemas para descripciones claras
- [ ] Asegurar que todos los parámetros de input tienen descripciones
- [ ] Agregar flag `toolEnabled: true` a schemas que deben ser tools
- [ ] Documentar requirements de schema en developer guide

---

## ✅ Fase 7: Testing & Validación (4 días)

### 7.1 Tests Unitarios

- [ ] Tests de `ToolDefinition` (creación, validación)
- [ ] Tests de `ToolCall` (parsing)
- [ ] Tests de `ToolResult` (serialización)
- [ ] Tests de `AgentService` (bucle ReAct con mocks)
- [ ] Tests de `DagToolExecutor` (ejecución de nodos)
- [ ] Tests de serialización de tools en adaptadores
- [ ] Cobertura de código >80%

### 7.2 Tests de Integración

- [ ] Crear DAG de prueba "Agente Matemático"
    - [ ] Pregunta: "¿Cuál es (5 + 3) * 2?"
    - [ ] Debe usar nodos `add` y luego `multiply`
    - [ ] Verificar respuesta correcta

- [ ] Crear DAG de prueba "Agente de Investigación Web"
    - [ ] Pregunta: "¿Cuál es el clima en Londres?"
    - [ ] Debe usar nodo `http_request`
    - [ ] Verificar que obtiene datos

- [ ] Tests con APIs reales de proveedores
- [ ] Tests de persistencia de memoria con tool usage
- [ ] Tests de manejo de errores
    - [ ] Tool calls inválidos
    - [ ] Fallos de ejecución
    - [ ] Argumentos malformados

- [ ] Tests de límite de iteraciones máximas

### 7.3 DAGs de Ejemplo

**Crear en** `examples/dags/agents/`:

- [ ] `math_agent.json` - Agente matemático
    - [ ] Configuración completa
    - [ ] Test payload de ejemplo
    - [ ] Documentación de comportamiento esperado

- [ ] `research_agent.json` - Agente de investigación
    - [ ] Configuración con HTTP requests
    - [ ] Test payload de ejemplo
    - [ ] Documentación

- [ ] Probar cada ejemplo end-to-end
- [ ] Documentar resultados esperados
- [ ] Agregar a documentación de ejemplos de uso

---

## 📚 Fase 8: Documentación (2 días)

### 8.1 Documentación Técnica

- [ ] Actualizar `docs/dds/MODULO_LLM_DISEÑO.md` con tool calling
- [ ] Actualizar `docs/dds/DISEÑO_AGENTES_Y_TOOLS.md`
- [ ] Actualizar `docs/developer_guide/12_dag_engine_guide.md`
- [ ] Crear `docs/guides/TOOL_CALLING_GUIDE.md`
- [ ] Actualizar referencia de API

### 8.2 Documentación de Usuario

- [ ] Actualizar `docs/USAGE_EXAMPLES.md` con ejemplos de agentes
- [ ] Actualizar `docs/PYTHON_USAGE_EXAMPLES.md`
- [ ] Crear guía de troubleshooting para tool calling
- [ ] Agregar sección de FAQ

### 8.3 Finalizar

- [ ] Marcar Fase 2 como completa en este documento
- [ ] Marcar Fase 3 como completa en este documento
- [ ] Documentar mejoras futuras potenciales
- [ ] Crear changelog entry

---

## 📊 Criterios de Éxito

- [ ] ✅ Los 3 proveedores (OpenAI, Anthropic, Gemini) soportan tool calling
- [ ] ✅ AgentService ejecuta el bucle ReAct exitosamente
- [ ] ✅ Los nodos del DAG se descubren automáticamente como tools
- [ ] ✅ Ejemplo de agente matemático funciona end-to-end
- [ ] ✅ Los errores de ejecución de tools se manejan correctamente
- [ ] ✅ La memoria de conversación persiste tool calls y resultados
- [ ] ✅ Cobertura de código >80%
- [ ] ✅ Toda la documentación actualizada
- [ ] ✅ Sin breaking changes a funcionalidad LLM existente

---

## 🎯 Próximos Pasos

1. ✅ Revisar plan detallado en `TOOL_CALLING_IMPLEMENTATION_PLAN.md`
2. ✅ Configurar tracking en GitHub issues/project board
3. ✅ Crear feature branch: `feat/tool-calling`
4. ⏭️ Comenzar Fase 2.1: Investigación de APIs de proveedores
5. ⏭️ Documentar hallazgos en `docs/research/PROVIDER_TOOL_FORMATS.md`
