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
## 6. El adapter de Anthropic descartaba mensajes `system` de más

**Qué cambió.** `anthropic_adapter.rs::convert_messages` colapsaba todos los mensajes de rol
`system` **sobrescribiendo** (`system_message = Some(...)` dentro del loop), así que de varios
sobrevivía solo el último. Ahora los acumula en orden y `build_request_body` los emite como bloques
separados del campo `system`, con el marker `cache_control: ephemeral` **solo en el primero** — lo
estable queda cacheado y lo volátil fuera del prefijo.

**Latente, no un fallo vivo.** El reporte que originó el cambio afirmaba que toda conversación
compactada perdía el system prompt del agente, vía el segundo `system` que agrega
`history_compaction`. No se reproduce, y la razón está en el orden real del historial:

- El nodo `llm_call` persiste el turno 1 como `[User(prompt), System(secciones)]` — el
  `build_initial_user_message` va primero y el `messages.push(LlmMessage::system(...))` después,
  y solo cuando `!history_exists`. Por eso `messages[keep_first - 1]` **es** un `system`, la rama
  de merge de `build_compacted_messages` corre siempre, y la compactación produce **uno**.
- Los otros cinco llamadores de `AgentService` (`reactor`, `planner`, `critic`, `orchestrator` y
  `util/extract_with_schema`) **sí** arman `[System, User]`, el orden que el reporte asumía — pero
  cada uno usa un `InMemoryConversationRepository` nuevo por invocación con exactamente 2 mensajes,
  así que `build_compacted_messages` sale por su guarda temprana (`total <= keep_first + 1`) y
  nunca compacta. Los salva el historial efímero, no el orden: si alguno pasara a historial
  persistente, la rama del segundo `system` se activaría.

**Por qué se arregla igual.** Varios `system` son legales —`LlmRequest` los admite intercalados— y
los otros dos adapters ya los manejan. La rama que emite un `system` nuevo no se alcanza hoy por
cómo cada llamador arma el historial; cualquier cambio ahí la activa, y el modo de fallo es
silencioso — sin error ni log, el agente deja de seguir sus instrucciones.

**Defecto secundario, misma raíz.** `openai_adapter.rs` inyectaba `volatile_system_suffix` en
**cada** mensaje `system`, duplicando el bloque temporal. Ahora va solo en el **último**, lo que
además alinea el orden con Anthropic y Gemini. Aplica a Chat Completions y Responses API.

**Fuera de alcance.** Como la compactación mergea el resumen DENTRO del `system` estable y ese
resumen se recomputa en cada run, el prefijo cacheado cambia turno a turno: una vez que una
conversación compacta, el prompt caching deja de acertar. Es un cambio de comportamiento en
`history_compaction`, no en el adapter — va en su propio ticket.

**Tests.** TDD, RED reproducido (`left: 1, right: 2` en el conteo de bloques `system`) antes del
GREEN: 3 tests en `anthropic_adapter.rs`, 2 en `openai_adapter.rs`, 1 candado de regresión en
`gemini_adapter.rs`.

**Verificación E2E** (Anthropic real, `claude-opus-5`, con un `--agent-session-id` estable):
[`..._fill.json`](../tests/graphs/agents/anthropic_compaction_system_prompt_fill.json) acumula
historial hasta disparar la compactación y
[`..._probe.json`](../tests/graphs/agents/anthropic_compaction_system_prompt_probe.json) pide un
código que vive **solo** en el system prompt y nunca aparece en un turno visible. Resultado: el
request compactado queda en **3 mensajes con un único bloque `system`**, y el agente devuelve el
código exacto — o sea, el system prompt sigue gobernando después de compactar. Ese run es también
la evidencia de que la ruta de dos `system` no se alcanza hoy.

Para reproducirlo (requiere API real de Anthropic; `unset ANTHROPIC_BASE_URL` si el `.env` local lo
define sin `/v1`, y `unset COLMENA_LOCAL` después de cargarlo):

```bash
for i in $(seq 1 3); do
  cargo run --bin dag_engine -- run \
    tests/graphs/agents/anthropic_compaction_system_prompt_fill.json \
    --agent-session-id orion_compaction_001
done
COLMENA_DUMP_PROMPT_SIZES=1 COLMENA_DUMP_PROMPT_FULL=1 \
  cargo run --bin dag_engine -- run \
    tests/graphs/agents/anthropic_compaction_system_prompt_probe.json \
    --agent-session-id orion_compaction_001
```

Con la frontera estructural de §4 alcanzan tres corridas de `fill`: cada una cierra su interacción
con un `assistant` sin `tool_calls`, así que `current_interaction_start` deja la ventana verbatim en
un solo mensaje y todo lo anterior se resume apenas el borde supera `SUMMARY_KEEP_FIRST_MSGS`. El
dump de la sonda muestra los mensajes exactos del request. La verificación es manual a propósito:
pega contra una API paga y no puede correr en CI.

**Estado.** done.

---

## 7. Prompt caching roto al compactar — el resumen sale del prefijo cacheado

**Qué cambió.** El `## Conversation summary` que produce `history_compaction` ya **no** se fusiona
dentro del `system_message` del agente. Viaja como un mensaje `system` propio, detrás del estable:

```
[ User(prompt inicial), System(prompt estable), System(## Conversation summary), ...recientes ]
```

**El defecto.** El adapter de Anthropic pone el marker `cache_control` en el **primer** bloque
`system`, así que el prefijo cacheado es `tools[] + system_blocks[0]`. El resumen se recomputa en
cada carga del run y crece con la conversación; fusionado dentro de ese bloque, reescribía los bytes
del breakpoint **en cada turno**. Efecto: apenas una conversación compactaba, el prompt entero del
agente se re-escribía a la tarifa de cache-write (1.25×) turno tras turno y no se leía nunca de
vuelta. No era degradación silenciosa de un ahorro: era gasto extra sobre el precio completo.

**Dos cambios, ambos necesarios.**

1. `history_compaction::build_compacted_messages` deja de mergear el resumen en el `system` previo.
2. `LlmRequest::new` exime a `System` de `coalesce_consecutive_same_role`, igual que ya eximía a
   `Tool`. **Sin esto el fix (1) es invisible**: el coalescer del dominio volvía a fusionar los dos
   `system` antes de que el adapter viera nada, y la medición E2E no se movía ni un token. El
   coalescer existe para normalizar la alternancia user/assistant que exigen los providers; los
   `system` nunca participan de esa alternancia (Anthropic los iza a `system[]`, Gemini a
   `systemInstruction`, OpenAI acepta consecutivos).

**Medición E2E contra Anthropic real** — `tests/graphs/agents/prompt_cache_compaction_measure.json`,
Sonnet 5, 5 turnos sobre el mismo `--agent-session-id`, prefijo estable de 3029 tokens:

| Turno | Antes — write / read | Después — write / read |
|---|---|---|
| 1 | 3029 / 3029 | 3029 / 3029 |
| 2 | 3457 / 3457 | **0** / 6058 |
| 3 | 3829 / 3829 | **0** / 3029 |
| 4 | 4260 / 4260 | **0** / 3029 |
| 5 | 4684 / **0** | **0** / 3029 |

Antes, `cache_creation_input_tokens` crecía exactamente al ritmo del resumen (3029 → 4684) y se
pagaba entero en cada turno. Después, la escritura desaparece a partir del turno 1, la lectura se
estabiliza en el tamaño del prefijo estable, y el resumen viaja como input normal (visible en el
`promptTokens` creciente). Los turnos con lectura duplicada (6058) son runs de dos iteraciones del
loop ReAct: las dos aciertan.

**Cómo se aisló.** La medición sola no bastaba: tras el fix (1) la escritura seguía creciendo. Una
sonda directa contra la Messages API — mismo shape que el request real, con `tools[]` marcado y tres
bloques `system` — confirmó que el marker en el bloque 0 **sí** aísla el prefijo (el bloque 1 podía
cambiar y crecer sin perder el `cache_read`). Eso descartó al adapter y a la API, y dejó al
coalescer del dominio como único candidato: el dump de `COLMENA_DUMP_PROMPT_SIZES` mostraba dos
`system`, pero `LlmRequest::new` los fusionaba después del dump.

**Alcance: solo Anthropic.** Medido en los tres providers (2026-08-23) — **OpenAI y Gemini nunca
estuvieron afectados y no ganan nada con este cambio.** La redacción original de esta sección decía
lo contrario sobre OpenAI; era una inferencia de lectura de código, no una medición, y resultó falsa.

La diferencia es la **granularidad del match**. El breakpoint de Anthropic es *por bloque*:
`cache_control` sobre `system_blocks[0]` cachea hasta ese límite y exige que los bytes del bloque
matcheen **exacto**, así que pegarle un resumen creciente destruye el acierto. OpenAI y Gemini
matchean un prefijo **dentro** de un string más largo, de modo que la cabecera estable seguía
acertando aunque el resumen viajara pegado detrás.

| Provider | Granularidad | Antes | Después | Veredicto |
|---|---|---|---|---|
| **Anthropic** | breakpoint por bloque | write 3029/3457/3829/4260/4684, cero reads cross-turn | 0 writes desde el turno 2, reads fijas en 3029 | **arreglado** |
| **OpenAI** | prefijo de tokens sobre el prompt serializado | `cached_tokens` 1280 con el resumen ya crecido | 1280 | **sin cambio** |
| **Gemini** | prefijo de bytes sobre el request serializado | reads 857 / 1728 | 858 / 1730 | **sin cambio** |

Evidencia de OpenAI: sonda directa a `chat/completions` con las dos formas del mensaje (`system`
fusionado vs `system` separados), creciendo el resumen entre llamadas — `cached_tokens` idéntico en
ambas. Evidencia de Gemini: 5 turnos por arm; el turno 1 (sin compactación todavía) da
`promptTokens` **1714 exacto en los dos arms**, y la divergencia posterior es de ~5 tokens, que es
el separador `---` que dejó de emitirse. Grafos:
`tests/graphs/agents/prompt_cache_compaction_measure_{openai,gemini}.json`.

Ninguna API pública cambia → ADP no se ve afectado.

**Documentación de referencia.**
- [§15 — Memory guide, "Dónde termina el resumen"](developer_guide/15_memory_guide.md)
- [§14 — LLM deep dive, "Más de un mensaje `system` en el request"](developer_guide/14_llm_deep_dive.md)

**Verificación.**

```bash
set -a; source .env; set +a
export ANTHROPIC_BASE_URL="https://api.anthropic.com/v1"
for t in 1 2 3 4 5; do
  cargo run --bin dag_engine -- run \
    tests/graphs/agents/prompt_cache_compaction_measure.json \
    --agent-session-id pc_measure_001
done
```

Leer `cacheWriteTokens` / `cacheReadTokens` del frame `finish` de cada corrida. Manual a propósito:
pega contra una API paga y no puede correr en CI. Los guards que **sí** corren en CI son
`summary_never_merges_into_the_agent_system_prompt`,
`the_cached_system_prefix_does_not_move_as_the_conversation_grows` (history_compaction) y
`consecutive_system_messages_are_not_coalesced` / `a_request_with_two_system_messages_is_valid`
(llm_request).

**Estado.** done.

---
---

## 8. `usage` separa el input fresco de los tokens de cache

**Qué cambió.** El dato de cache ya viajaba end-to-end desde 2026-06-09
(`cache_read_tokens` / `cache_write_tokens` en los tres adapters, propagados al
SSE en tres lugares). Lo que faltaba no era plomería sino **semántica**:
`prompt_tokens` significaba tres cosas distintas según el provider — Anthropic
lo reportaba neto de cache, OpenAI y Gemini con el cache adentro — y el motor las
sumaba en el mismo acumulador. Tabla completa por provider en
[§14](developer_guide/14_llm_deep_dive.md).

**Verificado en vivo contra las tres APIs reales el 2026-08-23**, no inferido de
su documentación. OpenAI (`gpt-4o`) dio la evidencia más limpia: run 1 sin cache
`prompt 2550 / cache_read 0`, run 2 con hit `prompt 118 / cache_read 2432`, y
`total_tokens: 2554` **idéntico en ambas** — `118 + 2432 = 2550`, el prompt de
run 1 exacto. Gemini: en un cache hit (8820 cacheados) el `promptTokenCount`
**no cayó** respecto al turno anterior sin hit (9235 → 9259) — prueba de que el
cache va adentro. Anthropic: `prompt_tokens: 404` con `cache_read_tokens: 1809`,
imposible si estuviera adentro.

**Dos consecuencias con plata de por medio, ambas cerradas.**

1. **`total_tokens` subcontaba los turnos cacheados de Anthropic un 81%.** Se
   calculaba como `prompt + completion + thinking`, sin el cache. Medición real:
   `prompt_tokens: 404`, `cache_read_tokens: 1809`, `total_tokens: 412` — el
   turno procesó 2213 tokens de entrada.
2. **La fórmula de costo que documentaba §14 daba negativo en Anthropic.** Decía
   `costo_input = (promptTokens − cacheReadTokens) × rate`; con esos números,
   `404 − 1809 = −1405`. Era correcta para OpenAI/Gemini y catastrófica para
   Anthropic, exactamente por la asimetría de arriba.

**Qué se hizo. Ningún campo cambió de nombre** — la restricción era explícita,
porque ADP factura sobre estos nombres.

- **Normalización en el adapter**, que es el único lugar donde se conoce la
  semántica del provider. Nuevo builder `LlmUsage::with_cached_input_tokens_included`
  para los providers que meten el cache adentro (resta y registra); Anthropic
  sigue usando `with_cache_read_tokens` (suma sin restar). `prompt_tokens` pasa a
  significar **input fresco** en los tres, y se sostiene la identidad
  `prompt + cache_read + cache_write = input real del turno`.
