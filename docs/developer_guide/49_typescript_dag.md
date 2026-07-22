# Ejecutar grafos DAG desde Node.js / TypeScript

Guía de la **superficie del motor DAG** expuesta por las bindings napi-rs: `runDag`,
`streamDag`, `validateGraph`, `serveDag` y la introspección del registro. Para llamadas LLM
directas (`ColmenaLlm.call` / `stream`) ver [`docs/examples/typescript_usage.md`](../examples/typescript_usage.md).

> El paquete se instala e importa como `colmena-ai`.

## Instalación

```bash
npm install colmena-ai          # release publicado
```

Para compilar las bindings desde el repo (desarrollo):

```bash
# Requiere Node.js 18+ y la toolchain Rust pinned en rust-toolchain.toml
npm run build                   # compila el crate con feature `node` via napi-rs + tsc (ts/ -> lib/)
npm test                        # node --test lib/test/*.js (requiere `npm run build` antes)
```

## `runDag` — ejecutar un grafo a término

A diferencia de Python, `runDag` devuelve el valor **ya parseado** (`unknown`), no un JSON
string. No es necesario llamar a `JSON.parse`.

```ts
import { runDag } from "colmena-ai";

const result = await runDag("tests/graphs/basic/power.json");
console.log((result as any).pow_step.output);   // 125.0
```

Firma completa:

```ts
runDag(
  graph,              // ruta al grafo JSON (string) O el grafo en memoria (GraphObject)
  resumeId?,          // session_id de un run suspendido a reanudar (fallback de agentSessionId)
  resumeAnswer?,      // respuesta en formato Q/A canónico (ver "Suspend → Resume")
  injectPayload?,     // objeto inyectado como payload del trigger (ver "injectPayload")
  includeExtraInfo?,  // incluye metadata (usage, tool_calls, ...) en el output (default: false)
  agentSessionId?,    // id estable de sesión de agente (memoria, resume, secure values)
): Promise<unknown>   // lanza DagError en error
```

`graph` acepta tanto una ruta a archivo como un grafo objeto en memoria (sin escribirlo a disco):

```ts
await runDag({ nodes: { /* ... */ }, edges: [] });
```

El resultado contiene la salida de cada nodo más `__colmena_session_id`. Ejemplo
(`power.json` = `mock_input 5 → exponential^3 → log`):

```json
{
  "start": { "input": 5 },
  "pow_step": { "output": 125.0 },
  "log_result": 125.0,
  "__colmena_session_id": "..."
}
```

Errores (archivo inexistente, grafo inválido, fallo de ejecución) se propagan como `DagError`:

```ts
import { runDag, DagError } from "colmena-ai";

try {
  await runDag("no/existe.json");
} catch (e) {
  if (e instanceof DagError) console.error("falló:", e.message);
}
```

## `validateGraph` — validar un grafo en memoria

Acepta un `GraphObject` y lanza `DagError` si el grafo no deserializa al `Graph` del engine
(misma estrictez que `cargo run -- run <file>`, sin red ni LLM):

```ts
import { validateGraph, DagError } from "colmena-ai";

const graph = {
  nodes: {
    start:      { type: "mock_input",  config: { input: 5 } },
    pow_step:   { type: "exponential", config: { exponent: 3 } },
    log_result: { type: "log" },
  },
  edges: [
    { from: "start",    to: "pow_step" },
    { from: "pow_step", to: "log_result" },
  ],
};

try {
  validateGraph(graph);   // OK -> void ; inválido -> DagError
  console.log("Grafo válido");
} catch (e) {
  if (e instanceof DagError) console.error("Inválido:", e.message);
}
```

## `injectPayload` — alimentar el trigger

`injectPayload` deposita un objeto como payload entrante en los nodos `trigger_webhook`. Útil
para correr un grafo "como si" hubiera llegado por webhook, sin levantar el servidor:

```ts
// power_webhook.json: trigger_webhook -> exponential^3 -> log
const out = await runDag(
  "tests/graphs/basic/power_webhook.json",
  null, null,
  { input: 7 },   // injectPayload
) as any;
console.assert(out.pow_step.output === 343.0);   // 7 ** 3
```

> Nota: `mock_input` NO consume `injectPayload` (usa su `config.input`). El payload aplica a
> nodos `trigger_webhook` (y otros triggers que lo lean).

## Suspend → Resume

Un grafo con un nodo `suspend` pausa la ejecución y devuelve el estado SUSPENDED. Para reanudar,
**pasa el mismo `agentSessionId` estable en ambas llamadas** y la respuesta en formato Q/A en
la segunda. Requiere un backend de estado (Postgres, `DATABASE_URL`).

```ts
import { runDag } from "colmena-ai";

const GRAPH = "graph_con_suspend.json";  // input -> suspend(id="approve_continue") -> log
const AGENT = "mi_agente_estable_001";

// Run 1 — suspende
const s = await runDag(GRAPH, null, null, null, false, AGENT) as any;
console.assert(s.__colmena_status === "SUSPENDED");
console.log(s.questions);
// [{ id: "approve_continue", question: "...", type: "open", ... }]

// Run 2 — reanuda con la respuesta (el <id> es config.id del nodo suspend)
const answer = "Q[approve_continue]: ¿Apruebas continuar?\nA[approve_continue]: sí, aprobado";
const r = await runDag(GRAPH, null, answer, null, false, AGENT) as any;
console.assert(r.controller.status === "resumed");
```

