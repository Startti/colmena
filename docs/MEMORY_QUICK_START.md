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

## 💡 Tips

- **SQLite**: Perfecto para desarrollo y testing
- **PostgreSQL**: Usa en producción para múltiples usuarios
- **Thread IDs**: Usa IDs únicos por usuario/sesión (ej: `user_${user_id}`)
- **Seguridad**: Siempre usa variables de entorno para credenciales
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
