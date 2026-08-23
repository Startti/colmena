# Cambios recientes — 2026-08

> **Alcance:** Commits sobre `develop` desde el cierre de `2026-07`.

## Cómo leer este documento

Una sección por feature. Cada sección contiene:
- **Qué cambió** — efecto observable.
- **Documentación de referencia** — spec, plan, dev guide, schema.
- **Commits** — rango o lista.
- **Estado** — done / partial.

---

## 1. Logging estructurado en nodos del DAG — cierre de finding #30 (payload redaction)

**Qué cambió.** `python_node.rs`, `sql.rs`, `orchestrator.rs`, `extraction.rs`, `reactor.rs` y `llm.rs` dejan de usar `println!`/`eprintln!` fuera de tests, y sus volcados de contenido crudo vía `colmena_log!` pasan al gate doble. Todo el logging ahora pasa por [`tracing`](https://docs.rs/tracing), bajo un namespace de targets documentado y estable:

| Target | Transporta |
|---|---|
| `colmena::python_node` | metadata segura del nodo `python_script` (`code_len`, `sandbox_mode`, `timeout_secs`) |
| `colmena::sql` | metadata segura del nodo `sql_query` (`query_len`, `session_id`, `max_rows`, errores de ejecución) |
| `colmena::orchestrator` | metadata del ciclo del orchestrator (`task_count`, `phase_count`) |
| `colmena::payload::python_code` | código Python crudo — sin truncar |
| `colmena::payload::sql_query` | SQL crudo — sin truncar |
| `colmena::payload::planner_plan` | plan renderizado del orchestrator (texto LLM-autoría) — sin truncar |

**El double-gate.** Emitir contenido bajo `colmena::payload::*` exige **dos condiciones independientes a la vez**: el `EnvFilter` del proceso debe habilitar el target, Y la variable de entorno `COLMENA_LOG_PAYLOADS` debe resolverse a verdadero al arrancar el proceso. Ninguna de las dos por sí sola alcanza — esto protege contra el reflejo operativo de subir `RUST_LOG=trace` para depurar algo no relacionado (no expone nada por accidente) y contra una variable heredada de otro entorno (si el filtro de producción nunca habilita `colmena::payload`, tampoco se expone nada). El guard se implementa en un nuevo módulo `pub(crate)` `dag_engine::log_policy` (`OnceLock`, lazy, self-resolving desde el entorno — no un flag seteado una sola vez al arrancar el CLI, porque el worker de ADP embebe colmena como librería y nunca llama a `main()`), y el macro `payload_trace!` hace estructuralmente imposible que un call site olvide el guard.

**Excepción documentada.** El error de ejecución SQL sigue visible en producción (`RUST_LOG=info` default) como `tracing::warn!` sobre `colmena::sql` — es una condición de error genuina, no payload. La redacción de ese string (que también viaja al LLM vía el `"error"` del output JSON del nodo) queda registrada como ítem separado en el ledger (#61).

**Subscriber wiring.** Ningún módulo dentro de `src/libs/colmena/src/` instala un `tracing_subscriber` ni lee `RUST_LOG` — principio "la librería emite, la aplicación decide". El binario `dag_engine` (`main.rs`) instala su propio `EnvFilter`-based subscriber una única vez antes de despachar cualquier subcomando: `RUST_LOG` gana cuando está definido y no vacío; si no, default `info`; `--verbose` (o `COLMENA_VERBOSE=1`) sube a `colmena=debug` sin habilitar ningún target de payload por sí mismo. `tracing-subscriber` gana la feature `env-filter` en la dependencia principal del crate (antes solo en dev-deps) — esto tiene un efecto colateral aditivo: el binario `attachment_gc` (`fmt::init()`) pasa de un filtro `Targets` a un `EnvFilter` real, sintaxis de directivas más rica sobre el mismo respeto por `RUST_LOG` que ya tenía (no pasa de sordo a oyente, ya escuchaba).

**Delivery.** Tres PRs apilados a `develop` por el forecast de presupuesto de revisión (>400 líneas estimadas para el slice completo):
- **PR 0/3** (#167, squash `321a6ba5`) — solo documentación: `docs/developer_guide/50_logging_and_observability.md` (la guía completa de contrato) + índice.
- **PR 1/3** (#168, squash `178e9d02`) — infraestructura (`log_policy.rs`, guard, macro, targets `python_node`/`python_code`), subscriber en `main.rs`, `COLMENA_VERBOSE` cableado, sitio de mayor severidad migrado (`python_node.rs:211`), test comportamental de cuatro ejes.
- **PR 2/3** (esta entrada) — los 11 sitios de `sql.rs` + el bloque del planner en `orchestrator.rs`, targets `sql`/`orchestrator`/`payload::sql_query`/`payload::planner_plan`, valla de regresión (cero `println!`/`eprintln!`/`print!` fuera de test), fixture E2E, y este changelog.

  **Ampliación durante el review.** La lente de riesgo encontró que la invariante publicada en el dev guide era falsa: `colmena_log!` es un `println!` con una sola compuerta, y varios sitios volcaban contenido crudo del LLM bajo `--verbose`. Se migraron los de mayor severidad — el `prompt` y la respuesta completos de `llm_call`, el prompt/contexto/respuesta de `reactor`, el I/O con el reactor y los agentes en `orchestrator`, y la salida parseada de `extraction` — con dos targets de evento nuevos (`colmena::reactor`, `colmena::llm`) y tres de payload (`payload::agent_io`, `payload::extraction_result`, `payload::llm_io`).

  **Cambio de comportamiento para autores de grafos:** `verbose: true` en un nodo ya **no** alcanza para ver un prompt o una respuesta. Ese contenido ahora exige `RUST_LOG=...colmena::payload::llm_io=trace` más `COLMENA_LOG_PAYLOADS=1`. `verbose` sigue existiendo y ahora emite tamaños, no cuerpos.

  **Lo que queda afuera:** ~23 sitios `colmena_log!` sin gate en `planner.rs` (3), `critic.rs` (3), el system message de entrada de `extraction.rs` (1) y ~16 interpolaciones de `task.task_name` en `orchestrator.rs`. Registrados como finding #64 en el ledger para una PR dedicada — incluirlos habría llevado este slice muy por encima del presupuesto de revisión.

**Verificado en vivo.** `tests/graphs/basic/logging_payload_e2e.json` — un nodo `python_script` con un canary literal encadenado a un nodo `sql_query` con un segundo canary independiente, corrido a través del CLI real (`cargo run --bin dag_engine -- run`) en las 4 combinaciones documentadas:
1. Postura default (`RUST_LOG` sin fijar, guard sin fijar): ningún canary en el stream de tracing.
2. `RUST_LOG=colmena=trace`, guard sin fijar: eventos de flujo visibles (`colmena::python_node`, `colmena::sql`), ningún canary — prueba que el guard es independiente del filtro.
3. `RUST_LOG=colmena=trace` + `COLMENA_LOG_PAYLOADS=1`: ambos canaries recuperables bajo sus targets documentados.
4. `--verbose` (`colmena=debug`): eventos de flujo a paridad con el comportamiento previo, ningún canary.

**Calibración de ruido del CLI.** Con el subscriber activo por primera vez a `info`, la corrida default mostró ~20 líneas (en su mayoría avisos de migración de `sqlx` que ya existían, más `engine_started`/`engine_shutdown`) — no se consideró ruido excesivo; el default de `init_tracing` se mantiene en `info` (no se bajó a `warn`).

**Qué NO cubre este cambio (registrado, no implementado).** Dos canales de exposición distintos, filados como follow-ups en el ledger:
- **#61** — el error de SQL (`sql.rs`) llega tanto al log (`warn!`, intencionalmente visible) como al LLM vía el `"error"` del output JSON del nodo; redactar solo el log sería un cierre falso.
- **#62** — el frame SSE `node-start` incluye `config`/`inputs` verbatim, así que el cuerpo de un `python_script` (o la query de un `sql_query`) sigue viajando por ese canal aunque el stream de logs ya no lo muestre. Verificado que NO llega a Cloud Logging (el worker de ADP no imprime eventos a stdout).
- **#63** — handoff cross-repo: `deploy_gcp.sh` en `apps/service/ia/platform/` (repo de ADP) necesita `RUST_LOG=colmena=trace` + `COLMENA_LOG_PAYLOADS=1` para `develop`, más el barrido del worker exigido por la disciplina de breaking-changes del proyecto — trabajo explícitamente fuera de este repo.

**Documentación de referencia.**
- Guía: [`docs/developer_guide/50_logging_and_observability.md`](developer_guide/50_logging_and_observability.md) — contrato completo (taxonomía de targets, matriz por entorno, límite honesto de la garantía).
- Ledger: [`docs/agent_context/audit/FINDINGS_LEDGER.md`](agent_context/audit/FINDINGS_LEDGER.md) — finding #30 (cerrado) + follow-ups #61/#62/#63.
- Fixture E2E: [`tests/graphs/basic/logging_payload_e2e.json`](../tests/graphs/basic/logging_payload_e2e.json).

**Estado.** done (código). Handoff a ADP (#63) pendiente en el repo de ADP.

---

## 2. Verificación de liveness en el resume de un run anidado — respuesta al handoff de ADP

**Qué cambió.** Nada del motor. Es una verificación E2E que cierra un reporte de ADP («`Stream
timeout: no events received in 60s` al reanudar un run suspendido de tres niveles») y deja los
grafos que la reproducen dentro del repo, para que la próxima vez que aparezca el síntoma la
medición sea un comando y no una investigación.

**Qué se midió.** Dos grafos nuevos reproducen la forma reportada — `creator` → `adp_resources` →
`db_connection_specialist`, tres `llm_call` con `connection_url`, el especialista con la
confirmación humana plegada como tool (`node_type: "suspend"`) más una tool de borrado real
(`python_script`):

- [`tests/graphs/advanced/nested_resume_liveness_e2e.json`](../tests/graphs/advanced/nested_resume_liveness_e2e.json)
- [`tests/graphs/advanced/nested_resume_liveness_slow_e2e.json`](../tests/graphs/advanced/nested_resume_liveness_slow_e2e.json)
  — misma forma, con dos diferencias deliberadas: la tool de borrado duerme 70 s (para forzar un
  silencio profundo por encima del umbral de 60 s) y **no** fija `sandbox_mode: restricted`, porque el
  modo restringido no permite importar `time` y además impone un `sandbox_timeout_secs` de 10 s que
  cortaría el sleep antes de tiempo

Corridos con `dag_engine run`, que emite por el mismo `SseMapper` que usa el worker de ADP, y
midiendo el silencio entre frames consecutivos (que es exactamente lo que cuenta el watchdog de
ADP):

| Escenario | Frames | Duración | Máx. silencio |
|---|---|---|---|
| Run inicial, 3 niveles | 30 | 31.8 s | 5.4 s |
| Resume | 40 | 32.0 s | 5.1 s |
| Resume con 70 s de silencio forzado en el agente más profundo (`level=4` en el SSE) | 49 | 103.5 s | **20.0 s** |
| Resume con `COLMENA_HEARTBEAT_INTERVAL_SECS=1` | 80 | 34.9 s | 1.0 s |

Los conteos de frames y las duraciones son de una corrida concreta y no son bit-reproducibles (dependen
de cuántos tokens emita el modelo); la columna estructural es la del silencio máximo.

**Resultado.** El heartbeat sostiene el resume anidado: un silencio de 70 s en el agente más profundo (`level=4` en el SSE) se
convierte en tres huecos de 20 s cerrados por frames `status` emitidos desde el nodo raíz. Se
confirmó, eso sí, que los tres relojes de liveness se inicializan dentro del bloque que envuelve
`node_impl.execute(...)`, de modo que el arranque de un run no está cubierto — medido con el
heartbeat en 1 s, son **1.1 s** (1.1–2.4 s entre corridas) sin un solo frame en un resume de tres niveles. Esa ventana no
escala con el anidamiento (la re-entrada a los subgrafos ocurre dentro del nodo, con el reloj ya
corriendo) y el worker de ADP ya la cubre con su propio keepalive de 20 s, así que no se cambió el
motor.

**Documentación de referencia.**
- Respuesta al handoff: [`docs/superpowers/handoff/2026-08-22-respuesta-stream-timeout-resume-anidado.md`](superpowers/handoff/2026-08-22-respuesta-stream-timeout-resume-anidado.md)
- Liveness de dos relojes: `CHANGELOG_2026-07.md` §Fase E (PR #146)

**Estado.** done (verificación). Sin cambios de código; el endurecimiento «arrancar el reloj al
aceptar el job» queda anotado y no implementado, porque no explica el síntoma reportado.

---

## 3. Panic al compactar el historial cuando el mensaje más nuevo excede el presupuesto — fix + respuesta a ADP

**Qué cambió.** `recent_boundary_by_tokens` (`history_compaction.rs`) devolvía `messages.len()` — un
índice fuera de rango — cuando el mensaje más nuevo por sí solo ya excedía el presupuesto de
~2.500 tokens de la ventana de recientes: el acumulador inicial (`b = messages.len()`) nunca se
reasignaba porque el loop cortaba en la primera iteración, antes de llegar a `b = i`.
`build_compacted_messages` indexaba ese resultado sin chequear el límite y el proceso panicaba con
`index out of bounds`. **Fix:** la función ahora clampa su resultado
(`b.min(messages.len().saturating_sub(1))`): nunca vuelve a devolver un índice fuera de rango, ni
siquiera con historial vacío, garantizado por una nueva sweep combinatoria
(`recent_boundary_is_always_a_valid_index`, n×size×budget×shape) que cubre casos que los dos tests
de reproducción originales no alcanzaban (p.ej. `n=1, budget=0`). El clamp bounda el **índice** de
la ventana, no el contenido: cuando el mensaje más nuevo por sí solo agota el presupuesto, la
ventana de recientes degenera a exactamente ese mensaje y viaja **verbatim, sin importar el rol**
(`user`, `assistant` o `tool`) — ver "Dónde se corta el historial: la interacción abierta" en
[`15_memory_guide.md`](developer_guide/15_memory_guide.md) (sección reemplazada en §4 de este mismo
changelog: el borde pasó de presupuesto de tokens a estructural).

**No es específico de "resume".** El reporte de ADP asumía que el disparador era "reanudar un run
suspendido". Es el disparador más frecuente en producción, pero no el único: un prompt de usuario
pegado de más de ~10.000 caracteres dispara el mismo panic en un turno normal, sin ningún resume de
por medio. Dos tests de regresión cubren ambos casos
(`repro_adp_panic_last_content_message_alone_exceeds_budget`,
`repro_panic_also_fires_on_a_large_user_prompt`), y el E2E de ruta limpia descrito más abajo lo
demuestra en vivo: el mismo turno ordinario aborta contra el binario pre-fix y completa contra el
binario con el fix.

**Qué se midió.**
- **14 tests unitarios** en `history_compaction.rs`: la sweep de contrato
  `recent_boundary_is_always_a_valid_index`; los dos tests de reproducción del panic
  (`repro_adp_panic_last_content_message_alone_exceeds_budget`,
  `repro_panic_also_fires_on_a_large_user_prompt`); la propiedad de costo acotado a un solo turno
  (`oversized_message_leaves_the_recent_window_on_the_next_turn`); los tests de verbatim por rol
  (`oversized_newest_user_prompt_stays_verbatim`, `oversized_newest_assistant_stays_verbatim`);
  `recent_window_is_never_empty`; el pin de la salida cruda con tool calls en paralelo
  (`parallel_tool_calls_with_oversized_last_result_ship_the_history_raw`); y los tests
  preexistentes de clasificación/digest/summarización — todos pasan.
- **E2E en vivo** contra Gemini 2.5 Flash + Postgres real — y es también la prueba directa de que el
  panic **no** es específico de "resume":
  [`history_compaction_oversized_prompt_turn1.json`](../tests/graphs/agents/history_compaction_oversized_prompt_turn1.json)
  + [`history_compaction_oversized_prompt_turn2.json`](../tests/graphs/agents/history_compaction_oversized_prompt_turn2.json).
  Dos corridas ordinarias del CLI con el mismo `--agent-session-id`, **sin sembrar filas en la DB,
  sin `suspend` y sin matar ningún proceso**; el turno 2 es sólo un prompt pegado de 11.053
  caracteres. Contra el binario **pre-fix** el turno 2 aborta con
  `index out of bounds: the len is 6 but the index is 6` en `history_compaction.rs:143:46` y
  `exit code 101`. Contra el binario **con el fix** el mismo turno termina con
  `finishReason: "stop"`. Con `COLMENA_DUMP_PROMPT_FULL=1` sobre la corrida arreglada, el wire
  queda `[user viejo, system+resumen, prompt de 11.124 chars VERBATIM]`: el `user` más nuevo sale
  completo, sin truncar.

**Documentación de referencia.**
- Guía: [`docs/developer_guide/15_memory_guide.md`](developer_guide/15_memory_guide.md) §Compactación
  → "Dónde se corta el historial: la interacción abierta" (sección renombrada en §4 de este mismo
  changelog: el borde pasó de presupuesto de tokens a estructural — el mecanismo descrito aquí abajo
  ya no existe en el código).
- Código (histórico — reemplazado en §4): `recent_boundary_by_tokens` ya no existe;
  `src/libs/colmena/src/llm/application/history_compaction.rs` usa `current_interaction_start`.
- Grafos E2E: [`history_compaction_oversized_prompt_turn1.json`](../tests/graphs/agents/history_compaction_oversized_prompt_turn1.json)
  + [`history_compaction_oversized_prompt_turn2.json`](../tests/graphs/agents/history_compaction_oversized_prompt_turn2.json)
  — se corren en ese orden con el mismo `--agent-session-id`, sin sembrar nada en la base.

**Respuesta al handoff de ADP.**

1. **Confirmado, con reproducción.** El panic existe y es real: `recent_boundary_by_tokens` devolvía
   un índice igual a `messages.len()` (fuera de rango) cuando el mensaje más nuevo por sí solo
   excedía el presupuesto de recientes, y `build_compacted_messages` lo indexaba sin chequear el
   límite. Reproducido con dos tests deterministas y con un run en vivo contra Postgres real (ver
   arriba).
2. **No es específico de "resume".** Corrige la lectura del reporte: reanudar un run suspendido es
   el camino más frecuente para llegar a este estado (el mensaje más nuevo persistido queda siendo
   el resultado de una tool grande, sin respuesta de seguimiento todavía), pero no es el único. Un
   usuario pegando un prompt de más de ~10.000 caracteres en un turno normal, sin ningún suspend de
   por medio, dispara el mismo panic — el mecanismo depende únicamente del tamaño del mensaje más
   nuevo, no de si el turno viene de un resume.
3. **Sobre "un panic tumba el worker entero".** Colmena no define `panic = "abort"` en ningún
   perfil de `Cargo.toml`, y no instala ningún `catch_unwind` alrededor de la ejecución de nodos.
   Con el perfil `unwind` de Rust (el default, que Colmena no sobrescribe), un panic dentro de una
   tarea de Tokio se propaga como un `JoinError` de esa tarea — no mata el proceso por sí solo. Si
   el binario de ADP sí termina el proceso entero ante ese panic, eso depende de decisiones propias
   de su binario (perfil de compilación, un panic hook que llame `abort()`, cómo se manejan los
   `JoinError`, etc.) que no podemos verificar leyendo el repo de Colmena. No hacemos ninguna
   afirmación sobre eso, ni ninguna recomendación operativa sobre su perfil de panics o su política
   de reinicio.

**Estado.** done. Fix + tests + E2E en vivo + documentación en el mismo cambio.

---

## 4. La frontera de compactación pasa de presupuesto de tokens a estructural — `current_interaction_start`

**Qué cambió.** El fix de §3 clampaba el índice para que `recent_boundary_by_tokens` nunca panicara,
pero dejaba en pie el defecto de fondo que ese mismo mecanismo tenía: decidir el borde por
presupuesto acumulado (`RECENT_TOKEN_BUDGET` = ~2.500 tokens) no sabe qué mensajes pertenecen a la
MISMA interacción que el más nuevo. Cuando esa interacción incluía un resultado de `tool`
sobredimensionado, el borde podía aterrizar **entre** la pregunta que disparó la tool call y su
resultado, resumiendo la pregunta mientras el resultado viajaba verbatim al lado — el propio modelo
se quedaba sin ver qué había preguntado.

Este cambio reemplaza el mecanismo entero por uno estructural: `current_interaction_start`
(`history_compaction.rs`) escanea el historial hacia atrás y devuelve la posición justo después del
**último `assistant` que respondió sin `tool_calls`** — la propia condición de salida del loop ReAct
de `agent_service` (`agent_service.rs:353` `if tool_calls.is_empty()`, `return` en `:359` para
`Some(vec![])`, `return` en `:676` para `None`). Un `assistant` persistido sin `tool_calls` es, por
construcción, el cierre de una interacción; todo lo que viene después sigue en curso y viaja
**verbatim, sin importar tamaño ni rol**. `RECENT_TOKEN_BUDGET`, `recent_boundary_by_tokens`,
`est_tokens` y el guard de pares que compensaba su punto ciego (retroceder el borde sobre cada
`tool` consecutivo) quedan eliminados — ya no hacen falta: el borde nuevo aterriza siempre en el
primer mensaje de una interacción, que nunca puede ser un `Tool` huérfano de su `Assistant`.

**El costo, dicho con honestidad — en las dos direcciones.** No hay tope de tamaño en la interacción
abierta, y esa interacción **no está acotada a un turno**: un `suspend` no la cierra (la ruta de
suspensión retorna en `agent_service.rs:500` sin persistir ningún `assistant` sin `tool_calls`), así
que sigue abierta a través de todos los runs de resume que hagan falta. Mientras siga abierta, cada
resultado de `tool` grande que contenga viaja completo en cada resume, y en el caso límite —nada
cerró todavía, `b = 0`— la historia entera viaja cruda, sin compactar, en cada run: el shape que
Colmena documenta para un agente HITL de varios pasos dirigido enteramente vía `suspend`. Un agente
así con un par de resultados grandes puede alcanzar el techo de contexto del proveedor con un error
duro, donde el presupuesto viejo degradaba en su lugar.

El otro lado, menos obvio: al arrancar un turno nuevo ordinario, la ventana verbatim es de
exactamente un mensaje (el prompt recién persistido, en `len-1`, justo después del cierre anterior
en `len-2`). Una respuesta previa del agente de más de `SUMMARY_SKIP_THRESHOLD_CHARS` (250 chars) ya
no tiene el margen que daba el viejo presupuesto de ~2.500 tokens y pasa a resumen semántico de
~250 chars un turno antes de lo que pasaba con el mecanismo eliminado.

Detalle completo, costos y casos borde: [`15_memory_guide.md`](developer_guide/15_memory_guide.md)
§Compactación → "Dónde se corta el historial: la interacción abierta". Es una decisión de diseño
deliberada, no un descuido: resumir por presupuesto es exactamente el mecanismo que causaba el
defecto de fondo. El mapa para acotar el peso de un resultado estructurado sobredimensionado sin
resumir la pregunta que lo disparó queda para un cambio aparte (ver "What this plan does NOT do" en
el plan de referencia, abajo).

**Qué se midió.**
- **`cargo test --verbose`**: 2.365 tests de la lib (2.294 passed, 71 ignored — requieren
  `DATABASE_URL`/API keys, se corren aparte con `--ignored`, 0 failed), incluyendo **16 tests** en
  `llm::application::history_compaction::tests` (todos pasan): la detección del borde
  (`interaction_start_is_after_the_last_assistant_without_tool_calls`,
  `interaction_start_uses_the_last_close_not_the_first`,
  `an_assistant_with_an_empty_tool_call_vec_also_closes`,
  `several_unanswered_user_messages_all_belong_to_the_open_interaction`,
  `without_a_closed_interaction_everything_is_current`,
  `a_closing_assistant_as_the_newest_message_leaves_nothing_open`), el caso que motivó este cambio
  (`the_current_question_survives_next_to_an_oversized_tool_result`) y la ventana nunca vacía
  (`recent_window_is_never_empty`, `a_closed_newest_interaction_still_leaves_a_recent_message`).
  `cargo fmt --check` y `cargo clippy --all-targets` limpios (el crate corre con
  `warnings = "deny"`).
- **E2E en vivo** contra Gemini 2.5 Flash + Postgres real, dos corridas del CLI con el mismo
  `--agent-session-id` y `COLMENA_DUMP_PROMPT_SIZES=1 COLMENA_DUMP_PROMPT_FULL=1` (el segundo env var
  por sí solo no imprime nada — el dump completo está anidado dentro del chequeo del primero, un
  detalle que no estaba documentado):
  [`interaction_boundary_e2e_turn1.json`](../tests/graphs/agents/interaction_boundary_e2e_turn1.json)
  + [`interaction_boundary_e2e_turn2.json`](../tests/graphs/agents/interaction_boundary_e2e_turn2.json).
  El turno 1 pide un conteo (una tool call + respuesta final que **cierra** la interacción,
  `finishReason: "stop"`, `"Hay 5 transacciones en total."`). El turno 2 pide el listado completo:
  la tool `listar_transacciones` devuelve 300 filas (15.363 chars de JSON crudo). En el dump del
  último iter del turno 2 (`n_msgs=5`), el wire queda `[user T1 (155ch, kept_first), System
  fusionado con el resumen de T1 (996ch), user T2 (234ch, VERBATIM — la pregunta completa "Ahora
  necesito el detalle completo de todas las transacciones…"), assistant+tool_calls T2 (309ch),
  Tool T2 (17.892ch, VERBATIM — las 300 filas completas, sin digest ni truncar)]`. Ni la pregunta
  del turno 2 ni el resultado de la tool aparecen como línea `[Tn]` dentro del bloque `## Conversation
  summary`: ese bloque solo cubre los índices `keep_first..b` (el `tool` y el `assistant` de cierre
  del turno 1), nunca los de la interacción abierta — la propia longitud del mensaje `System`
  (996ch) es demasiado chica para contener los 17.892ch del resultado. `finishReason: "stop"` en
  ambos turnos.

**Documentación de referencia.**
- Guía: [`docs/developer_guide/15_memory_guide.md`](developer_guide/15_memory_guide.md) §Compactación
  → "Dónde se corta el historial: la interacción abierta".
- Código: `src/libs/colmena/src/llm/application/history_compaction.rs` —
  `current_interaction_start`, `build_compacted_messages`.
- Plan de referencia:
  [`docs/superpowers/plans/2026-08-22-interaction-boundary.md`](superpowers/plans/2026-08-22-interaction-boundary.md).
- Grafos E2E:
  [`interaction_boundary_e2e_turn1.json`](../tests/graphs/agents/interaction_boundary_e2e_turn1.json)
  + [`interaction_boundary_e2e_turn2.json`](../tests/graphs/agents/interaction_boundary_e2e_turn2.json)
  — se corren en ese orden con el mismo `--agent-session-id`, sin sembrar nada en la base.

**Estado.** done. Cambio estructural + 16 tests unitarios + E2E en vivo + documentación en el mismo
cambio.

---

## 5. Tool calls huérfanas cuando un `suspend` corta un batch paralelo

**Qué cambió.** Cuando un `llm_call` emite **varias tool calls en un mismo turno**
(batch paralelo) y una de ellas se suspende para preguntarle al humano, las
llamadas ordenadas *después* de la que suspendió ya no quedan sin resultado: cada
una recibe un marcador honesto que dice, en texto que lee el modelo, que **no se
ejecutó**.

**El defecto.** El branch `SUSPENDED` de `agent_service.rs` hace `return` desde el
medio del `for tool_call in tool_calls`, así que las llamadas restantes nunca se
ejecutan. Pero el mensaje del asistente —persistido *antes* del loop— ya declaró
los N ids. En el resume, `find_pending_tool_call` reproduce **solo el primer** id
sin resolver, y el resto queda huérfano para siempre.

Anthropic y OpenAI validan el emparejamiento: un turno de asistente que declara un
id sin su resultado es un **400 duro**, no una respuesta degradada. La
conversación queda inutilizable de forma permanente:

```
messages.2: `tool_use` ids were found without `tool_result` blocks immediately
after: toolu_01A7MjfvpYUMQdtCzZGq7GJZ. Each `tool_use` block must have a
corresponding `tool_result` block in the next message.
```

El 400 de Anthropic está **verificado en vivo** contra la API real. OpenAI impone la
misma regla de emparejamiento por contrato de API, pero eso **no se re-verificó
empíricamente aquí** (la key de prueba no tenía crédito). Gemini es permisivo y
enmascara el fallo, por eso no se veía en los grafos que usan el stack por defecto. La población expuesta son los agentes HITL sobre Anthropic u
OpenAI — y como la forma de agente documentada en Colmena canaliza *toda* la
interacción con el usuario por nodos `suspend`, no es un caso de borde exótico.

Es un defecto **preexistente**. No lo introdujo ni lo agravó el cambio de borde de
compactación (PR #174 / `claude/interaction-boundary`): bajo el pair-guard anterior
la llamada huérfana viajaba por el cable exactamente igual.

**Por qué se eligió el marcador y no ejecutar las llamadas restantes.** Ejecutar
las que faltan antes de devolver `SUSPENDED` invierte la garantía que el `suspend`
existe para imponer. Un batch como `[preguntar("¿borro la base de producción?"),
borrar_base()]` ejecutaría el borrado **antes** de que el humano conteste. La otra
alternativa —resolver todas las pendientes en el resume— tiene el mismo problema
corrido en el tiempo: los efectos secundarios se disparan igual, ahora en diferido,
y el modelo nunca tiene ocasión de reconsiderar a la luz de la respuesta.

El marcador no ejecuta nada. Le dice la verdad al modelo y le devuelve la decisión:
si después de leer la respuesta del humano todavía quiere esa llamada, la vuelve a
emitir. El costo es, como mucho, un turno extra.

El id **que suspendió** se deja deliberadamente abierto: el resume lo localiza
precisamente por esa ausencia de resultado.

**Sesiones ya rotas.** El arreglo principal no puede rescatar una conversación que
un build anterior ya dejó con un id huérfano —el historial está escrito—, y esa
conversación devuelve 400 en cada turno siguiente para siempre. Por eso el camino
de resume en `llm.rs` cierra también, con el mismo marcador, cualquier id que
encuentre sin resolver junto al que acaba de reproducir. Sobre un historial escrito
por el build actual ese barrido no encuentra nada: cuesta un escaneo y cero
escrituras.

**Sin cambios de formato de wire.** No se agregan eventos SSE: la llamada que nunca
se ejecutó tampoco había emitido `tool-input-available`, así que ningún frontend
tenía un spinner colgado esperándola. El cambio vive enteramente en el historial de
conversación → **ADP no se ve afectado**.

**Verificación.** Reproducido en vivo contra Anthropic real antes de tocar nada, con
[`tests/graphs/agents/parallel_tool_suspend_orphan.json`](../tests/graphs/agents/parallel_tool_suspend_orphan.json).
Dos hallazgos empíricos que conviene no re-descubrir:

- El **orden** del batch lo elige el modelo, no el prompt. Pidiéndole explícitamente
  que pusiera el `suspend` primero, Claude lo puso último — que es justo el caso
  benigno. El repro determinista son **dos tools respaldadas por `suspend`** en el
  mismo batch: la que se ejecute primero deja huérfana a la otra, salga en el orden
  que salga.
- Anthropic **no exige que el orden de los `tool_result` coincida** con el de los
  `tool_use` declarados; solo exige que estén todos. Verificado en vivo: declarado
  `[ask_email, ask_city]`, enviado `[ask_city, ask_email]` → 200.

**Estado.** done.

---
