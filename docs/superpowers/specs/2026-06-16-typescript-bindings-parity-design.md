# Diseño — Paridad de bindings TypeScript con Python (Colmena)

- **Fecha:** 2026-06-16
- **Estado:** Aprobado (diseño) — pendiente plan de implementación
- **Autor:** daniel@startti.co
- **Alcance:** Llevar el binding napi-rs (TypeScript/Node.js) a paridad funcional y de
  documentación con el binding PyO3 (Python), que recibió un sprint de auditoría en
  junio 2026 mientras el lado Node quedó congelado desde marzo 2026.

---

## 1. Problema

El binding de TypeScript **no es solo "docs faltantes"**: está funcionalmente atrasado
respecto a Python. No se puede "documentar lo mismo" porque la funcionalidad no existe.

### Estado actual (verdad del código)

`src/libs/colmena/src/node_bindings/mod.rs` (160 líneas) expone únicamente:

- `ColmenaLlm` con `call`, `healthCheck`, `getProviders` (constructor)
- `NodeLlmConfigOptions`, `NodeLlmMessage` (objetos napi)
- `runDag(filePath, resumeId?, resumeAnswer?, injectPayload?, includeExtraInfo?)` —
  **solo acepta file path, y NO expone `agentSessionId`**
- `serveDag(filePath, host?, port?)`

### Gaps frente a Python

| Capacidad | Python | TypeScript |
|---|---|---|
| `ColmenaLlm.call` | ✅ | ✅ |
| `health_check` / `get_providers` | ✅ | ✅ |
| `runDag` (file path) | ✅ | ✅ |
| `serveDag` | ✅ | ✅ |
| **LLM `stream()`** (async iterator) | ✅ | ❌ falta |
| **`stream_dag()`** (iterador SSE async) | ✅ | ❌ falta |
| **`run_dag` con grafo en memoria (dict/objeto)** | ✅ | ❌ solo path |
| **`validate_graph`** | ✅ | ❌ falta |
| **`default_registry` / `node_types` / `toolkit_catalog`** | ✅ | ❌ falta |
| **Módulo `documents` (CRDT/sheets)** | ✅ + wrapper pandas | ❌ falta |
| **Excepciones tipadas** (`LlmException`/`DagException`) | ✅ | ❌ Error genérico |
| **`agent_session_id`** | ✅ | ❌ no expuesto en `runDag` |
| Docs de uso | ✅ `python_usage.md` + `48_python_dag.md` | ❌ cero |
| Type stubs curados | ✅ `.pyi` | ⚠️ `index.d.ts` autogenerado, escueto |
| Tests / ejemplos | ✅ ~10 archivos | ❌ cero |

**De-risk clave:** el lado Python no reimplementa lógica para streaming — llama a
`crate::dag_engine::api::stream_dag` / `stream_sse_parts`, que ya existen en el core.
El napi puede llamar **las mismas funciones**; el trabajo es el wrapper async-iterator
napi, no lógica de motor nueva.

---

## 2. Decisiones de diseño (tomadas en brainstorm)

1. **Alcance:** Paridad total — construir la superficie napi faltante Y documentarla/testearla
   espejo a Python.
2. **Estilo API:** Idiomático TS (camelCase, `Promise`, `for await...of`, objetos tipados,
   clases de `Error`), no copia literal de Python.
3. **Consumidor:** Paquete público en npm (`colmena-ai`). Implica DX completa: docs, types
   curados, ejemplos, README npm, binarios precompilados multiplataforma.
4. **Test runner:** `node:test` nativo (cero dependencias nuevas, alineado con el paquete
   cero-deps; tests de integración, no unitarios con mocks).
5. **`documents` / pandas:** El usuario necesita capacidades DataFrame pesadas/performance →
   **nodejs-polars** (addon napi backed por Rust, mismo stack). El core compilado queda
   **cero-deps** devolviendo `{ columns, rows }` crudo; un **companion opt-in** integra polars.
   Espejo exacto del split Python (módulo compilado + wrapper pandas separado).
6. **Estrategia de ejecución:** Slices verticales (B) con una **Fase 0 de infraestructura**
   al frente (CD/packaging/test-harness).

---

## 3. Arquitectura

### 3.1 Layout de paquetes (espejo del split Python core/wrapper)