- **`total_tokens` incluye el cache.** `recompute_total` suma las cinco columnas
  desde cero en cada mutación, así que el orden de los builders ya no puede
  alterar el resultado (hay un test que lo fija).
- **Las dos columnas de cache están siempre presentes**, `0` incluido. El gate
  `> 0` hacía indistinguible "no hubo cache hit" de "este provider no reporta el
  dato". `thinking_tokens` conserva su gate.
- **El `finish` de un run cancelado emite el mismo objeto que uno terminado.**
  Antes perdía las columnas de cache y thinking por completo
  (`sse_mapper.rs:386`). Ahora ambos caminos comparten `usage_snapshot()`.
- **Se cableó el cache en el path Responses API de OpenAI** (streaming y no
  streaming), que lo descartaba entero.

**Read y write no se colapsan en un solo campo.** Un cache read cuesta ~10% del
input rate y un cache write ~125%: más de 10x de diferencia. Un número único de
"cache" no sería facturable.

**Documentación de referencia.**
- [`docs/adp_migration/2026-08-23-usage-cache-token-split.md`](adp_migration/2026-08-23-usage-cache-token-split.md) — nota de migración; **acción requerida**: los tokens de cache dejaron de facturarse en Gemini/OpenAI, porque salieron de `promptTokens` y el cálculo de ADP no mira las columnas de cache
- [`docs/developer_guide/14_llm_deep_dive.md`](developer_guide/14_llm_deep_dive.md) — semántica por provider y fórmula de costo corregida
- [`docs/sse_events_reference.md`](sse_events_reference.md) — esquemas de `usage-summary.nodes` y `finish`

**Estado.** Done. 2313 tests unitarios en verde; semántica de Anthropic y Gemini
verificada contra las APIs reales.

---

## 9. La identidad del nodo anidado viaja en su frame de frontera

**Qué cambió.** Un `llm_call` (o `for_each`) despachado como tool emitía su
`subgraph-node-start` con `config: {}` e `inputs: {}`. Como el motor puebla su
tabla de metadatos leyendo `provider`/`model` de ese frame, la fila de ese nodo
en `usage-summary` salía con `model: null, provider: null`. Sus tokens se
**atribuían** bien pero **no se podían tarifar** — las tarifas son por modelo, y
el modelo no viajaba. Y el `fixed_config` de un tool es libre de nombrar otro
provider que el del agente que lo despacha (el patrón del hijo en tier barato),
así que heredar los valores del padre no era un sustituto válido.

Ahora el frame lleva la identidad del nodo que va a correr, y la fila queda
tarifable. Verificado en vivo con un padre `claude-sonnet-4-6` y un hijo
`gemini-2.5-flash` en el mismo run: cada fila reporta lo suyo.

**Es una allowlist deliberada** — `config` lleva **solo** `provider` y `model`,
porque en ese punto los inputs ya tienen los secure values descifrados y volcar
todo pondría el `api_key` en el stream. Ese es el motivo por el que el frame
salía vacío. Detalle completo en la nota de migración; la lógica vive en
`boundary_identity`, con seis tests.

**Grafos de evidencia.** Tres nuevos, uno por provider:
[`nested_cache_usage_anthropic_e2e.json`](../tests/graphs/agents/nested_cache_usage_anthropic_e2e.json),
[`nested_cache_usage_openai_e2e.json`](../tests/graphs/agents/nested_cache_usage_openai_e2e.json),
[`nested_cache_usage_gemini_e2e.json`](../tests/graphs/agents/nested_cache_usage_gemini_e2e.json).
Un `llm_call` padre expone un `llm_call` hijo como tool; ambos llevan ~2.6k
tokens de prefijo estable **distinto**, para que cacheen por separado. Corriendo
el mismo `--agent-session-id` varias veces se ve la inversión y el retorno del
cache por nivel de anidamiento:

| Provider | Turno 1 | Turno 2+ | Nota |
|---|---|---|---|
| Anthropic | padre write 3803, hijo write 3546 | padre read 7606, hijo read 3546, write 0 | El único que reporta `cache_write` |
| OpenAI | padre read 2816, hijo 0 | turno 3: hijo `prompt 97 / read 2688` | Nunca reporta write |
| Gemini | hijo read 0 | el hijo **recién cachea en el turno 4** | Warmup real; único con `thinkingTokens` |

**Documentación de referencia.**
- [`docs/adp_migration/2026-08-23-nested-node-identity.md`](adp_migration/2026-08-23-nested-node-identity.md) — nota de migración (aditiva, sin acción obligatoria)
- [`docs/sse_events_reference.md`](sse_events_reference.md) — `usage-summary.nodes`

**Estado.** Done. Verificado en vivo contra los tres providers.

---

## 10. OpenAI cobra por escribir cache desde GPT-5.6 — el adapter no lo leía

**Qué cambió.** Hasta GPT-5.5, OpenAI creaba sus entradas de cache **gratis**: no
había nada que cobrar ni que reportar, y por eso su adapter solo leía
`cached_tokens`. Desde **GPT-5.6** cobra la creación a **1.25×** y la reporta en
`prompt_tokens_details.cache_write_tokens` (Chat Completions) y
`input_tokens_details.cache_write_tokens` (Responses API). El adapter no leía
ese campo, así que en esos modelos Colmena reportaba `cache_write_tokens: 0`
sobre tokens que OpenAI **sí estaba facturando** — un subconteo silencioso.

**La semántica no es la de Anthropic.** El write de OpenAI es un **subconjunto**
de `prompt_tokens`, igual que su `cached_tokens`: las tres categorías
*particionan* el input, `cached + written + uncached = prompt_tokens` (ejemplo de
su propia doc: 2000 leídos + 400 escritos + 200 sin cachear = 2600). En Anthropic
las tres son disjuntas. Por eso el nuevo builder
`LlmUsage::with_cache_write_tokens_included` **resta**, mientras que
`with_cache_write_tokens` (Anthropic) solo registra.

**Un caso que se perdía entero.** El código leía `prompt_tokens_details` solo
cuando `cached_tokens > 0`, así que la primera llamada contra un prefijo nuevo
—todo escritura, cero lectura— se descartaba completa. Ahora las dos columnas se
leen por separado.

**Precios corregidos en la guía.** La tabla de §14 estaba desactualizada en sus
**tres** renglones: el descuento de lectura de OpenAI pasó de 50% a **90%** con
la serie gpt-5.x, el de Gemini no es "25-75%" sino **90%** en 2.5+, y faltaba la
columna de write. Se agregaron además tablas de precio por modelo para los tres
providers, con fecha y enlace a la fuente.

**Gemini explicit caching.** Documentado como el segundo modo con costo de
escritura que Colmena **no** usa: cobra almacenamiento por hora ($1.00 por 1M
tokens/hora en 2.5 Flash). El implicit que sí usamos no cobra creación ni
almacenamiento, así que su `cache_write_tokens` seguirá siendo `0` — y eso es
correcto, no un dato faltante.

**Estado.** Done. Cuatro tests nuevos sobre la forma de usage de GPT-5.6, incluido
uno que fija la identidad `prompt + read + write == input reportado`. No se pudo
verificar en vivo: no hay acceso a un modelo GPT-5.6 en esta cuenta.

## 11. La Responses API de OpenAI aprende a leer tool calls (groundwork)

**Qué cambió.** El adapter de OpenAI ya sabía **hablar** la Responses API para
adjuntos, pero solo leía el `output_text` del **primer** item de la respuesta.
Ahora el parseo entiende toda la forma de la respuesta:

- Nuevo `parse_responses_output` recorre **todos** los items del array `output`:
  los `message` aportan el texto (aunque haya un item `reasoning` delante, que
  antes tapaba el texto), y los `function_call` se convierten en `ToolCall`
  (id = `call_id`, el que se devuelve luego en `function_call_output`).
- `call_responses` adjunta esos tool calls al `LlmResponse`.
- `stream_responses` maneja los eventos `response.output_item.added`
  (function_call) y `response.function_call_arguments.delta`, emitiendo
  `ToolCallChunk` igual que el path de chat completions.

**Por qué solo groundwork.** Este slice **no cambia el ruteo**: hoy nada dirige
tools a la Responses API, así que el tráfico existente (adjuntos, sin tool calls)
se comporta idéntico — el escaneo de todos los items es una mejora estricta
incluso ahí. Habilitar el ruteo `gpt-5*` + tools es el slice siguiente
(§12), que se apoya en este parser.

**ADP no afectado.** Cambio interno del wire OpenAI↔Colmena; SSE y `usage`
idénticos.

**Estado.** Done. Un test nuevo sobre la forma de respuesta gpt-5 (texto + tool
call tras un item de reasoning).

## 12. GPT-5.6 fallaba en el primer turno — ruteo automático a la Responses API

**El síntoma.** Cualquier grafo con un modelo `gpt-5*` moría en el turno 0 con un
`400` de OpenAI:

```
Function tools with reasoning_effort are not supported for gpt-5.6 in
/v1/chat/completions. To use function tools, use /v1/responses or set
reasoning_effort to 'none'.
```

Lo desconcertante: el grafo **no declaraba ninguna tool**. Colmena inyecta la tool `recall_history` en
**cada** turno de agente (`llm.rs`, "Always eager"). Así que aunque el grafo no
declare tools, la request siempre lleva al menos una → `build_request_body`
escribe `body["tools"]`. La familia de razonamiento `gpt-5` rechaza *function
tools + `reasoning_effort` ≠ `'none'`* en Chat Completions, y su
`reasoning_effort` por defecto (del server) no es `'none'` → choca solo por la
presencia del array.

**El fix.** El adapter OpenAI ahora **rutea a `/v1/responses`** cuando el modelo
es `gpt-5*` **y** la request lleva tools (`is_gpt5_family` + `is_responses_api_required`).
La Responses API es el único endpoint que sirve *reasoning + tools* juntos.
Apoyándose en el parser del §11, se completó el lado del **request**:

- `build_responses_request_body` serializa `tools` (forma *plana*:
  `name`/`description`/`parameters` al nivel superior, **no** anidado bajo
  `"function"` como en Chat Completions), `tool_choice`, y `reasoning.effort`
  desde `thinking_budget`. `temperature`/`top_p` se **omiten** para `gpt-5*`
  (esa familia solo acepta el default; da `400` — confirmado por curl a
  `/v1/responses`), y `max_tokens` se manda como `max_output_tokens`.

El resto de modelos (`gpt-4o`, `gpt-4.1`, …) siguen en `/v1/chat/completions`
sin cambios — cero regresión en el path maduro.

**Verificado en vivo con gpt-5.6** (a diferencia de §10, esta cuenta sí tiene
acceso). Todo verde: grafo sin tools; tools no-stream + stream
([`gpt5_responses_tools_e2e.json`](../tests/graphs/agents/gpt5_responses_tools_e2e.json),
`add`→`multiply`); cache en el path Responses (`cache_read 1654` / `cache_write
1714`); anidado subgraph-as-tool
([`gpt5_responses_nested_subgraph_e2e.json`](../tests/graphs/agents/gpt5_responses_nested_subgraph_e2e.json),
gpt-5.6→gpt-5.6, total 600); HITL (`suspend` simple + batch paralelo con
suspend, sin el 400 de ids huérfanos del §5); skills (`load_skill`), lazy
(`describe_tool`) y memoria multi-turno (`recall_history`); y sin regresión en
`gpt-4o`. 30/30 tests del adapter, 506/506 del módulo `llm`.

Límite conocido, ajeno a este cambio: un adjunto no-imagen inline base64 + tools
no entrega el PDF al modelo — pre-existente (el grafo original
`tests/graphs/media/pdf_base64.json` en gpt-4o-mini falla idéntico), fileado
como issue separado.

**ADP no afectado.** El cambio vive en el wire OpenAI↔Colmena; la frontera SSE y
el `usage` que consume ADP son idénticos.

**Estado.** Done.

## 13. Un adjunto que falla al subir ya no se descarta en silencio (#200)

**Qué cambió.** En la resolución de archivos del `llm_call` **sin caché** (la rama
que corre cuando no hay `DATABASE_URL`), un fallo al subir el adjunto a la Files
API del provider solo emitía un `WARN` y **seguía sin el archivo** — el modelo
respondía como si el documento nunca hubiera existido, sin ningún error que lo
explicara. Ahora esa rama **falla cerrado** (propaga el error y aborta el run),
igual que la rama canónica (`LlmCallUseCase::resolve_files`, que ya usaba
`.await?`). Aplica a los tres puntos: descarga de signed URL, subida de signed
URL y subida de inline no-text.

Además, el backstop `_ => continue` del registro de catálogo (un archivo no-text
que llegó sin subir) pasa de salto silencioso a un `tracing::warn!` estructurado
(`event = "attachment.registration_skipped_unuploaded"`), para que cualquier drop
futuro quede auditable.

**Por qué.** Descubierto investigando el issue #197 (que resultó no ser un bug: la
entrega de adjuntos funciona; el síntoma original era un proxy TLS bloqueando la
Files API). El silent-drop es un problema de robustez independiente: perder un
archivo del usuario sin señal es peor que fallar.

**Alcance.** Solo afecta el path sin `DATABASE_URL` (dev/local); producción/ADP
siempre setea `DATABASE_URL` → rama canónica, ya fail-closed. El path de éxito no
cambia — verificado E2E (inline PDF sin `DATABASE_URL` → subida → registro →
`load_attachment` → respuesta correcta). Un test de inyección de fallo determinista
requiere un provider mockeable y queda como follow-up.

