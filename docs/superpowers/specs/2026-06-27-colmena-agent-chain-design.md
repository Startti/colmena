# Diseño: Cadena de 4 agentes para construir grafos Colmena

**Fecha:** 2026-06-27
**Estado:** Diseño aprobado (brainstorm). Pendiente: specs por agente → planes → implementación.
**Autor:** daniel@startti.co + Claude

## Resumen

Una cadena de cuatro agentes Colmena que lleva a un **usuario no-ingeniero** desde
"quiero automatizar esto" hasta un **grafo Colmena ejecutable y verificado**, sin que
nunca tenga que hablar de nodos ni escribir JSON.

1. **Specs** — entrevista al usuario y produce un documento de especificaciones.
2. **Plan** — traduce el specs a una arquitectura cerrada de nodos Colmena.
3. **Builder** — materializa el JSON ejecutable (con prompts y credenciales).
4. **Verificador** — ejecuta el grafo y verifica que cumple el specs.

Es un caso de **dogfooding**: Colmena se usa a sí misma para construirse. Cada bug que
encontremos en el motor mientras lo hacemos, lo arreglamos para todos los usuarios.

## Decisiones de arquitectura (transversales)

| Decisión | Elección | Por qué |
|---|---|---|
| **Sustrato** | Grafos Colmena (serve mode) | El usuario final es un no-ingeniero en producción (ADP); el runtime self-serve es Colmena, no Claude Code. |
| **Empaque** | 4 grafos **separados**, verificados por separado | Cada uno tiene input/output claro y se prueba solo. La orquestación/unificación se diseña después. |
| **Handoff** | Documentos Markdown (`specs.md`, `plan.md`) | Todos los consumidores son LLMs y todos los aprobadores son humanos → Markdown con secciones fijas gana sobre JSON estructurado (evita doble fuente de verdad). |
| **Memoria/conversación** | Gemini 2.5 Flash + Postgres + header `x-agent-session-id` | Stack por defecto; el header es obligatorio en cada turno (ver `ref_serve_mode_memory_keying`). |
| **Filosofía** | Patrones superpowers como guía, nodos nativos como implementación | Entrevista (brainstorming), plan estructurado (writing-plans), subagentes, verificación adversarial. |

## Topología de la cadena

```
[usuario] --charla--> (Agente 1: Specs) --specs.md-->
                      (Agente 2: Plan)  --plan.md-->
                      (Agente 3: Builder) --grafo.json-->
                      (Agente 4: Verificador) --reporte + grafo verificado--> [usuario]
```

Por ahora el "-->" entre agentes es **manual/externo**: la salida de un grafo se entrega
como input del siguiente. Unificar esto en una sola experiencia conversacional es una fase
posterior (ver Backlog).

## Resumen por agente

Cada agente tiene su propio spec detallado:

- **Agente 1 — Specs:** [2026-06-27-agente-1-specs-design.md](2026-06-27-agente-1-specs-design.md)
- **Agente 2 — Plan:** [2026-06-27-agente-2-plan-design.md](2026-06-27-agente-2-plan-design.md)
- **Agente 3 — Builder:** [2026-06-27-agente-3-builder-design.md](2026-06-27-agente-3-builder-design.md)
- **Agente 4 — Verificador:** [2026-06-27-agente-4-verificador-design.md](2026-06-27-agente-4-verificador-design.md)

### El contrato: `specs.md` (8 secciones fijas)

Es la interfaz de toda la cadena. Prosa Markdown con headers obligatorios:

1. **Objetivo** — el QUÉ en lenguaje de negocio.
2. **Disparador / trigger** — qué inicia la tarea.
3. **Actores y sistemas** — Gmail, CRM X, Excel/Sheet, APIs.
4. **Procedimiento paso a paso** — lo que hoy hace la persona a mano.
5. **Datos** — qué entra, qué sale, qué campos.
6. **Conexiones y credenciales** — qué tokens/passwords harán falta (alimenta el `secure_suspend` del Agente 3).
7. **Criterios de éxito** — cómo sabemos que quedó bien (alimenta al Agente 4).
8. **Casos borde y errores** — qué hacer cuando algo falla.

### El contrato: `plan.md`

Arquitectura **cerrada** (el Agente 2 se compromete con tipos de nodo y topología; el
Agente 3 solo materializa):

