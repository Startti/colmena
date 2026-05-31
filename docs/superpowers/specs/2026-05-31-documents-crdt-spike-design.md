# Spike — Documentos colaborativos CRDT (Fase 0)

**Fecha:** 2026-05-31
**Autor:** daniel@startti.co (con asistencia AI)
**Estado:** Diseño aprobado — listo para implementación
**Duración objetivo:** 2-3 semanas (time-box duro)
**Output:** GO/NO-GO informado, NO un producto

---

## 1. Contexto y motivación

El módulo `documents/` actual ([docs/developer_guide/27_documents_library.md](../../developer_guide/27_documents_library.md)) soporta creación y edición incremental de Excel/Word/HTML vía un IR JSON versionado con patches atómicos. Las limitaciones v1 documentadas en §10 son explícitas:

- **Sin ingesta de binarios existentes.**
- **Sin colaboración en tiempo real.**
- **Sin frontend** (espera integrar Univer/Tiptap).
- Excel sin fórmulas reales, charts, merged cells, pivot tables, formato condicional.

El objetivo a largo plazo es un servicio donde **múltiples humanos y sus agentes LLM editen documentos simultáneamente** (CRDT real), con ingesta de archivos reales, narración de cambios para el LLM y un nodo Python que aplique transformaciones masivas (pandas) sin gastar tokens.

Tras brainstorming, las decisiones de alto nivel fueron:

| Decisión | Razón |
|---|---|
| Modelo de concurrencia: CRDT multi-peer | Humanos + agentes editan en paralelo, no por turnos |
| Frontend: camino híbrido Univer + IR proyección | Univer resuelve canvas/fórmulas/colab; el IR sigue siendo contrato estable para Python/LLM bulk |
| Empezar por spike (no v1 completo) | Tres riesgos técnicos (R1, R2, R5) deben validarse antes de comprometer 3+ meses |

Este documento especifica **únicamente el spike**. El v1 real se diseñará en otro spec, después del resultado del spike.

## 2. Riesgos a validar

| ID | Riesgo | Costo si se confirma malo |
|---|---|---|
| **R1** | Univer en browser + Yrs en colmena hablan vía y-websocket sin requerir backend Node.js propio | Camino híbrido colapsa; replantear hacia Camino 1 puro |
| **R2** | Proyección `yrs::Doc → IR JSON` es estable, barata (<50ms para 1000 celdas) y acotada (<500 LOC) | IR deja de ser útil como contrato; colmena se reduce a "proxy WS" |
| **R5** | Importar un `.xlsx` real (merged cells, fórmulas, formato) en Univer preserva contenido visual | Feature no sirve para empresas reales — buscar canvas alternativo |

Riesgos **fuera del spike** (para v1 o más adelante):

- R3 — Python "snapshot-compute-emit" preserva semántica bajo concurrencia.
- R4 — Diff legible para el LLM es útil y barato.
- R6 — Performance multi-usuario sobre WS con sheets grandes.
- R7 — LLM emitiendo ops via tool produce ediciones sensatas.

## 3. Scope

### Dentro del spike

- Excel únicamente.
- Subcomando nuevo `dag_engine spike-yws --port 8080` que sirve:
  - `GET /` → HTML estático con Univer
  - `GET /spike.xlsx` → archivo de prueba
  - `WS /yjs/{artifact_id}` → endpoint protocol Yjs
  - `GET /projection/{artifact_id}.json` → snapshot del IR proyectado
- Univer cargado desde CDN (fallback: Vite minimal si CDN no resuelve).
- `yrs::Doc` en memoria por artifact (`HashMap<ArtifactId, Doc>`); sin persistencia entre runs.
- Importer `.xlsx` de Univer (sin exporter en el spike).
- Proyección read-only `Yrs → IR JSON` minimal: `{ sheets: [{ id, name, cells: { A1: value } }] }`.
- Agente peer en Rust como función + CLI (`dag_engine spike-agent --artifact <id> --op set_cell --addr A1 --value "hola"`) que muta el `yrs::Doc` en el mismo proceso.
- Archivo de prueba `.xlsx` con: 1 hoja, merged cells (2 rangos), 1 fórmula básica (`=SUM(A1:A5)`), formato de color en encabezado, 1000 celdas con valores mixtos string+number.

### Fuera del spike (queda para v1 o posterior)

- Word, HTML, Google Sheets.
- DAG node nuevo, LLM tools, integración con `llm_call`.
- Bundler de producción, integración ADP, auth, deploy.
- Snapshots, versioning, persistencia GCS.
- Exporter `.xlsx`, fidelidad round-trip completa.
- Proyección bidireccional (IR → Yrs).
- Python helper (cualquier nivel).
- UI de presencia, cursores remotos.
- Charts, pivot tables, validación de datos en el IR.

## 4. Arquitectura