**ADP no afectado.**

**Estado.** Done.

## 14. `COLMENA_EXTRA_CA_CERT` — convivir con un proxy TLS interceptor local

**Qué cambió.** Todos los `reqwest::Client` del crate ahora se construyen a
través de un factory único: `shared::http_client::{builder, client}`. El factory
lee la nueva variable de entorno **`COLMENA_EXTRA_CA_CERT`** (path a un PEM) y
**agrega** esa(s) CA(s) a las raíces de confianza embebidas (`webpki-roots`).

**Por qué.** El crate fija `rustls` con raíces embebidas (portable, reproducible
en Cloud Run, sin depender del trust store del host). El costo: rechaza un proxy
que **intercepta TLS** en la máquina del dev (p. ej. *Proxon*, que mide consumo
de IA re-firmando los certificados de los proveedores con su propia CA) →
`invalid peer certificate: UnknownIssuer`, disfrazado de "API key rechazada". El
factory cierra esa brecha sin bajar la seguridad.

**Garantías.**
- **Opt-in y aditivo.** Con la env **sin setear** (default en prod, CI y máquinas
  sin proxy), el factory es byte-por-byte un `reqwest::Client::builder()` normal —
  cero cambio de comportamiento. `webpki-roots` sigue siendo el único trust store.
- **No desactiva verificación.** Suma una CA conocida; **no** es
  `danger_accept_invalid_certs`. Los certificados públicos siguen validando igual.
- **Degrada seguro.** Si la env apunta a un archivo inexistente/ilegible o sin PEM
  válido, se sigue con las raíces embebidas y un `warn` estructurado
  (`target: colmena::http`), en vez de romper todas las requests.

**Uso (dev detrás de Proxon):**
```bash
export COLMENA_EXTRA_CA_CERT="$HOME/Library/Group Containers/group.com.proxon.observer/proxon-ca.cert.pem"
```

**Verificado.** gpt-5.6 contra OpenAI real detrás de Proxon: sin la env →
`UnknownIssuer` (como antes); con la env → responde OK. 48 tests del factory
(split de PEM, gating de env, degradación). Guía:
[`docs/developer_guide/18_troubleshooting.md`](developer_guide/18_troubleshooting.md)
("invalid peer certificate: UnknownIssuer").

**ADP no afectado.** Cambio interno; la env va sin setear en la nube y no hay
proxy interceptor en la ruta de Cloud Run. `Cargo.toml` (features de `reqwest`,
`rustls-tls` + `webpki-roots`) **no cambia**.

**Estado.** Done. Migrados los clientes de proveedores externos (adapters LLM,
Files APIs, TTS, image, signed-URL, web, gsheets/gdocs, google_oauth, http node,
crdt, storage callback). Único `Client::builder()` restante: uno en un `mod
tests`.

---

## 15. El `fixed_config` de un subgraph ya no cruza al prompt del hijo

**Qué cambió.** `SubGraphNode` excluye `child_graph_inline` y `child_graph_path` del
estado global que arma para el grafo hijo. El nodo sigue resolviendo su grafo desde
esas claves —la lectura ocurre antes del mapeo IN— pero ya no se las pasa al hijo.

**El bug.** Reportado y medido por ADP el 2026-08-25 en dev, con credenciales
reales. Todo sub-agente ejecutado como `subgraph` recibía el `child_graph_inline`
completo de su propio nodo **dentro de su mensaje de usuario**, con los secretos ya
resueltos: `api_key` del proveedor LLM, `api_key` de Tavily y un `connection_url` de
Postgres, los tres en claro. Un sub-agente además copió la cadena de conexión textual
dentro de un documento que redactó para el usuario final, sin que nadie se lo pidiera.

La cadena:

1. `dag_tool_executor.rs` mezcla el `fixed_config` del tool en los `inputs` del nodo y
   le pasa `config = {}`. Correcto: el nodo necesita su `child_graph_inline`.
2. `subgraph.rs` copiaba al estado del hijo **todos** los inputs salvo `__colmena_*` y
   `__node_id`. El filtro excluía lo interno del motor, no la config del operador.
3. El nodo `input` del hijo con `data: {}` toma la rama passthrough y devuelve todo lo
   que no empiece con `__`.
4. `resolve_prompt_or_task` ve un objeto no vacío y lo preserva como `prompt`.
5. `agent_service.rs` lo manda al modelo y persiste el mismo objeto — por eso lo que se
   lee en `llm_node_history` **es** lo que recibió el modelo.

`memory_mode` no introdujo la fuga (el mapeo IN es de abril), pero la agrava: los modos
con memoria **exigen** un `connection_url` dentro del `child_graph_inline`, y con
`persistent` el `node_id` es estable, así que el mensaje envenenado queda en un hilo
compartido y se reenvía en cada llamada posterior.

**Segundo bug que cierra el mismo cambio.** Con el plumbing en el estado, un `subgraph`
anidado sin `config` propia caía al fallback `inputs.get("child_graph_inline")` y
resolvía **el grafo del padre** — recursión silenciosa. ADP confirmó anidamiento a
profundidad 2 en 7 rutas de su grafo.

**Por qué exclusión por nombre.** Las dos alternativas quedaron descartadas por
medición, no por preferencia. Una *allowlist* de claves es imposible: ADP midió sobre
215 filas que el conjunto lo decide el modelo por llamada (apareció `confirmation`, que
nadie declara). Un filtro *por procedencia* tampoco: los tres caminos de merge del
executor (`node_schema`, `$DYNAMIC`, legacy `field_mapping`) producen un mapa plano donde
el valor del operador y el argumento del modelo son indistinguibles.

**Forma del fix.** Una constante única, `CHILD_GRAPH_SOURCE_KEYS`, de la que derivan
tanto `resolve_child_graph_source` como el predicado nuevo
`is_excluded_from_child_state`. Una fuente nueva del grafo hijo queda invisible para el
hijo por construcción, sin una segunda lista que recordar. El mapeo IN se extrajo a
`build_child_state`, una función pura, para que los tests ejerciten el mapeo real.

**Límite conocido.** La exclusión no cubre un secreto puesto directamente en el
`fixed_config` del tool `subgraph` (p. ej. un `api_key` al nivel del tool). Hoy nadie lo
hace, y cerrarlo exige conservar la procedencia en el executor — cambio aparte.

**Documentación de referencia.** `docs/developer_guide/19_nested_agents_and_subgraphs.md`
(secciones "Entrada" y el bloque de plumbing), grafo de repro en
`tests/graphs/advanced/subgraph_plumbing_isolation.json`.

**Tests.** 8 unitarios nuevos en `subgraph_child_state_isolation_tests`. Verificado que
4 de ellos fallan contra el predicado pre-fix (el resto fija lo que **no** debe cambiar:
`files`, las claves internas del motor y la propagación de profundidad).

**Estado.** done.

---

## 16. `head_truncate` sube a dominio como primitiva compartida

**Qué cambió.** Nada observable: es un refactor puro, sin cambio de comportamiento.

`head_truncate` y su constante `TRUNCATION_MARKER_RESERVE` vivían como funciones asociadas
privadas de `DagToolExecutor`. Ahora viven en un módulo nuevo,
`llm/domain/text_bounds.rs`, puro y sin dependencias, y
`DagToolExecutor::head_truncate` quedó como una delegación de una línea.

**Por qué.** La primitiva no tiene nada de específico del executor: recorta una cadena
conservando su cabeza y le anexa el marcador `[truncated: showing first N of M bytes]`,
respetando límites de carácter UTF-8. Estaba encerrada donde el resto del módulo LLM no
podía alcanzarla, y ese es justamente el escenario en que cada nuevo consumidor se escribe
su propia variante y los marcadores empiezan a divergir. Moverla la deja disponible como
única fuente de verdad antes de que aparezca el segundo consumidor, no después.

**Compatibilidad.** `scrub_tool_result_output` sigue llamando a `Self::head_truncate` y por
lo tanto no cambia en absoluto. La visibilidad es `pub(crate)`, así que no se agrega
superficie pública: ninguna firma exportada cambia y los bindings de Python y TypeScript no
se tocan. ADP no se ve afectado.

**Tests.** Los 2 tests preexistentes en `dag_tool_executor::scrubber_tests` siguen verdes
**sin modificarse** — son la prueba de que la delegación preserva el comportamiento. Se
suman 3 unitarios en `text_bounds`, entre ellos uno de aprobación que fija el formato exacto
del marcador y otro que ejercita entrada multibyte (`é` repetida), donde cortar por índice
de byte crudo habría paniqueado.

**Estado.** done.

---

## 17. Puerto de dominio para servidores MCP remotos (1/9)

**Qué cambió.** Nada observable todavía: es la capa de dominio de la feature que permitirá
conectar un servidor MCP (Model Context Protocol) remoto a un `llm_call` y exponer sus tools
al modelo. Este slice solo introduce contratos, sin adapter, sin registry y sin despacho.

Nuevo módulo `llm/domain/mcp.rs` con:

| Item | Rol |
|---|---|
| `McpClientPort` | Puerto async (`Send + Sync`) con `list_tools`, `call_tool`, `server_label` |
| `McpToolDescriptor` | Tool tal como la declara el servidor — `input_schema` **verbatim** |
| `McpToolResult` | Contenido ya colapsado a `String` más el flag `is_error` |
| `McpServerConfig` | URL, transporte, refs de headers **sin resolver**, timeouts |
| `McpError` | 8 variantes `thiserror`, sin catch-all: cada una debe poder distinguirse |
| `MCP_MAX_*` | Los 7 límites del diseño, en un solo lugar para que los tests afirmen sobre el mismo símbolo |

El módulo respeta la regla dura de `CLAUDE.md`: **cero dependencias de infraestructura**. Sus
imports se limitan a `std`, `serde_json`, `thiserror` y `async_trait` — nada de `reqwest` ni
`rmcp`. Un test de compilación (`send_sync_tests`) fija que todo implementador del puerto sea
`Send + Sync`, porque `ToolExecutor` y el resto de los puertos ya lo exigen.

**Relación con §16.** El truncado por bytes que la contención de contenido MCP necesitará
vive ya en `llm/domain/text_bounds.rs`, entregado por separado en §16. Este slice no lo toca:
lo consumirán las slices posteriores, que son las que capan descripciones y resultados de
terceros.

**Compatibilidad.** Puramente aditivo. Ninguna firma pública cambia, ningún binding se toca,
y un grafo sin entradas MCP se comporta byte a byte igual. ADP no se ve afectado.

**Tests.** 8 unitarios: `send_sync_tests` (prueba de compilación de que todo implementador
del puerto es `Send + Sync`) y `error_variant_tests` (una por variante de `McpError`,
afirmando que se distingue por patrón y que arrastra el contexto necesario para construir
aguas abajo o bien un aviso al operador o bien un error de tool corregible por el modelo).

**Estado.** done (slice 1a de 9).

---

## 18. Nombres y delimitador de contenido no confiable para MCP (2/9)

**Qué cambió.** Nada observable todavía: son las dos funciones puras que las slices de
exposición y despacho usarán para contener contenido de terceros. Se suman a
`llm/domain/mcp.rs`, que estrenó el puerto en §17.

**`normalize(alias, tool)`** deriva el nombre expuesto `<alias>__<tool>`: normaliza a
`[A-Za-z0-9_-]` (la clase que aceptan los tres proveedores) y, si supera 64 caracteres,
conserva los primeros 55 más `_` y 8 hex de `sha256` del nombre ya normalizado. Siempre ≤ 64,
determinista, con el prefijo del alias al frente para que un operador lo rastree, y dos
nombres largos con los mismos primeros 55 caracteres siguen siendo distintos. Los guiones se
preservan: `resolve-library-id` de Context7 queda `context7__resolve-library-id`.

**`wrap_untrusted_content(alias, tool, nonce, content)`** envuelve el texto que escribe un
servidor de terceros en un delimitador que lo declara dato, no instrucción. El `nonce` **lo
recibe por parámetro**: esta capa nunca lo genera, lo que la deja determinista y permite que
quien llama derive uno que el servidor no pueda falsificar. Un marcador de cierre falsificado
dentro del contenido no termina el bloque, porque no lleva el nonce correcto.

Ambas son puras y sin estado. El único import nuevo es `sha2`, hashing sin I/O, así que la
regla de cero dependencias de infraestructura sigue en pie.

**Corrección de la revisión de §17.** Los siete tests de variantes de `McpError` eran
tautológicos: construían la variante, la desestructuraban con el mismo patrón y afirmaban
sobre los valores que ellos mismos habían puesto — una garantía del sistema de tipos, no de
nuestro código — sin tocar nunca `Display`, que es el mensaje que ve un operador o el modelo.
Se reemplazan por dos:

- `every_variant_renders_its_context` fija el **texto exacto** de las 8 variantes. Verificado
  empíricamente: falla al quitar un campo de un `#[error(...)]` y ante una errata de una
  letra; los tests viejos pasaban en verde en ambos casos.
- `every_variant_is_classifiable_without_a_catch_all` clasifica cada variante en aviso al
  operador o error corregible por el modelo, con un `match` **sin brazo `_`**. Agregar un
  catch-all, o una variante sin clasificar, deja de compilar.

**Compatibilidad.** Puramente aditivo: ninguna firma pública cambia, ningún binding se toca,
ADP no se ve afectado.

**Tests.** 10 en `llm::domain::mcp`: 5 de `normalize`, 3 del delimitador, 2 de errores.

