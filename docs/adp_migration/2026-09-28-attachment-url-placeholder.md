# `$attachment_url:<document_id>`: un link corto al adjunto en el body JSON de `http_request`

**Acción de ADP:** subir el pin; regenerar el allowlist de campos (`attachment_url_ttl_seconds` es nuevo);
en el mismo cambio, actualizar los textos que hoy dicen que un adjunto nunca viaja como URL (el bloque
`<session-images>`, la skill de `http_request`). El adaptador de storage del worker ya implementa
`read_url` con su tope; no hay código nuevo ahí.

## Qué cambia

- `http_request` reemplaza `"$attachment_url:<document_id>"` (valor entero de un string del body JSON) por
  la URL que da `read_url` del storage del host para esa fila de la sesión (CHANGELOG 2026-09 §184).
- Solo con `base_url`, `endpoint` y cualquier header `Host` del autor, y solo en un body **JSON** (nunca en
  `query_params` ni en una parte multipart); si no, el nodo falla antes de pedir la URL y antes de
  conectarse, y no sigue una redirección a otro origen (§184, §194).
- TTL: `attachment_url_ttl_seconds` del autor (config o `fixed`), 900 por defecto, leído solo cuando el body
  trae la forma; la librería lo pasa sin tope (§194).
- El preludio de adjuntos agrega un párrafo sobre la forma solo si `supports_read_url()` es `true`
  (§195).
- Rust: `AttachmentStreamResolver::resolve_url` nuevo, con default `Ok(None)` (§184).

## Qué ve ADP

- En el SSE, el resultado de la tool y el fin del nodo traen el placeholder donde la API devolvió la URL.
- Con un storage que declara `supports_read_url()`, todo agente con adjuntos recibe el párrafo nuevo del
  preludio. Un texto propio que diga lo contrario («never a URL») lo contradice.
- `read_url` recibe el `ttl` del autor o 900; el adaptador del worker ya lo recorta a 24 h.

## Qué se rompe si se ignora

Nada al compilar. Sin regenerar el allowlist, una escritura del DAG con `attachment_url_ttl_seconds` se
rechaza como campo inventado; sin actualizar los textos propios, el modelo recibe dos reglas opuestas.
