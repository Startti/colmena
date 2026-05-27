# Attachment Migration — Walkthrough completo + Diagnósticos

Este doc es la **referencia de entendimiento** del proyecto de migración de attachments. Cubre:

1. **Parte 1:** Cada cambio que se hizo en colmena, qué hace y por qué.
2. **Parte 2:** Cada cambio que ADP debe hacer y por qué.
3. **Parte 3:** Cómo deben estar las tablas (target state) vs cómo están ahora (current state).
4. **Parte 4:** Script bash de diagnóstico para inspeccionar el estado real.

---

# Parte 1 — Cambios en colmena

El trabajo se dividió en 3 planes (A, B, C). Total: ~38 commits, todos en la rama `workingbranch/upload_documents_with_inline`.

## Plan A — Foundation (additive, 13 tasks)

> **Idea general:** crear la infraestructura para que cualquier doc (subido por el user o generado por una tool) tenga un identificador uniforme (`document_id`) y sus bytes persistidos en el storage. Sin esto, no podemos hacer cleanup, no podemos reenviar docs entre nodos, no podemos exponer un catálogo coherente al LLM.

### Task 1: Migración SQL + tipos de dominio
**Qué hizo:** agregó 3 columnas nuevas a `conversation_attachments`: `storage_key`, `origin`, `last_used_at`. Y los reflejó en los structs Rust.
**Por qué:** estas 3 columnas son la pieza central de Plan A. `storage_key` permite recuperar el blob, `origin` distingue "subido por user" vs "generado por X tool", `last_used_at` es la base del cleanup (Plan C).

### Task 2: Implementaciones Postgres + SQLite del registry
**Qué hizo:** agregó 2 métodos al `AttachmentRegistry`: `lookup_by_document_id` (búsqueda sin necesidad de saber el provider) y `touch_last_used` (actualiza el timestamp cuando se resuelve un attachment).
**Por qué:** el primer método lo usa el resolver para encontrar un doc por su ID público; el segundo es lo que mantiene los attachments "vivos" — sin esto, el cleanup borraría docs que sí se están usando.

### Task 3: Persistir bytes durante file resolution en `llm.rs`
**Qué hizo:** cuando el LLM node recibe un archivo del user (inline base64 o URL firmada), ahora **persiste los bytes a `OutputStorageRepository`** además de subirlo al provider LLM.
**Por qué:** sin esto, los docs subidos por el user solo viven dentro del provider (Gemini/OpenAI/Anthropic Files API). Si después queremos reenviarlos a otro endpoint, no tenemos los bytes localmente. El doble-storage es el costo para tener bytes recuperables.

### Tasks 4-6: Auto-registrar artifacts de image_generation, image_edit, tts
**Qué hizo:** los 3 nodos generadores ahora, además de guardar el blob, **registran una fila en `conversation_attachments`** con `origin = "generated_by:<tool_name>"` y `source = Path(storage_key)`.
**Por qué:** unificación. Antes había dos sistemas separados (uploads del user en `conversation_attachments`, generated artifacts solo en `OutputStorageRepository`). Ahora todos pasan por la misma tabla.

### Task 7: Trait `AttachmentStreamResolver`
**Qué hizo:** definió el puerto (interfaz) en el domain layer: dado un `agent_session_id` + `document_id`, devuelve un stream de bytes.
**Por qué:** hexagonal architecture. El domain layer no puede depender de infraestructura. Sin este trait, nodes como `http_request` tendrían que conocer directamente la implementación.

### Task 8: `AttachmentStreamResolverImpl` (composite impl)
**Qué hizo:** implementó el trait componiendo `AttachmentRegistry` + `OutputStorageRepository`. La estrategia: 1) lookup en registry para obtener storage_key, 2) read_stream del storage. Con un fallback de backward compat: si el ID no está en el registry, intentar como storage_key directo.
**Por qué:** el fallback permite que código viejo que pasa `$attachment:<storage_key>` (en lugar de document_id) siga funcionando. Sin esto, Plan A sería un breaking change inmediato.

### Task 9: Wire resolver en `http_request`
**Qué hizo:** cambió el nodo `http_request` para que use el resolver en lugar de llamar storage directamente cuando ve un placeholder `$attachment:<id>`.
**Por qué:** este es el primer consumidor del resolver. El node `http_request` ahora puede recibir un `document_id` (del LLM) o un `storage_key` (legacy), y el resolver maneja ambos.