**Estado.** done (slice 1b de 9).

---

## 19. Dependencia `rmcp` para clientes MCP remotos (3/N)

**Qué cambió.** Se agrega la crate `rmcp` (el SDK oficial de Rust para Model Context
Protocol) como dependencia. Todavía no la usa nadie: este cambio aísla la decisión de
dependencia para que se revise sola, separada del adapter que la consumirá.

```toml
rmcp = { version = "3.1.4", default-features = false, features = [
  "client", "transport-streamable-http-client-reqwest", "reqwest" ] }
```

**Por qué `default-features = false`.** Los defaults de `rmcp` incluyen `server`, que
arrastra todo el stack de servidor (`schemars`, `uuid`, `tower`). Colmena es cliente MCP
puro, así que serían dependencias compiladas a cambio de nada.

**Por qué NO `transport-child-process`.** Ese es el transporte stdio: lanza un binario
local y le habla por pipes. La decisión de producto es **solo remoto**: un worker capaz de
ejecutar procesos nombrados desde la configuración de un grafo es una postura de seguridad
distinta a uno que no puede. Verificado: `process-wrap` no aparece en `Cargo.lock`.

**El costo aceptado: una tercera versión de `reqwest`.** `rmcp 3.1.4` exige
`reqwest ^0.13.2` y el crate usa `reqwest 0.11`. El árbol queda con tres — la nuestra
`0.11.27`, la `0.12.28` que ya arrastraba `rust_socketio`, y la `0.13.4` de `rmcp`.

Se midió la alternativa antes de aceptarlo. Subir el crate a `reqwest 0.13` resultó ser
**una línea más dos features** (`.query()` y `.form()` pasaron a ser features en 0.13), con
los 2.397 tests en verde y cero cambios en los 26 archivos que usan `reqwest::`. Se
descartó igual: en 0.13 el feature `rustls` arrastra `rustls-platform-verifier`, que en
Linux lee el almacén de certificados **del sistema** en vez de los `webpki-roots`
embebidos que [`shared/infrastructure/http_client.rs`](../src/libs/colmena/src/shared/infrastructure/http_client.rs)
fija a propósito para que la confianza TLS sea idéntica en cualquier imagen. Y no se puede
conservar: `reqwest` eliminó el feature `webpki-roots` después de 0.13.1, y `rmcp` exige
≥0.13.2.

Mover la confianza TLS del binario a la imagen habría afectado **todas** las llamadas
salientes de Colmena. Con la duplicación de stack el efecto queda acotado a MCP — pero
**queda**, y conviene decirlo explícitamente: `reqwest 0.13.4` entra al árbol con
`rustls-platform-verifier`, así que **las llamadas MCP van a validar contra el almacén de
certificados del sistema, no contra los `webpki-roots` embebidos que usa el resto de
Colmena**, y además quedan fuera del alcance de `COLMENA_EXTRA_CA_CERT`. Son dos anclas de
confianza distintas conviviendo en el mismo binario.

Se eligió esa asimetría, no se la evitó: el radio de impacto de un cambio acotado a MCP es
menor que el de uno que alcanza a los proveedores de LLM, Google, Amadeus y los adjuntos.
Las imágenes de runtime —la de Colmena y las dos de ADP— instalan `ca-certificates` sobre
`debian:bookworm-slim`, así que el almacén del sistema existe. Una base futura sin él
rompería MCP en silencio; ese es el precio y queda anotado acá.

**Tests.** 2 de integración en `tests/rmcp_dependency_invariants.rs`. Ambos leen
`Cargo.lock`, nunca `Cargo.toml`: lo que importa no es cómo se *escribe* la dependencia
sino qué termina *compilado*, y difieren — un feature puede llegar por unificación de
features con otro miembro del workspace, un override, un bloque `[target.'cfg(…)']` o una
dependencia transitiva. Se afirma sobre la lista de dependencias **propias de rmcp**, no
sobre el árbol entero, porque `schemars`, `uuid` y `tower` aparecen legítimamente por otras
vías.

Una primera versión de estos guards hacía string-matching sobre `Cargo.toml`, y se
**reprodujo dando falso verde**: envolver el array de `features` en varias líneas —un
reformateo rutinario— dejaba el feature agregado fuera de lo que el check miraba, así que
los tests pasaban mientras `process-wrap` entraba de verdad al lockfile y el transporte
stdio quedaba compilado. Un guard que reporta éxito con la invariante rota es peor que no
tenerlo, porque se le va a creer. La segunda versión parseaba la tabla inline completa y
seguía siendo frágil ante llaves dentro de strings o comentarios; la revisión lo marcó y se
eliminó el parseo de manifiesto por completo.

Ambos ataques quedaron verificados rompiendo el manifiesto a propósito: reactivar
`transport-child-process` (con el reformateo multilínea incluido) y restaurar
`default-features` hacen fallar a un guard cada uno, nombrando qué crate se coló.

El `Cargo.toml` enuncia las dos restricciones en un comentario junto a la dependencia, para
que quien lea el manifiesto las vea sin depender de un test.

**Compatibilidad.** Aditivo. Ninguna firma pública cambia, ningún binding se toca, y sin
código que la consuma el comportamiento en runtime es idéntico. ADP no se ve afectado.

**Estado.** done.

---

## 20. Adapter `rmcp` para servidores MCP remotos (4/9)

**Qué cambió.** Nada observable todavía: nadie construye este cliente aún. Es la
implementación de `McpClientPort` sobre el transporte streamable-HTTP de `rmcp`, en
`llm/infrastructure/mcp_client/`. **Es el único archivo del crate autorizado a nombrar un
tipo de `rmcp`**; el dominio sigue sin saber que existe.

`RmcpHttpClient::connect` valida **HTTPS antes de tocar el socket** y luego deja que
`ServiceExt::serve` maneje el handshake (`initialize` → `notifications/initialized`).

**Las tres operaciones están acotadas por el `timeout` de la configuración**, el handshake
incluido. Eso último salió de la revisión: `ServiceExt::serve` no impone deadline propio y
`rmcp` no configura timeout en su cliente HTTP, así que un servidor que acepta la conexión
TCP y después se queda mudo habría colgado a quien llame a `connect`, sin techo — y
`timeout_seconds` no habría significado lo que dice. Verificado quitando el wrapper: el test
pasa de ~1s a **10,23s**, el retardo completo del mock.

**No hay `session.rs`, a propósito.** El worker de `rmcp` ya guarda el `mcp-session-id` que
devuelva un servidor y lo reenvía en cada request posterior, y no manda ninguno cuando el
servidor nunca emite uno. Escribir eso de nuevo sería el mecanismo paralelo que el diseño
prohíbe. Lo que sí hacemos es **probar ambas formas de servidor** con `wiremock`, que es lo
que da confianza de que funciona — no la existencia de código propio.

**`sampling` nunca se negocia.** `ClientCapabilities::default()` deja el campo en `None`, así
que los params serializados de `initialize` no llevan esa clave sin importar qué anuncie el
servidor. Un test lo afirma sobre el JSON real del request.

**`input_schema` viaja verbatim**, sin aplanarse. Es la razón por la que el tipo del dominio
lo guarda como `serde_json::Value`: los schemas reales usan `minItems`, `minimum` y
`$schema`, que el modelo plano de `ParameterProperty` no puede representar.

**Costura de test declarada.** `wiremock` solo sirve HTTP plano, así que los tests entran por
`connect_for_test`, que salta el guard de HTTPS. Está bajo `#[cfg(test)]` —no existe en
builds de producción— y el guard se afirma por separado en
`rmcp_connect_rejects_non_https_url`. Vale saberlo al leer los tests: no ejercitan el camino
guardado, lo ejercitan alrededor.

**Compatibilidad.** Aditivo. Ninguna firma pública cambia, ningún binding se toca. ADP no se
ve afectado.

**Tests.** 8 con `wiremock`, sin red: guard de HTTPS, handshake completo, `tools/list` contra
un servidor sin sesión, eco de `mcp-session-id` contra uno con sesión, timeout de `call_tool`
y de `connect` al valor configurado, ausencia de `sampling` en el handshake, y fidelidad
byte a byte del `input_schema`.

Dos de ellos salieron de la revisión, y valen por lo que cubren:

- **`rmcp_list_tools_forwards_input_schema_verbatim`** usa un schema con `minItems`,
  `minimum`, array-of-enum y `$schema` —la forma real que expone el servidor MCP de
  HuggingFace— y afirma igualdad byte a byte tras el viaje de ida y vuelta. Es la propiedad
  que sostiene toda la feature: una slice posterior reenvía ese schema tal cual a
  `ToolDefinition::input_schema_override`, y el modelo plano de `ParameterProperty` no puede
  representar esas restricciones, así que aplanar acá sería pérdida silenciosa de datos.
  Antes no lo cubría ningún test: el que había afirmaba solo `len()` y `name`.
- **`rmcp_handshake_initialize_then_notified`** ahora afirma sobre los cuerpos reales de los
  requests que se enviaron `initialize` y `notifications/initialized`, **en ese orden**.
  Antes afirmaba solo `is_ok()`, que habría pasado en verde aunque se mandara únicamente el
  primero.

Ambos verificados rompiendo el código a propósito. Los de red en vivo y la matriz completa
de protocolo quedan para la slice siguiente.

**Limitación conocida, para la slice de reintentos.** Cuando un `timeout` dispara, soltar el
future **no cancela** el request en `rmcp`: su pool interno de responders conserva la entrada
hasta que llegue una respuesta que quizá nunca llegue. Como el cliente se va a cachear y
reusar, timeouts repetidos contra un servidor colgado hacen crecer ese pool. `rmcp` expone
`send_cancellable_request` para eso; se aborda junto con el reintento acotado, que toca este
mismo código.

**Estado.** done (slice 4 de 9).

---

## 21. Propiedad del timeout y regla de idempotencia en el cliente MCP (5/9)

**Qué cambió.** Nada observable todavía: nadie construye este cliente aún. Son dos
correcciones al adapter de la §20, ambas salidas de la revisión.

### El timeout ahora cancela de verdad

La §20 imponía el plazo con un `tokio::time::timeout` externo alrededor de
`list_all_tools`/`call_tool`. Eso acotaba al caller, pero **soltar el future no cancela el
request en rmcp**: su `local_responder_pool` conservaba la entrada hasta una respuesta que
podía no llegar nunca. Como el cliente se va a cachear y reusar, timeouts repetidos contra un
servidor colgado lo hacían crecer.

El arreglo que el diseño proponía —`send_cancellable_request` con `PeerRequestOptions::timeout`,
dejando que rmcp fuera dueño del plazo y de la limpieza— **no se sostiene empíricamente** con
rmcp 3.1.4 sobre `transport-streamable-http-client-reqwest`: su timer interno dispara a
horario, pero después **espera** una notificación de cancelación que queda serializada detrás
del request todavía en vuelo, así que el caller no recupera control hasta que ese request
termine igual.

Lo que sí funciona: usar `send_cancellable_request` solo para obtener un `Peer` y un
`RequestId` clonables antes de consumir el handle, envolver `await_response()` en nuestro
propio `tokio::time::timeout`, y al expirar disparar la cancelación como **tarea desprendida**
(`tokio::spawn`) en vez de esperarla. Esa detención es lo que devuelve control en el plazo
prometido sin dejar la entrada colgada.

`list_tools` ya no llama a `Peer::list_all_tools`: pagina `tools/list` por su cuenta, de modo
que cada página se acota y se cancela por separado.

Y el bucle lleva **su propio techo de páginas**, derivado de `MCP_MAX_TOOLS_PER_SERVER`. Eso
salió de la revisión: el `next_cursor` lo controla el servidor, así que sin ceiling un
servidor que siga devolviendo cursor hace girar el bucle para siempre, con cada página
diligentemente acotada y el total sin acotar — trabajo ilimitado manejado enteramente por el
otro lado. Peor caso real: `max_pages * timeout_seconds`.

### `tools/call` no se reintenta nunca

`tools/list` se reintenta una vez ante un fallo transitorio de transporte: solo lee, así que
correrlo dos veces cuesta un round trip y nada más.

**`tools/call` no se reintenta, ni siquiera ante un error de transporte.** Un reset de
conexión puede llegar **después** de que el servidor ya ejecutó la tool —un corte mientras se
lee la respuesta es indistinguible, en esta capa, de uno previo al envío— y `service_error`
mapea ambos a `Transport`. MCP no ofrece forma de declarar una tool idempotente, y las tools
que vale la pena exponer son justamente las que tienen efectos: una escritura, un envío, un
cobro. Un reintento ciego podía cobrar una tarjeta dos veces por una sola llamada del modelo.

Ahora el fallo se le devuelve al modelo, que puede reintentar sabiendo lo que arriesga.

**Compatibilidad.** Aditivo. Ninguna firma pública cambia, ningún binding se toca.

**Tests.** 13 en total; 5 nuevos y todos verificados rompiendo el código:

- `rmcp_call_tool_is_never_retried_on_transport_error` afirma que el servidor ve
  `tools/call` **exactamente una vez**. Volver a poner el reintento lo hace fallar.
- `rmcp_transient_transport_error_retries_list_tools_once_then_succeeds` cubre el caso que sí
  se reintenta. La inyección de fallo transitorio del mock se generalizó de un arm cableado a
  `tools/call` a un helper por método, para poder ejercitar ambos caminos.
