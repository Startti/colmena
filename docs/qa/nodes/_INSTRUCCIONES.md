# Instrucciones para auditoría doc-vs-código por nodo (equipo QA)

Cada archivo `docs/qa/nodes/<node_type>.md` audita UN node_type comparando lo que
la documentación afirma contra lo que el código realmente hace, y entrega un plan
de pruebas accionable para QA.

## Fuentes canónicas de documentación (verdad para el equipo y los agentes)
- `docs/node_configurations.json` → esquema de configuración de cada nodo (campos, tipos, defaults).
- `docs/node_as_tools_reference.json` → cómo se usa el nodo como tool del LLM.
- `docs/agent_context/node_ports_reference.md` → puertos y outputs por nodo.
- `docs/DEVELOPER_GUIDE.md` y `docs/developer_guide/*.md` → guías por tema.
- Registro de texto LLM: `src/libs/colmena/text/` (prompts/descripciones que ve el modelo).

## Fuente de verdad del código
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/<archivo>.rs` (implementación `ExecutableNode`).
- El campo que valida config, el método `execute`, el `schema()`/outputs y cualquier
  validación fail-closed son la verdad. La documentación que contradiga el código es un hallazgo.

## Estructura obligatoria de cada `<node>.md`

```
# QA — Nodo `<node_type>`

Fuente de código: `ruta/al/archivo.rs`
Fuentes de doc revisadas: <lista>

## 1) Config documentada NO soportada por el código
(Campos/valores/comportamientos que la doc describe pero el código no implementa,
ignora, o valida distinto. Cada hallazgo: qué dice la doc → qué hace el código →
impacto para QA. Si no hay, decir "Sin discrepancias detectadas".)

## 2) Código NO documentado
(Campos de config, outputs, valores aceptados, errores fail-closed, o comportamientos
presentes en el código pero ausentes/incompletos en las 4 fuentes de doc. Cada hallazgo
con referencia al archivo:línea.)

## 3) Plan de pruebas QA
(Cómo probar CADA configuración distinta del nodo. El objetivo: que QA arranque a probar
sin leer el código. Incluir por cada caso:
  - Objetivo de la prueba
  - Grafo/JSON mínimo o comando `cargo run --bin dag_engine -- run <graph.json>`
  - Entrada / prompt
  - Resultado esperado (output esperado, evento SSE, o error esperado)
  - Cómo se verifica pass/fail
Cubrir: happy path por campo relevante, defaults, casos límite, y los errores
fail-closed que el código realmente lanza.)
```

Reglas: en español, conciso, factual. No inventes campos: si un campo no está ni en
doc ni en código, no lo menciones. Cita `archivo:línea` para los hallazgos de código.

Convenciones del repo para los grafos de prueba (sección 3):
- Stack LLM por defecto: `provider: "google"`, `model: "gemini-2.5-flash"`. NUNCA uses ids
  de modelo fechados (p.ej. `claude-3-5-sonnet-20241022`) ni `gemini-1.5-flash` (deprecado).
- Usa SIEMPRE `node_type` de nodos registrados reales como backing de tools; nunca `log`
  u otro placeholder como mock.
- Persistencia/estado entre runs (suspend/resume, memoria, secure_values): pasar
  `--agent-session-id <id_estable>`; para pausas, formato `--answer $'Q[<id>]: ...\nA[<id>]: ...'`.
- Ejecutar sin servidor: `cargo run --bin dag_engine -- run <graph.json>` (no requiere puerto).
