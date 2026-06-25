# Diseño — Agente Creador de Grafos (Graph Builder)

**Fecha:** 2026-06-25
**Estado:** Diseño aprobado, pendiente de plan de implementación
**Autor:** daniel + Claude

## 1. Resumen

Un grafo de Colmena cuyo agente conversacional (`llm_call` con memoria) ayuda a
**personas no técnicas** a construir **otros grafos de Colmena**. La persona habla
en lenguaje de capacidades ("quiero que una IA conteste preguntas y busque en
internet"), nunca en términos de nodos. El agente entrevista, entiende el objetivo,
arma el grafo borrador, **lo ejecuta de verdad para juzgar si funciona**, lo corrige,
y finalmente entrega el JSON listo para usar.

Es un "grafo que crea grafos". v1 es **standalone en Colmena** (modo `serve`, chat en
vivo). La migración a ADP se evaluará después y queda fuera de alcance.

## 2. Objetivos y no-objetivos

### Objetivos (v1)
- Conversación 100% en lenguaje de persona: nunca exige términos técnicos
  (node_type, puertos, edges) ni nombra nodos directamente.
- Entiende jerga coloquial y la mapea a capacidades ("Excel"/"planilla" → Sheets,
  "Word" → Docs, "guardar registros" → SQL, etc.), con desambiguación cuando hace falta.
- Entrevista adaptativa: entiende **qué problema** quiere resolver la persona antes
  del **cómo**, una pregunta a la vez.
- Genera un grafo JSON válido y ejecutable para un **set curado** de capacidades.
- **Cierra el ciclo**: ejecuta el grafo borrador (vía `subgraph` como tool), observa
  el resultado real y juzga/corrige antes de entregar.
- Entrega el grafo en el chat + un resumen en lenguaje simple de qué hace y cómo correrlo.

### No-objetivos (v1 → backlog)
- Cobertura de todos los nodos: orchestrator multi-agente, loops cíclicos, subgrafos
  anidados como capacidad de usuario, socket.io, CRDT, librería de documentos, math,
  secure_suspend, planner/critic/reactor sueltos.
- Despliegue/migración a ADP (canvas).
- Tool `guardar_grafo` (escribir el grafo final a un archivo en disco) — backlog.

## 3. Arquitectura — Opción A: un solo agente conversacional

Un único nodo `llm_call` con memoria por `agent_session_id` que ejecuta todo el ciclo:
entrevista → arma borrador → prueba con la tool de ejecución → juzga/corrige → entrega.

Se descartaron:
- **Orchestrator multi-agente:** pesado, pensado para resolver la tarea del usuario,
  no para una entrevista abierta de ida y vuelta; difícil de hacer conversacional/HITL.
- **Pipeline por etapas (router + varios llm_call):** rígido; la entrevista real es
  adaptativa (a veces la persona ya sabe qué quiere, a veces hay que indagar mucho).

La inteligencia no viene de orquestar varios modelos, sino de: **buen método de
entrevista + buen conocimiento de capacidades + el loop de probar-y-corregir**.

**Stack del propio builder:** `google/gemini-2.5-flash` + Postgres `DATABASE_URL`
(memoria conversacional). Corre en modo `serve`.

## 4. El menú de capacidades (v1 curada)

El agente razona y habla en estas capacidades; por debajo las mapea a nodos:

| Capacidad (lenguaje de persona) | Nodo(s) Colmena |
|---|---|
| Que una IA responda, escriba o transforme texto | `llm_call` |
| Buscar información en internet | `tavily_client` (search/fetch) |
| Traer datos de un servicio o API externa | `http_request` (+ `api_explorer` para descubrir endpoints) |
| Pedirle un dato o una decisión a la persona | `suspend` (HITL) |
| Crear una imagen / editar una imagen | `image_generation` / `image_edit` |
| Generar audio o voz | `tts` |
| Trabajar con una hoja de cálculo / un documento | tools `gsheets_*` / `gdocs_*` |
| Consultar o guardar datos en una base de datos | `sql_query` |
| Hacer un cálculo o transformar datos a medida | `python_script` |
| Decidir un camino según el caso | `router` |
| Por dónde entra / sale el flujo | `trigger_webhook`/`input` y `output` |

## 5. Capa de conocimiento (híbrida)

### `system_message` (siempre presente, liviano)
- El método de entrevista (§7).
- El menú de capacidades (§4).
- Tabla de **vocabulario coloquial → capacidad** (sinónimos de la calle).
- Anatomía mínima de un grafo: `nodes` (cada uno con `type` + `config`), `edges`
  (`from`/`to`, puertos default vs explícitos `nodo.campo`), `${ENV}` para llaves,
  stack LLM por defecto, nodo de entrada y `output`.
- Disciplina de "armar → probar → juzgar → corregir → entregar".
- **Cuándo cargar cada skill.**

### Skills (on-demand vía `load_skill`, desde `llm_call.skills.paths` — sin recompilar)
Una por familia de capacidad, con el detalle de config, campos requeridos, gotchas y
ejemplos completos:
- `building-graphs-core` — anatomía completa, edges/puertos, patrones comunes, errores típicos.
- `capability-ai-text` — `llm_call`: providers, model, `system_message`, `tool_configurations`.
- `capability-web-and-apis` — `tavily_client`, `http_request`, `api_explorer`.
- `capability-ask-user` — `suspend`, formato Q/A, HITL.
- `capability-multimedia` — `image_generation`, `image_edit`, `tts`.
- `capability-docs-and-sheets` — toolkits `gsheets`/`gdocs`, "Excel" online vs `.xlsx` descargable.
- `capability-data-sql` — `sql_query`: presets de permisos, validación.
- `capability-code-and-logic` — `python_script`, `router`.

Cada `SKILL.md` lleva frontmatter (`name`, `description`). El agente carga sólo la de
la familia que la conversación toca. Cuando cambien los nodos, se edita un archivo de
skill, no el prompt.

## 6. Tool de ejecución ("probarlo de verdad") + seguridad

- Tool `probar_grafo`: `node_type: subgraph`, con `child_graph_inline` como campo
  **no-fijo** de tipo `object` dentro de `node_schema`. El agente arma el grafo borrador
  y lo pasa entero; el nodo `subgraph` lo lee desde `inputs` y lo ejecuta de verdad,
  devolviendo el resultado para que el agente lo juzgue.
- **Factibilidad confirmada en código** (investigación 2026-06-25):
  `subgraph.rs::resolve_child_graph_source` lee `inputs.get("child_graph_inline")`, y
  el ejecutor de tools mergea los args del LLM a `inputs`. **Cero cambios de Rust.**
  Hay una nota en CLAUDE.md sobre subgraph-as-tool que conviene **confirmar en vivo con
  un E2E** antes de dar por cerrada la factibilidad (ver §9).
- **Test autocontenido:** para probar, el agente hornea valores de prueba en el nodo de
  entrada del borrador (`input` / `trigger_webhook.test_payload`), así la corrida no
  necesita inputs externos.
- **Seguridad / efectos reales:** antes de correr un grafo con capacidades de efecto
  (API con POST/PUT/DELETE, escritura en base de datos, envío de mensajes), el agente
  **avisa y pide confirmación** o usa datos de prueba inocuos. Grafos de sólo lectura /
  IA se prueban libremente.
- Las llaves (`${OPENAI_API_KEY}`, `${GEMINI_API_KEY}`, etc.) se resuelven del proceso
  del motor → probar grafos con IA funciona si el builder corre con el `.env` cargado.
- Guarda de recursión de subgrafos = 5 (suficiente para v1).

## 7. Método de entrevista (el corazón)

Disciplina en el `system_message`:
1. **Entender el objetivo primero**, no la solución. Preguntar *qué problema querés
   resolver* antes de *cómo*.
2. **Una pregunta a la vez**, en lenguaje de persona, con ejemplos concretos cuando ayuda.
3. **Mapear jerga → capacidad** (tabla de vocabulario) y **desambiguar** cuando es
   ambiguo. Ej. "Excel": ¿editable en línea (Sheets) o archivo descargable (.xlsx)?
4. **Proponer el flujo en palabras y confirmarlo** antes de generar JSON:
   "entonces: entra tu pregunta → la IA busca en internet → te devuelve un resumen, ¿correcto?".
5. **Armar → probar → juzgar → corregir en silencio** (sin abrumar con JSON), y entregar
   recién cuando funciona.
6. **Nunca pedir detalles técnicos** (puertos, node_types): si falta info técnica, el
   agente la infiere o usa defaults sensatos.

## 8. Entrega y archivos

- **Entrega:** el grafo final en un bloque de código + resumen en lenguaje simple de
  *qué hace y cómo correrlo*. (`guardar_grafo` queda en backlog.)
- **Archivos a crear:**
  - `tests/graphs/agents/graph_builder/graph_builder.json` — el meta-grafo.
  - `tests/graphs/agents/graph_builder/skills/*/SKILL.md` — skills por familia.
  - `tests/graphs/agents/graph_builder/README.md` — cómo levantarlo y usarlo.

## 9. Plan de pruebas (E2E real, no mocks)

Memoria del proyecto: verificar siempre con grafos E2E reales contra servicios vivos;
guardar SSE en `/tmp/colmena_e2e/<nombre>.sse` y presentar reporte amistoso.

1. **Verificación de factibilidad primero:** un grafo mínimo donde un `llm_call` con la
   tool `probar_grafo` (`subgraph` + `child_graph_inline` dinámico) ejecuta un grafo
   trivial armado por el LLM (ej. un `input`→`output`). Confirma la nota del CLAUDE.md
   sobre subgraph-as-tool con campo no-fijo.
2. **Conversación completa:** "quiero un bot que conteste preguntas" → entrevista →
   arma `llm_call` → prueba → entrega JSON.
3. **Caso con jerga + desambiguación:** "quiero pasar unos datos a un Excel" → el agente
   desambigua online vs descargable → arma con tools `gsheets_*`.
4. **Caso con efecto real:** un grafo que escribiría en una API → el agente avisa/pide
   confirmación antes de correrlo.

## 10. Riesgos y mitigaciones

- **La nota del CLAUDE.md sobre subgraph-as-tool podría contradecir el campo no-fijo.**
  → Mitigación: el paso 1 del plan E2E lo verifica en vivo antes de construir el resto.
- **Ejecutar grafos borrador tiene efectos/costos reales.** → Mitigación: disciplina de
  aviso + datos de prueba + preferencia por capacidades de sólo lectura al probar.
- **El LLM genera JSON con errores sutiles.** → Mitigación: el loop de ejecución real es
  el validador de verdad; los errores del motor vuelven al agente para corregir.
- **Contexto inflado si carga muchas skills.** → Mitigación: híbrido + carga on-demand
  de una skill por familia tocada.

## 11. Backlog (post-v1)
- Tool `guardar_grafo` (escribir a archivo).
- Cobertura de capacidades avanzadas (orchestrator, loops, subgrafos de usuario,
  socket.io, CRDT, documentos, math, secure_suspend).
- Migración/adaptación a ADP (canvas).
- Validador estático liviano (linter) como pre-chequeo barato antes de ejecutar.