- `rmcp_is_error_true_response_is_not_retried` fija que un `isError: true` llega como
  resultado con la bandera, no como reintento. Vale aclarar que eso se cumple
  **estructuralmente**: `call_tool` no pasa por `retry_transient`, así que no hay camino de
  reintento que saltear.
- `rmcp_list_tools_accumulates_across_pages` cubre el hilado del cursor y la acumulación
  entre páginas. La revisión notó que ningún test ejercitaba más de una página —todos los
  mocks devolvían la lista completa de una— así que el bucle nuevo no tenía cobertura
  alguna. El mock avanza de página **leyendo el cursor que mandó el cliente**, no contando
  requests: contar habría entregado la página 2 incluso a un cliente que dejara de enviarlo,
  y el test habría pasado mientras lo que nombra estaba roto. Verificado dejando de enviar el
  cursor — el cliente entonces pide la misma página hasta chocar contra el techo.
- `rmcp_list_tools_refuses_to_page_forever` fija el techo: un servidor que nunca deja de
  devolver cursor recibe un `Protocol` que dice por qué se paró, en vez de un bucle infinito.

**Tres limitaciones conocidas.**

El techo de páginas acota **páginas, no tools ni bytes**: un servidor que devuelva una sola
página con un array enorme de tools pasa igual. Acotar eso corresponde a la slice de
exposición, que es donde `MCP_MAX_TOOLS_PER_SERVER` cobra su sentido real — hoy no lo lee
nadie más, y el número se toma prestado a falta de uno mejor.

Los bloques de contenido que no son texto —imágenes, recursos embebidos— **se seguían
descartando en silencio** al plegar el resultado de un `tools/call`. Era el comportamiento que
ya tenía develop, y se conservó en esta slice porque manejarlos era trabajo de la siguiente.
**Corregido en §23** — ya no aplica: hoy cada bloque se convierte en un placeholder con
nombre.

Y un servidor genuinamente mudo —que acepta la conexión, nunca responde
y nunca resetea— traba de forma permanente el único worker compartido de rmcp. Cada llamada
sigue acotada por nuestro timeout, así que nadie se cuelga, pero el cliente cacheado para ese
servidor no se recupera en toda la vida del proceso.

Ninguna de las tres está activa: nada construye este cliente todavía.

**Estado.** done (slice 5 de 9).

## 22. `ExcelRenderer` producía bytes distintos para el mismo IR — fecha de creación fija

`ExcelRenderer::render_sync` (en
`src/libs/colmena/src/documents/infrastructure/render/excel_renderer.rs`) construía el
`Workbook` sin llamar nunca a `set_properties`. `rust_xlsxwriter` 0.77, ante la ausencia de
`DocProperties`, rellena `creation_time` con `ExcelDateTime::utc_now()` — que tiene resolución
de **un segundo** (trunca a `timestamp.as_secs()`). Ese valor se escribe en
`docProps/core.xml` como `dcterms:created` **y** `dcterms:modified` a la vez.