### Task 10: Wire resolver en `ServiceContainer`
**Qué hizo:** construyó el resolver en el engine builder y se lo pasó al `http_request` node + a los nodos generadores (image_gen/edit/tts).
**Por qué:** sin esto, el resolver existe pero nadie lo recibe. Es el "wiring" final.

### Task 11: Catálogo en el system message del LLM
**Qué hizo:** cuando el LLM node ejecuta, prepende al system message un bloque con todos los docs disponibles en la sesión (con `document_id`, `filename`, `mime_type`, `size`, `origin`, hints de uso).
**Por qué:** el LLM necesita saber qué docs hay y cómo referenciarlos. El catálogo es la forma de decirle "tenés estos N docs, podés leerlos con `load_attachment(<id>)` o reenviarlos con `$attachment:<id>`".

### Task 12: Tests E2E (3 grafos)
**Qué hizo:** tests de integración para los 3 caminos: doc inline → http_request multipart, signed URL → multipart, generated artifact → multipart.
**Por qué:** Plan A es la fundación. Sin tests E2E el primer bug se descubre en producción.

### Task 13: Documentación
**Qué hizo:** actualizó developer guides + CLAUDE.md.
**Por qué:** Plan A introduce conceptos nuevos (`document_id`, `AttachmentStreamResolver`, catálogo) — sin docs, nadie en el equipo entiende cómo usar nada de eso.

---

## Plan B — Catalog-driven behavior + tool result cleanup (9 tasks)

> **Idea general:** activar las optimizaciones de costo que Plan A habilitó (sin Plan B, el LLM seguía recibiendo todo el contenido en cada turno sin necesidad), y limpiar el schema de tool results eliminando los aliases legacy.

### Task 1: Desactivar autoinject en el primer turno
**Qué hizo:** cuando el LLM node recibe `inputs.files[]`, **ya NO incluye los bytes en el mensaje user del primer turno**. El modelo solo ve el catálogo.
**Por qué:** ahorro de tokens. Antes, si subías un PDF de 50 páginas, el modelo recibía las 50 páginas en su contexto en cada turno aunque solo necesitara leer una vez. Ahora el modelo decide explícitamente con `load_attachment(<id>)` cuándo cargar contenido.

### Task 2: Update `ATTACHMENTS_SYSTEM_PRELUDE`
**Qué hizo:** modificó el bloque de instrucciones del system message para explicar el nuevo contrato: "los docs no se autocargan, llamá `load_attachment` para leer, son efímeros".
**Por qué:** sin esta instrucción, el LLM no sabe que tiene que llamar `load_attachment`. Empezaría a inventar que no ve archivos.

### Task 3: `load_attachment` efímero por turno
**Qué hizo:** cuando el LLM llama `load_attachment(<id>)`, el contenido se inyecta **solo para ese turno**. En el `llm_node_history` se persiste un marker text en lugar del contenido real.
**Por qué:** misma idea del ahorro de tokens. Si en turno 3 el modelo carga el PDF para responder, en turno 4 NO debería cargarlo de nuevo en el contexto a menos que lo necesite. El marker permite al modelo recordar "ya consulté este doc" sin pagar tokens por el contenido.

### Tasks 4-6: Eliminar `attachment_id` y `url` de los tool results
**Qué hizo:** los outputs de `image_generation` / `image_edit` / `tts` ahora devuelven solo `{ document_id, mime_type, size_bytes }`. Eliminaron los campos legacy `attachment_id` (= storage_key) y `url` (signed URL).
**Por qué:** principio de namespace único. Antes había confusión: el modelo veía `attachment_id` (storage_key crudo) y `document_id` al mismo tiempo, sin saber cuál usar. Ahora hay uno solo (`document_id`).
**Impacto:** **este es el cambio que rompe ADP.** El backend NestJS y el frontend usan `attachment_id` + `url`.

### Task 7: Tests E2E de Plan B
**Qué hizo:** tests confirmando el comportamiento de no-autoinject + load_attachment efímero.
**Por qué:** estos cambios son sutiles y atraviesan múltiples componentes. Sin tests son fáciles de romper.

### Task 8: Actualización de docs
**Qué hizo:** actualizó developer guides + node configurations + CLAUDE.md.
**Por qué:** plumbing documentación.

### Task 9: ADP migration notes
**Qué hizo:** creó el spec autoritativo que el equipo ADP usa para hacer su parte.
**Por qué:** sin este doc, el equipo ADP no sabe qué tiene que hacer. Es el contrato cross-team.

