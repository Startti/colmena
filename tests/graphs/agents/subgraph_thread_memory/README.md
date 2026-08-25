# subgraph_thread_memory — `memory_mode: "dynamic"` E2E sequence

Four turns exercising a `subgraph` tool (`archivador`) configured with
`memory_mode: "dynamic"`, run in order against the **same** agent session so
state carries across turns:

| Turn | File | What it does |
|------|------|---------------|
| 1 | `turn1_store_alfa.json` | Stores a fact in thread `proyecto-alfa` (budget: 5000). |
| 2 | `turn2_store_beta.json` | Stores a fact in a separate thread `proyecto-beta` (budget: 8000), proving thread isolation. |
| 3 | `turn3_recall_alfa.json` | Recalls from `proyecto-alfa` and gets back the value stored in turn 1. |
| 4 | `turn4_list.json` | Calls the `list_threads` synthetic tool and expects both `proyecto-alfa` and `proyecto-beta` in the result. |

## Run order

All four graphs **must** run with the **same `--agent-session-id`**, in
order, e.g.:

```bash
ASID="thread_memory_demo_$(date +%s)"
for f in turn1_store_alfa turn2_store_beta turn3_recall_alfa turn4_list; do
  cargo run --bin dag_engine -- run tests/graphs/agents/subgraph_thread_memory/$f.json \
    --agent-session-id "$ASID"
done
```

Running `turn4_list.json` on its own (or with a fresh `--agent-session-id`)
lists zero threads — there is nothing to enumerate yet, since threads are
created by turns 1-2. This directory exists so the sequence is discoverable
and re-runnable as a unit; do not run `turn4_list.json` in isolation and
expect non-empty output.
