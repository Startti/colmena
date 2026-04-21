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
    "thread_id": "user_123",
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
    "thread_id": "user_123",
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
cargo run --bin dag_engine -- run tests/memory_sqlite_example.json
```

**Archivo:** `tests/memory_sqlite_example.json`
- Crea dos pasos de conversación
- Usa SQLite local (`colmena_memory.db`)
- El segundo paso recuerda lo que se dijo en el primero

### Ejemplo 2: PostgreSQL

```bash
# Configurar .env primero
echo 'DATABASE_URL="postgresql://user:pass@host:5432/db"' >> .env

# Ejecutar ejemplo
cargo run --bin dag_engine -- run tests/memory_postgres_example.json
```

**Archivo:** `tests/memory_postgres_example.json`
- Usa PostgreSQL para producción
- Soporta múltiples usuarios concurrentes
- Escalable y robusto

### Ejemplo 3: Memoria Dinámica (Webhook)

```bash
# Iniciar servidor
cargo run --bin dag_engine -- serve tests/dynamic_memory.json

# En otra terminal, hacer peticiones
curl -X POST http://localhost:3000/chat \
  -H "Content-Type: application/json" \
  -d '{"user_id": "alice", "message": "My name is Alice"}'

curl -X POST http://localhost:3000/chat \
  -H "Content-Type: application/json" \
  -d '{"user_id": "alice", "message": "What is my name?"}'
```

## 🔑 Campos Requeridos para Memoria

Para habilitar memoria en un nodo `llm_call`, necesitas:

1. **`thread_id`**: ID único de la conversación
2. **`connection_url`**: URL de la base de datos

Ambos pueden venir de:
- `config` (estático en el JSON)
- `inputs` (dinámico desde otro nodo)

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

## 💡 Tips

- **SQLite**: Perfecto para desarrollo y testing
- **PostgreSQL**: Usa en producción para múltiples usuarios
- **Thread IDs**: Usa IDs únicos por usuario/sesión (ej: `user_${user_id}`)
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