```
colmena-ai (npm, core)          ← módulo compilado .node, CERO deps runtime
├── index.js / index.d.ts       ← autogenerado napi + capa fina TS para iterators
├── npm/<platform>/             ← sub-paquetes por plataforma (patrón napi-rs)
└── binding: LLM + DAG + registry + documents (crudo)

@colmena-ai/documents (npm, companion opt-in)   ← análogo a colmena_documents (pandas)
└── envuelve `documents` crudo con nodejs-polars (DataFrame in/out)
```

El core nunca importa polars. El companion depende de `colmena-ai` + `nodejs-polars`.

### 3.2 Estructura de archivos en el repo

```
src/libs/colmena/src/node_bindings/
├── mod.rs              ← re-exports (existente, hoy monolítico de 160 líneas)
├── llm.rs              ← ColmenaLlm + stream
├── dag.rs              ← runDag / streamDag / validateGraph / serveDag
├── registry.rs         ← defaultRegistry / Registry
├── documents.rs        ← listSheets / readSheet / addSheet / writeSheet (crudo)
└── stream.rs           ← DagStream / LlmStream (async-iterator napi)
ts/                     ← NUEVO: capa TS no-generada + tests + companion
├── index.ts            ← attach [Symbol.asyncIterator], re-exports tipados
├── errors.ts           ← LlmError / DagError extends Error
├── test/*.test.ts      ← node:test (espejo de python/tests/)
└── examples/*.mjs
docs/examples/typescript_usage.md           ← espejo de python_usage.md
docs/developer_guide/49_typescript_dag.md   ← espejo de 48_python_dag.md
```

