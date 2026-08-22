# Respuesta — `Stream timeout` falso al REANUDAR un run anidado

**Fecha:** 2026-08-22
**De:** equipo `dag_engine` / `Startti/colmena`
**Para:** equipo ADP
**Responde a:** handoff «`Stream timeout` falso al REANUDAR un run anidado» (2026-08-22)
**Base auditada:** `develop` @ `71fb914f`

---

## TL;DR

Su hipótesis es **correcta como lectura del código y falsa como causa**. Los tres relojes de
liveness efectivamente no cubren el arranque de un run: lo medimos y son **poco más de un
segundo** (1.1–2.4 s entre corridas) en un resume real de tres niveles. No 60.

Esa ventana **no escala con el anidamiento** — la re-entrada a los subgrafos ocurre *dentro* del
nodo, con el reloj ya corriendo — y **su propio worker ya la cubre** con el keepalive de 20 s.
Reprodujimos la forma exacta que reportan, incluido un silencio forzado de 70 s en el agente más
profundo, y el stream nunca estuvo mudo más de 20 s.

El watchdog de 60 s vive en su repo, no en el nuestro. Con la evidencia que aportan no podemos
atribuir el síntoma a Colmena; en §5 dejamos dos comprobaciones concretas que sí lo cierran.

---

## 1. Confirmado: el heartbeat no cubre el arranque

Es exactamente como leyeron. En `run_use_case.rs`, `last_forwarded` / `last_any` / `last_beat` se
inicializan dentro del bloque que envuelve `node_impl.execute(...)`. Antes del primer `NodeStart`
no hay reloj corriendo.

Lo medimos en vez de deducirlo: corrimos un resume con `COLMENA_HEARTBEAT_INTERVAL_SECS=1`, que
convierte cualquier tramo cubierto en un frame por segundo.

| Marca | t |
|---|---|
| `engine_started` (construcción del engine, fuera de `execute_stream`) | 8.04 s |
| primer frame del stream (`node-start`) | 9.13 s |
| **ventana muda dentro de `execute_stream`** | **1.10 s — 0 frames** |

Cero frames en esa ventana con el heartbeat en 1 s: los 40 frames `status` de ese run son todos
posteriores al primer `node-start`. Su lectura del código es correcta. En las corridas que hicimos
la ventana osciló entre 1.1 s y 2.4 s, según lo que tardaran preflight y la lectura de estado.

## 2. Por qué esa ventana no es la causa

Tres razones, en orden de peso:

**a) Mide poco más de un segundo.** El tramo es preflight de proveedores (cacheado por TTL, así que a partir del
segundo turno no hace red), un `get_by_id` contra Postgres y `compute_resuming_node_ids` sobre el
estado ya cargado. Nada de eso es un candidato serio a 60 s.

**b) No escala con la profundidad.** Esto responde el confundidor que plantean en su §5. La
rehidratación del run raíz es lo único que cae en la ventana muda. Bajar a `ADP Resources` y de ahí
a `DB Connection Specialist` ocurre **dentro** de `node_impl.execute` del nodo raíz, es decir con
los tres relojes ya corriendo. Tres niveles no ensanchan la ventana; la trabajan del lado cubierto.

**c) Su worker ya la cubre.** En `apps/service/ia/platform/worker/src/main.rs`, el keepalive late
cada 20 s (hasta 6 beats) y solo se aborta cuando el primer frame real de Colmena llega al stream.
Entre que el worker acepta el job y que Colmena emite su primer frame, el stream ya está alimentado
por ustedes. Pedir que el reloj arranque al aceptar el job es duplicar lo que ese keepalive hace.

## 3. La medición: el heartbeat sí sostiene un resume anidado

Construimos la forma que reportan y la corrimos de punta a punta contra Gemini 2.5 Flash y Postgres
real. Tres niveles (`creator` → `adp_resources` → `db_connection_specialist`), cada uno un
`llm_call` con `connection_url`, el especialista con la confirmación humana **plegada como tool**
(`tool_configurations` con `node_type: "suspend"`) más una tool de borrado real (`python_script`).

- [`tests/graphs/advanced/nested_resume_liveness_e2e.json`](../../../tests/graphs/advanced/nested_resume_liveness_e2e.json)
- [`tests/graphs/advanced/nested_resume_liveness_slow_e2e.json`](../../../tests/graphs/advanced/nested_resume_liveness_slow_e2e.json)
  — misma forma, con dos diferencias deliberadas: la tool de borrado duerme 70 s, y no fija
  `sandbox_mode: restricted` (el modo restringido no permite importar `time` y su
  `sandbox_timeout_secs` de 10 s cortaría el sleep antes de los 70 s, invalidando la medición)

Los corrimos con `dag_engine run`, que emite los frames a través del **mismo `SseMapper`** que usa
su worker. Cada línea `data:` del CLI es, byte por byte, lo que su worker haría `XADD`. Marcamos
cada línea con su tiempo monótono y medimos el silencio entre frames consecutivos, que es
exactamente lo que su watchdog cuenta.

| Escenario | Frames | Duración | Máx. silencio | Resultado |
|---|---|---|---|---|
| Run inicial, 3 niveles | 30 | 31.8 s | 5.4 s | `finishReason: suspended` |
| Resume («sí, borrala») | 40 | 32.0 s | 5.1 s | `finishReason: stop`, borrado ejecutado |
| Resume con **70 s de silencio forzado en el agente más profundo** (`level=4` en el SSE) | 49 | 103.5 s | **20.0 s** | `finishReason: stop`, borrado ejecutado |
| Resume con `heartbeat=1 s` (instrumentado) | 80 | 34.9 s | 1.0 s | `finishReason: stop` |

