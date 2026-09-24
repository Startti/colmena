# Nombre del agente en la frontera de un `child_graph_ref` y `provider_key_id` en el consumo

**Acción de ADP:** (1) leer `node_label` del frame `subgraph-node-start` cuando la
frontera de un hijo por referencia arranca, y usarlo como etiqueta visible en vez
del `node_id` técnico del tool; (2) preferir `provider_key_id` de cada entrada de
`usage-summary`/`subgraph-usage-summary` por encima de cualquier mapeo propio
`node_id → clave` al facturar consumo. Ambos son **aditivos** — nada se rompe si
ADP no hace nada todavía.

## Qué cambia

Task 5/5 (última) de la cadena `child_graph_ref`. Cierra dos cabos sueltos que las
Tasks 1-4 dejaron abiertos a propósito:

1. **`ResolvedChildGraph::display_name`** (PR 2/5, #317) llegaba hasta
   `SubGraphNode::execute` pero se descartaba (`_display_name`, sin uso). Ahora
   viaja en el `config` del frame `NodeStart` de la frontera como
   `{ "node_label": "<display_name>" }`, y `SseMapper` lo levanta a un campo de
   primer nivel del `subgraph-node-start` envuelto. Ninguna variante de
   `NodeStart` cambió — es un campo aditivo en la salida del mapper, no en el wire
   interno del motor.

2. **`provider_key_id`** — nuevo campo opcional en `llm_call.config`, string
   opaco y no secreto que el embebedor escribe junto a `api_key`
   (`docs/node_configurations.json` → `llm_call.config_fields.provider_key_id`).
   El motor no lo interpreta ni lo valida: lo repite tal cual en la fila de esa
   entrada de consumo (`usage-summary`/`subgraph-usage-summary`), para que ADP
   pueda atribuir tokens a la clave que los pagó sin mantener su propio mapeo
   `node_id → provider_key_id` por fuera del grafo.

### Antes / después

```json
// subgraph-node-start de una frontera por referencia — antes
{ "type": "subgraph-node-start", "node_id": "Run_My_Agent", "node_type": "subgraph", "config": {}, "inputs": {} }

// después
{ "type": "subgraph-node-start", "node_id": "Run_My_Agent", "node_type": "subgraph", "config": { "node_label": "Agente de licitaciones" }, "inputs": {}, "node_label": "Agente de licitaciones" }
```

```json
// usage-summary.nodes[i] de un llm_call SIN provider_key_id — sin cambio
{ "node_id": "llm_1", "node_type": "llm_call", "model": "gemini-2.5-flash", "provider": "google", "prompt_tokens": 856, "completion_tokens": 2, "total_tokens": 900 }

// usage-summary.nodes[i] de un llm_call CON config.provider_key_id — campo nuevo
{ "node_id": "llm_1", "node_type": "llm_call", "model": "gemini-2.5-flash", "provider": "google", "provider_key_id": "test-key-123", "prompt_tokens": 856, "completion_tokens": 2, "total_tokens": 900 }
```

`node_label` solo aparece en la frontera de un `child_graph_ref` — una frontera
nombrada por agente (`orchestrator`), por arista, o un `child_graph_inline`/
`child_graph_path` no lo llevan (nunca resolvieron un `display_name`).
`provider_key_id` solo aparece cuando el `llm_call` de esa fila lo configuró; un
nodo sin él no trae la clave, nunca `null`.

## Brecha conocida — `node_label` no verificable end-to-end todavía

La CLI (`dag_engine run`) no tiene un `ChildGraphResolverPort` cableado, así que
un `subgraph` con `child_graph_ref` nunca llega a arrancar ahí — no hay forma de
capturar un `subgraph-node-start` con `node_label` fuera de un test unitario en
este repo. Cubierto solo por
`nodes::subgraph::child_graph_ref_tests::the_boundary_start_of_a_ref_child_carries_the_agent_name`
y por `sse_mapper::tests::a_wrapped_start_lifts_config_node_label_to_the_frame`.
La verificación end-to-end de `node_label` llega junto con el worker de ADP, que
sí provee el resolvedor real. `provider_key_id`, en cambio, no depende de ningún
resolvedor externo y sí tiene E2E en este repo
(`tests/graphs/agents/provider_key_id_usage_e2e.json`).
