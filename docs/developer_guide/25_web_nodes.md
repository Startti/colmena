# Nodos Web (tavily_client, api_explorer, browser)

Tres nodos toolkit que otorgan capacidades de internet a los agentes LLM:

- **tavily_client** — búsqueda web + fetch de URLs (API de Tavily). Ver [Spec A](../superpowers/specs/2026-04-23-web-nodes-a-tavily-client-design.md).
- **api_explorer** — descubrimiento de OpenAPI/Swagger y constructor de `http_request`. Ver [Spec C](../superpowers/specs/2026-04-23-web-nodes-c-api-explorer-design.md).
- **browser** — navegador headless auto-hospedado (Browserless + chromiumoxide). Ver [Spec B](../superpowers/specs/2026-04-23-web-nodes-b-browser-design.md).

## Runtime compartido: nodos toolkit

Un *nodo toolkit* es un `ExecutableNode` que también implementa `ToolkitNode` y expone
múltiples *sub-tools* al LLM desde una sola instancia de nodo.

Declaración en un `llm_call`:

```json
"tool_configurations": {
  "web": {
    "node_type": "tavily_client",
    "node_config": { "api_key": "${TAVILY_API_KEY}" },
    "expose_sub_tools": "all"
  }
}
```

- `node_type` debe apuntar a un nodo toolkit registrado.
- `node_config` es la configuración estática por instancia que el nodo recibe en el momento de la ejecución.
- `expose_sub_tools` es la cadena `"all"` o un arreglo con los nombres de sub-tools a exponer.

Flujo en runtime:

1. El engine invoca `ToolkitNode::sub_tool_catalog(&node_config)` para obtener la lista de sub-tools.
2. Cada sub-tool que pase el filtro de `expose_sub_tools` se convierte en un `ToolDefinition` con nombre `"{alias}__{sub_tool}"` (p. ej. `web__search`).
3. Cuando el LLM invoca uno, `DagToolExecutor` inyecta `__sub_tool` en los inputs del nodo, pasa `node_config` como config de ejecución y llama a `execute()` una sola vez.

## Sub-secciones

- [tavily_client](#tavily_client) — poblado por Spec A.
- [api_explorer](#api_explorer) — poblado por Spec C.
- [browser](#browser) — poblado por Spec B.