Consecuencia: renderizar el mismo IR dos veces producía bytes distintos cada vez que el
segundo boundary caía entre ambos renders. El test
`excel_renderer_output_is_deterministic_for_same_ir` (en
`src/libs/colmena/src/documents/application/apply_patch.rs`) pasaba casi siempre por pura
suerte de timing, y ya había cancelado el matrix de CI completo (7 jobs de Python, 3.8-3.14,
con `fail-fast` por defecto) en dos PRs distintas
(#209, #211) al caer justo sobre ese borde. El problema no era solo el flake: content
addressability es una promesa del sistema (mismo IR ⇒ mismos bytes), y la fecha de creación
de un artefacto generado por código no le sirve a nadie que lo consuma — no hay wall-clock
"real" que reportar honestamente.

**Fix.** Se fija la fecha de creación/modificación a una constante arbitraria
(`FIXED_CREATION_DATE`, 2026-01-01) vía `wb.set_properties(&DocProperties::new()
.set_creation_datetime(&created))`, con `ExcelDateTime::from_ymd` (no se habilitó el feature
`chrono` del crate — no era necesario y el proyecto lo excluye por defecto). Un solo
`set_creation_datetime` alimenta ambos campos de `core.xml`. Los timestamps de las entradas
del ZIP interno ya estaban fijados por la librería (`DateTime::default()` en
`packager.rs`), así que `core.xml` era la única fuente de no-determinismo.

**Test.** Nuevo `excel_renderer_output_is_deterministic_across_a_second_boundary` (mismo
archivo que el test preexistente) renderiza el mismo IR dos veces con un
`tokio::time::sleep(1100ms)` entre medio, forzando deliberadamente el cruce de un segundo —
falla de forma determinista contra el código anterior (no depende de la suerte de timing) y
pasa con el fix. Se verificó revirtiendo el fix localmente para confirmar que el test vuelve a
fallar, y reaplicándolo. No se agregó ninguna dependencia nueva (no hay crate de lectura de
ZIP entre las dev-dependencies del crate `colmena_dag_engine`, así que no se pudo hacer una
aserción directa sobre `docProps/core.xml`).

**Compatibilidad.** Aditivo, sin cambios de API pública ni de firma de `ExecutableNode` — solo
cambia el contenido de `docProps/core.xml` dentro de los archivos `.xlsx` generados. ADP no
afectado.

**Estado.** done.

## 23. Los bloques no-texto de un `tools/call` ya no se pierden en silencio (6/9)

Al plegar el resultado de un `tools/call` a la única `String` que puede llevar un mensaje de
tool-result, el cliente MCP se quedaba solo con los bloques `Text` y **descartaba el resto sin
dejar rastro** (`.filter_map` sobre `ContentBlock::Text`). Una tool MCP que respondiera con una
imagen, audio o un recurso binario entregaba una respuesta truncada que el modelo leía como
completa — que es peor que perder el contenido: lo perdía sin que nadie pudiera notarlo.

**Cambio.** Nueva función `content_block_to_text` que cubre las cinco variantes de
`ContentBlock` y convierte cada una en un placeholder **con nombre y acotado**:

| Variante | Resultado |
|---|---|
| `Text` | verbatim, sin pérdida |
| `Image` / `Audio` | `[image content omitted: image/png (N base64 bytes)]` |
| `Resource` + `TextResourceContents` | verbatim, sin pérdida (es contenido real, solo entregado bajo una uri) |
| `Resource` + `BlobResourceContents` | `[resource content omitted: <uri>, <mime>, N base64 bytes]` |
| `ResourceLink` | `[resource link omitted: <uri> (<name>)]` |

El placeholder nombra **qué** se omitió, no solo que algo se omitió: quien lee el resultado
puede decidir si le importa. Y **nunca** incluye el payload codificado — un blob base64
inyectado en el contexto del modelo cuesta una fortuna en tokens y no dice nada; el mime type y
el tamaño dicen todo lo útil.

**El techo.** Cada campo que el placeholder interpola —mime type, uri, nombre del recurso— lo
controla el **servidor**. Sin un tope, "te digo qué se omitió" se convierte en el mismo flood
de contexto que la omisión venía a evitar: un `uri` de un megabyte entra entero. Por eso todo
bloque renderizado —incluido un bloque de texto, que es igual de controlado por el servidor—
pasa por un techo de `MCP_MAX_CONTENT_BLOCK_BYTES` (4 KB) aplicado con `head_truncate`, el
mismo primitivo compartido que ya usa el resto del módulo, en vez de una variante nueva. Un
bloque recortado lo dice: conserva el marcador `[truncated: showing first N of M bytes]`, así
que nunca encoge en silencio.

La constante vive por ahora en `rmcp_http_client.rs`; su lugar natural es junto al resto de los
`MCP_MAX_*` en `llm::domain::mcp`, y se mueve allá cuando la slice de exposición toque ese
archivo.

`ContentBlock` y `ResourceContents` son `#[non_exhaustive]` en `rmcp`, así que ambos matches
llevan un brazo catch-all. Hoy está muerto —las cinco variantes actuales están cubiertas— pero
evita que el crate deje de compilar el día que `rmcp` agregue una sexta, con la misma política
de placeholder honesto.

**Tests.** Tres unitarios puros sobre `content_block_to_text` (cada placeholder nombra lo
elidido, empieza con `[`, se mantiene acotado y jamás reenvía el base64; el recurso de texto
embebido se preserva sin pérdida; y un servidor hostil que devuelve campos de 64 KB no logra
pasar el techo — verificado neutralizando el cap, que deja escapar 131 KB) y uno de protocolo
con wiremock: un `tools/call` que
mezcla un bloque de texto y uno de imagen debe surfacear los dos —el texto sin pérdida, la
imagen como placeholder— y no filtrar el base64. Verificados en rojo antes de implementar.

**Alcance.** Contenido enteramente dentro de `rmcp_http_client.rs`, el único archivo que puede
nombrar tipos de `rmcp`. Sin cambios en `llm/domain/mcp.rs` ni en la capa de aplicación, sin
dependencias nuevas, sin cambios de API pública → ADP no afectado.

Fuera de alcance, para la slice siguiente: la matriz de casos de protocolo
(`initialize` malformado, 0/1/N tools) y los dos tests de red contra servidores MCP reales.

**Estado.** done (slice 6 de 9).

## 24. Matriz de protocolo y pruebas contra servidores MCP reales (7/9)

Slice de **cobertura**, no de comportamiento: no cambia una línea de producción. Cierra los
huecos de prueba que las slices 4-6 habían ido dejando anotados como "para la siguiente".

**Matriz de protocolo** (wiremock, tres casos que antes no tenían ninguna prueba):

- **Catálogo vacío.** Un servidor sin tools que ofrecer es una respuesta normal, no un error.
  Sin esta prueba, nada impedía que exposición terminara reportando un servidor sano como roto.
- **Catálogo de un tool.** Fija el borde entre "vacío" y "paginado": un solo tool tiene que
  llegar entero y no disparar el loop de paginación.
- **`initialize` malformado.** Un envelope JSON-RPC bien formado cuyo resultado **no** es un
  `InitializeResult`. Es el caso incómodo: parsea, así que un cliente descuidado sigue adelante
  sobre una sesión que nunca negoció, y el problema reaparece después como un error confuso en
  `tools/list`. Ahora falla en el connect, que es donde se entiende. Requirió extender el mock
  con `with_malformed_initialize`.

**Dos pruebas en vivo** (`#[ignore]`, se corren con `cargo test -- --ignored` según la
convención del repo para tests que necesitan red):

- **DeepWiki** (`https://mcp.deepwiki.com/mcp`) — servidor **stateless**, no emite
  `mcp-session-id`. Lista tools, encuentra `read_wiki_structure` y lo llama de verdad,
  exigiendo texto real de vuelta.
- **HuggingFace** (`https://huggingface.co/mcp`) — servidor **stateful**. Si el transporte no
  devolviera el `mcp-session-id` que el servidor emite en el `initialize`, el segundo
  round-trip sería rechazado.

La distinción importa y es la razón de ser de esta slice: **todo el resto de los tests son
mocks, y un mock prueba que hablamos el protocolo como nosotros creemos que funciona.** Solo
estos dos prueban que lo hablamos como lo habla un servidor real. Ambos verificados corriendo
en vivo, no solo escritos.

**Estado.** done (slice 7 de 9).

## 25. Superficie de configuración MCP y validación fail-closed (8/9)

Primera pieza del lado `dag_engine`: hasta ahora todo el código MCP vivía en `llm/` y el motor no
sabía que existía. Esta slice declara **qué es un config MCP válido** y lo rechaza al cargar el
grafo. No abre conexiones — eso es la slice siguiente.

**Superficie** (`McpServerSpec`, presente solo cuando `node_type: "mcp"`):

| Campo | Default | Nota |
|---|---|---|
| `url` | — (obligatorio) | debe ser HTTPS |
| `transport` | `streamable_http` | o `sse` |
| `headers` | `{}` | valores tal como están en el grafo: normalmente referencias sin resolver |
| `timeout_seconds` | 30 | deadline por llamada |
| `cache_ttl_seconds` | 300 | sigue la convención por-node-config existente (`tavily_client`) |

El tipo se define acá pero **todavía no se monta como campo de `ToolConfiguration`**: la validación
trabaja sobre JSON crudo por diseño, y el único consumidor del campo tipado es el constructor de
bindings de la slice siguiente. Llega junto con quien lo lee.

**Validación fail-closed** en `Graph::validate`, sobre JSON crudo, espejando `validate_memory_mode`
en vez de inventar un mecanismo paralelo. Tres rechazos, cada uno porque el fallo alternativo es
**silencioso**:

1. `node_type: "mcp"` sin `mcp.url` — un tool MCP sin dirección no expone nada, y el operador lo lee
   como "el modelo ignoró mi servidor", no como un config roto.
2. URL no-HTTPS — estas conexiones llevan headers con credenciales. Se rechaza al cargar, no al
   conectar; `RmcpHttpClient::connect` lo revalida antes de tocar un socket (R2.1).
3. Un bloque `mcp` sobre un tool que **no** es MCP — config muerto que el operador cree activo. Esto
   **no** estaba en el diseño: se agrega porque es la misma clase de fallo que un `memory_mode` mal
   ubicado.

El chequeo de scheme es case-insensitive (RFC 3986): rechazar `HTTPS://` sería un falso rechazo.

**`Debug` redactado en `McpServerConfig` (G3).** `header_refs` está pensado para referencias sin
resolver, pero nada impide que un operador pegue un token literal en el grafo, y un `Debug` derivado
lo imprimiría en cualquier log. Verificado en rojo: el test mostraba `Bearer sk-live-SECRET`. Ahora
**todos** los valores pasan a `***` sin intentar adivinar cuáles son secretos — una redacción que
depende de reconocer el secreto falla justo con el que no reconoce. Los **nombres** de header
sobreviven: quien depura autenticación necesita verlos, y un nombre no es una credencial.

**Alcance.** Aditivo, sin API pública nueva, sin dependencias, sin conexiones. Un grafo sin entradas
MCP no ve diferencia (G2).

**Estado.** done (slice 8 de 9).

## 26. Identidad de conexión MCP (`McpServerKey`)

Segunda pieza del lado `dag_engine`. La anterior declaraba qué config es válido; esta define
**quién es un servidor y cuándo dos configuraciones son el mismo servidor**. El registry que la
consume para reusar conexiones llega en la slice siguiente.

**`McpServerKey` = sha256(url ‖ transport ‖ fingerprint de las REFERENCIAS de header).** Nunca de
los valores resueltos (R3.6). La distinción es el punto entero: dos grafos que apuntan la misma
referencia al mismo secreto comparten conexión, y **rotar el secreto no fragmenta el pool**. Una
clave construida sobre valores resueltos además significaría que la credencial en texto plano
decide la ubicación en el cache — a un accidente de quedar logueada como cache key.

**Cada campo se absorbe con un prefijo de longitud (`u64`), no separado por un byte.** La primera
versión usaba `0x1F` como separador, asumiendo que no podía aparecer dentro de una URL, un nombre de
header ni una referencia. Esa premisa es falsa: son strings escritos por el operador que salen del
JSON del grafo, y JSON codifica cualquier byte, `\u001F` incluido. Con separadores planos, los dos
headers `{"A":"1","B":"2"}` y el header unico `{"A":"1\u001FB\u001F2"}` producen **la misma
preimagen** — dos conjuntos de credenciales distintos, una sola conexion del pool, y el segundo
llamador mandando los headers del primero.

El framing por longitud elimina la ambiguedad **por construccion**, en vez de asumir algo sobre los
datos. Encontrado por el test `a_separator_byte_inside_a_header_cannot_forge_another_configs_identity`,
que fallo contra la implementacion con separadores y pasa con el framing.

La URL y el transporte también participan: cambiar cualquiera de los dos es otro servidor. Agregar
un header, o intercambiar cuál string es el nombre y cuál la referencia, también cambia la identidad.

El orden de los headers no cambia la clave, pero eso es **estructural, no verificado por un test**:
`header_refs` es un `BTreeMap`, así que dos configs escritos en distinto orden ya son el mismo mapa
antes de llegar al hash. Un test que los comparara no podría fallar con ninguna implementación
determinista — estaría afirmando una propiedad de `BTreeMap`, no de este módulo. Se deja dicho en un
comentario en vez de simulado con un test vacío.

**Alcance.** Módulo nuevo, aditivo, puro — sin red, sin estado, sin API pública cambiada.

## 27. Pool de conexiones MCP: una conexión por identidad de servidor

Una conexión MCP es cara: handshake TCP+TLS más un round-trip JSON-RPC `initialize` antes de poder
listar un solo tool. El `DagToolExecutor` se construye **de nuevo en cada ejecución de `llm_call`**,
así que la conexión no puede vivir ahí — cada turno de cada agente pagaría el handshake completo.
`McpConnectionRegistry` sobrevive a las ejecuciones y devuelve el mismo cliente para la misma
`McpServerKey`.

**El lock por clave no es defensivo, es la razón de ser del módulo.** Sin él, N llamadores que
llegan a la vez sobre una clave fría fallan todos el fast path y cada uno abre su propia conexión:
N handshakes y N-1 conexiones huérfanas. Verificado quitándolo: 16 tareas concurrentes producen 16
handshakes; con el lock, **uno**. El re-chequeo dentro del lock es igual de necesario — sin él el
lock serializaría los handshakes pero los haría todos igual.

**Un connect fallido no se cachea.** Un servidor caído en el primer intento tiene que estar
disponible en el turno siguiente; cachear el fallo convertiría una caída transitoria en permanente
para toda la vida del proceso.

**`McpConnector` es un puerto, no una llamada directa a `RmcpHttpClient::connect`.** Eso permite
probar el pooling por lo que realmente es —un problema de concurrencia y cacheo— sin abrir un
socket. Wiremock prueba que hablamos el protocolo; no puede probar que dos llamadores compitiendo
por una clave fría produzcan un solo handshake.

**Desviación deliberada de `pool_registry`** — **[CORREGIDO en §33: TODO este párrafo, premisa
incluida, quedó obsoleto. No es un matiz sobre una sola frase.]** El texto original decía: ahí las
entradas de `creation_locks` se borran tras crear, porque esa registry keyea sobre URLs arbitrarias
de base de datos y debe acotar el crecimiento; acá las claves son servidores MCP declarados por el
operador —un puñado, por toda la vida del proceso— y borrar abriría una ventana donde un waiter
tardío y un llamador nuevo sostienen dos mutexes distintos para la misma clave, lo cual sería
inofensivo porque el re-chequeo lo atrapa.

Las **dos** afirmaciones eran falsas:

1. **«Un puñado, por toda la vida del proceso» ya no vale.** El scope por credencial (§30, PR #222)
   hizo que la cardinalidad de claves escale con las sesiones concurrentes, no con la cantidad de
   servidores que el operador declaró. Esa es la premisa que justificaba no acotar el crecimiento, y
   es la que motiva la evicción de §33.
2. **El re-chequeo NO atrapa la ventana.** Los dos llamadores sostienen mutexes independientes,
   ninguno ve el insert del otro, y ambos conectan. El review lo marcó CRITICAL.

Todavía **no hay singleton de proceso**: nada lo dereferencia aún, así que el `Lazy` llega con su
cableado. El cache TTL de `tools/list` y la resolución de headers vía secure values son las piezas
siguientes.

**Alcance.** Módulo nuevo, aditivo, sin red y sin API pública cambiada.

**Estado.** done.

## 28. Cache TTL del catálogo de tools MCP

Bajo lazy loading la etapa de exposición corre **en cada iteración del loop del agente**. Sin cache,
cada turno paga un round-trip `tools/list` por un catálogo que casi nunca cambia.
`McpConnectionRegistry::tools` lo sirve desde cache mientras esté dentro de `cache_ttl`.

**Single-flight, no solo cache.** Sobre una entrada fría o recién vencida, los turnos concurrentes
dispararían cada uno su propio `tools/list` — una estampida contra el servidor justo en el momento
en que el cache rota. Verificado neutralizando el lock: 16 lectores concurrentes producen **16**
`tools/list`; con él, **uno**. El re-chequeo dentro del lock es igual de necesario: sin él el lock
serializaría los fetches y los haría todos igual.

El lock de fetch es **separado** del de creación de conexión, para que refrescar un catálogo nunca
quede serializado detrás de un handshake ajeno.

**Un `tools/list` fallido no se cachea**, por la misma razón que un connect fallido: un mal momento
dejaría el catálogo del servidor en blanco hasta que venciera el TTL.

**`tokio::time::Instant`, no `SystemTime`.** Es monótono, así que un salto de reloj de pared
—corrección NTP, un contenedor suspendido que despierta— no puede hacer que una entrada parezca más
vieja o más nueva de lo que es. Y es virtualizable: la expiración se prueba **adelantando un reloj
pausado**, no durmiendo. Difiere del precedente de `search_use_case`, que usa `Utc::now()` y por eso
no puede testear expiración sin esperar de verdad. (`test-util` se agregó a las dev-dependencies de
`tokio`; el `Cargo.lock` no cambió — es solo un flag de feature.)

La comparación es `elapsed() >= ttl`, así que **`cache_ttl: 0` significa "nunca cachear"**, no
"cachear para siempre". Un TTL cero es una elección legítima del operador y no debe leerse como un
acierto permanente accidental.

**Alcance.** Aditivo sobre el registry, sin red, sin API pública cambiada.

**Estado.** done.

## 29. Los headers de autenticación MCP llegan al servidor

**Bug real, no una mejora.** `RmcpHttpClient::connect` construía el transporte con
`with_uri(url)` y nada más: `config.header_refs` **se ignoraba por completo**. Un servidor MCP con
`Authorization` conectaba sin el header y devolvía 401, sin ninguna pista de que el header
configurado nunca había salido del proceso. La superficie de config existía desde §25; el cable no.

**Los valores resueltos viajan aparte del config.** `connect` toma ahora un tercer argumento,
`resolved_headers`, en vez de leerlos de `McpServerConfig`. Así la propiedad "una credencial
resuelta nunca toca la cache key, el `Debug` ni un log" es **estructural** y no una disciplina que
alguien tenga que recordar: el tipo que se hashea y se imprime simplemente no puede contenerla.

Es un cambio de firma sobre `pub fn connect`. Se hace igual y se dice: la API se introdujo en esta
misma cadena, no tiene consumidores y no está cableada. Dejar una firma que descarta los headers de
auth en silencio es peor que cambiarla ahora.

**Rechazo fail-closed al conectar, no en la primera tool call.** `rmcp` valida los headers
reservados **por request**, así que un operador que setee `Mcp-Session-Id` no se enteraría al cargar
ni al conectar, sino como un fallo de transporte oscuro más tarde. Se rechazan acá —`accept`,
`mcp-session-id`, `last-event-id`— nombrando el header. `MCP-Protocol-Version` está reservado
upstream pero permitido a propósito (el worker lo inyecta post-init), así que no se lista.

Un nombre o valor de header inválido también se rechaza nombrando el header, **sin citar el valor**:
el valor es el secreto resuelto, y un mensaje de error es exactamente el lugar por donde se filtra.

**Verificado contra el servidor, no contra la función.** El test asserta que el header llega en
**todas** las requests capturadas por wiremock, no solo en el handshake — un header que se mandara
únicamente en el `initialize` dejaría toda llamada posterior sin autenticar. Probado quitando la
entrega y dejando la validación: el test falla.

`http = "1"` pasa a dependencia directa: `HeaderName`/`HeaderValue` de rmcp son de **http 1.x**, no
del **http 0.2** que arrastra nuestro reqwest 0.11 — usar el tipo equivocado ni siquiera compila. No
entra ningún crate nuevo al `Cargo.lock`: `http 1.4.0` ya estaba ahí vía rmcp, y el diff es **una
línea**, la arista directa de `colmena_dag_engine` hacia él.

**Alcance.** Sin conexiones nuevas; todavía sin caller. La resolución de las referencias vía secure
values es la pieza siguiente — hoy el llamador debe pasar valores ya resueltos.

**Estado.** done.

## 30. Dos sesiones no comparten conexión MCP cuando los headers llevan credenciales

**Corrección a §26.** La `McpServerKey` hasheaba url + transporte + las **referencias** de header, a
propósito: así rotar un secreto no fragmenta el pool. Ese razonamiento asumía que una referencia
significa lo mismo en todas partes. **No es así.**

Los handles de secure values son `<value_1>`, `<sv_admin_token>` — contadores y nombres, sin nada
único por sesión — y `decrypt(session_id, agent_session_id, handle)` resuelve **el mismo handle a
secretos distintos según la sesión**. Dos sesiones de agente corriendo el mismo grafo producen
`header_refs` idénticos, por lo tanto la misma clave, por lo tanto **comparten una conexión del
pool**: la segunda sesión manda la credencial de la primera.

Es la misma clase de bug que el framing por longitud arregló en §26 —dos credenciales, una
conexión— reapareciendo una capa más arriba.

**Arreglo.** `from_config` toma ahora un `CredentialScope { session_id, agent_session_id }`, que
participa en la clave **solo cuando algún valor de header es una referencia**.

El principio, que la primera versión de este arreglo no tenía: **la clave debe particionar el pool
exactamente como el descifrado particiona los secretos.** `decrypt` resuelve por agente cuando hay
`agent_session_id` y **estrictamente por `session_id`** cuando no —el modo *session-only* legítimo y
documentado— así que la clave hace lo mismo. Un scope que solo llevara el id de agente colapsaría
todos los runs session-only a una clave y volvería a compartir credenciales; el review lo marcó como
CRITICAL antes de mergear, y ahora hay un test por cada mitad.

Se absorben **dos** campos, un discriminante y un id, así `("agent","")` y `("none","")` siguen
siendo preimágenes distintas y un id vacío no puede colisionar con el caso sin scope. Consecuencias, elegidas a
conciencia:

| Config | Comportamiento |
|---|---|
| Sin headers (servidor público) | pool **global**, sin fragmentar |
| Headers literales | pool **global** — es el mismo secreto en todas partes, compartir es correcto |
| Headers con referencia | aislado **por agent session** |

Por `agent_session_id` y no por `session_id`: una conversación reusa su conexión entre turnos —igual
que keyea la memoria conversacional— mientras que dos agentes distintos nunca comparten credencial.
Aislar por run habría cambiado una fuga de credenciales por un handshake en cada turno.

El scope se absorbe **length-framed** como cualquier otro campo, antes de los headers, así que no
puede confundirse con contenido de header.

**El predicado de placeholder se extrajo, no se duplicó.** `is_secure_value_placeholder` es ahora la
definición única; `collect_placeholders` delega ahí. Una segunda copia de esa regla terminaría
desincronizándose, y el modo de fallo de esa desincronización es exactamente que una sesión reuse la
credencial de otra.

`from_config` cambia de firma en vez de ganar una variante segura al lado: un llamador no debe poder
obtener la clave insegura por descuido. No hay consumidores en producción todavía.

**Verificado.** Neutralizando el scope, el test entre sesiones falla — el bug reproducido, no
argumentado. Y colapsando **solo el discriminante**, dejando los ids intactos, falla el test del id
vacío: aísla exactamente el campo que dice aislar.

Cuatro tests de este archivo fueron reescritos durante el review por el mismo defecto: comparaban
dos valores que diferían en **más de una cosa**, dejando que una diferencia ajena cargara la
aserción. Las versiones finales mantienen todo constante y varían un solo campo, y cada una se
verificó mutando exactamente ese campo. El detalle por test está en sus doc comments.

**Dos comentarios pre-existentes corregidos.** El doc de `inject_secrets` y el del mock de tests
decían que la resolución es "agent-first con fallback a sesión". No lo es:
`PostgresSecureValueRepository::decrypt` es un if/else sobre dos `WHERE` mutuamente excluyentes, sin
fallback. El efecto que describían —que un resume con un `session_id` nuevo encuentre secretos del
mismo agente— sí ocurre, pero por **precedencia** del agente, no por fallback. La diferencia importa
para cualquier cosa que particione sobre esos ids, que es exactamente lo que hace esta clave. El
mock, además, **sí** implementa el fallback que producción no tiene, así que ahora lleva una
advertencia de no razonar sobre particionamiento leyéndolo.

**Cardinalidad, dicho de frente.** El scope cambia el orden de magnitud del pool: antes era una
conexión por servidor en todo el proceso, ahora es una por sesión de agente para los servidores con
credencial. `McpConnectionRegistry` no tiene evicción, ni tope de entradas, ni timeout de
inactividad —`pool_registry` tiene los tres— y eso pasa de ser defendible a ser un hueco que **hay
que cerrar antes de cablear** la registry a un executor. Queda anotado en el código, no solo acá.

**Estado.** done.

## 31. Errores tragados en memoria de tareas — cierre parcial del finding #18 (mitad A)

**Qué cambió.** `information_extraction` y `task_memory_writer` dejan de reportar
como exitosas operaciones de memoria de tareas que nunca ocurrieron.

Los dos nodos tenían el **mismo bloque duplicado** —el audit solo había registrado
el de `extraction.rs`— con dos silencios:

```rust
repo.add_task(&new_task).await?;          // propagaba
let _ = repo.delete_task(id_str).await;   // tragaba

if let Ok(tasks) = repo.get_tasks_for_run(&session_id).await { ... }  // tragaba
```

La asimetría del primer par estaba dentro del mismo `if let Some(repo)`: `add`
propagaba, `delete` no. El segundo era peor: un fallo de lectura producía
`all_tasks = []`, y en `task_memory_writer` esa lista **es el `default_output` del
nodo**. Un hipo transitorio de Postgres se leía río abajo como *"esta sesión no
tiene tareas pendientes"*, y el orquestador ruteaba sobre eso.

**El bloque ahora vive una sola vez** en
[`nodes/task_mutations.rs`](../src/libs/colmena/src/dag_engine/infrastructure/nodes/task_mutations.rs)
(`apply_critic_mutations` + `fetch_session_tasks`), que es lo que impide que un
fix vuelva a aterrizar en una sola de las dos copias.

**Dos clases de fallo, no una.** El E2E encontró que propagar todo por igual
rompía un caso legítimo: `delete_task` valida que el id sea un UUID
([`postgres_dag_state_repository.rs:385`](../src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_dag_state_repository.rs)),
y colapsaba esa validación de input con los errores de base en el mismo
`StateError`. Como `add_task` **genera** su UUID y `delete_task` lo **recibe** del
modelo, solo el borrado puede fallar por input — y un crítico que alucina un id es
rutinario. La política quedó así:

| Situación | Comportamiento |
|---|---|
| Base caída (insert, delete o lectura) | propaga, el run falla |
| Lista de tareas ilegible | propaga — una lista vacía es una afirmación sobre la sesión |
| Id de `delete_tasks` inválido | se omite y se **reporta** en `extra_info.skipped_deletes` + `warn` estructurado |

Para que el puerto pueda expresar la diferencia se agregó
`DagError::InvalidTaskId`, distinta de `DagError::StateError`. El defecto que
cierra este cambio era el **silencio**, no la supervivencia.

**Documentación de referencia.**
[§20 Orchestrator](developer_guide/20_orchestrator_architecture.md) ("Fallos al
escribir memoria de tareas"),
[node_ports_reference](agent_context/node_ports_reference.md),
[FINDINGS_LEDGER](agent_context/audit/FINDINGS_LEDGER.md) #18.

**Verificación.** 2303 tests unitarios (3 nuevos en `task_mutations`), clippy
limpio, y E2E contra Postgres real con
[`tests/graphs/advanced/task_memory_error_propagation.json`](../tests/graphs/advanced/task_memory_error_propagation.json):
`information_extraction` inserta las tareas que pidió el crítico,
`task_memory_writer` las lee de vuelta, y un `delete_tasks` con id inválido
aparece en `skipped_deletes` sin matar el run.

**Alcance.** Solo la mitad **A** del finding. Las mitades **B**
(`crdt_doc_run_python`: `cells_written` cuenta escrituras fallidas) y **C**
(`crdt_doc_tools`: `record_event` fallido → id `0`, y cursor reseteado a `0` por
un error de DB) quedan como issues
[#181](https://github.com/Startti/colmena/issues/181) y
[#182](https://github.com/Startti/colmena/issues/182), congeladas bajo el freeze
de CRDT Documents.

**Estado.** partial — mitad A cerrada; B y C diferidas al descongelamiento de CRDT.

---
---

## 32. `information_extraction` reportaba éxito sin extraer nada — y rompía dos grafos del propio repo

**Qué cambió.** El nodo `information_extraction` junta sus documentos **solo** de inputs cuya key
empieza con `texts.` (lo que produce un edge `{"from": "src", "to": "<nodo>.texts.<nombre>"}`) o del
objeto estático `config.texts`. Cuando ninguno de los dos aportaba nada, hacía:

```rust
colmena_log!("⚠️ [ExtractionNode] Skipped execution because 'texts' input was missing or empty.");
return Ok(Value::Null);
```

Ahora distingue **dos casos que antes se veían iguales desde adentro del nodo**:

| Caso | Comportamiento |
|---|---|
| No hay ninguna fuente declarada (ni un input `texts.*`, ni `config.texts`) | `Err` que nombra el cableado esperado, lista las keys que sí llegaron y dice si había `config.texts` |
| Hay fuente declarada pero toda resolvió a null o vacío | `Ok(null)` — el comportamiento previo, sin cambios |

La discriminación es por **declaración**, no por contenido. Esa distinción no estaba en la primera
versión de este fix y la encontró la lente de review — ver "Lo que corrigió el review" más abajo.

**Por qué importaba.** `Ok(Value::Null)` es "corrí bien y mi salida es null", no "no corrí". El motor
lee ese null y emite `node-skipped` con `reason: upstream_null_output` para **toda** la rama
downstream. El run terminaba con exit 0, `finishReason: "stop"` y 0 tokens. La única señal era un
`colmena_log!` invisible sin logging verbose — al stream SSE no llegaba **nada**. Es la misma clase
de defecto que el finding #18 (reportar éxito por trabajo que no ocurrió), en otra instancia del
mismo archivo.

El efecto práctico: un edge mal escrito era **indistinguible** de un run correcto.

**Dos grafos del repo estaban mal cableados por esto**, y no se había notado nunca:

| Grafo | Edge que tenía | Edge correcto | En esta PR |
|---|---|---|---|
| [`tests/graphs/agents/extraction_example.json`](../tests/graphs/agents/extraction_example.json) | `{"from": "slack_message", "to": "extract_info"}` | `{"to": "extract_info.texts.slack_message"}` | **arreglado y verificado E2E** |
| [`tests/graphs/advanced/trip_planner.json`](../tests/graphs/advanced/trip_planner.json) | `{"from": "trigger", "to": "planner"}` | `{"to": "planner.texts.request"}` | **NO tocado** — ver abajo |

Ambos son pre-existentes (verificado contra un checkout limpio), no los causó trabajo reciente. Las
copias viejas en `src/libs/colmena/tests/` **tampoco funcionan**, aunque fallan distinto: usan
`config.data` con `from: "<nodo>.output"`, y como un `input` con `data` string devuelve el string
crudo, el pointer `/output` no resuelve y el motor skipea el nodo con `reason: pointer_unresolved`
(comprobado corriéndolo). O sea que no hay ninguna copia sana de la que copiar el cableado — el
ejemplo de este repo nunca anduvo en ninguna de sus formas.

**`trip_planner.json` queda sin tocar a propósito.** Se intentó arreglarlo y se encontró que el
cableado `texts.` es apenas el primero de **cuatro** defectos independientes, cada uno destapado al
arreglar el anterior — se recorrieron los cuatro con el motor real antes de revertir:

1. `{"from": "trigger", "to": "planner"}` sin prefijo `texts.` → el planner no recibía nada.
2. El edge `{"from": "trigger.plan", "to": "state_merger.injected_plan"}` es **irresoluble por
   construcción**: un nodo `input` con config no vacía emite exactamente las keys que declara, y
   `plan` no es una de ellas (`input.rs`, rama "config has declared keys"). El motor skipeaba
   `state_merger` con `reason: pointer_unresolved`.
3. El script de `state_merger` lee `llm_plan['output']`, pero el payload de un
   `information_extraction` es el JSON parseado según su `schema` — acá `{"type": "array", "items": [...]}`,
   sin key `output`. Devolvía `None` y el plan quedaba vacío.
4. El nodo `orchestrator` tiene `config: {}`, sin bloque `agents`, así que aun con un plan válido
   corta con `Configuration for agent 'clothing_expert' not found in orchestrator config`.

Los defectos 2-4 no son de cableado sino de diseño del grafo, y arreglarlos exige decidir la config
de agentes del orchestrator. Queda como trabajo aparte (ledger, finding #66): con este cambio ese
grafo pasa de terminar en silencio con exit 0 a fallar con un mensaje accionable en el planner, que
es exactamente la mejora buscada.

**No hay grafo que dependa del retorno null.** Se revisaron las tres definiciones del repo que usan
`information_extraction`: `product_sales_assistant_cards.json` ya cableaba `parse_cards.texts.sales_response`
correctamente, y los otros dos estaban rotos, no apoyados en el null. En `trip_planner.json` el
`state_merger` tolera un `llm_plan` nulo por su `try/except`, pero eso nunca se ejercitaba: el motor
skipeaba el propio `state_merger` antes de llegar ahí.

**Lo que corrigió el review.** La primera versión disparaba el error cuando el **texto formateado**
quedaba vacío, no cuando faltaba la **declaración**. La lente `review-reliability` mostró que eso
rompía grafos correctamente cableados: `extraction.rs:109` ya descartaba los valores null bajo
`texts.` (`Value::Null => continue`, código pre-existente — nadie escribe esa rama para un caso
imposible), y el motor **sí** puede entregar un null ahí. Con un edge sin punto `{"from": "http_node",
"to": "extract.texts.api_response"}` y un `http_request` que contesta 204:

- `run_use_case.rs:967` skipea la rama solo si el output upstream es null **entero**, y
  `{"status": 204, "body": null}` no lo es;
- `run_use_case.rs:973-975` pone `has_data = true` incondicionalmente para un `from` sin punto;
- `run_use_case.rs:1237-1251` inserta `Value::Null` cuando el `default_output` resuelto es null.

O sea: el nodo se ejecutaba con `texts.api_response = null`, el texto quedaba vacío, y la primera
versión mataba el run entero con un mensaje que mandaba al operador a arreglar un cableado correcto.
Era una regresión introducida por el propio fix. Peor: uno de los unit tests **fijaba** ese
comportamiento, o sea le daba apariencia de decisión deliberada.

**Segundo hallazgo del review: el `to` correcto no alcanza.** La lente encontró que el grafo ya
recableado seguía entregando basura al LLM. Un nodo `input` con claves declaradas emite el **objeto
completo** (`input.rs`, rama "config has declared keys"), no el string; con un `from` sin path el
resolver cae al fallback `is_object()` de `run_use_case.rs:1237-1251` y pasa ese objeto entero, y
`extraction.rs` lo serializa por la rama `_ => val.to_string()`. Resultado: el modelo recibía
`{"slack_message":"Hi team..."}` bajo el header `# slack_message` en vez del texto.

Se veía bien porque **funcionaba**: el LLM parsea JSON sin quejarse y la extracción salía correcta.
Ese es exactamente el motivo por el que pasó desapercibido — el resultado no delata el defecto. El
cableado final apunta al campo, no al nodo:

```json
{ "from": "slack_message.slack_message", "to": "extract_info.texts.slack_message" }
```

Comprobado con el motor, no deducido: con el path el frame `node-start` muestra
`"texts.slack_message":"Hi team..."` (string); sin él, `"texts.email_body":{"email_body":"..."}`
(objeto).

**Tests.** Tres unit tests en `extraction.rs`, todos offline y deterministas (el guard corre antes de
cualquier llamada de red): sin fuente declarada → `Err` nombrando el cableado y listando las keys
recibidas, con el plumbing interno `__colmena_*` / `session_id` filtrado; `texts.<name> = null` bien
cableado → `Ok(null)`, no error; `config.texts` declarado pero vacío → `Ok(null)`.

**Verificado con el motor real**, no solo con unit tests — los cuatro estados, vía
`cargo run --bin dag_engine -- run`:

1. **Antes del fix**, `extraction_example.json` con su cableado original → `"extract_info": null`,
   `node-skipped` con `reason: upstream_null_output` en `log_result`, `finishReason: "stop"`,
   `totalTokens: 0`. Éxito silencioso.
2. **Después del fix**, mismo grafo ya recableado → el frame `node-start` muestra
   `"texts.slack_message":"Hi team..."` (string limpio, no un objeto serializado), `result` poblado
   (`{main_objective, dead_line, people_assigned}`), `log_result` corre, cero frames `node-skipped`.
3. **Sin fuente declarada**, con una copia del grafo re-rota a propósito → frame SSE
   `{"type":"error","errorText":"... [information_extraction] no text sources to extract from ..."}`
   con `Input keys received: [email_body, slack_message]`. El error llega al stream, que era
   exactamente lo que faltaba.
4. **Fuente declarada que resolvió vacía** — el escenario que encontró el review, reproducido con un
   `http_request` real contra `https://httpbin.org/status/204` y el edge sin punto
   `{"from": "empty_source", "to": "extract_info.texts.api_response"}` → `"output": null`,
   `node-skipped` con `reason: upstream_null_output` en `log_result`, y **ningún frame de error**.
   O sea: la regresión no está, comprobado contra el motor y no solo con un unit test.

```bash
cargo run --bin dag_engine -- run tests/graphs/agents/extraction_example.json \
  --agent-session-id agent_extraction_fix_001
```

**Alcance y lo que queda afuera.** Cambio de comportamiento acotado a `information_extraction` y a un
solo caso: un grafo **sin fuente declarada** que antes "pasaba" ahora falla — que es el punto. Un
grafo correctamente cableado se comporta exactamente igual que antes, incluso cuando su upstream no
produce nada. Sin cambio de API pública ni de
formato de wire → **ADP no afectado** salvo que tenga un grafo persistido con este mismo error de
cableado, en cuyo caso pasa de romperse en silencio a romperse con un mensaje accionable.

- `critic` y `reactor` comparten el patrón `texts.*` con la misma forma de skip. **No** se tocaron —
  cada uno merece su propia verificación E2E.
- `tests/graphs/advanced/trip_planner.json` sigue roto por los defectos 2-4 de arriba (finding #66).
- El finding #18 (mitad A) **ya no está abierto en este archivo**: lo cerró la §31, que extrajo el bloque de
  mutaciones a `task_mutations.rs` y reemplazó el `let _ = repo.delete_task(...)` por un `match` real. Esta
  sección se escribió cuando esa PR todavía no estaba en `develop`; se corrigió al rebasar. Lo que sí sigue
  abierto en la misma función es el finding #20 (strip de comillas por slicing del output de serde).

**Documentación de referencia.**
- Schema canónico: [`docs/node_configurations.json`](node_configurations.json) — `information_extraction`
  (descripción, `texts.<name>`, `result`).
- Guía: [`docs/developer_guide/12_dag_engine_guide.md`](developer_guide/12_dag_engine_guide.md) — Ejemplo 4,
  con el aviso sobre el prefijo y el ejemplo alineado al grafo verificado.
- Comparativa: [`docs/developer_guide/37_router_and_output_parser.md`](developer_guide/37_router_and_output_parser.md)
  — `information_extraction` ya no "skipea en silencio".
- Puertos: [`docs/agent_context/node_ports_reference.md`](agent_context/node_ports_reference.md).
- Ledger: [`docs/agent_context/audit/FINDINGS_LEDGER.md`](agent_context/audit/FINDINGS_LEDGER.md) — Batch 14, findings #65 (este fix) y #66 (`trip_planner.json`, abierto).

**Estado.** done.
## 33. Evicción LRU en el pool de conexiones MCP

Cierra el hueco que el §30 dejó declarado en el código: `McpConnectionRegistry` no tenía evicción,
ni tope de entradas, ni timeout de inactividad, mientras que `pool_registry` tiene los tres. Mientras
la clave era una por servidor declarado eso era defendible; desde que incorpora el scope por
credencial, la cardinalidad escala con **sesiones concurrentes**, y dejarlo así habría sido cablear
la registry sobre un mapa que crece sin límite.

**Se evicta la huella completa de la clave** —cliente, catálogo cacheado y los dos mapas de locks—,
no solo el cliente. Borrar únicamente el cliente dejaría los otros tres creciendo igual, que es
exactamente el problema.

**Se revierte una decisión anterior, y el primer intento de revertirla tenía un bug.** Las entradas
de `creation_locks` antes no se borraban nunca, justificado en que la cardinalidad era "un puñado de
servidores declarados". Esa premisa cayó con el scope por credencial.

La primera versión las borraba incondicionalmente, con un comentario que afirmaba que la ventana
resultante era inofensiva porque "el re-chequeo la atrapa". **Era falso, y el review lo marcó como
CRITICAL.** Si un waiter ya tiene un clon del `Arc` viejo y la entrada se borra, un llamador nuevo
crea un mutex **independiente** para la misma clave: los dos mutexes no se serializan entre sí, los
dos pasan el re-chequeo mientras `clients` está vacío, y los dos conectan. Dos conexiones vivas para
una clave — justo el invariante que la registry existe para sostener.

El arreglo: una entrada de lock se borra **solo cuando la registry es su única referencia**, vía
`remove_if`, cuyo predicado se evalúa bajo el lock del shard, así que nadie puede clonar el `Arc`
entre el chequeo y el borrado. Un lock contendido sobrevive esa pasada y lo recoge después
`sweep_orphan_locks` — sin ese barrido quedaría para siempre, porque la evicción nunca vuelve sobre
una clave que ya sacó del LRU.

`DEFAULT_MAX_POOLED_SERVERS = 128`, configurable con `with_max_entries`. Un tope de 0 se clampea a 1:
sin eso el pool evictaría cada conexión al instante de crearla, convirtiéndose en un no-op que
re-handshakea en cada llamada.

**Un hit de catálogo cuenta como uso.** El review también marcó que los aciertos de cache en
`tools()` no refrescaban el rango LRU. Bajo lazy loading la etapa de exposición llama a `tools` en
cada iteración del loop del agente y puede no volver a llamar a `client` nunca, así que un servidor
en uso constante se hundía hasta el fondo del LRU y podía ser evictado **antes** que uno realmente
ocioso — el orden exactamente al revés para el patrón de acceso que el cache existe para servir.

**Verificación.** Cada test se probó mutando **exactamente** el mecanismo que dice aislar, no
borrando la funcionalidad entera — el detalle por test vive en su propio doc comment. Vale registrar
una vuelta de más: para hacer verdadero un comentario que decía que el barrido "corre solo en la
evicción", lo gateé detrás de un flag. Eso **introdujo una fuga**: un connect fallido crea la entrada
de lock y retorna antes de llegar a la evicción, así que en un deployment que nunca toca el tope
nadie la recogía. El comentario era lo equivocado, no la cadencia. Revertido, con el comentario
diciendo la verdad y la ruta de fallo limpiando lo suyo.

**Lo que todavía falta antes de cablear**, nombrado para que no se pierda: un `idle_timeout` y un
`close_all` —`pool_registry` tiene ambos— y alguna observabilidad, porque hoy `len()` no tiene
llamador de producción y un operador no podría ver que el tope se está tocando ni que hay thrashing.
El tope de 128 además es heurístico, sin datos de carga detrás.

Y una propiedad inherente a cualquier cache con tope LRU, dicha para que quien cablee la conozca:
alguien que pueda acuñar muchos `agent_session_id` contra un servidor con credencial puede empujar
la cardinalidad hasta el tope y forzar la evicción de la conexión caliente de otra sesión. **No
rompe el aislamiento de credenciales** —solo obliga a re-handshakear— pero es un costo real entre
tenants.

**`touch` usa `push`, no `put` — y eso es lo que hace que el tope sea un invariante.**
`LruCache::put` devuelve `Option<V>`: descarta la mitad de la clave, así que **no puede** informar
que el cache soltó un registro propio para mantenerse en capacidad. `push` devuelve el `(clave,
valor)` desplazado. La distinción carga peso: `evict_if_needed` solo puede elegir víctimas que el
LRU todavía conoce, de modo que una clave cuyo registro desapareciera en silencio quedaría en
`clients` para siempre, ineviccionable, y el tope dejaría de sostenerse sin ruido alguno.

La holgura del LRU (`max_entries * 10`, mínimo 1024) hace ese desplazamiento raro — **no imposible**.
La primera versión de este código lo daba por descartado en un comentario que nombraba el modo de
fallo y acto seguido lo afirmaba cerrado por probabilidad. El review lo marcó, y con razón: una
holgura no es un invariante. El desplazamiento ahora se atiende borrando el footprint
completo de la clave desplazada, igual que una evicción normal. Lo prueba
`a_key_the_lru_displaces_on_its_own_is_not_left_stranded`, con `max_entries` deliberadamente
generoso para que `evict_if_needed` no dispare nunca: lo único que puede sacar la clave es el manejo
del desplazamiento. Revertir `push` a `put` lo tumba.

El test ejercita `register_and_pool`, **no** `touch` — sus tres claves son nuevas, así que todas van
por la ruta fría. Ambos comparten `collect_displaced`, de modo que la lógica común queda cubierta;
la rama de desplazamiento de `touch` en particular **no está fijada por ningún test**, y es
casi inalcanzable: `touch` solo corre en un hit, y una clave que está en `clients` está en el LRU,
así que su `push` es un re-rank. Solo dispara vía una ventana estrecha.

**El LRU se registra ANTES que `clients`, y ese orden es la otra mitad del invariante.** El fix de
`push` cierra el desplazamiento por capacidad del LRU, y **solo eso**. La ronda siguiente del review
encontró la misma falla por otra ruta: `touch` se suspende en el mutex del LRU, así que un llamador
cancelado en ese await — un timeout, un `select!`, una task abortada — dejaba, con `clients.insert`
primero, la clave en `clients` sin registro en el LRU. `evict_if_needed` solo puede elegir víctimas
que el LRU nombra, de modo que esa clave quedaba ineviccionable para toda la vida del proceso.

Invertir el orden no alcanzaba. Al enumerar las rutas restantes apareció una tercera, que el review
no había levantado: entre `touch()` retornando y el `insert` no hay await, pero el runtime es
multi-thread, así que otro hilo puede desplazar esa misma clave del LRU en ese instante y correr
`drop_footprint` sobre ella ANTES de que esté en `clients`. La eliminación no encuentra nada,
nuestro insert aterriza después, y la clave queda otra vez ineviccionable. Es la misma falla por una
tercera ruta.

La solución no es ordenar sino hacerlo atómico: el método `register_and_pool` toma el mutex del LRU
**una vez** y
hace el `push` y el `clients.insert` dentro de la misma sección crítica. Todos los demás escritores
del LRU pasan por ese mutex, así que no queda ventana. `drop_footprint` nunca toma el mutex del LRU,
de modo que recolectar la clave desplazada después no puede trabarse contra él.

Qué está fijado por test y qué no, dicho para no venderlo de más:

- La **cancelación** sí: `a_cancelled_connect_does_not_strand_an_unevictable_key` sostiene el mutex
  del LRU para parquear el connect exactamente en ese await y suelta el future ahí — determinista,
  sin sleeps ni timings. Mover el `insert` fuera de la sección crítica lo tumba.
- El **desplazamiento concurrente** no. Queda cerrado por construcción —una sola sección crítica— y
  eso es un argumento, no una aserción ejecutable. Un test que lo demostrara tendría que probar que
  no existe un entrelazado, que es justo lo que un test no hace.

Y el comentario que escribí para documentar el fix de `push` decía «invariante en lugar de
probabilidad» — de más otra vez, porque solo valía contra el desplazamiento interno. Ahora declara su
alcance explícitamente.

**Dos hallazgos del review que quedan abiertos a propósito**, con el motivo:

- **`touch()` serializa el fast-path.** Se llama en cada `client()`/`tools()`, incluidos los hits de
  cache que antes solo hacían un `DashMap::get` sin lock, y toma un `tokio::sync::Mutex` de proceso.
  Es inherente a un LRU con recencia exacta: quitarlo es un LRU sharded o un CLOCK aproximado, o sea
  un rediseño, no una corrección. La sección crítica es CPU pura, sin I/O. Se mide cuando haya un
  llamador de producción; hoy no lo hay.
- **Un `tool_cache` puede quedar huérfano en una ventana estrecha.** En el fill frío de `tools()`,
  entre que `client()` retorna (ya habiendo hecho `touch`) y el `tool_cache.insert`, corre el
  `list_tools()` — un await real. Si en esa ventana la evicción de otra clave saca ESTA del LRU, el
  insert deja una entrada de catálogo que la evicción ya no alcanza, porque solo borra `tool_cache`
  de las claves que saca del LRU. Se recupera sola en el próximo acceso a esa clave (el hit hace
  `touch` y la reinserta); solo persiste si la clave no se vuelve a tocar nunca. Esa recuperación
  descansa en que el LRU no suelte registros por su cuenta — cierto ahora que `touch` maneja el
  desplazamiento, y NO cierto en la versión que el review examinó.

  El review lo clasificó `introduced` y **no lo comparto**: sobre el árbol base no había evicción
  alguna, así que `tool_cache` crecía sin tope de forma incondicional. Este cambio **reduce** la
  fuga, no la abre. Es un residuo, no una regresión — y por eso va como follow-up y no como
  corrección de este candidato. Cerrarlo bien pide un test de concurrencia con el fill retenido, que
  es trabajo propio, no una línea.

**Estado.** done (evicción y tope; pendientes `idle_timeout`, `close_all`, observabilidad, el
`tool_cache` huérfano de la ventana estrecha, y medir el costo de `touch` en el fast-path).