1. Resumen de la solución (una frase técnica).
2. Topología del DAG (nodos + edges).
3. **Nodo por nodo:** tipo de nodo Colmena, intención, tools, inputs/outputs (system variables), **efecto: `lectura` | `escritura`**.
4. Puntos de `suspend` / `secure_suspend` (credenciales).
5. Prompts pendientes (intención de cada `llm_call`, a redactar por el prompt-writer del Agente 3).
6. Criterios de verificación (derivados de la sección 7 del specs).

La anotación **lectura/escritura** por nodo/tool es un acople deliberado: sirve para
saber qué credenciales pedir (Agente 3) y, a futuro, qué nodos mockear (verificador).

## Frontera entre agentes

- **1 → 2:** el specs es lenguaje de capacidades; el plan introduce los nodos Colmena.
- **2 → 3:** el plan es arquitectura cerrada (tipos de nodo + topología decididos); el builder materializa JSON + prompts + credenciales. El builder NO re-decide la arquitectura.
- **3 → 4:** el builder entrega JSON validado estructuralmente (su loop ya lo validó); el verificador prueba **ejecución y semántica**, no estructura.

## Orden de construcción

Secuencial **1 → 2 → 3 → 4**. Cada agente se construye y verifica con un **fixture de
input hecho a mano** (ej. un `specs.md` de ejemplo para construir el Agente 2 sin depender
del Agente 1). Esto hace que cada grafo sea independientemente testeable y desbloquea
trabajo en paralelo si hace falta.

## Hallazgos técnicos del motor (evidencia)

Investigado en `src/libs/colmena/src/dag_engine/`:

- **`mock_input`** (`debug.rs`) — nodo que devuelve su config como output. Stub nativo.
- **`mock` provider** + `mock_adapter.rs` — para mockear nodos `llm_call`.
- **`python_script`** — devuelve cualquier payload constante (`output = {...}`).
- **`tool_configurations`** (`llm.rs`, `domain/tool_configuration.rs`) — las tools de un
  `llm_call` se declaran como objeto JSON `{ nombre_arbitrario: { node_type, fixed_config, node_schema } }`.
  Permite **swapear** la implementación de una tool manteniendo su nombre/schema → clave
  para mockear tool-calls sin tocar Rust.
- **`child_graph_inline`** (`subgraph.rs`) — subgrafo inline como tool, con nombre custom y
  grafo no-fijo. Es el mecanismo del loop de validación del Agente 3 y de los subagentes.
- **SSE `events.rs`** — emite `node_finish {node_id, output}` y
  `llm_tool_call_finish {tool_name, output}` → toda salida es capturable (base del backlog de cassettes).
- **NO existe** hoy: `dry_run`, `replay`, `record`, sistema de fixtures.

## Backlog (documentado, diferido)

### Estrategia de mock de resultados (cassette / record-then-replay)

Diferida por pragmatismo: v1 del verificador usa **pruebas reales 100%**. Cuando el verificador
necesite dry-run barato y repetible para iterar correcciones sin efectos secundarios:

- **Patrón:** record-then-replay (estilo VCR/cassette). Primera corrida real (en sandbox)
  graba las respuestas; corridas siguientes las reproducen.
- **Cero-Rust confirmado** vía reescritura de JSON:
  - Escritura standalone (`http_request`) → reemplazar nodo por `mock_input`/`python_script` con el `{status, body}` grabado.
  - Escritura como tool-call → swapear la entrada en `tool_configurations` por un stub del mismo nombre que devuelve el output grabado.
- **Fuente de "qué es escritura":** la anotación `lectura/escritura` del `plan.md` (Agente 2).
- **Keying de cassettes:** v1 simple (una respuesta por nodo/tool); robusto por `hash(args)` después.
- **Grabar requiere credenciales sandbox** (la escritura inicial es real).
- **Ergonomía opcional (no requerida):** flag nativo `__colmena_skip_io` (~50 LOC Rust) para
  anotar nodos en vez de reestructurarlos.

### Otros pendientes
- Unificar los 4 agentes en una sola experiencia conversacional con checkpoints de aprobación.
- Loop automático builder↔verificador (auto-corrección).
- Librería de skills de arquetipos de dominio para el Agente 1 (hoy: entrevista en frío genérica).
- Migración a ADP.

## Fuera de alcance (v1)
- Mocking/cassettes (backlog).
- Orquestación automática entre agentes (handoff manual por ahora).
- Cambios al motor Colmena en Rust.