---

## Plan C — TTL cleanup binary (5 tasks)

> **Idea general:** con Plan A persistiendo bytes uniformemente, el storage crece sin tope. Plan C agrega el proceso periódico que purga attachments stale.

### Task 1: Domain trait methods `find_stale_attachments` + `delete_attachment`
**Qué hizo:** agregó 2 métodos al `AttachmentRegistry`: encontrar filas con `last_used_at` viejo, y borrar una fila por ID.
**Por qué:** son los building blocks del cleanup loop.

### Task 2: Implementaciones Postgres + SQLite
**Qué hizo:** SQL para los dos métodos. Query usa `COALESCE(last_used_at, registered_at)` para que filas que nunca se accedieron también puedan expirar.
**Por qué:** sin el COALESCE, attachments registrados pero nunca usados quedarían vivos para siempre.

### Task 3: `OutputStorageRepository::delete` en los 3 adapters
**Qué hizo:** método `delete(storage_key)` en LocalCache (memoria), LocalHttp (filesystem) y HttpCallback (POST a `/internal/gcs/delete`).
**Por qué:** sin esto el binario solo puede borrar filas de DB pero no los blobs en GCS — los blobs quedarían huérfanos.

### Task 4: Binario `attachment_gc`
**Qué hizo:** binario standalone que lee env config, queries stale attachments en batches, y borra blob + fila para cada uno. Storage-first deletion (si falla el blob, se preserva la fila para retry).
**Por qué:** este es el proceso operacional real. Sin el binario, todo lo anterior es teoría.

### Task 5: Runbook operacional
**Qué hizo:** documentación de cómo deployar el binario en Cloud Run Job + Cloud Scheduler, monitoring, alerts.
**Por qué:** el binario tiene que correr periódicamente en prod. Sin runbook, devops no sabe cómo schedulearlo.

---

# Parte 2 — Cambios que ADP debe hacer

Hay 4 cambios principales en el backend NestJS de ADP. Cada uno tiene una razón clara basada en lo que cambió en colmena.

## Cambio 1: Verificación de schema (~10 min)

**Qué hacer:**
- Confirmar que `conversation_attachments` tiene las 3 columnas nuevas (`storage_key`, `origin`, `last_used_at`).
- Agregar un comment en `schema.prisma` aclarando que esa tabla es externamente gestionada.

**Por qué:**
La migración la corre colmena (sqlx), no ADP (Prisma). Pero si alguien en el futuro corre `prisma db pull` sin saber esto, Prisma va a "descubrir" la tabla y querer manejarla — generando conflictos.

**Sin esto:** riesgo latente. No rompe nada ahora pero deja un trap para más adelante.

## Cambio 2: Migrar `chat.service.ts::extractGeneratedAttachments` (~3-4 horas)

**Qué hacer:**
Modificar la función que extrae artifacts de los tool results de colmena. Ahora debe:
1. Leer `document_id` (nuevo campo) en lugar de `attachment_id` + `url` (eliminados).
2. Hacer query a `conversation_attachments` (raw SQL, porque es tabla colmena-owned) para resolver `storage_key`.
3. Generar signed URL fresca con `gcsService`.

**Por qué:**
Plan B eliminó `attachment_id` y `url` del tool result de `image_generation`/`image_edit`/`tts`. La función actual asume que ambos están presentes — si están undefined, no crea ningún `agent_attachment` y los artifacts generados desaparecen del chat.

**Sin esto:** **catástrofe user-visible.** Cada vez que un agente genera una imagen o audio, no aparece en el chat. El usuario ve la respuesta del modelo pero ningún artifact adjunto.

## Cambio 3: Nuevo endpoint `GET /api/attachments/:documentId/url` (~1 hora)

**Qué hacer:**
Endpoint público (auth de sesión) que recibe un `document_id` y devuelve una signed URL fresca. Verifica ownership joineando `conversation_attachments` con `agent_session.userId`.

**Por qué:**
El frontend usa `<img src={att.url}>` para renderizar. Con `url` eliminado del tool result, el frontend no tiene qué poner en `src`. Este endpoint le da una forma de pedirla on-demand. Bonus: signed URLs expiran ~7 días — el endpoint también sirve para refrescar URLs viejas.

