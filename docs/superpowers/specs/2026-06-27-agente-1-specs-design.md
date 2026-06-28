# Spec: Agente 1 — Specs (entrevistador)

**Fecha:** 2026-06-27 · **Parte de:** [cadena de 4 agentes](2026-06-27-colmena-agent-chain-design.md)

## Propósito

Entrevistar a un **usuario no-ingeniero** sobre la tarea que quiere automatizar y producir
un documento de especificaciones (`specs.md`) completo y sin ambigüedades, que sirva de
**contrato** para el resto de la cadena. El usuario habla en lenguaje de negocio; el agente
nunca menciona nodos, JSON ni términos técnicos.

## Inputs / Outputs

- **Input:** ninguno formal — arranca la conversación con el usuario.
- **Output:** `specs.md` en Markdown con **8 secciones fijas** (ver más abajo).

## Comportamiento clave

### Entrevista en frío y genérica
- Sirve para **cualquier** tarea (triaje de correos, data-entry, update de CRM, reportes…).
- **No** usa playbooks de dominio. Puede cargar un par de skills *genéricas* sobre qué hace
  bueno a un specs (anatomía de un buen specs, disciplina de entrevista), nunca específicas.

### Disciplina de entrevista (patrón superpowers `brainstorming`)
- **Una pregunta por turno**, vía la tool `suspend`.
- Multiple-choice cuando se pueda; abierta cuando haga falta.
- Lenguaje de capacidades: "¿el correo siempre trae un Excel adjunto?", nunca "¿usamos un nodo extraction?".
- **Prohibido terminar** hasta cubrir las 8 secciones. El system prompt lo fuerza.

### Gate de completitud + aprobación
1. Antes de emitir, auto-review contra las 8 secciones: ¿alguna vacía o ambigua?
2. Presenta el borrador del `specs.md` al usuario.
3. Pide aprobación explícita (otro `suspend`).
4. Solo entonces emite el documento final.

## Arquitectura Colmena

- **Nodo central:** `llm_call` conversacional (Gemini 2.5 Flash, Postgres memory).
  - Header `x-agent-session-id` estable en cada turno (obligatorio).
- **Tool `suspend`:** para cada pregunta y para la aprobación final.
- **Skills on-demand** (`skills.paths` + `load_skill`): 1–2 skills genéricas de calidad de specs.
- **Salida:** el `specs.md` como output del grafo (texto).

## Contrato de salida: `specs.md`

Prosa Markdown, headers obligatorios, mismos títulos siempre:

1. **Objetivo** — el QUÉ en lenguaje de negocio.
2. **Disparador / trigger** — qué inicia la tarea (un correo, una hora, un formulario).
3. **Actores y sistemas** — Gmail, CRM X, Excel/Sheet, APIs involucradas.
4. **Procedimiento paso a paso** — la narración completa de lo que hoy se hace a mano.
5. **Datos** — qué entra, qué sale, qué campos.
6. **Conexiones y credenciales** — qué tokens/passwords harán falta.
7. **Criterios de éxito** — cómo sabemos que quedó bien.
8. **Casos borde y errores** — qué hacer cuando algo falla.

## Verificación independiente (con fixture)

- **Fixture:** un guion de respuestas de usuario simuladas para 2–3 tareas distintas
  (ej. "triaje de correos a Sheet", "update de CRM desde formulario").
- **Criterios:**
  - El `specs.md` resultante tiene las 8 secciones, ninguna vacía.
  - No menciona términos técnicos de Colmena.
  - Un revisor humano (o un `llm_call` juez) confirma que un Agente 2 podría planear con él.
- Correr real con `serve`, guardar SSE a `/tmp/colmena_e2e/agente1_*.sse`, presentar reporte.

## Riesgos
- **Entrevista superficial** en dominios que el LLM no conoce → mitigar con el gate de
  completitud y, si hace falta, la librería de arquetipos (backlog).
- **Fuga de lenguaje técnico** → reforzar en el system prompt + verificar en los tests.

## Fuera de alcance
- Playbooks de dominio (backlog).
- Cualquier traducción a nodos (eso es del Agente 2).
