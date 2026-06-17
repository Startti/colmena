# Ejemplos de Uso en TypeScript — Colmena

Guía práctica de cómo usar Colmena desde Node.js/TypeScript. **Todos los ejemplos coinciden con la API real**
expuesta por las bindings napi-rs (`src/libs/colmena/src/node_bindings/`).

> El paquete se instala como `colmena-ai` (`npm install colmena-ai`) y **también se importa como
> `colmena-ai`**.

## Tabla de Contenidos

- [Configuración Inicial](#configuración-inicial)
- [LLM — llamada simple](#llm--llamada-simple)
- [LLM — streaming](#llm--streaming)
- [Opciones de configuración](#opciones-de-configuración)
- [Conversaciones](#conversaciones)
- [Health checks y providers](#health-checks-y-providers)
- [Motor DAG desde Node.js](#motor-dag-desde-nodejs)
- [documents (hojas CRDT)](#documents-hojas-crdt)
- [Manejo de errores](#manejo-de-errores)
- [Buenas prácticas](#buenas-prácticas)
- [Diferencias clave respecto a Python](#diferencias-clave-respecto-a-python)

---

## Configuración Inicial

### Instalación

```bash
npm install colmena-ai
```

### Importar e inicializar

```ts
import { ColmenaLlm } from "colmena-ai";

const llm = new ColmenaLlm();
```

`ColmenaLlm()` carga automáticamente las API keys desde el entorno al construirse.

### API Keys

```bash
# Recomendado: variables de entorno
export OPENAI_API_KEY="sk-..."
export GEMINI_API_KEY="AIza..."
export ANTHROPIC_API_KEY="sk-ant-..."
```

Para sobrescribir la key en una llamada puntual, usa `NodeLlmConfigOptions.apiKey` (ver abajo).

### Strings de provider

El parámetro `provider` acepta exactamente: `"openai"`, `"google"` (Gemini), `"anthropic"` y `"mock"`.

> Es `"google"`, **no** `"gemini"`.

---

## LLM — llamada simple

`call()` recibe:
- `messages`: array de objetos `{ role: string, content: string }` (roles: `system`, `user`, `assistant`).
- `provider`: string del proveedor.
- `options`: objeto `NodeLlmConfigOptions` opcional con modelo y parámetros de sampling.

Devuelve la respuesta como `Promise<string>`.

```ts
import { ColmenaLlm } from "colmena-ai";

const llm = new ColmenaLlm();
const text = await llm.call(
  [{ role: "user", content: "Hola" }],
  "google",
  { model: "gemini-2.5-flash", temperature: 0.7 },
);
console.log(text);
```

### Mensaje de sistema + usuario

```ts
const respuesta = await llm.call(
  [
    { role: "system", content: "Eres un experto en Rust que responde en español." },
    { role: "user", content: "¿Qué ventajas tiene Rust sobre TypeScript?" },
  ],
  "google",
);
console.log(respuesta);
```

### Comparar proveedores

```ts
import { ColmenaLlm, LlmError } from "colmena-ai";

const llm = new ColmenaLlm();
const pregunta = [{ role: "user", content: "¿Qué es Rust en una frase?" }];

for (const provider of ["openai", "google", "anthropic"] as const) {
  try {
    const respuesta = await llm.call(pregunta, provider);
    console.log(`\n${provider.toUpperCase()}:\n${respuesta.slice(0, 200)}...`);
  } catch (e) {
    if (e instanceof LlmError) {
      console.error(`Error con ${provider}: ${e.message}`);
    }
  }
}
```

---

## LLM — streaming

`stream()` devuelve una `Promise<LlmStream>`. Se consume con `for await...of`:

```ts
const stream = await llm.stream([{ role: "user", content: "Cuenta hasta 5" }], "google");
for await (const chunk of stream) process.stdout.write(chunk);
```

Cada `chunk` es un `string` con el fragmento de texto. Los errores durante el streaming se propagan
como `LlmError` al iterar.

### Streaming desde asyncio (patrón completo)

```ts
import { ColmenaLlm } from "colmena-ai";

async function historia() {
  const llm = new ColmenaLlm();
  const stream = await llm.stream(
    [{ role: "user", content: "Cuenta una historia corta sobre un robot programador" }],
    "openai",
  );
  for await (const chunk of stream) {
    process.stdout.write(chunk);
  }
  console.log(); // salto de línea final
}

historia();
```

---

## Opciones de configuración

Todos los parámetros de modelo/sampling viven en `NodeLlmConfigOptions` y se pasan como tercer
argumento de `call` o `stream`:

```ts
import { ColmenaLlm, type NodeLlmConfigOptions } from "colmena-ai";

const llm = new ColmenaLlm();

const opts: NodeLlmConfigOptions = {
  apiKey: "sk-...",       // opcional: override por llamada (si no, se toma del entorno)
  model: "gpt-4o",
  temperature: 0.8,       // creatividad (0.0 - 2.0)
  maxTokens: 200,         // longitud máxima de la respuesta
  topP: 0.9,              // nucleus sampling
  frequencyPenalty: 0.5,  // reduce repetición
  presencePenalty: 0.5,   // fomenta temas nuevos
};

const respuesta = await llm.call(
  [{ role: "user", content: "Escribe un poema corto sobre Rust" }],
  "openai",
  opts,
);
console.log(respuesta);
```

Campos disponibles: `apiKey`, `model`, `temperature`, `maxTokens`, `topP`, `frequencyPenalty`,
`presencePenalty`. Lo que no se asigna usa los defaults del proveedor.

---

## Conversaciones

El historial se mantiene como un array de objetos `{ role, content }`, alternando `user` y `assistant`:

```ts
import { ColmenaLlm } from "colmena-ai";

const llm = new ColmenaLlm();
const historial: { role: string; content: string }[] = [
  { role: "system", content: "Eres un mentor de programación conciso." },
  { role: "user", content: "Soy dev Python y quiero aprender Rust. ¿Por dónde empiezo?" },
];

const primera = await llm.call(historial, "anthropic");
console.log(primera);

// Agregar la respuesta al historial y continuar
historial.push({ role: "assistant", content: primera });
historial.push({ role: "user", content: "¿Qué herramientas debo instalar?" });

console.log(await llm.call(historial, "anthropic"));
```

---

## Health checks y providers

```ts
import { ColmenaLlm } from "colmena-ai";

const llm = new ColmenaLlm();

console.log(llm.getProviders());           // -> string[] de proveedores disponibles
console.log(await llm.healthCheck("google")); // -> boolean
```

---

## Motor DAG desde Node.js

Más allá de llamadas sueltas, Colmena ejecuta **workflows de agentes definidos como grafos JSON**
(nodos LLM, tools, HTTP, control de flujo, human-in-the-loop, etc.).

### Ejecutar un grafo: `runDag`

A diferencia de Python, `runDag` devuelve directamente el valor parseado (`unknown`), **no** un JSON
string. No es necesario llamar a `JSON.parse`.

```ts
import { runDag } from "colmena-ai";

const result = await runDag("tests/graphs/basic/power.json");
console.log(JSON.stringify(result, null, 2));
```

Firma completa:

```ts
runDag(
  graph,              // ruta al grafo JSON (string) O el grafo en memoria (GraphObject)
  resumeId?,          // id de resume para flujos suspend/resume
  resumeAnswer?,      // respuesta en formato Q/A canónico (ver "Suspend → Resume")
  injectPayload?,     // objeto inyectado como payload inicial del trigger
  includeExtraInfo?,  // incluye metadata extra en el resultado (default: false)
  agentSessionId?,    // id estable de sesión de agente (memoria, resume, secure values)
): Promise<unknown>
```

`graph` puede ser una ruta a archivo **o** un objeto en memoria:

```ts
await runDag({ nodes: { /* ... */ }, edges: [] });
```

> Para flujos con estado entre ejecuciones (suspend/resume, memoria conversacional), pasa siempre un
> `agentSessionId` estable.

### Streaming de eventos: `streamDag`

`run_dag` devuelve solo el resultado final. Para recibir los eventos de los nodos en tiempo real,
usa `streamDag` (devuelve un `DagStream` iterable con `for await...of`):

```ts
import { streamDag } from "colmena-ai";

const stream = await streamDag("tests/graphs/basic/power.json");
for await (const event of stream) {
  if (event.type === "text-delta") process.stdout.write(event.delta as string);
  else if (event.type === "finish") console.log("\nOutput final:", event);
}
```

También acepta un objeto en memoria:

```ts
const stream = await streamDag({ nodes: { /* ... */ }, edges: [] });
for await (const event of stream) {
  console.log(event.type, JSON.stringify(event));
}
```

El iterador se agota con el evento `{ type: "finish" }`. Errores de build se propagan al hacer `await`.

### Validar un grafo: `validateGraph`

Acepta un `GraphObject` y lanza `DagError` si el grafo no es válido.

```ts
import { validateGraph } from "colmena-ai";

const graph = {
  nodes: {
    start:      { type: "mock_input",   config: { input: 5 } },
    pow_step:   { type: "exponential",  config: { exponent: 3 } },
    log_result: { type: "log" },
  },
  edges: [
    { from: "start",    to: "pow_step" },
    { from: "pow_step", to: "log_result" },
  ],
};

validateGraph(graph);   // OK -> void ; inválido -> DagError
console.log("Grafo válido");
```

### Servir webhooks: `serveDag`

Levanta un servidor HTTP que expone los webhooks declarados en el grafo. **Es bloqueante.**

```ts
import { serveDag } from "colmena-ai";

// El grafo declara un trigger_webhook en "/power".
// POST http://localhost:8080/power  con  {"input": 10}  -> 1000
await serveDag("tests/graphs/basic/power_webhook.json", "0.0.0.0", 8080);
```

### Inspeccionar el registro de nodos: `defaultRegistry`

```ts
import { defaultRegistry } from "colmena-ai";

const reg = defaultRegistry();
console.log(reg.nodeTypes());                        // -> string[] de node types registrados

// Catálogo de sub-tools de un toolkit (sin conexión a DB)
const catalogo = reg.toolkitCatalog("api_explorer", {});
console.log(catalogo);                               // -> array de descriptores de tools
```

### Suspend → Resume

Un grafo con un nodo `suspend` pausa la ejecución. Para reanudar, pasa el mismo `agentSessionId`
estable en ambas llamadas y la respuesta en formato Q/A en el segundo run. Requiere un backend de
estado (Postgres, `DATABASE_URL`).

```ts
import { runDag } from "colmena-ai";

const GRAPH = "graph_con_suspend.json";  // input -> suspend(id="approve_continue") -> log
const AGENT = "mi_agente_estable_001";

// Run 1 — suspende
const s = await runDag(GRAPH, null, null, null, false, AGENT) as any;
console.assert(s.__colmena_status === "SUSPENDED");
console.log(s.questions);

// Run 2 — reanuda con la respuesta (el <id> es config.id del nodo suspend)
const answer = "Q[approve_continue]: ¿Apruebas continuar?\nA[approve_continue]: sí, aprobado";
const r = await runDag(GRAPH, null, answer, null, false, AGENT) as any;
console.assert(r.controller.status === "resumed");
```

### `injectPayload` — alimentar el trigger

```ts
const out = await runDag(
  "tests/graphs/basic/power_webhook.json",
  null, null,
  { input: 7 },   // injectPayload
) as any;
console.assert(out.pow_step.output === 343.0);  // 7 ** 3
```

---

## `documents` (hojas CRDT)

> **El subsistema CRDT aún está en desarrollo.** El objeto `documents` es funcional pero su
> superficie puede cambiar; trátalo como experimental hasta que CRDT se cierre.

El objeto `documents` exportado por `colmena-ai` expone operaciones sobre hojas de cálculo CRDT.
**Todos los métodos son async** (devuelven `Promise`), a diferencia de Python donde son síncronos.

```ts
import { documents } from "colmena-ai";

// artifactId debe tener el formato "art_" + ULID, e.g. "art_00000000000000000000000000"
const sheetId = await documents.addSheet(artifactId, "Ventas Q1");
await documents.writeSheet(artifactId, sheetId, ["producto", "total"], [["Widget", 100]], "replace");

const cells = await documents.readSheet(artifactId, sheetId);
// cells es un mapa de dirección de celda → valor: { "A1": "producto", "B1": "total", "A2": "Widget", ... }

const sheets = await documents.listSheets(artifactId);
// sheets: [{ sheetId: "...", name: "Ventas Q1" }]
```

### Companion `@colmena-ai/documents` — DataFrames con polars

El paquete opcional `@colmena-ai/documents` añade ergonomía polars encima de `documents`:

```bash
npm install @colmena-ai/documents
```

```ts
import { readSheetAsDataFrame, dataFrameToSheet } from "@colmena-ai/documents";
import { documents } from "colmena-ai";

// Leer como DataFrame de polars
const cells = await documents.readSheet(artifactId, sheetId);
const df = readSheetAsDataFrame(cells);
console.log(df.toString());

// Escribir de vuelta desde DataFrame
const { columns, rows } = dataFrameToSheet(df);
await documents.writeSheet(artifactId, sheetId, columns, rows, "replace");
```

---

## Manejo de errores

Las operaciones LLM lanzan `LlmError`; las del motor DAG lanzan `DagError`. Ambas extienden `Error`.

```ts
import { ColmenaLlm, runDag, LlmError, DagError } from "colmena-ai";

const llm = new ColmenaLlm();

try {
  const respuesta = await llm.call(
    [{ role: "user", content: "Explica qué es napi-rs" }],
    "google",
  );
  console.log(respuesta);
} catch (e) {
  if (e instanceof LlmError) console.error("Error de LLM:", e.message);
  else throw e;
}

try {
  await runDag("grafo_inexistente.json");
} catch (e) {
  if (e instanceof DagError) console.error("Error de DAG:", e.message);
  else throw e;
}
```

### Wrapper con reintentos (patrón útil)

```ts
import { ColmenaLlm, LlmError, type NodeLlmConfigOptions } from "colmena-ai";

class ColmenaWrapper {
  private llm = new ColmenaLlm();

  async callSafe(
    messages: { role: string; content: string }[],
    provider: string,
    options?: NodeLlmConfigOptions,
    maxRetries = 3,
  ): Promise<{ success: boolean; response: string | null; error: string | null }> {
    for (let attempt = 0; attempt < maxRetries; attempt++) {
      try {
        const response = await this.llm.call(messages, provider, options);
        return { success: true, response, error: null };
      } catch (e) {
        if (e instanceof LlmError && e.message.toLowerCase().includes("rate limit")) {
          await new Promise((r) => setTimeout(r, 2 ** attempt * 1000)); // backoff exponencial
          continue;
        }
        return { success: false, response: null, error: e instanceof Error ? e.message : String(e) };
      }
    }
    return { success: false, response: null, error: "max retries reached" };
  }
}

const wrapper = new ColmenaWrapper();
console.log(await wrapper.callSafe([{ role: "user", content: "Hola" }], "google"));
```

---

## Buenas prácticas

1. **Mensajes como objetos**: `messages` siempre es `{ role: string, content: string }[]`. Si falta
   alguna key, se lanza `LlmError`.
2. **Config vía `NodeLlmConfigOptions`**: modelo y sampling van en el tercer argumento de `call`/`stream`.
3. **Streaming con `for await...of`**: la forma idiomática Node.js para consumir `LlmStream` y `DagStream`.
4. **Provider `"google"`** para Gemini, nunca `"gemini"`.
5. **DAG con estado**: pasa `agentSessionId` estable para suspend/resume y memoria.
6. **`runDag` devuelve un valor parseado**: a diferencia de Python, **no** es un JSON string — no
   uses `JSON.parse` sobre el resultado.
7. **`documents` es async**: todos los métodos de `documents.*` devuelven `Promise` — usa `await`.

---

## Diferencias clave respecto a Python

| Aspecto | Python | Node.js / TypeScript |
|---|---|---|
| `documents.*` | Síncrono | **Async** — todos los métodos devuelven `Promise` |
| `runDag` resultado | JSON string → `json.loads` | Valor parseado directamente (`unknown`) |
| Streaming | `async for chunk in stream` | `for await (const chunk of stream)` |
| Opciones LLM | `LlmConfigOptions` (objeto con atributos) | `NodeLlmConfigOptions` (plain object) |
| Errores LLM | `colmena.LlmException` | `LlmError` (extends `Error`) |
| Errores DAG | `colmena.DagException` | `DagError` (extends `Error`) |

---

**Colmena** — *Orquestación de agentes de IA en Rust, con bindings nativos de Node.js y Python.*