**Sin esto:** aunque el cambio 2 esté hecho, el frontend no puede renderizar imágenes. La fila en `agent_attachment` existe pero sin URL utilizable.

## Cambio 4: Nuevo endpoint `POST /internal/gcs/delete` + `GcsService.deleteByKey` (~1.5 horas)

**Qué hacer:**
1. Endpoint interno (auth `X-Internal-Token`, mismo guard que `sign-put`) que recibe `{ storage_key }` y borra el blob de GCS. Idempotente: 204 si borró, 204 si no existía, 5xx si error transitorio.
2. Método `deleteByKey(storageKey)` en `GcsService` usando `bucket.file(key).delete({ ignoreNotFound: true })`.

**Por qué:**
El binario `attachment_gc` de colmena (Plan C) corre periódicamente y necesita borrar blobs. ADP es el único servicio con credenciales de GCS — colmena no puede borrar directamente. Este endpoint es el puente.

Importante: este endpoint **solo borra el blob de GCS**. NO toca filas de DB. Colmena maneja la fila por su lado (en `conversation_attachments`).

**Sin esto:** el storage crece sin tope. Cada imagen generada se queda en GCS forever, incluso después de que la sesión termine y nadie consulte el chat. Eventualmente sale caro.

---

# Parte 3 — Tablas: target state vs current state

Hay 2 tablas relevantes en la DB compartida (`colmena_llm_memory`):

## Tabla 1: `conversation_attachments` (colmena-owned)

### Cómo debe estar (target state después de Plan A)

```
                                    Table "public.conversation_attachments"
       Column        |           Type           | Collation | Nullable |      Default
---------------------+--------------------------+-----------+----------+--------------------
 agent_session_id    | text                     |           | not null |
 document_id         | text                     |           | not null |
 provider            | text                     |           | not null |
 provider_file_id    | text                     |           | not null |
 mime_type           | text                     |           | not null |
 filename            | text                     |           | not null |
 size_bytes          | bigint                   |           |          |
 label               | text                     |           |          |
 description         | text                     |           |          |
 source_kind         | text                     |           | not null |
 source_value        | text                     |           |          |
 registered_at       | timestamp with time zone |           | not null | now()
 refreshed_at        | timestamp with time zone |           | not null | now()
 storage_key         | text                     |           |          |              ← Plan A
 origin              | text                     |           |          |              ← Plan A
 last_used_at        | timestamp with time zone |           |          |              ← Plan A
Indexes:
    "conversation_attachments_pkey" PRIMARY KEY, btree (agent_session_id, document_id, provider)
    "idx_conv_attachments_session_used" btree (agent_session_id, last_used_at)  ← Plan A
```

### Cómo está ahora (current state)

Depende de si la migración de colmena Plan A se aplicó o no. Posibilidades:
- **Estado A** (pre-migración): NO tiene `storage_key`, `origin`, `last_used_at`. La columna `idx_conv_attachments_session_used` tampoco.
- **Estado B** (post-migración correcta): las 3 columnas + el índice presentes.
- **Estado C** (drift): las columnas existen pero la fila de `_sqlx_migrations` tiene un checksum diferente al del archivo → worker no arranca.

El script de la Parte 4 te dice en cuál estado estás.

### Semántica de cada columna nueva

| Columna | Tipo | Significado |
|---|---|---|
| `storage_key` | `TEXT` (nullable) | Path del blob en `OutputStorageRepository` (en prod, path en GCS). Null para filas viejas pre-Plan-A. |
| `origin` | `TEXT` (nullable) | `user_upload` si el user subió el doc, `generated_by:<tool>` si lo generó una tool (ej. `generated_by:image_generation`). |
| `last_used_at` | `TIMESTAMPTZ` (nullable) | Timestamp de la última vez que se accedió al doc vía resolver o `load_attachment`. Null si nunca se accedió desde la migración. |

## Tabla 2: `agent_attachment` (ADP-owned)

### Cómo debe estar (target state — NO CAMBIA)

```
                            Table "public.agent_attachment"
   Column     |           Type           | Collation | Nullable |      Default
--------------+--------------------------+-----------+----------+----------------
 id           | text                     |           | not null |
 messageId    | text                     |           | not null |
 fileName     | text                     |           | not null |
 mimeType     | text                     |           | not null |
 sizeInBytes  | integer                  |           | not null |
 url          | text                     |           | not null |
 storageKey   | text                     |           |          |
 source       | text                     |           | not null | 'user'
 createdAt    | timestamp(3) with time z |           | not null | now()
 updatedAt    | timestamp(3) with time z |           | not null |
Indexes:
    "agent_attachment_pkey" PRIMARY KEY, btree (id)
    "agent_attachment_messageId_storageKey_idx" btree (messageId, storageKey)
```

