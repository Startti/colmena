# subgraph_fixed_thread_id — `memory_mode: "dynamic"` with a PLATFORM-fixed `thread_id`

Three turns exercising a `subgraph` tool (`archivador`), `memory_mode: "dynamic"`
**and** `node_schema.thread_id: { "fixed": "${agentId}" }` — the platform names the
memory thread from a model-supplied `agentId`, not the model (Task 4/5,
`child_graph_ref` chain). Unlike `subgraph_thread_memory/` (model invents and
echoes its own `thread_id`), here `thread_id` is never a tool parameter at all.

| Turn | File | What it does |
|------|------|---------------|
| 1 | `turn1_tell_a1.json` | Tells agent `a1` a fact (an access code). |
| 2 | `turn2_ask_a2.json` | Asks agent `a2` for it — must NOT know (isolated thread). |
| 3 | `turn3_recall_a1.json` | Asks `a1` again — must remember it. |

Run all three, in order, with the **same `--agent-session-id`**:

```bash
ASID="fixed_thread_id_demo_$(date +%s)"
for f in turn1_tell_a1 turn2_ask_a2 turn3_recall_a1; do
  cargo run --bin dag_engine -- run tests/graphs/agents/subgraph_fixed_thread_id/$f.json \
    --agent-session-id "$ASID"
done
```

What to check: `node_schema` exposes only `agentId`/`task` to the model
(`thread_id` carries `fixed`); the child's `node_id` is
`tool/archivador/a1/keeper` for turns 1+3 and `tool/archivador/a2/keeper` for turn
2; the parent's stored `tool` message for `archivador` never carries `[hilo: <id>]`
— a fixed thread was never the model's choice, so there is nothing to echo back.
