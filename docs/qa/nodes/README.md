# QA — Auditoría por nodo (documentación vs código)

Un archivo por `node_type` registrado. Cada uno compara lo que la documentación
canónica afirma contra lo que el código realmente hace, y entrega un plan de
pruebas accionable para el equipo QA.

Cada `<node>.md` tiene tres secciones:

1. **Config documentada NO soportada por el código** — lo que la doc describe pero el código no implementa/valida distinto.
2. **Código NO documentado** — campos, outputs, errores fail-closed o comportamientos presentes en el código y ausentes en la doc.
3. **Plan de pruebas QA** — cómo probar cada configuración distinta (objetivo, grafo/comando, entrada, resultado esperado, verificación).

> **Vista consolidada:** [`RESUMEN_GAPS.md`](RESUMEN_GAPS.md) — todos los gaps de las 37 fichas, priorizados por severidad (Alta verificada contra el código) + índice por archivo de doc a tocar.
>
> Convenciones de ejecución y fuentes canónicas: ver [`_INSTRUCCIONES.md`](_INSTRUCCIONES.md).
>
> **Nota para QA:** los grafos `tests/graphs/**.json` citados en la sección 3 son
> **plantillas a crear** por el equipo — todavía no existen en el repo. Son el
> punto de partida de cada prueba, no archivos ya versionados.

## Índice

### Aritmética / utilidad
- [`add`](add.md) · [`subtract`](subtract.md) · [`multiply`](multiply.md) · [`divide`](divide.md) · [`exponential`](exponential.md) · [`log`](log.md)
- [`current_time`](current_time.md) · [`input`](input.md) · [`output`](output.md) · [`mock_input`](mock_input.md) · [`trigger_webhook`](trigger_webhook.md)

### LLM / agentes
- [`llm_call`](llm_call.md) · [`information_extraction`](information_extraction.md) · [`output_parser`](output_parser.md)
- [`orchestrator`](orchestrator.md) · [`planner`](planner.md) · [`critic`](critic.md) · [`reactor`](reactor.md) · [`router`](router.md)

### Control de flujo / anidamiento
- [`for_each`](for_each.md) · [`loop_controller`](loop_controller.md) · [`subgraph`](subgraph.md)

### HITL / estado
- [`suspend`](suspend.md) · [`secure_suspend`](secure_suspend.md) · [`task_memory_writer`](task_memory_writer.md)

### Integraciones externas
- [`http_request`](http_request.md) · [`socketio_request`](socketio_request.md) · [`sql_query`](sql_query.md) · [`python_script`](python_script.md)
- [`api_explorer`](api_explorer.md) · [`tavily_client`](tavily_client.md)

### Documentos
- [`document_create`](document_create.md) · [`document_edit`](document_edit.md) · [`document_read`](document_read.md)

### Multimedia
- [`image_generation`](image_generation.md) · [`image_edit`](image_edit.md) · [`tts`](tts.md)