**Estructura: SIN CAMBIOS.** Plan A/B/C no toca esta tabla.

### Lo que cambia: el comportamiento de cómo se popula

**Antes:**
```
storage_key  = (from tool result attachment_id, was a UUID-like storage path)
url          = (from tool result url, signed URL with ~7d expiry)
```

**Después:**
```
storage_key  = (looked up from conversation_attachments by document_id)
url          = (freshly signed via GcsService.generateReadSignedUrlForKey)
```

Los valores siguen siendo del mismo tipo. La diferencia es **cómo se obtienen**.

## Tabla 3: `_sqlx_migrations` (sqlx internal)

### Cómo debe estar

Debe contener una fila para la migración de Plan A:

```
 version             | description                       | installed_on | success | checksum | execution_time
---------------------+-----------------------------------+--------------+---------+----------+----------------
 20260525000001      | attachment_uniform_resolution     | 2026-05-25.. | t       | <hash>   | ...
```

El `<hash>` debe coincidir con el del archivo `migrations/postgres/20260525000001_attachment_uniform_resolution.sql` del repo colmena.

### Estados posibles

- **Estado A:** no hay fila con `version = 20260525000001`. Próximo deploy del worker la aplica.
- **Estado B:** fila presente, checksum coincide. Todo OK.
- **Estado C:** fila presente, checksum NO coincide. El worker entra en boot loop con error `VersionMismatch(20260525000001)`. Acción: borrar la fila con `DELETE FROM _sqlx_migrations WHERE version = 20260525000001`. La migración es idempotente (`ADD COLUMN IF NOT EXISTS`) así que reaplicarla no rompe nada.

---

# Parte 4 — Script bash de diagnóstico

Copiar y pegar el siguiente script. Asume que `DATABASE_URL` está exportado (apuntando a la DB compartida `colmena_llm_memory`).