**Justificación del split de `mod.rs`:** hoy es un solo archivo de 160 líneas; va a crecer
~4x. Archivos enfocados por capacidad son más fáciles de revisar y mantener (regla "un
archivo por responsabilidad"). El split es parte del trabajo, no un refactor no relacionado.

---

## 4. Superficie API (Python → TypeScript idiomático)

| Python | TypeScript | Estado |
|---|---|---|
| `ColmenaLlm.call(msgs, provider, opts)` | `call(msgs, provider, opts?): Promise<string>` | existe |
| `ColmenaLlm.stream(...)` → `async for chunk` | `stream(...): Promise<LlmStream>` + `for await (const chunk of s)` | **nuevo** |
| `health_check` / `get_providers` | `healthCheck` / `getProviders` | existe |
| `LlmConfigOptions` (clase) | `LlmConfigOptions` (interface/object napi) | existe |
| `run_dag(str\|dict, ...)` | `runDag(graph: string \| GraphObject, ...): Promise<unknown>` | **agregar dict input** |
| `stream_dag(...)` → `async for event` | `streamDag(...): Promise<DagStream>` + `for await (const ev of s)` | **nuevo** |
| `validate_graph(dict)` | `validateGraph(graph): void` | **nuevo** |
| `serve_dag(path, host, port)` | `serveDag(...)` | existe |
| `default_registry()` → `Registry` | `defaultRegistry(): Registry` | **nuevo** |
| `Registry.node_types()` / `.toolkit_catalog()` | `.nodeTypes()` / `.toolkitCatalog()` | **nuevo** |
| `colmena.documents.*` | `documents.listSheets/readSheet/addSheet/writeSheet` | **nuevo, crudo** |
| `LlmException` / `DagException` | `LlmError` / `DagError extends Error` | **nuevo** |
| `agent_session_id` param | `agentSessionId` | **falta en runDag hoy** |

### 4.1 Async iterators (pieza técnica central)

napi expone una clase `DagStream` con `async next(): Promise<{ value, done }>`. La capa
fina TS (`ts/index.ts`) le adjunta `[Symbol.asyncIterator]()` para que
`for await (const ev of stream)` funcione idiomáticamente. Mismo patrón para `LlmStream`.
El core Rust reusa `dag_engine::api::stream_dag` / `stream_sse_parts` — sin lógica de motor
nueva.

### 4.2 Tipado de eventos

`DagEvent` como **union discriminada** por `type`
(`'node-start' | 'node-end' | 'text-delta' | 'finish' | ...`) en `index.d.ts` curado —
mejor DX que el `Dict[str, Any]` de Python.

### 4.3 Errores

napi-rs no tiene clases de error custom nativas. Las clases JS (`LlmError`, `DagError`,
ambas `extends Error`) viven en `ts/errors.ts`; la capa fina re-envuelve los `napi::Error`
por código de `Status`. Espejo funcional de `LlmException` / `DagException`.

### 4.4 Módulo `documents` y companion polars

- **Core (`documents.rs`):** `listSheets(artifactId)`, `readSheet(artifactId, sheetId)` →
  `{ columns: string[], rows: unknown[][] }`, `addSheet(artifactId, name)` → `sheetId`,
  `writeSheet(artifactId, sheetId, columns, rows, mode?)`. Cero deps.
- **Companion (`@colmena-ai/documents`):** `readSheet → pl.DataFrame`,
  `writeSheet(df, ...)` aceptando DataFrame de polars. Análogo al wrapper pandas de Python.

---

## 5. Testing

`node:test` nativo, espejo de `python/tests/`. Un archivo por capacidad:

- `llm.test.ts` — call, stream, healthCheck, getProviders (mock + real provider)
- `run-dag.test.ts` — path + objeto en memoria, injectPayload, resume, validateGraph
- `stream-dag.test.ts` — iteración async, lifecycle de nodos, evento finish, errores
- `registry.test.ts` — defaultRegistry, nodeTypes, toolkitCatalog
- `documents.test.ts` — roundtrip add/write/read/list (+ companion polars)

Más `ts/examples/*.mjs` ejecutables. Corren con `node --test` en CI develop (espejo de
`pytest python/`).

---

## 6. CI/CD — Fase 0 (arreglo de `cd-main.yml` job `publish-npm`)

El job `publish-npm` existe pero está **roto para un paquete público**:

| Falla actual | Fix |
|---|---|
| Solo compila el `.node` del runner (ubuntu/linux-x64) pese a declarar 4 `triples` → macOS/Windows reciben el paquete sin binario nativo → `require` falla | **Matrix multiplataforma** (ubuntu/macos/windows + aarch64), igual que el job de wheels; publicar sub-paquetes `npm/<triple>` (patrón `napi prepublish`) |
| `checkout@v4` sin `ref` → toma el SHA pre-bump → publica versión stale (PyPI sí usa `ref: v${new_version}`) | Agregar `ref: v${{ needs.release.outputs.new_version }}` |
| Sin `files`/`.npmignore` → empaqueta el repo entero en el tarball npm | Campo `files` en `package.json` limitado a `index.js`, `index.d.ts`, `*.node` |
| Changelog solo dice `pip install` | Agregar bloque `npm install colmena-ai` |
| No corre tests TS en CI | Scaffold `node --test` en `ci-develop.yml` |

---

## 7. Docs

- `docs/examples/typescript_usage.md` — espejo de `python_usage.md`
- `docs/developer_guide/49_typescript_dag.md` — espejo de `48_python_dag.md`
- Sección Node/TypeScript en `README.md` (hoy no menciona Node)
- Entrada en `docs/DEVELOPER_GUIDE.md` (hoy cero entradas Node)

---

## 8. Secuencia de implementación (B + Fase 0)

Cada slice = binding Rust + capa TS + types + test + doc, mergeable de forma independiente;
la paridad crece monótona.

- **Fase 0 — Infra:** CD matrix multiplataforma + version `ref` + `files` + scaffold
  `node:test` en CI.
- **Slice 1 — LLM stream + errores:** `LlmStream` (async iterator) + `LlmError`/`DagError`.
- **Slice 2 — DAG inputs:** `runDag(objeto en memoria)` + `validateGraph` + `agentSessionId`.
- **Slice 3 — DAG streaming:** `streamDag` (async iterator sobre `stream_sse_parts`).
- **Slice 4 — Registry:** `defaultRegistry` / `nodeTypes` / `toolkitCatalog`.
- **Slice 5 — Documents + companion:** `documents` crudo + `@colmena-ai/documents` (polars)
  + docs finales (`typescript_usage.md`, `49_typescript_dag.md`, README, DEVELOPER_GUIDE).

---

## 9. Restricciones y notas

- **Sin breaking changes en el core Rust:** todo el trabajo es aditivo en `node_bindings/`
  y reusa funciones `api` existentes. No toca firmas públicas consumidas por el worker ADP.
- **Cero-deps en el core npm:** polars vive solo en el companion opt-in.
- **Toolchain:** Rust pinned 1.95.0; `napi build --features node`. Tests Rust con
  `cargo test --verbose` antes de push (no solo `--lib`).
- **No-objetivos:** edge/serverless runtimes (napi nativo no corre ahí); no se rediseña el
  motor DAG; no se porta la ergonomía pandas literal (se usa polars idiomático).
