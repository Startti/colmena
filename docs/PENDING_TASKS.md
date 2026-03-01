# 📋 Tareas Pendientes: Integración de Agentes

Este documento rastrea el trabajo restante para completar la transformación del `dag_engine` en un sistema de Agentes Autónomos.

**📄 Plan Detallado**: Este documento rastrea el plan de implementación de Tool Calling. (Referencia a `TOOL_CALLING_IMPLEMENTATION_PLAN.md` eliminada ya que este documento es el principal).

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

## ✅ Fase 2: Planificación e Investigación - COMPLETADO

- [x] Investigación de APIs de Proveedores (OpenAI, Anthropic, Gemini)
- [x] Diseño de Modelo de Dominio de Tools
- [x] Matriz de Compatibilidad de Formatos

---

## ✅ Fase 3: Capa de Dominio - Abstracciones de Tools - COMPLETADO

- [x] Implementar structs `ToolDefinition`, `ToolCall`, `ToolResult`
- [x] Crear Trait `ToolExecutor`
- [x] Actualizar `LlmRequest` y `LlmResponse` para soporte nativo de tools
- [x] Agregar soporte para mensajes de tipo `Tool` en el historial

---

## ✅ Fase 4: Capa de Infraestructura - Adaptadores de Proveedores - COMPLETADO

- [x] Implementar soporte de tools en `OpenAiAdapter`
- [x] Implementar soporte de tools en `AnthropicAdapter`
- [x] Implementar soporte de tools en `GeminiAdapter`
- [x] Actualizar `MockAdapter` para escenarios de testing

---

## ✅ Fase 5: Capa de Aplicación - Servicio de Agente - COMPLETADO

- [x] Crear `AgentService` con implementación del bucle ReAct
- [x] Manejo de persistencia de conversaciones con herramientas
- [x] Control de iteraciones máximas y errores de ejecución

---

## ✅ Fase 6: Integración con DAG Engine - COMPLETADO

- [x] Implementar `DagToolExecutor` para ejecutar nodos como herramientas
- [x] Actualizar `LlmNode` para delegar en `AgentService` cuando hay tools habilitados
- [x] Descubrimiento automático de herramientas basado en el registro de nodos

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
