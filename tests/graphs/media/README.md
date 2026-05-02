# Test graphs con signed URLs

Estos graphs validan el path de archivos grandes con la nueva schema `id`+`url`.

## Test files

- `image_url_anthropic.json` / `image_url_openai.json` / `image_url_gemini.json` — imágenes JPEG vía signed URL. Path URL passthrough en Anthropic+OpenAI; Files API en Gemini.
- `pdf_url_anthropic.json` / `pdf_url_openai.json` / `pdf_url_gemini.json` — PDFs ≥ 30 MB vía signed URL. Path Files API en los 3 providers.

Las signed URLs de GCS tienen TTL típico de 6 h. **Hay que regenerarlas manualmente antes de cada corrida.**

## Pasos para regenerar la URL firmada

1. Subir un archivo de prueba a un bucket GCS al que tengas acceso:

   ```sh
   gsutil cp tu-archivo.pdf gs://<your-bucket>/test/
   ```

2. Generar la signed URL con TTL de 6 h:

   ```sh
   gsutil signurl -d 6h <your-service-account.json> gs://<your-bucket>/test/tu-archivo.pdf
   ```

   El comando emite la URL completa (`https://storage.googleapis.com/...?X-Goog-Signature=...`).

3. Reemplazar en el JSON correspondiente:
   - `url` → la URL recién firmada.
   - `id` → un id único (ej. `tu-doc-2026-05-02`). Usar el mismo en los 3 archivos si quieres validar cache cross-provider, o distintos para forzar uploads independientes.
   - `size_bytes` → tamaño real en bytes (`stat -c%s archivo.pdf` en Linux).

4. Cargar las API keys del `.env` y ejecutar (con verbose para ver los logs `[file-resolve]`):

   ```sh
   set -a; source .env; set +a
   COLMENA_VERBOSE=1 cargo run --bin dag_engine -- run tests/graphs/media/pdf_url_anthropic.json
   ```

## Qué valida cada graph

- **Primera corrida**: cache miss → download GCS → (URL passthrough o upload a Files API según provider+mime) → respuesta del LLM.
- **Segunda corrida con el mismo `id`**: cache HIT alive → skip download/upload completos. Solo se hace la llamada al LLM con el `provider_file_id` cacheado.
- **48h+ con Gemini**: cache HIT pero `expires_at` pasado → re-upload automático.
- **Si el provider borra el archivo manualmente**: el LLM devuelve `ProviderFileNotFound` → invalidate cache + 1 retry duro (best-effort, ver deuda en `docs/developer_guide/28_large_files_api.md`).

## Verificar la fila en Postgres

```sh
psql "$DATABASE_URL" -c "SELECT document_id, provider, provider_file_id, expires_at, last_used_at FROM provider_file_cache;"
```

## Límites de producto observados (no del transporte, sino de cada API)

Si tu archivo excede el límite del modelo, el upload se hace bien pero la generación falla:

| Provider | Límite del modelo |
|----------|-------------------|
| Anthropic | 100 páginas máx por PDF; ventana de contexto ~200k tokens en Haiku 4.5 |
| OpenAI | 32 MB de pull interno tras Files API; gpt-4o-mini procesa ~1M tokens vía `file_id` en Responses |
| Gemini | 3000 páginas teóricas; algunos modelos rechazan referencias > 20 MB con "files bytes too large to be read" |

La integración nuestra es correcta; lo que falla en esos casos es el modelo. Para documentos muy grandes la estrategia recomendada es RAG (extracción de chunks de texto antes del LLM call).

## Más detalles

Para el comportamiento completo del manejo de archivos, ver:

- `docs/developer_guide/28_large_files_api.md` — guía de usuario.
- `docs/superpowers/specs/2026-05-02-large-document-files-api-design.md` — diseño + sección "Hallazgos de integración real".
