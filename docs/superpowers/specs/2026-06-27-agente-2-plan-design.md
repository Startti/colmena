# Spec: Agente 2 — Plan (traductor técnico)

**Fecha:** 2026-06-27 · **Parte de:** [cadena de 4 agentes](2026-06-27-colmena-agent-chain-design.md)

## Propósito

Tomar el `specs.md` y producir un **plan de implementación cerrado**: la traducción completa
de la tarea de negocio a una arquitectura de nodos Colmena. Es el agente con el conocimiento
más profundo de la plataforma. El plan debe ser **tan específico** que el Agente 3 solo tenga
que materializar JSON, sin re-decidir arquitectura.

## Inputs / Outputs

- **Input:** `specs.md` (8 secciones fijas).
- **Output:** `plan.md` (estructura más abajo).

## Comportamiento clave

### Arquitectura cerrada
- **Se compromete** con tipos de nodo Colmena concretos, topología exacta del DAG, y qué
  tools van en qué agente. No deja decisiones de arquitectura abiertas.

### Conocimiento de Colmena vía skills
- Catálogo de nodos como **skills on-demand** (`skills.paths` + `load_skill`): una skill por
  capacidad/tipo (`llm_call`, `subgraph`, `http_request`/api-call-como-tool, `suspend`,
  `secure_suspend`, routing, SQL, docs/sheets, multimedia, etc.).
- Carga solo las skills que el `specs.md` necesita.

### Anotación de efecto (acople deliberado)
- Cada nodo/tool se marca **`lectura`** o **`escritura`** (efecto secundario).
- Esta anotación es fuente autoritativa para: credenciales a pedir (Agente 3) y, a futuro,
  qué mockear (verificador).

## Arquitectura Colmena

- **Nodo central:** `llm_call` (Gemini 2.5 Flash). Puede ser conversacional si se quiere que
  el usuario revise el plan, o one-shot si el handoff es documento-a-documento.
- **Skills on-demand:** el catálogo de nodos.
- **(Opcional)** `suspend` para una aprobación del plan por un operador.
- **Salida:** `plan.md`.

## Contrato de salida: `plan.md`

1. **Resumen de la solución** — una frase técnica.
2. **Topología del DAG** — lista de nodos + las aristas/edges.
3. **Nodo por nodo** — para cada uno:
   - Tipo de nodo Colmena.
   - Qué logra (intención).
   - Config/intención (sin escribir el JSON final).
   - Tools (qué api-calls van como tools, en qué agente).
   - Inputs/outputs (system variables).
   - **Efecto: `lectura` | `escritura`**.
4. **Puntos de `suspend` / `secure_suspend`** — dónde se pide al usuario, qué credenciales.
5. **Prompts pendientes** — qué nodos `llm_call` necesitan prompt, con la *intención* de cada uno
   (a redactar por el prompt-writer del Agente 3).
6. **Criterios de verificación** — derivados de la sección 7 del specs, en forma chequeable.

## Verificación independiente (con fixture)

- **Fixture:** 2–3 `specs.md` de ejemplo (hechos a mano) de tareas distintas.
- **Criterios:**
  - El `plan.md` tiene las 6 secciones.
  - Cada nodo tiene tipo concreto + efecto anotado.
  - Los tipos de nodo existen en Colmena (validable contra el catálogo).
  - Un `llm_call` juez (o humano) confirma que el plan es suficiente para construir sin re-decidir arquitectura.
- Correr real con `serve`, guardar SSE a `/tmp/colmena_e2e/agente2_*.sse`.

## Riesgos
- **Elegir nodos inexistentes o mal-encajados** → mitigar con skills-catálogo precisas y un
  chequeo de existencia de tipos.
- **Plan demasiado abierto** (contradice "arquitectura cerrada") → el juez de verificación lo detecta.

## Fuera de alcance
- Escribir prompts (Agente 3).
- Generar JSON (Agente 3).
