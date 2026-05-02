# Test graphs con signed URLs

Los archivos `pdf_url_anthropic.json`, `pdf_url_openai.json` y `pdf_url_gemini.json`
contienen placeholders `REPLACE_WITH_*` porque las signed URLs de GCS expiran a las
6 horas. **Hay que regenerarlas manualmente antes de cada corrida.**

## Pasos para regenerar

1. Subir un PDF de prueba (puedes usar `tests/graphs/media/fixtures/tiny.pdf`) a un
   bucket GCS al que tengas acceso:

   ```sh
   gsutil cp tests/graphs/media/fixtures/tiny.pdf gs://<your-bucket>/test/tiny.pdf
   ```

2. Generar la signed URL con TTL de 6 horas:

   ```sh
   gsutil signurl -d 6h <your-service-account.json> \
       gs://<your-bucket>/test/tiny.pdf
   ```

   El comando emite la URL completa
   (`https://storage.googleapis.com/...?X-Goog-Signature=...`).

3. Reemplazar en cada `pdf_url_*.json`:
   - `REPLACE_WITH_GCS_SIGNED_URL` → la URL recién firmada.
   - `REPLACE_WITH_UNIQUE_DOC_ID` → un id único (ej. `tiny-2026-05-02`). Mismo id
     en los 3 archivos si quieres que el cache cubra los tres providers, o ids
     distintos para forzar uploads independientes por provider.
   - `size_bytes` → tamaño real del archivo en bytes (`stat -c%s tiny.pdf` en Linux).

4. Cargar las API keys del `.env` y ejecutar:

   ```sh
   set -a; source .env; set +a
   cargo run --bin dag_engine -- run tests/graphs/media/pdf_url_anthropic.json
   ```

## Qué valida cada graph

- **Primera corrida**: cache miss → download GCS → upload a la Files API del provider →
  llamada al LLM con `file_id` referenciado.
- **Segunda corrida con el mismo `id`** (sin cambiar el JSON, mientras la URL siga
  válida y la fila de cache exista): cache hit, el use case **no** descarga ni sube
  de nuevo.
- **Después de 48h con Gemini**: cache hit pero `expires_at` pasado → re-upload
  automático.
- **Si Anthropic/OpenAI borran el archivo manualmente**: el LLM devuelve
  `ProviderFileNotFound` → invalidate cache + 1 retry duro automático.

## Verificar la fila de cache

Con `DATABASE_URL` apuntando al mismo Postgres usado por el runtime:

```sh
psql "$DATABASE_URL" -c "SELECT document_id, provider, provider_file_id, expires_at FROM provider_file_cache;"
```

## Regenerar el fixture base (opcional)

Si `tests/graphs/media/fixtures/tiny.pdf` no existe, ya hay otros PDFs en
`tests/graphs/media/fixtures/`. Cualquier archivo > 30 MB válido sirve para
ejercitar la ruta de signed URL (la ruta de `data` inline ya está cubierta por
los graphs `pdf_anthropic.json`, `pdf_openai.json`, `pdf_gemini.json`).