```bash
#!/bin/bash
# attachment_migration_status.sh
# Inspecciona el estado real de la DB para entender en qué punto de la migración estamos.

set -uo pipefail

if [[ -z "${DATABASE_URL:-}" ]]; then
  echo "❌ DATABASE_URL no está exportada. Setearla antes de correr este script."
  echo "   ej: export DATABASE_URL='postgresql://user:pass@host:5432/colmena_llm_memory?sslmode=require'"
  exit 1
fi

REDACTED_URL="${DATABASE_URL%%@*}@***"
echo "🔍 Inspeccionando: $REDACTED_URL"
echo ""

# ============================================================
# 1. Conectividad básica
# ============================================================
echo "═══════════════════════════════════════════════════════════"
echo " 1. Conectividad"
echo "═══════════════════════════════════════════════════════════"
if ! psql "$DATABASE_URL" -tAc "SELECT 1" >/dev/null 2>&1; then
  echo "❌ No se puede conectar a la DB. Verificar DATABASE_URL y red."
  exit 1
fi
echo "✅ Conexión OK"
DB_VERSION=$(psql "$DATABASE_URL" -tAc "SELECT version()" | head -1 | cut -c1-60)
echo "   Postgres: $DB_VERSION"
echo ""

# ============================================================
# 2. Estado de la tabla conversation_attachments
# ============================================================
echo "═══════════════════════════════════════════════════════════"
echo " 2. conversation_attachments (colmena-owned)"
echo "═══════════════════════════════════════════════════════════"

TABLE_EXISTS=$(psql "$DATABASE_URL" -tAc "
  SELECT EXISTS (
    SELECT FROM information_schema.tables
    WHERE table_schema = 'public' AND table_name = 'conversation_attachments'
  );
")

if [[ "$TABLE_EXISTS" != "t" ]]; then
  echo "❌ La tabla 'conversation_attachments' NO existe."
  echo "   Es la tabla principal que maneja colmena. Si no existe, algo está muy mal."
  exit 1
fi
echo "✅ Tabla existe"

echo ""
echo "Columnas actuales:"
psql "$DATABASE_URL" -c "\d conversation_attachments" 2>/dev/null | head -40

echo ""
echo "Chequeo específico de columnas de Plan A (storage_key, origin, last_used_at):"
PLAN_A_COLUMNS=$(psql "$DATABASE_URL" -tAc "
  SELECT column_name FROM information_schema.columns
  WHERE table_name = 'conversation_attachments'
    AND column_name IN ('storage_key', 'origin', 'last_used_at')
  ORDER BY column_name;
")

if [[ -z "$PLAN_A_COLUMNS" ]]; then
  echo "❌ NINGUNA de las columnas de Plan A está presente."
  echo "   → Estado: PRE-MIGRACIÓN. El worker debe correr la migración al próximo deploy."
elif [[ $(echo "$PLAN_A_COLUMNS" | wc -l) -lt 3 ]]; then
  echo "⚠️  Solo algunas columnas de Plan A están presentes:"
  echo "$PLAN_A_COLUMNS"
  echo "   → Estado: migración parcial / corrupta. Investigar."
else
  echo "✅ Las 3 columnas de Plan A presentes: $(echo $PLAN_A_COLUMNS | tr '\n' ' ')"
  echo "   → Estado: POST-MIGRACIÓN."
fi

echo ""
echo "Conteo de filas:"
ROW_COUNT=$(psql "$DATABASE_URL" -tAc "SELECT COUNT(*) FROM conversation_attachments;")
echo "   Total: $ROW_COUNT filas"

# Si las columnas de Plan A existen, mostrar distribución por estado
if [[ $(echo "$PLAN_A_COLUMNS" | wc -l) -eq 3 ]]; then
  echo ""
  echo "Distribución de filas:"
  psql "$DATABASE_URL" -c "
    SELECT
      COUNT(*) FILTER (WHERE storage_key IS NULL) AS sin_storage_key,
      COUNT(*) FILTER (WHERE storage_key IS NOT NULL) AS con_storage_key,
      COUNT(*) FILTER (WHERE origin IS NULL) AS sin_origin,
      COUNT(*) FILTER (WHERE origin = 'user_upload') AS user_uploads,
      COUNT(*) FILTER (WHERE origin LIKE 'generated_by:%') AS generated,
      COUNT(*) FILTER (WHERE last_used_at IS NULL) AS nunca_accedidos,
      COUNT(*) FILTER (WHERE last_used_at IS NOT NULL) AS accedidos
    FROM conversation_attachments;
  "

  echo ""
  echo "Sample de filas recientes (top 3):"
  psql "$DATABASE_URL" -c "
    SELECT document_id,
           LEFT(provider, 10) AS provider,
           LEFT(filename, 30) AS filename,
           LEFT(mime_type, 25) AS mime,
           CASE WHEN storage_key IS NOT NULL THEN '✅' ELSE '❌' END AS has_sk,
           origin,
           registered_at::date AS reg_date,
           last_used_at::date AS last_used
    FROM conversation_attachments
    ORDER BY registered_at DESC
    LIMIT 3;
  "
fi

# ============================================================
# 3. Estado de la tabla agent_attachment
# ============================================================
echo ""
echo "═══════════════════════════════════════════════════════════"
echo " 3. agent_attachment (ADP-owned)"
echo "═══════════════════════════════════════════════════════════"

ADP_TABLE_EXISTS=$(psql "$DATABASE_URL" -tAc "
  SELECT EXISTS (
    SELECT FROM information_schema.tables
    WHERE table_schema = 'public' AND table_name = 'agent_attachment'
  );
")

if [[ "$ADP_TABLE_EXISTS" != "t" ]]; then
  echo "❌ La tabla 'agent_attachment' NO existe."
  echo "   Algo está muy mal con el schema de ADP."
else
  echo "✅ Tabla existe"

  ADP_COUNT=$(psql "$DATABASE_URL" -tAc "SELECT COUNT(*) FROM agent_attachment;")
  echo "   Total: $ADP_COUNT filas"

  echo ""
  echo "Distribución por source:"
  psql "$DATABASE_URL" -c "
    SELECT source, COUNT(*) AS total
    FROM agent_attachment
    GROUP BY source
    ORDER BY total DESC;
  "

  echo ""
  echo "Sample de filas recientes (top 3):"
  psql "$DATABASE_URL" -c "
    SELECT id,
           LEFT(\"fileName\", 30) AS filename,
           LEFT(\"mimeType\", 25) AS mime,
           CASE WHEN \"storageKey\" IS NOT NULL THEN '✅' ELSE '❌' END AS has_sk,
           source,
           \"createdAt\"::date AS created
    FROM agent_attachment
    ORDER BY \"createdAt\" DESC
    LIMIT 3;
  "
fi

# ============================================================
# 4. Estado de _sqlx_migrations
# ============================================================
echo ""
echo "═══════════════════════════════════════════════════════════"
echo " 4. _sqlx_migrations (migración de Plan A)"
echo "═══════════════════════════════════════════════════════════"

SQLX_EXISTS=$(psql "$DATABASE_URL" -tAc "
  SELECT EXISTS (
    SELECT FROM information_schema.tables
    WHERE table_schema = 'public' AND table_name = '_sqlx_migrations'
  );
")

if [[ "$SQLX_EXISTS" != "t" ]]; then
  echo "⚠️  La tabla '_sqlx_migrations' NO existe. El worker la creará al primer arranque."
else
  PLAN_A_MIG=$(psql "$DATABASE_URL" -tAc "
    SELECT version FROM _sqlx_migrations WHERE version = 20260525000001;
  ")

  if [[ -z "$PLAN_A_MIG" ]]; then
    echo "⚠️  La migración 20260525000001 (Plan A) NO está aplicada."
    echo "   → El worker la aplicará en el próximo arranque."
  else
    echo "✅ La migración 20260525000001 está aplicada."
    echo ""
    echo "Detalles:"
    psql "$DATABASE_URL" -c "
      SELECT version,
             description,
             installed_on::date AS installed_date,
             success,
             execution_time / 1000000 AS exec_time_ms
      FROM _sqlx_migrations
      WHERE version = 20260525000001;
    "

    echo ""
    echo "⚠️  Si tenés drift de checksum (worker no arranca con VersionMismatch):"
    echo "   psql \"\$DATABASE_URL\" -c \"DELETE FROM _sqlx_migrations WHERE version = 20260525000001;\""
    echo "   La migración es idempotente, el worker la re-aplica al próximo arranque."
  fi

  echo ""
  echo "Últimas 5 migraciones aplicadas:"
  psql "$DATABASE_URL" -c "
    SELECT version, description, installed_on::date AS installed, success
    FROM _sqlx_migrations
    ORDER BY version DESC
    LIMIT 5;
  "
fi

# ============================================================
# 5. Análisis para Plan C (TTL cleanup)
# ============================================================
if [[ $(echo "$PLAN_A_COLUMNS" | wc -l) -eq 3 ]]; then
  echo ""
  echo "═══════════════════════════════════════════════════════════"
  echo " 5. Estado para Plan C (cleanup TTL)"
  echo "═══════════════════════════════════════════════════════════"
  echo ""
  echo "Distribución por antigüedad (basado en COALESCE(last_used_at, registered_at)):"
  psql "$DATABASE_URL" -c "
    WITH age_buckets AS (
      SELECT
        CASE
          WHEN COALESCE(last_used_at, registered_at) > now() - interval '1 day' THEN '< 1 día'
          WHEN COALESCE(last_used_at, registered_at) > now() - interval '7 days' THEN '1-7 días'
          WHEN COALESCE(last_used_at, registered_at) > now() - interval '30 days' THEN '7-30 días'
          WHEN COALESCE(last_used_at, registered_at) > now() - interval '90 days' THEN '30-90 días'
          ELSE '> 90 días (limpieza obvia)'
        END AS edad,
        storage_key IS NOT NULL AS tiene_blob
      FROM conversation_attachments
    )
    SELECT edad,
           COUNT(*) AS total,
           COUNT(*) FILTER (WHERE tiene_blob) AS con_blob,
           COUNT(*) FILTER (WHERE NOT tiene_blob) AS sin_blob
    FROM age_buckets
    GROUP BY edad
    ORDER BY total DESC;
  "

  echo ""
  echo "💡 Tip: el binario attachment_gc borraría todo lo que tenga COALESCE(last_used_at, registered_at) < now() - COLMENA_ATTACHMENT_TTL_DAYS días."
  echo "   Default es 7 días, recomendado arrancar con 30 en prod."
fi

# ============================================================
# 6. Resumen
# ============================================================
echo ""
echo "═══════════════════════════════════════════════════════════"
echo " 6. Resumen"
echo "═══════════════════════════════════════════════════════════"

if [[ $(echo "$PLAN_A_COLUMNS" | wc -l) -eq 3 ]]; then
  echo "✅ Plan A: migración aplicada. ADP API puede empezar a usar las columnas nuevas."
else
  echo "⏳ Plan A: migración pendiente. Esperar al próximo deploy del worker de colmena."
fi

if [[ -n "$PLAN_A_MIG" ]] && [[ "$SQLX_EXISTS" == "t" ]]; then
  echo "✅ _sqlx_migrations: row de Plan A presente. Sin drift aparente."
elif [[ "$SQLX_EXISTS" == "t" ]]; then
  echo "⏳ _sqlx_migrations: row de Plan A pendiente."
fi

echo ""
echo "Próximos pasos:"
echo "  • Si Plan A está aplicado: el equipo ADP puede empezar los 4 cambios"
echo "    documentados en /Users/danielgarcia/startti/adp/docs/COLMENA_PLAN_B_C_PENDING.md"
echo "  • Si Plan A NO está aplicado: esperar el deploy del worker"
echo "    (apps/service/ia/platform/worker) que aplica la migración al startup."
echo ""
```

