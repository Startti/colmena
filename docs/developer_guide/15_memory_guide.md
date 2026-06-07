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

## 📚 Más Información

Ver la [Guía Completa del DAG Engine](./12_dag_engine_guide.md) para:
- Arquitectura detallada
- Cómo funciona internamente
- Más ejemplos y casos de uso
- Best practices
