# Graph Builder — un agente que crea grafos

Un grafo de Colmena cuyo agente conversacional ayuda a **personas sin conocimientos
de programación** a construir **otros grafos de Colmena**. La persona habla en lenguaje
de capacidades ("quiero que una IA conteste preguntas y busque en internet"), nunca en
términos técnicos. El agente entrevista, entiende el objetivo, arma el grafo, **lo
ejecuta de verdad para verificar que funciona**, y entrega el JSON listo para usar.

## Requisitos

- Variables de entorno (en el `.env` del repo): `GEMINI_API_KEY` y `DATABASE_URL` (Postgres).
- Cargá el entorno antes de correr: `set -a; source .env; set +a`.

## Cómo levantarlo

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- serve tests/graphs/agents/graph_builder/graph_builder.json
# Servidor en http://localhost:3000
```

## Cómo hablarle

> ⚠️ **IMPORTANTE — memoria conversacional.** En modo `serve`, la memoria se keya por
> `agent_session_id`, **no** por el `session_id` del grafo. Tenés que mandar el header
> **`x-agent-session-id` con un valor estable en CADA turno** de la misma conversación.
> Si no lo mandás, cada mensaje arranca una conversación nueva (el agente "se olvida"
> de lo anterior) — este es el error más común al probarlo.

```bash
# Turno 1
curl -s -X POST http://localhost:3000/chat \
  -H "Content-Type: application/json" \
  -H "x-agent-session-id: mi_charla_001" \
  -d '{"message":"quiero algo que conteste preguntas de mis clientes"}'

# Turno 2 (mismo x-agent-session-id)
curl -s -X POST http://localhost:3000/chat \
  -H "Content-Type: application/json" \
  -H "x-agent-session-id: mi_charla_001" \
  -d '{"message":"que conteste en español y sea amable"}'
```

Para una conversación nueva e independiente, usá otro valor de `x-agent-session-id`.

## Qué capacidades entiende (v1)

El agente nunca te pide términos técnicos; razona en estas capacidades:

- Que una IA responda, escriba, resuma o transforme texto.
- Buscar información en internet.
- Traer datos de un servicio o API externa.
- Pausar para pedirte un dato o una decisión.
- Crear o editar una imagen.
- Generar audio o voz a partir de texto.
- Trabajar con hojas de cálculo o documentos (entiende jerga: "Excel", "planilla",
  "Word"…) y desambigua online vs archivo descargable.
- Consultar o guardar datos en una base de datos.
- Hacer un cálculo o transformación de datos a medida.
- Decidir un camino distinto según el caso.

## Cómo funciona por dentro

1. **Entrevista** en lenguaje de persona (entiende el objetivo, una pregunta a la vez).
2. **Propone el flujo en palabras** y pide confirmación.
3. **Carga skills** on-demand (`tests/graphs/agents/graph_builder/skills/`) con el detalle
   de cada familia de capacidad.
4. **Arma el grafo** y lo **prueba de verdad** con la herramienta `probar_grafo`
   (un nodo `subgraph` que ejecuta el grafo borrador y devuelve el resultado real).
5. **Corrige** hasta que funciona y **entrega** el JSON + un resumen en lenguaje simple.

Antes de probar grafos con efectos reales (escribir en una API/base de datos, enviar
mensajes), el agente **avisa y pide confirmación**.

## Limitaciones conocidas y backlog

- **Multimedia** (imagen/voz): esos nodos solo se registran si hay un storage adapter
  configurado (`COLMENA_LOCAL=true` en local). Si no, esa capacidad no está disponible.
- **Google Sheets/Docs**: requieren las variables OAuth `COLMENA_GOOGLE_OAUTH_*` en el
  entorno del motor.
- **Cobertura curada**: v1 cubre el set anterior. Quedan en backlog capacidades
  avanzadas (orquestación multi-agente, loops cíclicos, subgrafos de usuario, socket.io,
  CRDT, documentos versionados, math, secretos).
- **`guardar_grafo`** (escribir el grafo final a un archivo automáticamente): backlog.
  Por ahora el grafo se entrega en el chat para copiar/pegar.
- **Migración a ADP** (canvas): pendiente; v1 es standalone en Colmena.

## Referencias

- Diseño: [`docs/superpowers/specs/2026-06-25-graph-builder-agent-design.md`](../../../../docs/superpowers/specs/2026-06-25-graph-builder-agent-design.md)
- Plan de implementación: [`docs/superpowers/plans/2026-06-25-graph-builder-agent.md`](../../../../docs/superpowers/plans/2026-06-25-graph-builder-agent.md)