### Cómo usar el script

```bash
# Guardar el contenido como attachment_migration_status.sh
chmod +x attachment_migration_status.sh
export DATABASE_URL="postgresql://user:pass@host:5432/colmena_llm_memory?sslmode=require"
./attachment_migration_status.sh
```

### Qué interpretar del output

| Output | Significado | Acción |
|---|---|---|
| ✅ Las 3 columnas de Plan A presentes | Migración aplicada | ADP puede empezar a hacer queries con `storage_key`, `origin`, `last_used_at` |
| ⏳ Plan A: migración pendiente | El worker no ha aplicado la migración | Esperar el próximo deploy del worker; el `sqlx::migrate!` al startup la aplica |
| ❌ NINGUNA columna de Plan A presente + worker reciente | Posible problema | Mirar logs del worker (`gcloud run services logs read worker --limit 100`); buscar errores de migración |
| `sin_storage_key > 0` (filas con storage_key NULL) | Filas legacy de antes de Plan A | Normal. El binario `attachment_gc` las maneja correctamente (skip storage delete). Con el tiempo se borran por TTL. |
| Row de Plan A en `_sqlx_migrations` con `success = f` | Migración falló | Investigar logs del worker; resolver issue; borrar row y reintentar |
| Drift de checksum (worker boot loop) | Archivo SQL editado después de aplicarse | `DELETE FROM _sqlx_migrations WHERE version = 20260525000001` y reintentar |

