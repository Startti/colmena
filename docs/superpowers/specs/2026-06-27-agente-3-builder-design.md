# Spec: Agente 3 — Builder (materializador de JSON)

**Fecha:** 2026-06-27 · **Parte de:** [cadena de 4 agentes](2026-06-27-colmena-agent-chain-design.md)

## Propósito

Tomar el contexto de los dos agentes previos (`specs.md` + `plan.md`) y **materializar el
grafo Colmena en JSON**: ejecutable, con prompts redactados, credenciales cableadas y
validado contra el motor. No re-decide arquitectura (eso lo cerró el Agente 2); la construye.

## Inputs / Outputs

- **Inputs:** `specs.md` + `plan.md` (necesita ambos).
- **Output:** el grafo `.json` Colmena válido y ejecutable.

## Comportamiento clave

### Orquestador + prompt-writer + loop de validación
1. **Orquestador:** ensambla la topología/JSON a partir del plan cerrado (ensamblaje mecánico,
   de un tiro — no necesita subagente).
2. **Subagente `prompt-writer`** (especializado): redacta cada prompt de `llm_call` a partir
   de su *intención* (sección 5 del `plan.md`). Implementado como subgrafo (`child_graph_inline`)
   expuesto como tool, o como llamada interna dedicada.
3. **`secure_suspend`:** para cada credencial de la sección 6 del specs / sección 4 del plan,
   pausa y pide el token/password; lo cablea como secure value (backend `SECURE_VALUES_KEY`).
4. **Loop validar-y-reparar:** carga el draft en el motor vía `child_graph_inline`, lee los
   errores del SSE, repara, reintenta — hasta que el grafo valida estructuralmente.

### División de responsabilidad
- El ensamblaje mecánico (nodos, edges) NO es subagente.
- Lo genuinamente generativo (prompts) SÍ es subagente especializado.

## Arquitectura Colmena

- **Nodo central:** `llm_call` orquestador (Gemini 2.5 Flash).
- **Tools:**
  - `prompt_writer` — subgrafo/llm_call especializado en prompts de nodos.
  - `validar_grafo` — `subgraph` con `child_graph_inline` no-fijo (pasa el draft, lo ejecuta/valida).
  - `secure_suspend` — para credenciales.
- **Salida:** el JSON final.

## Notas técnicas (confirmadas en el motor)
- `tool_configurations` permite tools con nombre arbitrario y `fixed_config` con
  `child_graph_inline` → así se expone tanto el `prompt_writer` como el `validar_grafo`.
- El patrón `child_graph_inline` no-fijo para "ejecutar un draft pasado" ya está probado
  (ver `project_graph_builder_agent`): funciona sin cambios en Rust cuando se expone vía `node_schema`.
- `secure_suspend` y el backend de secure values ya están en producción (ver
  `feedback_secure_values_key_required`).

## Verificación independiente (con fixture)

- **Fixture:** un `specs.md` + `plan.md` de ejemplo (hechos a mano) para una tarea concreta.
- **Criterios:**
  - El JSON producido carga sin errores en el motor (lo garantiza el propio loop).
  - Tiene un prompt no-vacío en cada `llm_call` que el plan marcó.
  - Tiene `secure_suspend` para cada credencial listada.
  - La topología corresponde a la del `plan.md`.
- Correr real con `serve`, guardar SSE a `/tmp/colmena_e2e/agente3_*.sse`.

## Riesgos
- **JSON inválido / schemas equivocados** → mitigado por el loop validar-y-reparar.
- **Prompts pobres** → mitigado por el subagente prompt-writer especializado.
- **Concentrar demasiado en un prompt gigante** → evitado por la división orquestador/prompt-writer.

## Fuera de alcance
- Ejecutar el grafo contra sistemas reales (eso es del Agente 4).
- Re-decidir arquitectura (la cerró el Agente 2).
