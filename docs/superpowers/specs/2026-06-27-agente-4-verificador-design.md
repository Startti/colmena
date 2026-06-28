# Spec: Agente 4 — Verificador

**Fecha:** 2026-06-27 · **Parte de:** [cadena de 4 agentes](2026-06-27-colmena-agent-chain-design.md)

## Propósito

Tomar el grafo `.json` producido por el Agente 3 + el `specs.md` inicial, **ejecutarlo de
verdad**, y verificar que cumple los criterios de éxito del specs. Produce un reporte de
verificación (pass/fail + qué falló + por qué).

## Inputs / Outputs

- **Inputs:** grafo `.json` + `specs.md` (criterios de éxito, sección 7).
- **Output:** reporte de verificación (Markdown).

## Comportamiento clave (v1 = pruebas reales 100%)

### Tres niveles de verificación
1. **Estructural** — *no se hace aquí*; ya lo garantizó el loop del Agente 3.
2. **Ejecución** — el grafo corre end-to-end sin crashear. Captura SSE a `/tmp/colmena_e2e/`.
3. **Semántico (el valioso)** — un `llm_call` **juez adversarial** compara la salida real de la
   corrida contra los criterios de éxito del specs. Intenta **refutar** que cumple (patrón
   superpowers `verification-before-completion`), no asume que sí.

### Ejecución real con confirmación
- v1 corre el grafo **de verdad**. Para no causar daño en una prueba, antes de ejecutar nodos
  de escritura hace un `suspend` de advertencia: "esto va a enviar correos/escribir de verdad,
  ¿confirmas? (idealmente apunta a credenciales sandbox)".
- El usuario provee credenciales (sandbox o reales) vía `secure_suspend`.

### Reporte, no loop automático (por ahora)
- La salida es un **reporte informativo**: pass/fail, qué criterio falló, evidencia del SSE.
- El loop automático builder↔verificador (auto-corrección) se difiere a la fase de unificación.

## Arquitectura Colmena

- **Tool de ejecución:** `subgraph` / `child_graph_inline` que corre el grafo a verificar
  (pasado como input), o invocación a `dag_engine run`.
- **`suspend`** de confirmación antes de escrituras + **`secure_suspend`** para credenciales.
- **Nodo juez:** `llm_call` que recibe (criterios del specs + salida/SSE de la corrida) y emite
  el veredicto adversarial.
- **Salida:** el reporte.

## Verificación independiente (con fixture)

- **Fixture:** un grafo `.json` simple conocido-bueno + su `specs.md`, y un grafo conocido-malo
  (que no cumple un criterio) para confirmar que el juez lo detecta.
- **Criterios:**
  - Con el grafo bueno → reporte `pass`.
  - Con el grafo malo → reporte `fail` que identifica el criterio incumplido.
- Correr real con `serve`, guardar SSE a `/tmp/colmena_e2e/agente4_*.sse`.

## Riesgos
- **Efectos secundarios reales en una prueba** → mitigado por el `suspend` de confirmación y
  el uso de credenciales sandbox; la solución completa (cassettes/dry-run) está en backlog.
- **Juez complaciente** → mitigado por el prompt adversarial (refutar, no confirmar).

## Backlog (ver diseño general)
- **Cassette / record-then-replay** para dry-run barato y repetible sin efectos secundarios.
  Cero-Rust confirmado vía reescritura de JSON (`mock_input` + swap de `tool_configurations`).
  Diferido por pragmatismo: primero pruebas reales, después optimizamos.

## Fuera de alcance (v1)
- Mocking / dry-run (backlog).
- Auto-corrección automática del grafo (solo reporta).