---

# Apéndice — Quick reference de IDs

Hay 3 namespaces de IDs en este sistema. Confundirlos es la fuente número 1 de bugs.

| ID | Forma | Quién lo emite | Quién lo consume | Notas |
|---|---|---|---|---|
| `document_id` | string legible (`img_revenue_chart_a1b2c3` o user-provided como `q3_report`) | Colmena worker (al registrar en `conversation_attachments`) | LLM (en catálogo), `http_request` (en placeholder), ADP API (en queries + endpoint) | **Este es el ID público** que el LLM y el frontend ven. |
| `storage_key` | path interno (`chat-attachments/USER123/SESSION456/generated/img_xyz.png`) | ADP API (al crear blob) | Colmena worker (al leer), ADP API (al borrar/firmar), GCS | Detalle interno. Nunca expuesto al LLM. |
| `attachment_id` (Plan A) / id (Prisma) | cuid de Prisma (`clr1234567...`) | ADP API (Prisma autoincrement) | ADP frontend (legacy) | Solo dentro de ADP. No tiene nada que ver con colmena. |

**Antes de Plan B:** colmena confundía estos al emitir tanto `document_id` como `attachment_id` (= storage_key) en el tool result. Plan B unificó a solo `document_id`.

---

## Referencias

- Spec del diseño: `/Users/danielgarcia/startti/colmena/docs/superpowers/specs/2026-05-25-attachment-uniform-resolution-design.md`
- Plans implementados (A, B, C): `/Users/danielgarcia/startti/colmena/docs/superpowers/plans/`
- ADP migration spec autoritativo: `/Users/danielgarcia/startti/colmena/docs/superpowers/specs/2026-05-25-adp-migration-detailed.md`
- ADP API task checklist: `/Users/danielgarcia/startti/adp/docs/COLMENA_PLAN_B_C_PENDING.md`
- Runbook del binario `attachment_gc`: `/Users/danielgarcia/startti/colmena/docs/developer_guide/36_attachment_gc.md`
