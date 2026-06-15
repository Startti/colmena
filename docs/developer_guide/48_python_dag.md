# Ejecutar grafos DAG desde Python

Guía de la **superficie del motor DAG** expuesta por las bindings PyO3: `run_dag`,
`validate_graph`, `serve_dag` y la introspección del registro. Para llamadas LLM directas
(`ColmenaLlm.call` / `stream`) ver [`docs/examples/python_usage.md`](../examples/python_usage.md).

> El paquete se instala como `colmena-ai`; el módulo a importar es `colmena`.

## Instalación / build desde fuente

```bash
pip install colmena-ai          # release publicado
```

Para compilar las bindings desde el repo (desarrollo):

```bash
# pyo3 0.21 soporta hasta Python 3.12 — usar un venv 3.12 para compilar
python3.12 -m venv .venv-dev && source .venv-dev/bin/activate
pip install maturin pytest pytest-asyncio python-dotenv
maturin develop                 # compila el crate con feature `python` hacia el venv
pytest python/tests/            # corre la suite de bindings
```

## `run_dag` — ejecutar un grafo a término

```python
import colmena
import json

result_json = colmena.run_dag("tests/graphs/basic/power.json")
result = json.loads(result_json)            # run_dag devuelve un JSON string
print(result["pow_step"]["output"])         # 125.0
```

Firma completa:

```python
colmena.run_dag(
    graph,                      # ruta al grafo JSON (str) O el grafo en memoria (dict)
    resume_id=None,             # session_id de un run suspendido a reanudar (fallback de agent_session_id)
    resume_answer=None,         # respuesta en formato Q/A canónico (ver "Suspend → Resume")
    inject_payload=None,        # dict inyectado como payload del trigger (ver "inject_payload")
    include_extra_info=False,   # incluye metadata (usage, tool_calls, ...) en el output
    agent_session_id=None,      # id estable de sesión de agente (memoria, resume, secure values)
) -> str                        # JSON string; lanza colmena.DagException en error
```

`graph` acepta tanto una ruta a archivo como un grafo dict en memoria (sin escribirlo a disco):

```python
colmena.run_dag({"nodes": {...}, "edges": [...]})   # mismo dict que validate_graph
```

El resultado contiene la salida de cada nodo más `__colmena_session_id`. Ejemplo
(`power.json` = `mock_input 5 → exponential^3 → log`):

```json
{
  "start": {"input": 5},
  "pow_step": {"output": 125.0},
  "log_result": 125.0,
  "__colmena_session_id": "…"
}
```

Errores (archivo inexistente, grafo inválido, fallo de ejecución) se propagan como
`colmena.DagException`:

```python
try:
    colmena.run_dag("no/existe.json")
except colmena.DagException as e:
    print(f"falló: {e}")
```

## `validate_graph` — validar un grafo en memoria

Acepta un **dict** y lanza `DagException` si el grafo no deserializa al `Graph` del engine
(misma estrictez que `cargo run -- run <file>`, sin red ni LLM):

```python
graph = {
    "nodes": {
        "start":      {"type": "mock_input", "config": {"input": 5}},
        "pow_step":   {"type": "exponential", "config": {"exponent": 3}},
        "log_result": {"type": "log"},
    },
    "edges": [
        {"from": "start", "to": "pow_step"},
        {"from": "pow_step", "to": "log_result"},
    ],
}
colmena.validate_graph(graph)   # OK -> None ; inválido -> DagException
```

## `inject_payload` — alimentar el trigger

`inject_payload` deposita un dict como payload entrante en los nodos `trigger_webhook`. Útil
para correr un grafo "como si" hubiera llegado por webhook, sin levantar el servidor:

```python
# power_webhook.json: trigger_webhook -> exponential^3 -> log
out = json.loads(colmena.run_dag(
    "tests/graphs/basic/power_webhook.json",
    inject_payload={"input": 7},
))
assert out["pow_step"]["output"] == 343.0   # 7 ** 3
```

> Nota: `mock_input` NO consume `inject_payload` (usa su `config.input`). El payload aplica a
> nodos `trigger_webhook` (y otros triggers que lo lean).

## Suspend → Resume

Un grafo con un nodo `suspend` pausa la ejecución y devuelve el estado SUSPENDED. Para reanudar,
**pasa el mismo `agent_session_id` estable en ambos runs** (es la key canónica de persistencia) y
la respuesta en formato Q/A en el segundo run. Requiere un backend de estado (Postgres,
`DATABASE_URL`).

```python
import colmena, json

GRAPH = "graph_con_suspend.json"   # input -> suspend(id="approve_continue") -> log
AGENT = "mi_agente_estable_001"

# Run 1 — suspende
s = json.loads(colmena.run_dag(GRAPH, agent_session_id=AGENT))
assert s["__colmena_status"] == "SUSPENDED"
print(s["questions"])   # [{"id": "approve_continue", "question": "...", "type": "open", ...}]

# Run 2 — reanuda con la respuesta (el <id> es config.id del nodo suspend)
answer = "Q[approve_continue]: ¿Apruebas continuar?\nA[approve_continue]: sí, aprobado"
r = json.loads(colmena.run_dag(GRAPH, resume_answer=answer, agent_session_id=AGENT))
assert r["controller"]["status"] == "resumed"
```

Detalles del formato Q/A (id-keyed, orden-independiente, multilínea) y de `secure_suspend` en
[`44_suspend_node.md`](44_suspend_node.md) y el spec de
[suspend-qa-response-format](../superpowers/specs/2026-05-08-suspend-qa-response-format-design.md).

## `serve_dag` — servir webhooks como API HTTP

Levanta un servidor que expone cada `trigger_webhook` del grafo como ruta POST. **Es bloqueante**
(corre hasta Ctrl-C). Cada request ejecuta el grafo con el body como payload del trigger.

```python
import colmena

# Expone POST /power (definido en el grafo). El body es el payload del trigger.
colmena.serve_dag("tests/graphs/basic/power_webhook.json", host="0.0.0.0", port=8080)
```

```bash
curl -X POST http://localhost:8080/power -H "Content-Type: application/json" -d '{"input": 10}'
# => 1000
```

También registra `POST /resume` para reanudar runs suspendidos. Acepta SSE
(`Accept: text/event-stream`) para streaming estilo Vercel AI SDK.

## Introspección del registro

`default_registry()` construye un registro sin conexión a DB para inspección:

```python
reg = colmena.default_registry()
reg.node_types()                       # -> lista de node types registrados
reg.toolkit_catalog("api_explorer", {})  # -> [{"name", "description", "required"}, ...]
```

## `agent_session_id` vs `session_id`

Para cualquier flujo con estado entre runs (suspend/resume, memoria conversacional, secure
values) **pasa siempre un `agent_session_id` estable**. Los subsistemas de persistencia keyan
primero por `agent_session_id`; el `session_id` efímero rota por invocación. Misma regla que el
CLI con `--agent-session-id` (ver `CLAUDE.md`).

## Cobertura de tests

`python/tests/test_run_dag.py` y `python/tests/test_serve_dag.py` cubren esta superficie:
`run_dag` (output final, archivo inexistente, `inject_payload`, suspend→resume), `validate_graph`
(válido/inválido), `default_registry` y un smoke de `serve_dag`. El test de suspend→resume se
salta si no hay `DATABASE_URL`.

Estado y brechas conocidas: [`audit_python_bindings.md`](audit_python_bindings.md).