Los conteos de frames y las duraciones son de una corrida concreta y no son bit-reproducibles —
dependen de cuántos tokens emita el modelo. Lo que sí es estructural, y lo que importa acá, es la
columna de silencio máximo.

El tercero es el que decide la discusión. Con la tool más profunda muda durante 70 s, el nodo raíz
emitió `status` cada 20 s:

```
20.0s  [ 22.1 ->  42.1]  siguiente=status  level=0  path=creator
20.0s  [ 42.1 ->  62.1]  siguiente=status  level=0  path=creator
20.0s  [ 62.1 ->  82.1]  siguiente=status  level=0  path=creator
```

Un silencio profundo de 70 s se convirtió en tres huecos de 20 s. Su watchdog no habría disparado.

El cuarto cierra el resto del recorrido: con el heartbeat en 1 s, **una vez arrancado el primer
nodo no hay un solo hueco mayor a 1.0 s** en 80 frames — ni entre nodos, ni en la persistencia de
estado, ni al cerrar el run (0.73 s entre el último `node-end` y el `finish`).

## 4. El watchdog de 60 s

Está en su repo: `apps/service/ia/platform/api/src/stream.rs:52`, `MAX_EMPTY_READS = 12` sobre
`XREAD BLOCK 5000` = 60 s. Cuenta lecturas vacías **consecutivas** sobre `events:{job_id}` desde
`"0"`, y se resetea con cualquier entrada. El string no aparece en ningún `.rs` de Colmena.

Lo aclaramos sin ánimo de devolver la pelota: importa porque define qué hay que probar. La condición
que dispara no es «el run tardó», es «nadie escribió nada en ese stream durante 60 s corridos».

## 5. Dos comprobaciones que cierran el caso de su lado

**a) Daten el deploy sin acceso a la consola.** Los frames
`{"type":"status","stage":"running","idleSecs":N}` existen desde el PR #144 (`0f886b37`,
2026-07-04); los dos relojes desde el #146 (`6d532355`, 2026-07-05). Si en un tramo largo de dev no
aparece **ningún** `status`, su deploy es anterior a esas fechas y el caso se cierra ahí — es la
posibilidad que ustedes mismos dejaron abierta en su §4.

**b) Miren dónde persisten el texto del agente.** Si lo persisten *desde el stream*, entonces el
stream sí entregó el texto y recién **después** estuvo 60 s mudo esperando el `finish`. Colmena no
hace eso: medimos 0.73 s entre el último `node-end` y el `finish`. Ese escenario apunta a que el
worker deja de escribir después del último texto, lo que encaja con el OOM ya diagnosticado en el
worker (512Mi / 1 vCPU / concurrency 80, sin `--memory` ni `--cpu` en `deploy_gcp.sh`) y explicaría
por qué el efecto quedó aplicado y el texto guardado mientras el turno quedó marcado como fallido.

Si ninguna de las dos aplica, instrumenten el arranque del resume con tiempos y lo retomamos: lo que
nos falta para seguir es cuántos segundos estuvo mudo el stream y si llegó algún evento antes del
timeout, que es justo lo que su §4 no pudo medir.

## 6. Sobre su pedido #2

«Que el reloj empiece a correr al aceptar el job» es un endurecimiento razonable —un run que
rehidrata está vivo y hoy es indistinguible de uno colgado— y lo dejamos anotado. Pero con los
números de arriba es cosmético frente a este síntoma: la ventana es de poco más de un segundo y
ustedes ya la cubren. No lo vamos a shippear como fix de este reporte, porque cerraría el caso sin
haber tocado la causa.

Coincidimos con su §6 en no subir el timeout.

---

## Reproducir

```bash
set -a && source .env && set +a

# 1. Run inicial → suspende en el tercer agente
cargo run --bin dag_engine -- run tests/graphs/advanced/nested_resume_liveness_e2e.json \
  --agent-session-id adp_liveness_001

# 2. Resume → completa el borrado
cargo run --bin dag_engine -- run tests/graphs/advanced/nested_resume_liveness_e2e.json \
  --agent-session-id adp_liveness_001 \
  --answer "$(printf 'Q[confirm_delete]: ¿Estás seguro?\nA[confirm_delete]: sí, borrala')"
```

El `--answer` necesita un salto de línea **real**; un `\n` literal falla con
`Q[id] has no matching A[id]`.

Para la variante con 70 s de silencio profundo van los mismos dos pasos sobre
`nested_resume_liveness_slow_e2e.json`, pero **con un `--agent-session-id` distinto**. El sleep de
70 s vive en la tool de borrado, que solo se dispara tras la confirmación — es decir, en el paso 2:

```bash
# 1. Run inicial → suspende igual que el rápido
cargo run --bin dag_engine -- run tests/graphs/advanced/nested_resume_liveness_slow_e2e.json \
  --agent-session-id adp_liveness_slow_001

# 2. Resume → acá aparece el silencio de 70 s y los heartbeats que lo cubren
cargo run --bin dag_engine -- run tests/graphs/advanced/nested_resume_liveness_slow_e2e.json \
  --agent-session-id adp_liveness_slow_001 \
  --answer "$(printf 'Q[confirm_delete]: ¿Estás seguro?\nA[confirm_delete]: sí, borrala')"
```

No es cosmético. La memoria conversacional se busca por `(agent_session_id, node_id)` y no incluye
ninguna identidad del grafo, y los dos grafos usan los mismos node ids en las mismas posiciones. Si
reusan la sesión del run rápido, el `creator` del run lento arranca con la conversación ya terminada
—donde el borrado figura como hecho— y puede no volver a llamar la tool lenta, que es justamente lo
que se quiere medir.

Para medir los tramos que el heartbeat no cubre, exporten `COLMENA_HEARTBEAT_INTERVAL_SECS=1` y
busquen huecos entre frames mayores a 1.5 s.
