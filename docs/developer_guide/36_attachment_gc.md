# 36. Attachment GC — Cleanup binary for TTL'd attachments

Binario standalone que recorre `conversation_attachments`, encuentra filas cuyo
`COALESCE(last_used_at, registered_at) < now() - N días`, y borra:
1. El blob asociado en `OutputStorageRepository` (vía `delete(storage_key)`).
2. La fila en `conversation_attachments` (`delete_attachment(agent_session_id, document_id)`).

## Configuración

| Env var | Default | Descripción |
|---|---|---|
| `COLMENA_ATTACHMENT_TTL_DAYS` | `7` | Edad máxima (en días) que una fila puede tener sin ser usada antes de eliminarse. |
| `COLMENA_ATTACHMENT_GC_BATCH_SIZE` | `100` | Tamaño del batch que se procesa por iteración. |
| `DATABASE_URL` | (requerido) | Misma DB que el dag_engine. |
| `COLMENA_LOCAL` | `false` | Si `true`, usa `LocalHttpStorageAdapter` o `LocalCacheStorageAdapter` (para dev). |
| `COLMENA_STORAGE_CALLBACK_URL` | (prod) | Mismo callback que el dag_engine para sign-put. El GC usará `<base>/internal/gcs/delete`. |
| `COLMENA_STORAGE_CALLBACK_SECRET` | (prod) | Mismo secret. |

El binario reutiliza `EngineConfig::from_env` para el wiring del adapter de
storage — la lista de variables de entorno relevantes es exactamente la misma
que consume el `dag_engine`. Migrations de Postgres se corren al inicio sobre
el pool interno (no-op si ya están aplicadas), así que el binario es seguro
contra una DB recién aprovisionada.

CLI flags (override env):
- `--ttl-days N` — override de `COLMENA_ATTACHMENT_TTL_DAYS`.
- `--batch-size N` — override de batch size.
- `--dry-run` — log lo que borraría sin borrar nada.

## Comportamiento

1. Calcula `cutoff = now() - ttl_days`.
2. En loop: `find_stale_attachments(cutoff, batch_size)` → procesa cada fila:
   1. Borra el blob: `storage.delete(storage_key)`. Si falla, **skip** (la fila se preserva, próximo run reintenta).
   2. Borra la fila: `registry.delete_attachment(sid, doc_id)`. Si falla, log error pero el blob ya está borrado.
3. Si el batch devuelve menos filas que `batch_size`, terminamos.
4. Sleep de 100ms entre batches para no martillar DB / storage backend.

Resultado final: log estructurado con `total_deleted` y `total_storage_errors`.

## Deployment

### Local (dev)

```bash
source .env
COLMENA_ATTACHMENT_TTL_DAYS=1 \
cargo run --bin attachment_gc -- --dry-run
```

Sin `--dry-run` borra de verdad. Usar con cuidado si tu DB local tiene data importante.

### Producción (GCP)

Recomendación: **Cloud Scheduler → Cloud Run Job**.

1. Build una imagen Docker que contenga el binario `attachment_gc` (puede ser la misma imagen del dag_engine — el binario está en el mismo workspace).
2. Crear un Cloud Run Job:
   ```bash
   gcloud run jobs create attachment-gc \
     --image gcr.io/PROJECT/colmena:latest \
     --command attachment_gc \
     --set-env-vars=COLMENA_ATTACHMENT_TTL_DAYS=7 \
     --set-env-vars=COLMENA_STORAGE_CALLBACK_URL=https://your-host-api.example.com/internal/gcs/sign-put \
     --set-secrets=DATABASE_URL=projects/PROJECT/secrets/database-url:latest \
     --set-secrets=COLMENA_STORAGE_CALLBACK_SECRET=projects/PROJECT/secrets/storage-callback-secret:latest \
     --max-retries=1 \
     --task-timeout=10m
   ```
3. Crear un Cloud Scheduler trigger:
   ```bash
   gcloud scheduler jobs create http attachment-gc-trigger \
     --schedule="0 3 * * *" \
     --uri="https://run.googleapis.com/v2/projects/PROJECT/locations/REGION/jobs/attachment-gc:run" \
     --http-method=POST \
     --oauth-service-account-email=cloud-scheduler@PROJECT.iam.gserviceaccount.com
   ```
   (Diario a las 3 AM UTC.)

### Endpoint requerido en la host application

El `HttpCallbackStorageAdapter::delete` postea a `<base>/internal/gcs/delete` con body `{ "storage_key": "..." }` + header `X-Internal-Token`. La host application debe:
1. Validar el header.
2. Borrar el blob de GCS por su path (`storage_key`).
3. Devolver 200 si tuvo éxito, 404 si el blob no existía (también OK — el GC lo trata como idempotente).
4. Devolver 5xx en caso de error transitorio (el GC reintentará en la próxima corrida).

## Monitoring

Logs estructurados con target `colmena::attachment_gc`. Filtros útiles en Cloud Logging:

```
resource.type="cloud_run_job"
resource.labels.job_name="attachment-gc"
jsonPayload.event=("gc.start" OR "gc.end")
```

Métricas a vigilar:
- `total_deleted` por corrida — debería crecer linealmente con el throughput de attachments.
- `total_storage_errors` — debería ser 0 o casi 0 en estado normal. Si crece, la host application está fallando o la API del backend de storage tiene problemas.
- Duración de la corrida — si crece más allá de la mitad del task-timeout, considerar bajar `batch_size` o paralelizar.

## Rollback

El binario no tiene rollback — los blobs/filas borrados son permanentes. Para "rollback" en sentido operacional:
- Pausar el Cloud Scheduler job (`gcloud scheduler jobs pause attachment-gc-trigger`).
- Si descubrís que el TTL está muy agresivo y borraste algo importante, subí `COLMENA_ATTACHMENT_TTL_DAYS` a un valor que no vuelva a ser superado pronto. Pero los datos ya borrados no vuelven.

## Riesgos conocidos

- **Storage delete failure → blob huérfano**: si la host application falla al borrar el blob pero responde 5xx, el GC preserva la fila y reintenta. Si la host application borra el blob pero responde 5xx (raro), tendremos una fila sin blob — el próximo run intentará borrar el blob, recibirá 404, y borrará la fila. Idempotencia salva.
- **Registry delete failure post storage delete**: si borramos el blob pero falla la fila, el próximo run intentará borrar el blob de nuevo (404 OK) y luego borrará la fila. Idempotencia salva.
- **TTL muy bajo**: borra docs que el usuario quería conservar. Mitigation: empezar con `COLMENA_ATTACHMENT_TTL_DAYS=30` en prod por las primeras semanas, monitorear quejas, bajar gradualmente.
- **TTL muy alto**: storage crece sin tope. Mitigation: monitorear el tamaño de la tabla `conversation_attachments` y el tamaño del bucket GCS.