```
┌──────────────────┐         ┌────────────────────────────────────────┐
│  Browser (tab)   │         │  colmena (binario dag_engine)          │
│  Univer @CDN     │  WS     │  ┌──────────────────────────────────┐  │
│  Yjs WS provider │ ◄─────► │  │ axum + tokio-tungstenite         │  │
│                  │         │  │ /yjs/{artifact_id}               │  │
└──────────────────┘         │  └──────────────────────────────────┘  │
┌──────────────────┐         │                │                       │
│  Browser (tab 2) │  WS     │                ▼                       │
│  Univer @CDN     │ ◄─────► │  ┌──────────────────────────────────┐  │
└──────────────────┘         │  │ yrs::Doc registry                │  │
                             │  │  HashMap<ArtifactId, Doc>        │  │
┌──────────────────┐         │  └──────────────────────────────────┘  │
│  CLI agent-peer  │         │                │                       │
│  cargo run ...   │ in-proc │                ▼                       │
│  --op set_cell A1│ ◄─────► │  ┌──────────────────────────────────┐  │
└──────────────────┘         │  │ projection_to_ir(doc) → JSON     │  │
                             │  │  (dump a /tmp/spike/<id>.json)   │  │
                             │  └──────────────────────────────────┘  │
                             │                                        │
                             │  GET / → spike.html                    │
                             │  GET /spike.xlsx → archivo de prueba   │
                             │  GET /projection/{id}.json → IR        │
                             └────────────────────────────────────────┘
```

### 4.1 Stack técnico

