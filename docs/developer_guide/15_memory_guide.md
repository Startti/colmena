# 🧠 Memoria en DAG Engine - Guía Rápida

Esta guía proporciona ejemplos rápidos para usar memoria persistente en el DAG Engine con SQLite y PostgreSQL.

## 🎯 Configuración Rápida

### SQLite (Local/Desarrollo)

**1. No necesitas configurar `.env` para SQLite**

**2. Usa directamente en tu DAG:**
```json
{
  "type": "llm_call",
  "config": {
    "provider": "openai",
    "api_key": "${OPENAI_API_KEY}",
    "connection_url": "sqlite://my_memory.db",
    "prompt": "Hello!"
  }
}
```

### PostgreSQL (Producción)

**1. Configura tu `.env`:**
```bash
DATABASE_URL="postgresql://user:password@host:5432/database"
OPENAI_API_KEY="sk-..."
```

**2. Usa en tu DAG:**
```json
{
  "type": "llm_call",
  "config": {
    "provider": "openai",
    "api_key": "${OPENAI_API_KEY}",
    "connection_url": "${DATABASE_URL}",
    "prompt": "Hello!"
  }
}
```

## 📝 Formatos de Connection URL

| Base de Datos | Formato | Ejemplo |
|---------------|---------|---------|
| SQLite | `sqlite://filename.db` | `sqlite://memory.db` |
| PostgreSQL | `postgresql://user:pass@host:port/db` | `postgresql://postgres:pwd@localhost:5432/mydb` |
| PostgreSQL (alt) | `postgres://user:pass@host:port/db` | `postgres://postgres:pwd@localhost:5432/mydb` |

### SSL / TLS en PostgreSQL

sqlx usa `runtime-tokio-rustls`, por lo que TLS funciona sin dependencias nativas. El modo se elige con el query param `sslmode`:

| `sslmode` | Comportamiento |
|-----------|----------------|
| omitido | `prefer`: intenta TLS, cae a texto plano si el server no lo soporta |
| `disable` | Sin TLS |
| `require` | TLS obligatorio, **no** valida el certificado |
| `verify-ca` | TLS + valida cadena contra CA (requiere `sslrootcert`) |
| `verify-full` | `verify-ca` + valida hostname (recomendado en producción) |

**Ejemplo Cloud SQL:**
```
postgresql://user:pass@10.0.0.3:5432/db?sslmode=require
```

**Ejemplo verify-full con CA propia:**
```
postgresql://user:pass@host:5432/db?sslmode=verify-full&sslrootcert=/etc/ssl/ca.pem
```

> Importante: distintos valores de `sslmode` en la misma URL base generan **pools separados** en el registry. Esto es intencional — una conexión cifrada y otra en plano no deben compartir pool.

## 🚀 Ejemplos Completos

### Ejemplo 1: SQLite

```bash
# Ejecutar ejemplo con SQLite
cargo run --bin dag_engine -- run tests/graphs/memory/memory_sqlite_example.json
```

**Archivo:** `tests/graphs/memory/memory_sqlite_example.json`
- Crea dos pasos de conversación
- Usa SQLite local (`colmena_memory.db`)
- El segundo paso recuerda lo que se dijo en el primero

### Ejemplo 2: PostgreSQL

```bash
# Configurar .env primero
echo 'DATABASE_URL="postgresql://user:pass@host:5432/db"' >> .env

# Ejecutar ejemplo
cargo run --bin dag_engine -- run tests/graphs/memory/memory_postgres_example.json
```

**Archivo:** `tests/graphs/memory/memory_postgres_example.json`
- Usa PostgreSQL para producción
- Soporta múltiples usuarios concurrentes
- Escalable y robusto

### Ejemplo 3: Chat persistente con `agent_session_id`

Para conversaciones que duran varios runs (cada mensaje del usuario = un run nuevo),
usá `--agent-session-id` para que el `llm_call` recupere el historial de runs previos.
El par `agent_chat_say.json` / `agent_chat_ask.json` demuestra el patrón:

