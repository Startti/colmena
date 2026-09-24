# subgraph_resume_fresh_graph — un hijo reanudado corre el grafo que su fuente nombra ahora

E2E determinista (sin LLM, sin `python3`): un `subgraph` inline suspende en
`pregunta` y se reanuda en un turno siguiente. Prueba que la reanudación deriva
el grafo del padre en ESE turno, no la copia guardada, y que un cambio de
estructura rechaza el resume con `SUBGRAPH_RESUME_INCOMPATIBLE:` sin correr nada
(entrada 78 del CHANGELOG).

Solo `turn1_suspend.json` está en el repo. Los dos turnos 2 se derivan con `jq`
(no se commitean, para no duplicar casi todo el grafo dos veces):

```bash
D=tests/graphs/advanced/subgraph_resume_fresh_graph
T2C=/tmp/turn2_config_nueva.json   # sello.config.data.sello: v1 -> v2, mismo esqueleto
T2E=/tmp/turn2_estructura_nueva.json  # nodo "pregunta" -> "confirmar", config.id sigue pregunta_sello

jq '.nodes.delegado.config.child_graph_inline.nodes.sello.config.data.sello = "SELLO=v2"' \
  $D/turn1_suspend.json > $T2C

jq '
  .nodes.delegado.config.child_graph_inline as $c
  | .nodes.delegado.config.child_graph_inline.nodes.confirmar = $c.nodes.pregunta
  | del(.nodes.delegado.config.child_graph_inline.nodes.pregunta)
  | .nodes.delegado.config.child_graph_inline.edges |= map(
      (if .from == "pregunta" then .from = "confirmar" else . end)
      | (if .to == "pregunta" then .to = "confirmar" else . end)
    )
' $D/turn1_suspend.json > $T2E
```

## Correr

Requiere `DATABASE_URL` (ver `docs/developer_guide/30_database_schema.md`). Cada
pareja de turnos usa su propio `--agent-session-id` (la fila hija se busca por
`agent_session_id` con `parent_session_id IS NOT NULL`; el CLI no imprime el id
raíz).

```bash
ANS=$'Q[pregunta_sello]: ¿Seguimos con el sello?\nA[pregunta_sello]: sí'

# 1. Solo cambió la config -> el hijo corre v2
A=rf_cfg_$(date +%s)
cargo run --bin dag_engine -- run $D/turn1_suspend.json --agent-session-id $A
cargo run --bin dag_engine -- run $T2C --agent-session-id $A --answer "$ANS"

# 2. Estructura nueva -> rechazo; nada corre; fila del hijo FAILED
S=rf_shape_$(date +%s)
cargo run --bin dag_engine -- run $D/turn1_suspend.json --agent-session-id $S
cargo run --bin dag_engine -- run $T2E --agent-session-id $S --answer "$ANS"

# 3. Válvula (vuelve al comportamiento hasta v0.16) -> el hijo corre v1
V=rf_valve_$(date +%s)
cargo run --bin dag_engine -- run $D/turn1_suspend.json --agent-session-id $V
COLMENA_SUBGRAPH_RESUME_GRAPH=stored cargo run --bin dag_engine -- run $T2C \
  --agent-session-id $V --answer "$ANS"
```

## Qué mirar

Afirmar contra el frame concreto con `jq`, nunca con `grep` sobre la captura
entera: el `node-start` del `delegado` raíz trae el `child_graph_inline` COMPLETO
en su `config`, así que "SELLO=v2" aparece ahí corra lo que corra el hijo. Mirar
la salida real de `sello`:

```bash
sed -n 's/^data: //p' captura.sse | grep -v '^\[DONE\]$' \
  | jq -c 'select(.type=="subgraph-node-end" and .node_id=="sello") | .output'
```

Corrida 1: `{"sello":"SELLO=v2"}`. Corrida 3 (válvula): `{"sello":"SELLO=v1"}`.
Corrida 2: sin `subgraph-node-end` de `sello`; el frame `error` trae `errorText`
con `SUBGRAPH_RESUME_INCOMPATIBLE:`, nombrando `pregunta` y `confirmar`. El
turno 1 de cada pareja: `finish.finishReason == "suspended"`.
