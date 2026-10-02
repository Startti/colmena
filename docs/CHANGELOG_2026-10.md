# Cambios recientes — 2026-10

> **Alcance:** Commits sobre `develop` desde el cierre de `2026-09`.

## Cómo leer este documento

Una sección por feature. Cada sección contiene:
- **Qué cambió** — efecto observable.
- **Documentación de referencia** — spec, plan, dev guide, schema.
- **Commits** — rango o lista.
- **Estado** — done / partial.

---

## 1. Tavily: el mensaje de la página de bloqueo ya no dice que la key está bien

**Qué cambia.** El texto fijo que §208 de [CHANGELOG_2026-09](CHANGELOG_2026-09.md) puso para un 403 con página
de bloqueo afirmaba que no era un problema de la key y que se levantaba en unos minutos. Medido el 2026-10-01, era
falso: la página salió 13+ horas seguidas y la causa era la key. El proveedor rechazaba con la página esa key
concreta cuando llegaba desde IPs de Google Cloud (la misma key desde otra red daba 200; sin key desde la misma red,
el 401 JSON normal; otra key desde Google Cloud, 200); cambiarla lo arregló. Los agentes leyeron «not a problem with
the API key / clears within a few minutes», le dijeron al usuario que esperara, y un chat reintentó tres veces y
después contestó de memoria. El mensaje nuevo:

> The search provider refused this search with a block page (HTTP 403); the cause may be the network the call
> comes from or the provider blocking this API key. Retrying in this turn will not fix it: tell the person web
> search is unavailable and do not answer as if you had searched.

La clasificación no cambia: sigue siendo `Upstream { status: 403 }`, recuperable, el use case no lo reintenta (solo
5xx y transporte) y llega al modelo como `upstream_error` con `retryable: false`. Guía:
[25_web_nodes.md](developer_guide/25_web_nodes.md).
**Tests.** En `tavily_adapter`: el mensaje nombra las dos causas (la red y la key bloqueada), pide no reintentar,
decir que la búsqueda no está disponible y no contestar como si se hubiera buscado, y es ASCII; un test nuevo fija que
no dice que la key está bien ni promete que esperar lo arregla; `search` sobre un servidor que contesta 403 con la
página ya no trae «not a problem with the API key». En `search_use_case`: un `Upstream` 403 se intenta una sola vez.
En `tavily_client`: un `Upstream` 403 llega al modelo con `retryable: false` y el mensaje tal cual.
**ADP.** Subir el pin; nada más. **Estado.** done.

## 2. `http_request`: una respuesta que es un archivo queda como adjunto de la sesión

**Qué cambia.** Antes, si la API respondía un archivo (un PDF, una imagen), `http_request` intentaba parsearlo como
JSON, fallaba y devolvía `body: null`: los bytes se perdían y el agente no tenía cómo entregarlo. Caso real: un
agente de reservas de hoteles no podía devolver el voucher (`GET /reservations/{id}/voucher` de Despegar responde un
PDF). Ahora, cuando la respuesta es un archivo, el nodo lo guarda por el puerto de storage, lo registra en el registro
de adjuntos de la sesión con un `document_id` `file_*` (origin `generated_by:http_request`, fail-soft como en
`image_generation`) y devuelve:

```json
{ "status": 200, "body": null,
  "files": [{ "document_id": "file_voucher_f49b0ccb", "mime_type": "application/pdf",
              "filename": "voucher.pdf", "size_bytes": 37975 }] }
```

Qué es un archivo lo deciden primero los bytes (PDF, PNG, JPEG, GIF, WebP, WAV, MP3, OGG, MP4; un ZIP cede al
`Content-Type` si dice qué Office es) y después el `Content-Type`: Despegar manda
`application/json;charset=utf-8,application/pdf;charset=utf-8`, con JSON primero para un PDF. El JSON se sigue
parseando en `body`; texto, HTML y cuerpos vacíos siguen siendo `body: null`; un archivo por encima de
`max_file_size_bytes`, sin storage adapter o con `store` fallido también. `body` sigue en `null`, así que quien lee
`body` no ve diferencia. Guía: [32_multimedia_generation.md](developer_guide/32_multimedia_generation.md); catálogo:
puerto `files` en [node_configurations.json](node_configurations.json).
**Tests.** `response_file` (la cabecera con JSON primero no gana sobre los bytes de un PDF; texto y HTML no son
archivos; un ZIP cede al Office declarado; el nombre sale del `Content-Disposition` o de la URL y no lleva `/`);
`file_response_tests` en `http.rs` (un PDF se guarda, se registra y sale en `files`; sin storage, `body: null`; por
encima del tope `store` no se llama; HTML no es archivo y el JSON se sigue parseando). E2E con el motor real:
`tests/graphs/external/http_file_response.json` y el voucher del sandbox de Despegar.
**Commits.** #459 (clasificador), #460 (el nodo). **ADP.** Subir el pin. Startti/adp#1036 ya muestra el `files[]`
de cualquier tool en el chat como archivo descargable y en `output.files` de `/v1/run`. **Estado.** done.

## 3. OAuth: puerto `HostTokenPort` y `HostRefreshTokenProvider` (token refrescado por el host)

**Qué cambia.** El motor puede pedirle al embebedor un access token nuevo a mitad de corrida, para una conexión cuyo
`client_secret` nunca le llega. Aditivo: `AuthTokenProvider::invalidate()` (vacío por defecto;
`OAuthRefreshTokenProvider` vacía su caché), el puerto en `ports.rs`, `EngineConfig.host_token_port` (llega al
`http_request` del registry), `HostRefreshTokenProvider` y `OAuthError::HostRefused`. Ningún nodo lo usa todavía.
Guía: [47_google_oauth.md](developer_guide/47_google_oauth.md#token-refrescado-por-el-host-hosttokenport).
**Tests.** `host_refresh_provider`, `token_provider` (`invalidate`), `registry` (el puerto llega al nodo).
**Mutación.** Margen `>=`, no limpiar el rechazo, sin hash, `invalidate` vacío, lock suelto antes del puerto, sin
timeout, `ClientCredsInvalid`: cada una tumba un test. **E2E.** No aplica hasta `bearer_refresh`.
**ADP.** Un literal `EngineConfig { … }` necesita `host_token_port: None`; el worker usa `from_env`. `corpus_noise`
pasa a 344: #460 sumó un grafo sin subirlo y dejó `develop` en rojo. **Estado.** partial.