```bash
# Configurar .env con DATABASE_URL apuntando a Postgres

# Run 1 — el usuario "dice" algo (color favorito, profesión)
cargo run --bin dag_engine -- run \
  tests/graphs/memory/agent_chat_say.json \
  --agent-session-id chat_alice

# Run 2 — el usuario "pregunta" sobre lo que dijo antes
cargo run --bin dag_engine -- run \
  tests/graphs/memory/agent_chat_ask.json \
  --agent-session-id chat_alice
```

El segundo run debe responder con el color y la profesión del primero, porque
ambos runs comparten `agent_session_id = chat_alice` y el `llm_call` indexa su
historial por `(agent_session_id, node_id)`.

> Para más grafos de memoria, ver el directorio
> [`tests/graphs/memory/`](../../tests/graphs/memory/).

## 🔑 Campos Requeridos para Memoria

Para habilitar memoria persistente en un nodo `llm_call`, sólo necesitás:

1. **`connection_url`**: URL de la base de datos donde se guardan los mensajes del historial. Puede venir de:
   - `config` (estático en el JSON)
   - `inputs` (dinámico desde otro nodo)

El identificador con el que se llaveea cada mensaje (`session_id` en la tabla `llm_node_history`) lo deriva el engine automáticamente del run actual — no lo configurás vos en el nodo. Si querés que la conversación persista a través de múltiples runs (caso típico de un chat), usá `agent_session_id` en lugar de intentar fijar el `session_id`. Ver la sección "Agent Session ID — memoria a través de runs" más abajo para detalles.

> **Nota histórica**: Versiones anteriores de la documentación describían un campo `thread_id` configurable en el JSON del nodo. Ese campo nunca fue leído por el código del motor — el `__colmena_session_id` inyectado automáticamente por el engine siempre tenía prioridad. La forma correcta y actual de controlar la memoria entre runs es `agent_session_id`.

## 🔁 Pool compartido (`ColmenaEngine`)

Desde la introducción de `ColmenaEngine`, todas las conexiones PostgreSQL pasan por un **registry único** (`PgPoolRegistry`). Implicaciones prácticas:

- Varios nodos con el mismo `connection_url` **reusan el mismo pool** dentro del proceso — no se abre uno por nodo.
- El pool del `DATABASE_URL` del proceso queda **pinned**: nunca se desaloja.
- Las URLs adicionales (las que aparecen en `connection_url` de nodos SQL/LLM) se cachean con política **LRU** con tope configurable.
- `?sslmode=require` y sin sslmode **son claves distintas** (pools separados), como se explicó arriba.

Tuning con variables de entorno (todas opcionales):

| Variable | Default | Descripción |
|----------|---------|-------------|
| `COLMENA_POOL_MAX_ENTRIES` | 100 | Máximo de pools no-pinned en caché |
| `COLMENA_POOL_MAX_CONN_PER_URL` | 2 | Conexiones concurrentes por pool |
| `COLMENA_POOL_MIN_CONN_PER_URL` | 0 | Conexiones siempre-abiertas por pool |
| `COLMENA_POOL_IDLE_TIMEOUT_SEC` | 30 | Cierra conexiones idle tras N segundos |
| `COLMENA_POOL_MAX_LIFETIME_SEC` | 600 | Recicla conexiones tras N segundos |
| `COLMENA_POOL_ACQUIRE_TIMEOUT_SEC` | 10 | Timeout para pedir una conexión del pool |