Detalles del formato Q/A (id-keyed, orden-independiente, multilínea) y de `secure_suspend` en
[`44_suspend_node.md`](44_suspend_node.md) y el spec de
[suspend-qa-response-format](../superpowers/specs/2026-05-08-suspend-qa-response-format-design.md).

## `streamDag` — consumir los eventos del DAG en proceso

`runDag` devuelve **solo el resultado final**. Para recibir el play-by-play de los nodos
(`node-start`, `node-end`, `text-delta`, `tool-input-available`, `finish`, …) **en proceso** (sin
levantar el servidor HTTP), usa `streamDag`: devuelve una `Promise<DagStream>` donde `DagStream`
es un async iterable que entrega cada evento como `DagEvent`.

```ts
import { streamDag } from "colmena-ai";

const stream = await streamDag(
  "tests/graphs/agents/agent_with_tools_gemini.json",
  null, null, null, false,
  "session_001",           // agentSessionId
);

for await (const event of stream) {
  if (event.type === "text-delta") {
    process.stdout.write(event.delta as string);
  } else if (event.type === "node-end") {
    console.log(`\n[nodo ${(event as any).node_id} listo]`);
  } else if (event.type === "finish") {
    console.log("\nOutput final:", JSON.stringify(event));
  }
}
```

### `DagEvent` — unión discriminada

`DagEvent` es una unión discriminada en la propiedad `type`:

| `type` | Descripción | Campos extra notables |
|---|---|---|
| `"node-start"` | Un nodo comenzó a ejecutarse | `node_id` |
| `"node-end"` | Un nodo terminó | `node_id`, `output` |
| `"text-delta"` | Fragmento de texto streaming de un LLM | `delta` (string) |
| `"finish"` | El grafo terminó | `output` |
| otros strings | Eventos extendidos (tools, HITL, etc.) | varía |

```ts
import { type DagEvent } from "colmena-ai";

function handleEvent(event: DagEvent) {
  switch (event.type) {
    case "text-delta":
      process.stdout.write(event.delta as string);
      break;
    case "finish":
      console.log("\nFinalizado");
      break;
  }
}
```

- **Firma:** `streamDag(graph, resumeId?, resumeAnswer?, injectPayload?, includeExtraInfo?,
  agentSessionId?)`. `graph` es path **o** objeto, igual que `runDag`.
- Errores (archivo inexistente, grafo inválido) se lanzan como `DagError` al hacer `await`.
- El iterador se agota con el evento `{ type: "finish" }`.

## `serveDag` — servir webhooks como API HTTP

Levanta un servidor que expone cada `trigger_webhook` del grafo como ruta POST. **Es bloqueante**
(la Promise no resuelve hasta Ctrl-C o error fatal). Cada request ejecuta el grafo con el body
como payload del trigger.

```ts
import { serveDag } from "colmena-ai";

// Expone POST /power (definido en el grafo). El body es el payload del trigger.
await serveDag("tests/graphs/basic/power_webhook.json", "0.0.0.0", 8080);
```

```bash
curl -X POST http://localhost:8080/power -H "Content-Type: application/json" -d '{"input": 10}'
# => 1000
```

También registra `POST /resume` para reanudar runs suspendidos. Acepta SSE
(`Accept: text/event-stream`) para streaming estilo Vercel AI SDK.

## Introspección del registro

`defaultRegistry()` construye un registro sin conexión a DB para inspección:

```ts
import { defaultRegistry } from "colmena-ai";

const reg = defaultRegistry();
console.log(reg.nodeTypes());                           // -> string[] de node types registrados
console.log(reg.toolkitCatalog("api_explorer", {}));    // -> array de descriptores de tools
```

## `agentSessionId` vs `sessionId`

Para cualquier flujo con estado entre llamadas (suspend/resume, memoria conversacional, secure
values) **pasa siempre un `agentSessionId` estable**. Los subsistemas de persistencia keyan
primero por `agentSessionId`; el `sessionId` efímero rota por invocación. Misma regla que el
CLI con `--agent-session-id` (ver `CLAUDE.md`).

## Diferencias respecto a Python

| Aspecto | Python | Node.js |
|---|---|---|
| Resultado de `runDag` | JSON string (`str`) — usa `json.loads` | Valor parseado (`unknown`) — listo para usar |
| Streaming DAG | `async for event in stream` | `for await (const event of stream)` |
| `documents.*` | Síncrono | Async — devuelven `Promise` |

## Cobertura de tests

`ts/test/` cubre la superficie de las bindings: `runDag` (output final, archivo inexistente,
`injectPayload`, objeto en memoria), `validateGraph` (válido/inválido), `streamDag`, `defaultRegistry`
y un smoke de `ColmenaLlm`. Los tests que requieren `DATABASE_URL` se saltan si no está disponible.