| Capa | Elección | Notas |
|---|---|---|
| CRDT runtime | [yrs](https://crates.io/crates/yrs) 0.18+ | Port Rust de Yjs, mismo wire protocol |
| WS server | `axum` + `tokio-tungstenite` (ya en el árbol) | Implementar protocol Yjs ([spec](https://github.com/yjs/y-websocket)) sobre WebSocket |
| Frontend canvas | [Univer](https://github.com/dream-num/univer) — facade Excel | Cargado desde `unpkg.com` CDN; fallback Vite minimal |
| Frontend WS provider | `y-websocket` JS client | Standard Yjs WS provider, compatible con yrs |
| Importer XLSX | Univer's built-in xlsx importer | El browser carga `.xlsx`, lo abre, el provider sincroniza al backend |

### 4.2 Modelo Yjs para Excel (versión spike)

Sujeto a iteración tras el spike. Mantener mínimo para validar R1/R2:

```
Y.Doc
  └─ Y.Map "workbook"
       └─ "sheets" : Y.Array<Y.Map>
            └─ Y.Map
                 ├─ "id"    : string
                 ├─ "name"  : string
                 └─ "cells" : Y.Map<string, Y.Map>
                                 └─ Y.Map
                                      ├─ "v" : any (string | number | bool)
                                      └─ "t" : string ("s"|"n"|"b")
```

Notas:

- **Fórmulas y formato visual viven en Univer, NO en este modelo.** El spike acepta que la proyección IR solo capte valores celulares. Si Univer mantiene fórmulas/formato en su propio estado Y.Doc paralelo, no es problema del spike.
- Direcciones de celda son strings A1-style ("A1", "B2", …).
- Tipo `t` simplifica la proyección y evita ambigüedades JS-numbers vs JSON-numbers.

### 4.3 Proyección Yrs → IR JSON

Función pura Rust en el spike:

```rust
fn project(doc: &yrs::Doc) -> serde_json::Value {
    // Walk del Y.Map raíz "workbook" → emite JSON normalizado.
    // Sin Yjs awareness; toma una snapshot del estado actual.
}
```

Output:

```json
{
  "sheets": [
    {
      "id": "s1",
      "name": "Hoja1",
      "cells": { "A1": "Hola", "B1": 42, "A2": true }
    }
  ]
}
```

Dump automático a `/tmp/spike/{artifact_id}.json` después de cada batch de cambios (debounce 500ms) para inspección.

### 4.4 Agente peer en Rust

CLI que muta el `yrs::Doc` del proceso `spike-yws` por dos rutas, para validar dos escenarios distintos:

**Ruta A — Peer via WS (recomendada para validar R1):**
El subcomando `spike-agent` se conecta al endpoint `WS /yjs/{artifact_id}` igual que un browser, aplica una transacción Yjs sobre un `yrs::Doc` cliente y deja que el sync protocol propague el cambio. Esto valida que cualquier peer (browser o Rust) funciona vía el mismo wire protocol.

```bash
dag_engine spike-agent ws --url ws://localhost:8080/yjs/a1 --op set_cell --sheet s1 --addr A1 --value "agente"
```

**Ruta B — Mutación in-process directa (solo para sanity check):**
El subcomando `spike-yws` también expone un endpoint interno `POST /spike/agent-op` que muta el `yrs::Doc` del registry directamente con `doc.transact_mut()`. Sirve para descartar problemas de WS si Ruta A falla y aislar la causa.

```bash
curl -X POST localhost:8080/spike/agent-op -d '{"artifact":"a1","sheet":"s1","addr":"A1","value":"agente"}'
```

**Criterio R1.1 se valida con Ruta A**, no con Ruta B.

## 5. Criterios GO/NO-GO

Al final del spike se escribe `docs/superpowers/specs/2026-XX-XX-documents-crdt-spike-results.md` con resultados medidos.

| Criterio | Métrica | GO si... |
|---|---|---|
| **R1.1** — Convergencia multi-peer | 2 tabs + agente Rust editan A1, A2, A3 en simultáneo, dejar 5s, comparar estados | Idénticos en los 3, latencia visual <1s |
| **R1.2** — Univer sin backend propio | Funciona con nuestro WS Yjs custom | No requiere `@univerjs/collaboration-client` atado al backend de Univer |
| **R2.1** — Costo proyección | Sheet con 1000 celdas mixed, correr `project()` 100 veces, p50 | p50 <50ms |
| **R2.2** — Tamaño código proyección | LOC del módulo `projection.rs` | <500 LOC |
| **R2.3** — Estabilidad proyección | Tras 50 ediciones aleatorias, IR es válido JSON | 100% válido |
| **R5.1** — Ingesta visual | Importar `.xlsx` con merged + fórmula + color → ver en Univer | Visualmente correcto (acepta diffs menores) |
| **R5.2** — Proyección capta valores | El IR del XLSX importado contiene los valores celulares no-fórmula | 100% correctos |

**NO-GO si falla cualquier criterio.** Documentamos hallazgos y replanteamos (probablemente: Camino 1 puro Univer, o canvas alternativo como [Spread.JS](https://www.grapecity.com/spreadjs)/[Luckysheet](https://github.com/dream-num/Luckysheet)).

## 6. Plan de trabajo

| Semana | Hito | Cierra qué riesgo |
|---|---|---|
| 1 | Subcomando `spike-yws` sirve HTML + WS Yjs; 1 tab abre Univer; una celda persiste en `yrs::Doc` | R1 parcial |
| 1.5 | 2 tabs + agente peer Rust editan en paralelo y convergen | R1 cierre |
| 2 | Proyección Yrs→IR + endpoint que la sirve + benchmark de 1000 celdas | R2 cierre |
| 2.5 | Importar `.xlsx` real, ver fidelidad visual, verificar IR capta valores | R5 cierre |
| 3 | Escribir spec de resultados con GO/NO-GO + grabar demo manual | Cierre |

## 7. Entregables

| Tipo | Item |
|---|---|
| Código | Rama `spike/documents-crdt` con subcomandos `spike-yws` y `spike-agent` |
| Código | `spike/` (raíz del repo) con HTML estático, fixtures XLSX |
| Doc | Este spec (`2026-05-31-documents-crdt-spike-design.md`) |
| Doc | Spec de resultados (`2026-XX-XX-documents-crdt-spike-results.md`) — a llenar al final |
| Demo | Video corto (Loom/screen recording) mostrando 2 tabs + agente editando |

## 8. Riesgos del propio spike

| Riesgo | Mitigación |
|---|---|
| Univer requiere bundler (CDN insuficiente) | Plan B: Vite minimal (1 día). No bloquea. |
| `y-websocket` y `yrs` difieren en wire protocol | Validar primero con un POC mínimo (1 día) antes de codear el HTML. yrs documenta compat con yjs. |
| Univer colab depende de su backend `@univerjs/collaboration-server` | Esto **ES** R1. Un NO-GO temprano y útil. |
| Tiempo se infla más de 3 semanas | Time-box duro. Día 15 sin GO/NO-GO claro → detener y reportar hallazgos parciales. |

## 9. Lo que el spike NO compromete

Este spike no compromete el v1. Aunque pase GO, las siguientes decisiones quedan abiertas y se diseñarán en el spec de v1:

- Cómo el LLM emite ops (peer Yrs in-proc vs tool que traduce a Yjs).
- Vocabulario completo de ops de edición.
- Persistencia: snapshots vs apppend-only log vs híbrido.
- Auth y multi-tenancy con ADP.
- API Python helper exacta.
- Estrategia de export (Univer exporter vs renderer propio).
- Word, HTML, Google Sheets — orden y profundidad de soporte.

## 10. Referencias

- Módulo actual: [docs/developer_guide/27_documents_library.md](../../developer_guide/27_documents_library.md)
- Diseño original (interno): [docs/superpowers/specs/2026-04-21-documents-feature-design.md](2026-04-21-documents-feature-design.md)
- Univer: https://github.com/dream-num/univer
- yrs: https://crates.io/crates/yrs
- Yjs WS protocol: https://github.com/yjs/y-websocket