Ver [`12_dag_engine_guide.md`](./12_dag_engine_guide.md#ciclo-de-vida-de-colmenaengine) para el ciclo de vida completo.

## 🆕 Agent Session ID — memoria a través de runs

Hasta ahora, la memoria de un nodo `llm_call` estaba ligada al `session_id` del run.
Cada nueva ejecución (con un nuevo session_id) empezaba con historial vacío. Para
casos de chat — donde el usuario manda múltiples mensajes y cada mensaje es un run
distinto — esto era inconveniente.

`agent_session_id` introduce un identificador a nivel de **conversación** que vive
por encima del `session_id` del run individual. Los `llm_call` ahora indexan su
historia por `(agent_session_id, node_id)` cuando hay un agent presente, lo que
permite que la memoria persista entre runs.

### CLI

```bash
cargo run --bin dag_engine -- run mi_grafo.json --agent-session-id chat_abc
```

- Si no hay un run SUSPENDED para `chat_abc` → arranca un run nuevo con ese chat.
- Si hay uno SUSPENDED → lo reanuda automáticamente sin necesidad de pasar el
  session_id del run.

### HTTP

```bash
curl -X POST http://localhost:3000/webhook \
  -H "X-Agent-Session-Id: chat_abc" \
  -H "Content-Type: application/json" \
  -d '{"prompt": "hola"}'
```

(Alternativa: `"agent_session_id": "chat_abc"` en el body JSON.)

### Diferencia clave: session_id vs agent_session_id

| Identificador | Alcance | Persiste a través de | Llave principal en |
|---|---|---|---|
| `session_id` | Un run del DAG (root o subgrafo) | Suspend/resume del MISMO run | `dag_runs.session_id` (PK), `llm_node_history.session_id` (legacy reads) |
| `agent_session_id` | Conversación / chat | Múltiples runs (root, subgraph, etc.) | `dag_runs.agent_session_id`, `llm_node_history.agent_session_id` (new reads) |

### Compatibilidad hacia atrás

Si no pasás `--agent-session-id`, el comportamiento es idéntico al previo: cada
run obtiene su propio session_id, y la memoria se llavea por `(session_id, node_id)`.
Los grafos existentes no requieren cambios.

---

## 🧱 Aislamiento por nodo — memoria NO compartida entre llm_call distintos

**Importante:** la memoria conversacional de un `llm_call` está aislada **por nodo**, incluso cuando varios nodos comparten el mismo `agent_session_id`. La llave compuesta `(agent_session_id, node_id)` (o `(session_id, node_id)` cuando no hay agent) significa que cada nodo abre su propio thread y NO ve la historia de otros nodos `llm_call` en el mismo grafo o run.

### Por qué es así

Históricamente la memoria se keaba SOLO por `agent_session_id`. Cuando un grafo tenía 2+ `llm_call` nodes con el mismo agent (típico en orchestrators), todos escribían sobre el mismo thread y se pisaban entre sí — un **collision silenciosa**. El bug es trivial de reproducir: nodo A le decía a Gemini "soy Juan", nodo B le preguntaba "¿quién soy?" y dependiendo del orden de escritura recibía respuestas inconsistentes o errores tipo "no me dijiste tu nombre".

La migración [`20260428000002_llm_history_agent_and_node.sql`](../../src/libs/colmena/migrations/postgres/20260428000002_llm_history_agent_and_node.sql) agregó `node_id` como segunda mitad de la llave compuesta para **eliminar la collision**. Spec completa: [`docs/superpowers/plans/2026-04-28-agent-session-id.md`](../superpowers/plans/2026-04-28-agent-session-id.md) §3.2.

### Comportamiento observable

Dado este grafo:

```json
{
  "nodes": {
    "step_1": { "type": "llm_call", "config": { ..., "prompt": "Mi nombre es Daniel. ¿Hola?" } },
    "step_2": { "type": "llm_call", "config": { ..., "prompt": "¿Cuál es mi nombre?" } }
  },
  "edges": [{ "from": "step_1", "to": "step_2" }]
}
```

Aunque ambos compartan `agent_session_id`, `step_2` responderá *"No me dijiste tu nombre"* — no porque la memoria esté rota, sino porque `step_2.node_id != step_1.node_id` y la historia de step_1 vive en otro thread. Si mirás los tokens consumidos por `step_2`, verás un `prompt_tokens` bajo (~300) porque solo carga su propio history (vacío en el primer turn) + el system message + el prompt.

### Cómo compartir información entre nodos `llm_call`

**Opción 1 — Edge data-flow (recomendado, idiomático):**

Pasá el output del nodo previo como parte del prompt del siguiente, vía edge:

```json
{
  "nodes": {
    "step_1": {
      "type": "llm_call",
      "config": {
        "system_message": "Sos un detective. Recordá lo que el usuario te diga.",
        "prompt": "Mi nombre es Daniel y mi perro se llama Toby."
      }
    },
    "step_2": {
      "type": "llm_call",
      "config": {
        "system_message": "Sos el mismo detective. El interlocutor te acaba de decir esto:\n\n${prev_turn}\n\nResponde a su siguiente mensaje.",
        "prompt": "¿Cómo se llama mi perro?"
      }
    }
  },
  "edges": [
    { "from": "step_1.result", "to": "step_2.config.prev_turn" }
  ]
}
```

El operador del DAG controla qué información cruza entre nodos. El LLM de `step_2` ve el contenido inyectado en su system message como contexto inicial, sin recurrir a memoria persistida.

**Opción 2 — Reutilizar el mismo `node_id`:**

Si querés que dos turnos compartan thread (caso típico de un chat multi-mensaje), modelalos como **el mismo nodo ejecutado en runs sucesivos** con el mismo `agent_session_id`. Los grafos en [`tests/graphs/memory/`](../../tests/graphs/memory/) usan este patrón: `agent_chat_say.json` y `agent_chat_ask.json` son DOS grafos distintos cuyo único `llm_call` tiene el mismo `node_id`. Ejecutados con `--agent-session-id chat_alice`, el segundo ve lo que el primero escribió.

**Opción 3 — Orchestrator/Planner pattern:**

Para flows complejos con múltiples agentes especializados, el nodo [`orchestrator`](19_nested_agents_and_subgraphs.md) modela esto explícitamente: cada child agent corre en su propio subgrafo, y el orchestrator coordina pasando outputs como inputs. NO depende de memoria conversational compartida — la composición es explícita.

### Anti-patterns

- ❌ **Esperar que dos `llm_call` con distintos `node_id` compartan historia** porque tienen el mismo `agent_session_id`. No la comparten — es comportamiento deliberado.
- ❌ **Forzar el mismo `node_id` en dos nodos distintos del mismo grafo** para sortear el aislamiento. El engine te lo permite, pero las escrituras intercaladas vuelven a producir el collision original. Si necesitás compartir, usá una de las 3 opciones de arriba.

---

## 💡 Tips

- **SQLite**: Perfecto para desarrollo y testing
- **PostgreSQL**: Usa en producción para múltiples usuarios
- **Agent Session IDs**: Para chats continuos, usa `--agent-session-id` (CLI) o `X-Agent-Session-Id` (HTTP) con un ID único por usuario/sesión (ej: `chat_user_${user_id}`)
- **Seguridad**: Siempre usa variables de entorno para credenciales y `sslmode=verify-full` en producción
- **Auto-creación**: Las bases de datos y tablas se crean automáticamente

## 🐛 Troubleshooting

**Error: "Unsupported database protocol"**
```bash
# ✅ Correcto
"connection_url": "sqlite://memory.db"
"connection_url": "postgresql://user:pass@host:5432/db"

# ❌ Incorrecto
"connection_url": "mysql://..."  # No soportado
"connection_url": "memory.db"    # Falta protocolo
```

**Error: "Environment variable not found"**
```bash
# Verifica que .env exista y tenga la variable
cat .env | grep DATABASE_URL

# Debe mostrar algo como:
# DATABASE_URL="postgresql://..."
```

## 🗜️ Compactación y recuperación de memoria

A medida que una conversación crece, reenviar todo el historial en cada turno es
caro y eventualmente excede la ventana de contexto del modelo. El `llm_call`
**compacta el historial automáticamente** al cargar cada run — manteniendo los
turnos recientes completos y condensando los viejos en un resumen direccionable,
sin perder nada (el original siempre vive en la DB).

### Cuándo y cómo se compacta

- **Cuándo:** una sola vez, **al cargar el run** (lazy). Nunca por iteración del
  loop ReAct, y el bloque compactado queda fijo durante el run (cache-friendly).
- **Recientes (full):** todo lo que pertenece a la **interacción abierta** va
  **completo**, sin importar cuánto pese. El borde es estructural, no de
  presupuesto: arranca justo después del último `assistant` que respondió sin
  `tool_calls` (ver "Dónde se corta el historial" más abajo).
- **Primeros 2 (full):** el mensaje inicial del usuario (el objetivo) y el
  `system_message` se preservan completos.
- **Medio (resumido):** todo lo que queda entre medio se colapsa en **un** mensaje
  `system` titulado `## Conversation summary`, con **una línea `[Tn]` por mensaje**.

> **Dónde termina el resumen.** `build_compacted_messages` lo emite **siempre como un mensaje
> `system` propio**, nunca fusionado dentro del `system_message` del agente — aunque eso deje dos
> `system` consecutivos. El request compactado queda así:
>
> ```
> [ User(prompt inicial), System(prompt estable), System(## Conversation summary), ...recientes ]
> ```
>
> **Por qué separados: prompt caching.** El adapter de Anthropic pone el marker `cache_control`
> en el **primer** bloque `system` (ver [§14 — Prompt caching](14_llm_deep_dive.md)), así que el
> prefijo cacheado es `tools[] + system_blocks[0]`. El resumen se **recomputa en cada carga** y
> crece con la conversación: fusionarlo dentro de ese bloque reescribía el prefijo cacheado turno
> a turno. Separado, el bloque 0 queda byte-idéntico y el resumen cae en `system_blocks[1]`, fuera
> del prefijo.
>
> Dos piezas del código sostienen esa separación, y **hacen falta las dos**:
>
> 1. `history_compaction::build_compacted_messages` emite el resumen aparte.
> 2. `LlmRequest::new` exime a `System` de `coalesce_consecutive_same_role`, igual que ya eximía a
>    `Tool`. El coalescer normaliza la alternancia user/assistant que exigen los providers; los
>    `system` nunca participan de esa alternancia, y volver a fusionarlos deshacía (1) por completo.
>
> **Medido E2E contra Anthropic real** (`tests/graphs/agents/prompt_cache_compaction_measure.json`,
> Sonnet 5, 5 turnos, prefijo estable de 3029 tokens):
>
> | Turno | Antes — write / read | Después — write / read |
> |---|---|---|
> | 1 | 3029 / 3029 | 3029 / 3029 |
> | 2 | 3457 / 3457 | **0** / 6058 |
> | 3 | 3829 / 3829 | **0** / 3029 |
> | 4 | 4260 / 4260 | **0** / 3029 |
> | 5 | 4684 / **0** | **0** / 3029 |
>
> Antes, el `cache_creation_input_tokens` **crecía exactamente al ritmo del resumen** y se pagaba
> completo (a 1.25×) en cada turno: el prefijo se reescribía siempre y nunca se reusaba entre runs.
> Después, la escritura desaparece a partir del turno 1 y la lectura se estabiliza en el tamaño del
> prefijo estable; el resumen viaja como input normal (visible en el `promptTokens` creciente).
>
> **Varios `system` en un request son legales** y cada adapter los maneja así:
>
> | Provider | Qué hace con varios `system` |
> |---|---|
> | **Anthropic** | Un bloque por `system`, en orden; `cache_control` solo en el primero |
> | **OpenAI** | Los deja en el array `messages` tal cual, en su posición |
> | **Gemini** | Los une con `\n\n` en un único `systemInstruction` |
>
> Ningún adapter puede descartar uno. El de Anthropic lo hacía hasta el 2026-08-22 (sobrescribía en
> vez de combinar) — era un defecto latente entonces; desde este cambio esa ruta **sí** está viva.
>
> Los otros cinco llamadores de `AgentService` —`reactor`, `planner`, `critic`, `orchestrator` y
> `util/extract_with_schema`— usan un `InMemoryConversationRepository` nuevo por invocación con
> exactamente 2 mensajes, así que `build_compacted_messages` sale temprano
> (`total <= keep_first + 1`) y nunca compacta.

### Política por rol / tipo de mensaje

Cada línea del resumen se construye según el rol. La DB **nunca guarda nada
truncado** (ver la garantía de persistencia más abajo), pero el texto que se
**envía al modelo** sí puede truncarse por caracteres — como puente de runtime,
nunca como límite persistido — cuando un mensaje de la zona de memoria se queda
sin resumen semántico. Eso pasa en **tres** situaciones: el summarizer devuelve
error, no hay summarizer configurado, o ya se agotó el cupo de
`SUMMARIZE_PER_LOAD_CAP` (30) resúmenes para esa carga. En cualquiera de las
tres la línea cae a los primeros `SUMMARY_TARGET_CHARS` (250) caracteres del
original, más el puntero a `recall_history`.

Ese truncado vive **solo en la zona de memoria**. Nada dentro de la interacción
abierta se trunca, sin importar su rol — ver la sección "Dónde se corta el
historial: la interacción abierta" más abajo. Fuera de ese caso:

| Tipo de mensaje | Cómo aparece en el resumen |
|---|---|
| Texto (user/assistant) `< 250` chars | **Verbatim** (ya es conciso; sin llamada LLM) |
| Texto (user/assistant) `≥ 250` chars | **Resumen semántico** (~250 chars) generado por un modelo barato y **cacheado** |
| `assistant` con `tool_calls` | Línea estructural: `AGENT llamó <tool>(<args>)` |
| Resultado de `tool` | Verbatim si `< 250`, si no resumen semántico |
| Andamiaje (`load_skill`, `describe_tool`, `load_attachment`) viejo | Marker corto (re-llamar la tool para releer) |

El resumen semántico de cada mensaje se calcula **una sola vez** y se guarda en la
columna `summary` de `llm_node_history`; los loads siguientes lo reusan desde la DB.

### Digest estructurado de tool-results (v1.1)

Cuando un resultado de tool **estructurado** (JSON: objeto, array de objetos, o
array de escalares) envejece y sale de la ventana reciente, en vez de un resumen
NL con pérdida se genera un **digest determinista** que conserva la FORMA:

- **Array de objetos** (p.ej. `sql_query`, listas de una API):
  `600 filas · cols: month, region, revenue, units · muestra: {month:2026-01, region:Norte, …}; {…} · revenue: min 12000 max 480000`
- **Objeto** (p.ej. detalle de un pedido):
  `objeto · campos: order_id, status, total, items[8], shipping_address · order_id=5512, status=en transito, total=340 · items[8] cols: sku, qty`
- **Array de escalares:** `40 elementos · muestra: [0, 1, 2, …]`

El digest **no usa LLM, no se cachea y no toca la DB** (es determinista y barato,
se recalcula en cada load). La línea cita `recall_history(turn=N)`: el resultado
completo se recupera **verbatim** (recall lossless). Si el contenido del tool NO
es JSON estructurado (texto NL de una búsqueda web, etc.), cae al resumen
semántico normal.

**Por qué importa:** un resumen NL ("devolvió ventas mensuales por región") borra
las columnas; el modelo no sabe que existía `revenue` ni `margin`, así que alucina
o no sabe que puede recuperar. El digest preserva el esquema → el modelo decide
con precisión si responde del digest o hace `recall_history` del detalle.

**v1.2 — mapa nominal (2026-06-19).** El drill-down ya no muestra solo nombres de
columna: lista la *identidad* de cada fila (`<type> "<name>"`), sacando campos
`name`/`title`/`label` + `type`/`kind` de la fila o **un nivel** dentro de columnas-objeto
(alcanza p.ej. `data.label`). Así, el resultado de un `load_canvas` envejecido se ve como
`nodes[17] … muestra: [llmCall "Tool-using Agent", webSearch "Web Search", …]` y el modelo
sabe qué nodo es cuál sin recuperar. Identificadores más profundos que un hop → marcador +
`recall_history` (degradación elegante).

### Dónde se corta el historial: la interacción abierta

El borde entre "recientes" (verbatim) y "viejo" (resumido) es **estructural**,
no de presupuesto de tokens. Lo calcula `current_interaction_start`
(`history_compaction.rs`): escanea el historial hacia atrás y devuelve la
posición justo después del **último `assistant` que respondió sin
`tool_calls`**. Ese es, por construcción, el cierre de la última interacción
que terminó — el loop ReAct de `agent_service` solo retorna al llamador
cuando el `assistant` no pide ninguna tool más — así que todo lo que viene
después sigue en curso.

Una interacción abierta **no está acotada a un turno.** Un `suspend` no la
cierra: la ruta de suspensión retorna en `agent_service.rs:500` con
`LlmResponse::suspended(...)` sin persistir ningún `assistant` sin
`tool_calls`, así que la interacción sigue abierta a través de todos los runs
de resume que hagan falta, hasta que el agente por fin responda sin pedir otra
tool.

Todo lo que cae en la interacción abierta —la pregunta que la disparó, cada
`assistant` con `tool_calls`, cada resultado de `tool`, sin importar tamaño ni
rol— viaja **verbatim** al proveedor. No hay tope de tokens que decida dónde
se corta: el corte lo decide la estructura de la conversación (dónde terminó
la última interacción cerrada), no cuánto pesa el contenido ni cuántos runs
lleva abierta.

Casos borde:

- **Nada cerró todavía** (`current_interaction_start` devuelve `0`) → todo el
  historial es la interacción actual; no hay zona vieja que resumir.
- **`suspend` mantiene la interacción abierta entre resumes** (ver arriba) —
  un agente HITL de varios pasos, el shape que Colmena documenta para dirigir
  la interacción con el usuario, puede acumular todos sus resumes dentro de
  una sola interacción abierta sin que ninguno la cierre.
- **El mensaje más nuevo es, él mismo, el cierre** de una interacción
  (alcanzable en un resume sin prompt nuevo: lo último persistido es la
  respuesta final del turno anterior) → la ventana de recientes retrocede un
  mensaje para no vaciarse del todo. Vaciarla dejaría el wire terminando en un
  turno viejo — en Anthropic y Gemini, que sacan el `system`/resumen del
  arreglo de mensajes, el modelo vería como último mensaje real el primer
  prompt de la sesión, no el actual.

**El costo, dicho con honestidad — en las dos direcciones.** No hay tope de
tamaño en la interacción abierta, y no está acotada a un turno (ver arriba):
mientras siga abierta, cada resultado de `tool` grande que contenga viaja
completo en cada resume, y en el caso límite —nada cerró todavía, `b = 0`—
`build_compacted_messages` retorna temprano por `b <= keep_first`: la historia
entera viaja cruda, sin compactar. Un agente HITL de varios pasos con un par
de resultados grandes puede así alcanzar el techo de contexto del proveedor
con un error duro, donde el presupuesto viejo degradaba en su lugar.

El otro lado, menos obvio: al arrancar un turno nuevo ordinario, el cierre
anterior queda en el índice `len-2` y el prompt recién persistido en `len-1`,
así que la ventana verbatim es de exactamente **un** mensaje. Una respuesta
previa del agente de más de `SUMMARY_SKIP_THRESHOLD_CHARS` (250 chars) ya no
tiene el margen que daba el viejo presupuesto de ~2.500 tokens y pasa a
resumen semántico de ~250 chars un turno antes de lo que pasaba con el
mecanismo eliminado: un "reescribí más corto lo que me acabás de decir"
después de una respuesta de 3.000 caracteres ahora ve solo esa línea
resumida, y necesita un `recall_history` o una paráfrasis con pérdida.

Es una **decisión de diseño deliberada**, no un descuido: la alternativa
(recortar por presupuesto) es justo el mecanismo que este cambio reemplazó,
porque podía resumir la propia pregunta que disparó una tool call
sobredimensionada — ver [`docs/CHANGELOG_2026-08.md`](../CHANGELOG_2026-08.md)
§4 para el detalle y las cifras del E2E que lo verifica. El mapa para
resultados estructurados sobredimensionados dentro de la interacción abierta
(acotar su peso sin resumir la pregunta) queda para un cambio aparte — ver
"What this plan does NOT do" en
[`docs/superpowers/plans/2026-08-22-interaction-boundary.md`](../superpowers/plans/2026-08-22-interaction-boundary.md).

### Ejemplo del bloque que recibe el modelo

```text
## Conversation summary (older turns)
Cada línea es un mensaje anterior. El [Tn] es el índice de turno: usá
recall_history(turn=N) para releer el original completo.

[T2] AGENT llamó current_time({})
[T3] TOOL: {"output":"2026-06-19T13:43:07+00:00"}
[T4] AGENT: Son las 13:43:07 UTC del 19 de junio de 2026.
[T5] USER: Importante: el código de proyecto es PRJ-9931 y el presupuesto es 47800 dólares...
[T8] AGENT: La multiplicación (125x8=1000) es clave en presupuestos para escalar costos...
[T10] AGENT: Presupuestar SW (PRJ-9931): WBS, estima esfuerzo, contingencia 10-20%; el estimado $50.6K supera los $47.8K aprobados...
```

### Recuperación verbatim — `recall_history`

Cada línea lleva su `[Tn]`, donde `n` es el **índice de turno** (ordinal del
mensaje en la DB, estable porque la historia es append-only). El agente recupera
el contenido **original y completo** de cualquier turno con la tool sintética
`recall_history(turn=N)` (siempre expuesta). Para artefactos grandes, el resultado
viene **paginado**: cada llamada devuelve `content` + `offset` + `returned_chars` +
`total_chars` + `next_offset`; si `next_offset` no es `null`, se vuelve a llamar
con ese `offset` hasta reconstruir todo el mensaje verbatim.

> **Garantía:** la compactación solo afecta lo que se le **envía** al modelo. La
> tabla `llm_node_history` siempre guarda el `content` **completo**; nada
> truncado se persiste, y todo es recuperable por turno.

### Modelo del summarizer (cadena de resolución)

El resumen semántico usa un modelo barato/rápido, resuelto en este orden:

1. **`summary_model`** en el `config` del nodo `llm_call` (override por grafo).
2. Env **`COLMENA_CHEAP_MODEL_<PROVIDER>`** (runtime, sin recompilar) — ej.
   `COLMENA_CHEAP_MODEL_GOOGLE=gemini-2.5-flash`.
3. El registro versionado **`src/libs/colmena/text/config/cheap_models.yaml`**
   (default por provider: Google→`gemini-2.5-flash`, OpenAI→`gpt-4o-mini`,
   Anthropic→`claude-haiku-4-5-…`).

Siempre usa el **mismo provider** que la llamada principal (reusa su `api_key`).
La llamada de resumen es one-shot y **no** entra a `llm_node_history`.

### Parámetros (defaults)

| Parámetro | Default | Qué controla |
|---|---|---|
| Umbral verbatim | `250` chars | Por debajo → se manda tal cual, sin resumir |
| Target del resumen | `~250` chars | Pedido por prompt (no hard-cut) |
| Frontera de recientes | estructural (sin presupuesto) | Arranca justo después del último `assistant` sin `tool_calls`; todo desde ahí va completo, sin importar tamaño |
| Primeros completos | `2` | Objetivo original + system |
| Máx. líneas del resumen | `100` | Tope del bloque (las más viejas se omiten, recuperables) |

Diseño completo:
[`docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md`](../superpowers/specs/2026-06-18-conversation-semantic-summary-design.md)
(resumen semántico, digest, `keep_first`). El borde estructural que reemplazó
la ventana por presupuesto se diseñó por separado:
[`docs/superpowers/plans/2026-08-22-interaction-boundary.md`](../superpowers/plans/2026-08-22-interaction-boundary.md).

## 📚 Más Información

Ver la [Guía Completa del DAG Engine](./12_dag_engine_guide.md) para:
- Arquitectura detallada
- Cómo funciona internamente
- Más ejemplos y casos de uso
- Best practices
