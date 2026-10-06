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

## 4. Google OAuth: bloque `google_workspace_auth` por nodo (porte de `feat/google-workspace-auth`, parte 1)

**Qué cambia.** `GoogleWorkspaceAuth` (`from_node_config`, `provider()` → `Arc<dyn AuthTokenProvider>` por
identidad) y el parser compartido con `http_request.auth`, donde un campo en blanco ahora cuenta como faltante.
Ausente = cuenta de plataforma; inválido = error. `Debug` redactado. Ningún nodo lo lee todavía.
Guía: [47_google_oauth.md](developer_guide/47_google_oauth.md#credenciales-por-nodo-google_workspace_auth).
**Commits portados.** `32a7d46c`, `438f0580`. **Tests.** `config`, `workspace_auth`, `http_oauth`.
**Mutación.** Ver la PR. **E2E.** No aplica hasta que un nodo lo lea. **ADP.** Ninguno. **Estado.** partial.

## 5. gsheets: fuente de token de la cuenta conectada (porte de `feat/google-workspace-auth`, parte 2)

**Qué cambia.** `TokenProvider` de gsheets guarda `Arc<dyn AuthTokenProvider>` (antes el concreto) y suma
`from_shared_provider`; un 401 llama `invalidate()` del trait. Un `invalid_grant` de la cuenta conectada da
`SheetsError::GoogleAccountReconnectRequired` → `google_account_reconnect_required`. La cuenta de plataforma no
cambia y ningún cliente usa la fuente nueva todavía. De la revisión de #463: una sola constante del endpoint de token
(`DEFAULT_TOKEN_ENDPOINT`, sale `GOOGLE_TOKEN_ENDPOINT`). Guía: [39_gsheets.md](developer_guide/39_gsheets.md).
**Commits portados.** Parte de `32a7d46c` y de `58a69afe` (`gsheets/infrastructure/auth.rs`, `errors.rs`,
`error_to_json`). **Tests.** `gsheets::infrastructure::auth`, `gsheets_tools` (payload) y un 401 con
`HostRefreshTokenProvider` que pide un token al host una vez y reintenta. **Mutación.**
`invalidate` sin efecto, sin el brazo de reconexión. **E2E.** No aplica. **ADP.** Ninguno. **Estado.** partial.

## 6. gsheets: cliente con la cuenta conectada (porte de `feat/google-workspace-auth`, parte 3)

**Qué cambia.** `GoogleSheetsHttpClient::from_config_with_auth(cfg, Option<&GoogleWorkspaceAuth>)`: `None` =
`from_config` de siempre; `Some` usa el provider compartido de esa identidad, no lee las env vars y deja
`share_email` vacío. Su 403 es `SheetsError::ConnectedAccountPermissionDenied` → `permission_denied` sin
`share_email`. Ningún dispatcher le pasa `auth` todavía. Guía: [39_gsheets.md](developer_guide/39_gsheets.md).
**Commits portados.** Parte de `32a7d46c` y `d94d2d37` (`http_client.rs`, `errors.rs`, `error_to_json`).
**Tests.** Credenciales del bloque sin env (con señuelos en env), `None` = camino env, el bearer sale del
`token_url` del bloque, 403 de cuenta conectada vs. de plataforma, payload. **Mutación.** 403 sin mirar la cuenta.
**E2E.** No aplica hasta el cableado de `llm_call`. **ADP.** Ninguno. **Estado.** partial.

## 7. `http_request`: `bearer_refresh` pide al host un token nuevo ante un 401 o cerca del vencimiento

**Qué cambia.** Clave hermana de `bearer_token`: `bearer_refresh: {handle, expires_at}`, en config o como entrada
`fixed` de `node_schema` (la que escribe el modelo se ignora). Con `EngineConfig.host_token_port`, el nodo arma por
ejecución un `HostRefreshTokenProvider` con el `agent_session_id` de la corrida y manda por `send_with_oauth_retry`,
que ahora toma `&dyn AuthTokenProvider` y llama `invalidate()`: ante un 401 pide un token y reintenta una vez; a
menos de 60 s del vencimiento lo pide antes de mandar. Si el host no da token, sale el sembrado y el 401 vuelve como
respuesta. Sin puerto se ignora; con multipart el `bearer_token` va estático; con `auth` o sin `bearer_token` es
error (sin repetir valores). Guía: [25_web_nodes.md](developer_guide/25_web_nodes.md#token-refrescado-por-el-host-bearer_refresh).
**Tests.** `http_oauth` (parser: ausente, bien formado, `fixed` vs del modelo, errores sin valores); `http.rs`
`bearer_refresh_tests` con wiremock: grafo por `DagRunUseCase` (401 → 1 pedido al puerto con el sha256 del token
sembrado y la sesión → 200), sin puerto, host que rechaza, vencimiento cercano, multipart, herramienta por
`DagToolExecutor`. **Mutación.** Sin `invalidate`, reintento tras el sembrado, sin respaldo (primer pedido o
reintento), `bearer_refresh` del modelo aceptado, sin sesión, handle vacío, multipart rechazado: cada una tumba un
test. **E2E.** Solo el camino sin puerto (CLI); con puerto espera al worker de ADP. **ADP.** Emitir
`bearer_refresh` solo con la compuerta abierta. **Estado.** partial.

## 8. gdocs: fuente de token y cliente con la cuenta conectada (porte de `feat/google-workspace-auth`, parte 4)

**Qué cambia.** `TokenCache` de gdocs guarda `Arc<dyn AuthTokenProvider>` y suma `from_shared_provider`; su
`invalidate()` es el del trait. `GoogleDocsHttpClient::from_config_with_auth(cfg, Option<&GoogleWorkspaceAuth>)`:
`None` = `from_config` de siempre; `Some` usa el provider compartido de esa identidad, no lee las env vars y vacía
`share_email` y la carpeta por defecto de la plataforma. Su 403 es `DocsError::ConnectedAccountPermissionDenied` y
un `invalid_grant` es `DocsError::GoogleAccountReconnectRequired`, con los mismos payloads que gsheets. Ningún
dispatcher le pasa `auth` todavía. Guía: [45_gdocs.md](developer_guide/45_gdocs.md).
**Commits portados.** La parte gdocs de `32a7d46c`, `d94d2d37` y `58a69afe` (`auth.rs`, `errors.rs`,
`http_client.rs`, `error_to_json`). **Tests.** Credenciales del bloque sin env (con señuelos en env), `None` = camino
env, 403 de la cuenta conectada, la carpeta de la plataforma no se usa, reconexión vs. error del operador, payloads
iguales a gsheets. **Mutación.** 403 sin mirar la cuenta; sin el brazo de reconexión; `share_email` sin vaciar.
**E2E.** No aplica hasta el cableado de `llm_call`. **ADP.** Ninguno. **Estado.** partial.

## 9. MCP: el cliente renueva un header bearer del host ante un 401 (parte 1 de `auth_refresh`)

**Qué cambia.** `RmcpHttpClient::connect_refreshing` recibe un `HeaderRefresh` (`header`, `seed`, un
`AuthTokenProvider`): conecta con `Bearer <token del proveedor>` (el sembrado si el host no da uno) y, ante un
**401** de cualquier pedido (con o sin `WWW-Authenticate`), llama `invalidate()`, pide un token, reconecta y
reintenta **una vez**. Un 401 se contesta antes de que el servidor corra la tool, por eso `tools/call` puede
reintentarse solo en ese caso; un 500 sigue sin reintento. Llamadas concurrentes renuevan una sola vez; el header
renovado reemplaza cualquier otro con el mismo nombre sin importar mayúsculas, y un pedido ya en vuelo termina en la
conexión vieja. Un 401 con cuerpo JSON-RPC y sin `WWW-Authenticate` rmcp lo devuelve como respuesta: no renueva. `Debug`
y logs no muestran el token. Nadie lo llama todavía: la entrada `auth_refresh` y el cableado llegan en la parte 2.
**Tests.** `rmcp_http_client` `auth_refresh` con un servidor MCP de prueba: 401 (las dos formas) → 1 pedido al
puerto con el sha256 del sembrado, reconexión y 1 sola ejecución de la tool, con el header sembrado en minúsculas;
dos 401 a la vez, una renovación; el host rechaza, el 401 queda y la tool no corre; 500 sin reintento ni pedido al
puerto; sin renovación el 401 sube como hoy. **Mutación.** Sin cada forma de 401, sin `invalidate`, reintento
ante cualquier error, sin reconexión, reemplazo sensible a mayúsculas, sin generación, reintento tras el rechazo:
cada una tumba un test. **E2E.** No aplica hasta la parte 2. **ADP.**
Ninguno. **Estado.** partial.

## 10. gdocs: carpeta de la cuenta conectada, 403 con el correo y reintento ante 401 (porte de `feat/google-workspace-auth`, parte 5)

**Qué cambia.** Con la cuenta conectada, `create`, `create_from_markdown` y `create_from_docx` sin carpeta dejan el
documento en la raíz de su Drive (antes `NoParentFolder`); con carpeta la respetan. El 403 de la cuenta de plataforma
lleva el `share_email` configurado en vez del contexto y el cuerpo de Google, que van al log. **Nuevo:** un 401
invalida la fuente de token y reintenta una vez (la rama no lo hacía; gdocs devolvía `AuthFailed` de inmediato).
Guía: [45_gdocs.md](developer_guide/45_gdocs.md). **Commits portados.** La parte gdocs de `d94d2d37` (carpeta),
`1fa1047b` y `91b1b48d` (`http_client.rs`). **Tests.** Carpeta: sin carpeta → sin `parents`, con carpeta → la
respeta, plataforma sin carpeta → `NoParentFolder`, plataforma usa la suya; 403 de plataforma con y sin correo; 401
con `HostRefreshTokenProvider` (un pedido, sha correcto) y segundo 401 → `AuthFailed`. **Mutación.** Sin reintento,
reintento sin `invalidate`, carpeta de plataforma con la cuenta conectada. **E2E.** No aplica hasta el cableado de
`llm_call`. **ADP.** Ninguno. **Estado.** partial.

## 11. Google OAuth: la caché de providers queda acotada y `google_workspace_auth` nunca expande `${VAR}`

**Qué cambia.** De las revisiones del porte de `feat/google-workspace-auth`. `OAuthProviderCache` (la usan
`http_request.auth` y `google_workspace_auth`) guarda hasta `MAX_CACHED_PROVIDERS` = 1024. Al llegar al tope, un
alta descarta los providers que nadie más tiene; uno en uso nunca se descarta. Antes no desalojaba nunca.
`GoogleWorkspaceAuth::provider()` documenta y prueba que usa los valores literales: un `${VAR}` no se reemplaza
por la env del motor, así un grafo no puede actuar con la cuenta de plataforma.
Guía: [47_google_oauth.md](developer_guide/47_google_oauth.md).
**Tests.** Al tope se barren los que nadie tiene y se conserva el que está en uso; `${VAR}` llega literal al
endpoint de token. **Mutación.** Sin barrido; barrer también los que están en uso. **E2E.** No aplica.
**ADP.** Ninguno. **Estado.** done.

## 12. gsheets: las tools sintéticas reciben `google_workspace_auth` (porte de `feat/google-workspace-auth`, parte 7)

**Qué cambia.** Cada dispatcher de gsheets (`gsheets_*`, `gsheets_run_python`, la fuente Sheets de
`data_run_python`) recibe `auth: Option<&GoogleWorkspaceAuth>` y arma su cliente por llamada con
`build_client(auth)`, así una cuenta conectada nunca reusa un cliente de otra identidad ni de la plataforma.
`DagToolExecutor` suma `with_google_workspace_auth` / `google_workspace_auth()` y lo pasa a cada dispatcher.
`for_each` pasa `None` por ahora. Ningún `llm_call` llama a `with_google_workspace_auth` todavía: todo sigue
con la cuenta de plataforma. Guía: [47_google_oauth.md](developer_guide/47_google_oauth.md).
**Commits portados.** La parte gsheets de `db76489c` (`gsheets_tools.rs`, `gsheets_run_python.rs`,
`data_run_python.rs`, el campo del executor). De la revisión de #470: el test de `${VAR}` limpia la env var
aunque falle, y `provider_cache.rs` dice que con 1024+ providers en uso cada alta recorre el mapa. **Tests.** El dispatcher actúa con el bloque (señuelos en env)
y sin él toma el camino env; el builder del executor. **Mutación.** `build_client` ignorando `auth`.
**E2E.** No aplica hasta el cableado de `llm_call`. **ADP.** Ninguno. **Estado.** partial.

## 13. gsheets: tests de cableado de `google_workspace_auth` en el executor (porte de `feat/google-workspace-auth`, parte 8)

**Qué cambia.** Solo tests (de `db76489c` y `9051dbf7`, parte gsheets): con el bloque en el executor, **cada** tool
`gsheets_*` (la lista sale del catálogo, así una tool nueva sin cablear rompe el test), `gsheets_run_python` tras
una vista previa exitosa y la fuente Sheets de `data_run_python` actúan con esas credenciales; sin él, toman el
camino env. **Mutación.** Una tool que pasa `None`; la corrida tras la vista previa sin `auth`; `data_run_python`
sin `auth`. **E2E.** No aplica. **ADP.** Ninguno. **Estado.** partial.

## 14. MCP: entrada `auth_refresh` — el header bearer del host se renueva por el puerto (parte 2 de `auth_refresh`)

**Qué cambia.** Una entrada `mcp` acepta `auth_refresh: {header, scheme: "Bearer", handle, expires_at}`. La carga
exige que `header` sea un nombre válido presente en `headers` (sin importar mayúsculas), `scheme` `Bearer` y un
`handle` no vacío; los errores no repiten valores y `Debug` tacha el handle. Con `EngineConfig.host_token_port`
(que ahora también llega a `llm_call`), `bind_with` arma por ejecución un `HostRefreshTokenProvider` con el handle,
el token sembrado (el header resuelto sin `Bearer `) y la sesión de `__colmena_agent_session_id`, y conecta con
`connect_refreshing` (§9). La clave del pool usa la huella del handle y la sesión en vez del token: renovar no parte
el pool y otra sesión no reusa el proveedor; las corridas sin sesión de agente comparten una conexión por handle
(misma credencial). El valor del header debe empezar exactamente con `Bearer ` (sensible a mayúsculas); si no, error
fijo. `auth_refresh` vale solo si el autor escribió `tool_configurations`
(`mcp_specs_for`); sin puerto se ignora y todo queda como hoy. Referencia: `mcp.auth_refresh` en
[node_as_tools_reference.json](node_as_tools_reference.json); evento `mcp.auth_refresh_failed` en
[52_mcp_observability.md](developer_guide/52_mcp_observability.md).
**Tests.** Validación (header ausente, scheme, nombre inválido; sin valores en el error); `bind` (mismo handle con
otro token → misma clave; otro handle u otra sesión → otra; el proveedor pide al host con el handle y la sesión);
`expose` (`auth_refresh` de inputs no del autor se descarta); `llm_call` con `tool_configurations` que llegan por
inputs no del autor y un puerto instalado: el puerto nunca se llama (la copia del autor sí lo llama); `registry` (el puerto llega a `llm_call`).
**Mutación.** Sesión fuera de la clave, clave por token, proveedor sin sesión, `auth_refresh` del modelo aceptado,
header sin chequear, scheme sin chequear, puerto sin llegar a `llm_call`, `tools_authored` invertido o fijo en
`true`: cada una tumba un test. **E2E.** Con
puerto, espera al worker de ADP. **ADP.** Emitir `auth_refresh` en la entrada MCP solo con la compuerta abierta;
cortar el tag A después de esta PR. **Estado.** partial.

## 15. `llm_call`: el registro de adjuntos y la caché de archivos usan el pool compartido del motor

**Qué cambia.** Cada ejecución de `llm_call` con `__colmena_agent_session_id` y `DATABASE_URL` armaba un
`PgPoolRegistry::new(PoolConfig::defaults())` propio para `PostgresAttachmentRegistry`, y otro para
`PostgresFileCache` cuando el nodo trae archivos: un pool nuevo por ejecución, que además ignoraba `COLMENA_POOL_*`.
Ahora los dos usan el registro compartido del motor (`ConversationRepositoryFactory::pool_registry()`, el mismo que
el motor arma con `PoolConfig::from_env()`): un pool por URL normalizada, cacheado, que respeta `COLMENA_POOL_*`.
Medido en ADP dev el 2026-10-02: el worker logueó `pool_created pinned=false` 53 veces en 20 minutos para la misma
URL; con la base al tope de conexiones, las corridas caían con `attachment registry init: … pool timed out while
waiting for an open connection`. Los mensajes de error no cambian; SQLite no se toca.
**Tests.** `repository_factory`: la fábrica y sus clones devuelven el mismo registro (`Arc::ptr_eq`). Los 664 tests
de `dag_engine::infrastructure::nodes::llm` pasan sin cambios. **Mutación.** Volver a `PgPoolRegistry::new` en
cualquiera de los dos sitios no tumba un test unitario (necesita Postgres real; los tests de Postgres son
`#[ignore]`). **E2E.** No aplica.
**ADP.** Subir el pin. El pool compartido para `DATABASE_URL` toma su tamaño de `COLMENA_POOL_MAX_CONN_PER_URL`
(default 2) y el worker corre 4 runs por instancia: fijarlo en el deploy del worker (Startti/adp#1057 lo pone en 8
en develop, con tope de 5 instancias). **Estado.** done.

## 16. SSE: `node-start` y `subgraph-node-start` ya no llevan credenciales en `config` ni en `inputs`

**Qué cambia.** Esos frames repetían el `config` entero del nodo (y sus `inputs` limpios) y viajan al embebedor,
que puede guardarlos horas en su store de eventos; el motor solo enmascaraba los secure values descifrados. Ahora
`SseMapper` pasa los dos por `frame_redaction::redact_secrets`: a cualquier profundidad, el valor de una clave de
`SECRET_KEYS` pasa a `"[redacted]"`, también dentro de `headers` y de un bloque entero. Las claves se comparan
normalizadas (minúsculas, `-` como `_`: `X-Goog-Api-Key` = `x_goog_api_key`); la lista incluye `authorization`,
`bearer_token`, `bearer_refresh`, `auth_refresh`, `auth`, `google_workspace_auth`, `api_key`, `x_api_key`,
`x_goog_api_key`, `ocp_apim_subscription_key`, `private_token`, `x_auth_token`, `client_secret`, `refresh_token`,
`access_token`, `session_token`, `secret_key`, `aws_secret_access_key`, `private_key`, `credentials`, `password`,
`token`, `connection_url`, `cookie`, …. En una URL `http(s)://` se tacha solo el valor de un parámetro de query con
un nombre de esa lista o de `SECRET_QUERY_PARAMS` (`key`, `sig`, `signature`, `x_amz_signature`, `code`, …), que
solo vale en URLs: un campo de config llamado `key` queda. El resto
(`node_label`, `provider`, `model`, urls, headers no secretos) queda igual. ADP ya borra `config` antes de mostrar
nada: esto es endurecimiento en tránsito y en reposo.
**Tests.** `frame_redaction` (anidado en objeto y en array, header en mayúsculas, header con guiones
`X-Goog-Api-Key`, `?key=` y una firma `X-Amz-Signature` en una URL, un campo `key` que queda, lo no secreto intacto); `sse_mapper` (un `node-start` de `http_request` con `bearer_token`, `headers.authorization`,
`bearer_refresh` y una entrada MCP con `auth_refresh`, y su forma `subgraph-node-start`, no llevan ningún valor
secreto y conservan lo demás). **Mutación.** `config` sin tachar en cada frame, `inputs` sin tachar, comparación
sensible a mayúsculas, guiones sin normalizar, sin recorrer arrays, query de URL intacta, `bearer_refresh` fuera
de la lista: cada una tumba un test. **E2E.** No aplica. **ADP.** Ninguno. **Estado.** done.

## 17. gdocs: cliente y caché de outline por identidad (porte de `feat/google-workspace-auth`, parte 9)

**Qué cambia.** Los dispatchers de gdocs reciben `auth: Option<&GoogleWorkspaceAuth>` del executor. La cuenta de
plataforma conserva su cliente y su `OutlineCache` de proceso; una cuenta conectada arma su cliente por llamada y
usa su propia `OutlineCache` (mapa por huella, acotado a 256, barre las que nadie usa). Con la cuenta conectada,
`gdocs_create*` no usa la carpeta de la plataforma. **Desvío deliberado de `db76489c`:** la rama guardaba un
cliente por identidad sin desalojo; cada cliente retenía su provider y la caché acotada de providers (#470) no
habría podido soltarlo nunca. Ningún `llm_call` pasa `auth` todavía. Guía: [45_gdocs.md](developer_guide/45_gdocs.md).
**Tests.** Caché por identidad; cliente por llamada; carpeta de plataforma nunca con la cuenta conectada; tope de
cachés. **Mutación.** La cuenta conectada usa el singleton; la caché ignora la identidad; carpeta de plataforma con
la cuenta conectada; sin barrido. **E2E.** No aplica. **ADP.** Ninguno. **Estado.** partial.

## 18. gdocs: tests de cableado de `google_workspace_auth` en el executor y estado de co-edición por identidad (porte de `feat/google-workspace-auth`, parte 10)

**Qué cambia.** Solo tests (de `db76489c` y `9051dbf7`, parte gdocs): con el bloque en el executor, **cada** tool
`gdocs_*` (la lista sale de `build_all_gdocs_tools`) llega al endpoint de token del bloque y ninguna cae a env; sin
él, ninguna llega a ese endpoint. Un `RevisionStore` en memoria (solo en tests) deja que las tools de edición
lleguen al cliente sin base de datos. **Mutación.** Una tool que pasa `None`. **E2E.** No aplica. **ADP.** Ninguno.
**Estado.** partial.
**Además (revisión de #477).** El estado del co-edit guard (`gdocs_session_state` y la caché de outline) se guarda
con una clave por identidad: la cuenta de plataforma conserva el `agent_session_id` de siempre (las filas
existentes siguen valiendo) y una cuenta conectada usa `<session>#gws:<huella>`. Así la cuenta B nunca compara
contra un snapshot que guardó la cuenta A en la misma sesión. Sin migración (`agent_session_id` es `TEXT`).
**Test.** `session_scope_separates_identities_without_secrets`. **Mutación.** La clave ignora la identidad.

## 19. `llm_call`: `google_workspace_auth` en el config — las tools de Google actúan como la cuenta conectada (porte de `feat/google-workspace-auth`, B7)

**Qué cambia.** `llm_call` lee `config.google_workspace_auth` con `GoogleWorkspaceAuth::from_node_config`, solo
del `config` (nunca de `inputs`) y antes de cualquier llamada al proveedor. Inválido → el nodo falla con un error
que nombra el bloque, sin valores. Válido → `DagToolExecutor::with_google_workspace_auth`, y los `gsheets_*` /
`gdocs_*` actúan con esa cuenta. Ausente → la cuenta de plataforma, como antes. Los valores van tal cual: un
`${VAR}` no se expande. Prelude: `build_google_workspace_prelude_for(UserConnection | Platform)`; el de plataforma
queda byte a byte igual y el de la cuenta conectada no pide compartir nada. Campo `google_workspace_auth` (object)
en el `config_schema` y en `node_configurations.json`. Guía: [47_google_oauth.md](developer_guide/47_google_oauth.md).
**Tests.** Bloque inválido falla sin llamar al LLM; ausente → prelude de plataforma; válido → prelude de la cuenta
conectada; la tool refresca contra el `token_url` del bloque; el mismo bloque llegado por `inputs` no se usa (sin
refresh, prelude de plataforma); `${COLMENA_GOOGLE_OAUTH_CLIENT_SECRET}` llega literal al endpoint con la env var
puesta; el linter de grafos acepta el campo. **Mutación.** Leer el bloque también de `inputs` y expandir `${VAR}`
tumban un test cada una; sacar el campo de `node_configurations.json` tumba el del linter. **E2E.** No aplica.
**ADP.** Puede inyectar el bloque en el `config` del `llm_call` al correr. **Estado.** done.

## 20. `for_each`: lee y escribe hojas con su `google_workspace_auth` (porte de `feat/google-workspace-auth`, B8a)

**Qué cambia.** `for_each` lee `config.google_workspace_auth` (solo del `config`, nunca de `inputs` ni de las
filas). Inválido → el nodo falla antes de leer o escribir una hoja, sin valores en el error. Toda llamada a
Sheets (`items_from: sheet`, y en `results_to: sheet` la creación, el encabezado y las escrituras `incremental`
y `final`) pasa por un `SheetsAccess` con esas credenciales; ausente → la cuenta de plataforma, como antes.
Campo `google_workspace_auth` en el `config_schema` y en `node_configurations.json`. Guía:
[49_for_each.md](developer_guide/49_for_each.md). **Tests.** Lectura y creación del sink con el bloque; sin
bloque, el entorno; bloque inválido falla antes de tocar hojas; las tres operaciones de `SheetsAccess` llevan
las credenciales; el bloque llegado por `inputs` no se usa; el linter acepta el campo en `for_each`.
**Mutación.** Leer el bloque también de `inputs` tumba un test; sacar el campo del catálogo tumba el del
linter. **E2E.** No aplica. **ADP.** Ninguno todavía. **Estado.** done.

## 21. `for_each` como tool actúa con el `google_workspace_auth` del `llm_call` (porte de `feat/google-workspace-auth`, B8b)

**Qué cambia.** Cuando un `llm_call` con `google_workspace_auth` despacha `for_each` como tool, el executor le
pone el bloque en su `config` (`GoogleWorkspaceAuth::to_config_block`, inverso de `from_node_config`), así lee
y escribe sus hojas con la cuenta conectada. Ningún otro tipo de nodo lo recibe: un `llm_call` hijo sigue con
`config` vacío y no hereda el bloque del padre. Guía: [47_google_oauth.md](developer_guide/47_google_oauth.md).
**Tests.** `for_each_tool_uses_the_llm_call_credentials` (sin bloque, el entorno; con bloque, el refresh del
bloque); `only_for_each_receives_the_credentials_in_its_config` (un `http_request` y un `llm_call` hijo no lo
reciben); `to_config_block` ida y vuelta. **Mutación.** Darle el bloque también a `llm_call` y no dárselo a
`for_each` tumban un test cada una. **E2E.** No aplica. **ADP.** Ninguno. **Estado.** done.

## 22. gsheets/gdocs: textos de tools y skills que no asumen la cuenta de plataforma (porte de `feat/google-workspace-auth`, B8c)

**Qué cambia.** Los `summary`/`description` de `gsheets_*` y `gdocs_*` (`text/tools/*.yaml`) hablan de «the
Google account these tools act as»: sin `agents@startti.co`, sin env vars del operador, sin «AGENT account» ni
«prefer sharing». Lo que dependía de la cuenta pasa al prelude de plataforma (las dos variantes): qué ven las
tools de descubrimiento, que un archivo creado no lo ve el usuario hasta compartirlo, y qué hacer con
`no_parent_folder_configured`. La skill `gsheets-editing` (`create-and-populate.md`) remite a esas notas en vez
de pedir que el usuario comparta. Guías: [47_google_oauth.md](developer_guide/47_google_oauth.md),
[39_gsheets.md](developer_guide/39_gsheets.md). **Tests.** `google_workspace_tool_texts_do_not_assume_the_platform_account`
(lista de términos prohibidos, incluidos «service account», «share email», «ask the operator»);
`google_workspace_skills_do_not_assume_the_platform_account` (cinco skills de gsheets/gdocs);
`platform_prelude_carries_the_account_guidance_moved_out_of_tool_descriptions`. **Mutación.** Volver a poner
«prefer sharing» en un summary o en la skill tumba el test correspondiente. **E2E.** No aplica. **ADP.**
Ninguno. **Estado.** done.

## 23. Google OAuth: `google_workspace_auth` de tipo `host_refresh_bearer` (CX7, parte 1: el bloque y su provider)

**Qué cambia.** `google_workspace_auth` acepta `{type: "host_refresh_bearer", access_token, expires_at, handle,
account_key}`: ADP siembra un access token de la cuenta conectada y el motor lo renueva pidiéndolo al host por
el `HostTokenPort`. `GoogleWorkspaceAuth` pasa a enum (`RefreshToken {…}` | `HostRefresh {…}`); el bloque mal
formado lista los campos que faltan. `provider()` devuelve `Result` y sin puerto falla (`*_not_configured`),
nunca la cuenta de plataforma. `identity_key()` = `sha256("host:" + account_key)` (cachés de gdocs y
`session_scope`): `account_key` es obligatorio, 1 a 128 caracteres, no secreto y estable (el handle se re-emite
en cada turno). `to_config_block` reescribe las dos variantes; el `Debug` oculta token y handle. Ningún nodo ata
todavía el puerto (parte 2). Guía: [47_google_oauth.md](developer_guide/47_google_oauth.md). **Tests.** Parseo,
`Debug`, campos faltantes, sin puerto → error; dos handles con el mismo `account_key` dan la misma identidad y
el mismo `session_scope`, otra clave, otros; `to_config_block` ida y vuelta. **Mutación.** Caer en la cuenta de
plataforma sin puerto e imprimir el handle en el `Debug` tumban un test cada una. **E2E.** No aplica. **ADP.**
Ninguno todavía. **Estado.** done (la parte 2 es §24).

## 24. Google OAuth: `llm_call` y `for_each` renuevan `host_refresh_bearer` por el puerto del host (CX7, parte 2)

**Qué cambia.** `llm_call` y `for_each` (como nodo y como tool) atan al bloque `host_refresh_bearer` de §23 el
`HostTokenPort` del motor y el `__colmena_agent_session_id` de la corrida (`with_host_context`, nuevo aquí), nunca un
valor del bloque. `for_each` como tool recibe el bloque que reescribe `to_config_block` y le ata su propio contexto;
`set_host_token_port` llega también a `for_each`. Si el host rechaza la renovación (`HostRefused`), gsheets y
gdocs dan `google_account_reconnect_required` con `RECONNECT_GOOGLE_MESSAGE` para la cuenta conectada (para la
de plataforma sigue `*_not_configured`). Guías: [47_google_oauth.md](developer_guide/47_google_oauth.md),
[49_for_each.md](developer_guide/49_for_each.md); `node_configurations.json` describe el tipo. **Tests.**
gsheets sin puerto, con env de señuelo → `NotConfigured`; gsheets con wiremock: 401 con el token sembrado → un pedido al puerto → 200, y host que rechaza → reconnect;
mapeo de `HostRefused` en gdocs; `llm_call` con semilla vencida pide al puerto una vez con su sesión; `for_each`
como nodo y como tool lee la hoja con el token del host (handle y sesión); el registro le da el puerto a
`for_each`. **Mutación.** No atar el contexto en `llm.rs` o en `for_each`, no pasar la sesión, mapear
`HostRefused` a un error genérico (gsheets y gdocs), alterar el handle en `to_config_block` y no darle el puerto
a `for_each` en el registro tumban al menos un test cada una. **E2E.** No aplica. **ADP.** Puede mandar el
bloque `host_refresh_bearer` con el `HostTokenPort` conectado. **Estado.** done.

## 25. Facturación: cada llamada al proveedor se cuenta una sola vez

**Qué cambia.** Un `llm_call` con `stream: true` (el default) contaba cada llamada al proveedor dos veces en
`usage-summary`, `subgraph-usage-summary` y `finish.usage`: emitía un `LlmUsage` por la parte `Usage` de cada
llamada y al final otro con `response.usage()`, que ya es el total acumulado del loop (desde 4400f201, 2026-03-11).
`node-end` → `extra_info.usage` estaba bien, así que la fila de facturación era el doble del nodo (medido: 17542
contra 8771 en un `llm_call` de una llamada). Los hosts facturan con `usage-summary`. Ahora el `llm_call` reporta
cada parte `Usage` al llegar, haga stream o no (el SSE no muestra frame para ese evento; una llamada anterior a
una cancelación o a un error se cuenta), y al final solo lo que el total tenga por encima de lo ya reportado
(`LlmUsage::beyond`, normalmente nada). `AgentService::invoke_llm` pasa al observer una sola parte `Usage` por llamada, la última (la que ya guardaba como total), al terminar el stream: un proveedor que mande usage acumulado por chunk no se cobra de más en ningún nodo. Además `critic`, `planner` y `reactor` sin `streaming` (el default, y como
los corre un `orchestrator`) no reportaban su llamada: ahora la reportan al volver la respuesta, una vez. Guías:
[17_technical_reference.md](developer_guide/17_technical_reference.md) §6,
[sse_events_reference.md](sse_events_reference.md) (`usage-summary.nodes`).
**Tests.** `tests/usage_counted_once.rs` corre el loop real con un modelo que reporta un uso distinto por llamada
y compara, por el `SseMapper`, `usage-summary`, `finish.usage` y `extra_info.usage` con la suma de lo reportado:
`llm_call` con 3 turnos de tools y con 1 llamada, con y sin stream; el mismo agente dentro de un `subgraph`
(`subgraph-usage-summary`, `usage-summary` del padre y `finish`); `critic`, `planner` y `reactor` con y sin
`streaming`. El modelo manda además un `Usage` acumulado intermedio por llamada (sin el arreglo de `invoke_llm`, `llm_call` y `critic` cobraban de más). Antes del arreglo, los tres casos con stream daban el doble y los tres nodos de revisión sin
`streaming`, cero. Unitarios de `LlmUsage::add` y `beyond`.
**E2E.** `dag_engine run` de un `llm_call` con `add` contra un servidor OpenAI falso local (adapter real, 3
llamadas): `usage-summary`, `finish.usage` y `extra_info.usage` iguales a lo que reportó el servidor (3300/33 con `stream`, 4200/42 sin).
**ADP.** Subir el pin: lo facturado desde `usage-summary` baja a la mitad en los `llm_call` con stream y suma las
llamadas internas de planner/critic/reactor de un orquestador. **Estado.** done.

## 26. `llm_call`: una respuesta vacía de Gemini en streaming dice por qué

**Qué cambia.** Medido el 2026-10-05 en ADP dev: `gemini-2.5-flash` con el toolkit de gsheets devolvió, en
streaming, una respuesta sin texto y sin llamadas a tools; el nodo terminó `done` con `result: ""` y
`completion_tokens: 0`, sin decir por qué. Sin streaming, el adaptador ya escribía
`[Empty response - finish_reason: <X>]`; en streaming el contenido quedaba vacío y el agregador del loop
(`AgentService::invoke_llm`) tiraba el `finish_reason` de los chunks, para cualquier proveedor. Ahora:
- el stream de Gemini, si no trajo texto ni llamadas, emite al final el mismo texto que `call`;
- un prompt bloqueado (`promptFeedback.blockReason`, sin `candidates`) ya no es un error de parseo en ninguno de los
  dos caminos: el texto es `[Empty response - block_reason: <X>]`;
- un `{"error": …}` dentro de un 200 (también a mitad del stream) es un error del nodo, no una respuesta vacía o
  cortada que termina `done`; un elemento sin `candidates`, `promptFeedback` ni `usageMetadata` falla en `call` y el
  stream lo salta;
- el agregador guarda `finish_reason` y `block_reason` del stream, y `llm_call` los pone en `extra_info` cuando
  existen (también sin streaming y para OpenAI y Anthropic, cuyos streams ya los traían).

Con texto o con llamadas a tools nada cambia: ni el contenido ni el despacho; `extra_info` suma la clave.
Guía: [14_llm_deep_dive.md](developer_guide/14_llm_deep_dive.md).
**Tests.** `gemini_adapter`: cada `finishReason` vacío da el mismo texto en `stream` y `call`; prompt bloqueado;
`functionCall` (también tras el chunk final) sin aviso; solo razonamiento con aviso; `error` falla; elemento solo de
metadatos saltado; texto intacto. `agent_service` conserva las razones; `llm` las nombra solo si existen.
**ADP.** Subir el pin; `extra_info.finish_reason` permite explicar una respuesta vacía. **Estado.** done.

## 28. Facturación: el razonamiento de OpenAI se cuenta una sola vez

**Qué cambia.** OpenAI cuenta `reasoning_tokens` *dentro* de `completion_tokens` (Chat Completions) y de
`output_tokens` (Responses API). El adaptador de Chat Completions ponía `completion_tokens` entero **y**
`thinking_tokens = reasoning_tokens`, así que `total_tokens` sumaba el razonamiento dos veces y un host que factura
la salida como `completion + thinking` (ADP) lo cobraba dos veces. El de Responses no leía
`output_tokens_details.reasoning_tokens`: no duplicaba, pero dejaba todo en `completion_tokens`. Ahora
`completion_tokens` y `thinking_tokens` son disjuntos en todos los proveedores, como ya lo eran las columnas de cache:
`LlmUsage::with_thinking_tokens_included` resta el razonamiento de la salida (saturando), y lo usan Chat Completions
(con y sin stream) y Responses (con y sin stream, ahora por un solo `responses_usage_to_llm_usage`). Gemini ya era
disjunto (`candidatesTokenCount` excluye los thoughts). Anthropic no reporta thinking aparte: queda dentro de
`completion_tokens` y `thinking_tokens` es `None`. Ningún consumidor del motor cambia: `recompute_total`, el
`SseMapper` y `usage_entry` ya sumaban las columnas como disjuntas. Guía:
[sse_events_reference.md](sse_events_reference.md) (`usage-summary`).
**Tests.** `openai_adapter`: `{prompt 100, completion 300, reasoning 250}` da completion 50, thinking 250, total 400
en Chat Completions, su chunk final de stream y Responses (body y evento `response.completed`), también con cache;
sin razonamiento la salida queda entera. `llm_config`: el helper resta y satura. Rojo antes del arreglo (5 de 10) y
mutación (sin la resta: 7 fallan).
**ADP.** Subir el pin: la salida de los modelos de razonamiento de OpenAI (gpt-5, o-series) baja a lo real; la
fórmula `completion + thinking` no cambia. **Estado.** done.

## 29. Facturación: las llamadas laterales de un nodo también se cuentan

**Qué cambia.** Después de §25, cada llamada del loop de respuesta se cuenta una vez, pero un nodo también llama
al proveedor por fuera de ese loop, sin callback ni observer, y esas llamadas no llegaban a `usage-summary` ni a
`finish.usage`: el resumen con modelo barato de un mensaje viejo al compactar la historia de un `llm_call`
(`LlmMessageSummarizer`), el resumen de un adjunto sin descripción (`LlmAttachmentSummaryGenerator`) y el crítico
`guardrail_llm` del nodo `sql` (`LlmCriticAdapter`). Ahora las tres pasan por `nodes/util/billed_llm.rs`, un
`LlmRepository` que reporta el uso de cada llamada (`call`: el de la respuesta; `stream`: la última parte `Usage`,
al terminar) como un `LlmUsage` del nodo que la hizo. El repositorio del loop de respuesta no se envuelve: ya se
reporta y se cobraría dos veces. Usan otro modelo que el nodo (el barato del provider o el de `guardrail_llm`), y el
host precia cada fila por su modelo: el evento lleva un `side_call` (`purpose`, `model`, `provider`, `node_key`) y
`usage-summary` les da una fila propia, `<nodo>::<propósito>` (ver [sse_events_reference.md](sse_events_reference.md)).
Revisadas y ya contadas una vez: `llm_call` (loop), `critic`/`planner`/`reactor`, el `final_reactor` del
`orchestrator` (por su callback), `extract_with_schema` (`output_parser`, `extraction`, `router` en `llm_direct` y
`extract_and_route`) y el `router` `decision_model`. Sin contexto de facturación: el preflight de claves
(`validate_credentials`, sin tokens), el health check y `ServiceContainer` (`LlmCallUseCase`/`LlmStreamUseCase`
fuera de un run: el host recibe el uso en la respuesta). `image_generation`, `image_edit` y `tts` no pasan por
`LlmRepository` y no reportan tokens; quedan fuera.
Además, `SqliteAttachmentRegistry` leía un `description` (y `label`) `NULL` como `""`: en modo SQLite un adjunto
nunca se resumía, porque parecía ya descrito. Ahora es `None`, como en Postgres. Guía:
[17_technical_reference.md](developer_guide/17_technical_reference.md) §6.
**Tests.** `tests/usage_counted_once.rs`: un segundo turno de una conversación en SQLite compacta la respuesta
larga del primero (2 llamadas: el resumen y la respuesta) y un `llm_call` con un adjunto de texto lo resume
mientras responde (2 llamadas), también dentro de un `subgraph`: la fila del nodo es su respuesta (= `extra_info.usage`),
la de la llamada lateral su uso con `gpt-4o-mini`/`openai` y la clave del nodo, y `finish.usage` la suma; igual bajo un
`llm_call` como tool y en una fila de `for_each` (`ChildScopeObserver` no envuelve `<scope>::<propósito>`); dos adjuntos en un `subgraph`, una fila.
Antes del arreglo los dos daban solo la respuesta. Unitarios de `billed_llm` (`call` y `stream`, con un `Usage`
acumulado intermedio), del crítico SQL y de la lectura de `NULL` en SQLite. Mutaciones: sin envolver el resumen de
historia o el del adjunto, o con la lectura vieja de SQLite, falla su caso; envolver también el loop de respuesta
hace fallar los casos de §25 (cobro doble).
**ADP.** `EventTreeBuilder` no estampa las filas `<nodo>::<propósito>` (no son nodos del árbol): para cobrarlas hay
que estamparlas, mapear su clave a la del dueño y leer `guardrail_llm.api_key` (hoy BYOK). **Estado.** done.

## 30. Facturación: cada resumen cobra su propio grafo, y las filas de `for_each` llevan su modelo

**Qué cambia.** Un host cobra cada fila de `usage-summary`/`subgraph-usage-summary` por su `model`/`provider` y
descarta la que no los tiene. (1) La fila `N` de un `for_each` (`<for_each>#N`) no tiene `NodeStart` y salía con
`model`/`provider` en `null` y sin `provider_key_id`: ahora el `for_each` emite antes de cada fila un
`DagExecutionEvent::UsageIdentity` (sin frame SSE) con `node_type`, `model`, `provider` y `provider_key_id` del
target, solo si el target llama al proveedor. (2) Un `llm_call` como tool dentro de un `subgraph` y (3) las filas de
un `for_each` como tool llegaban envueltas (`SubgraphWrapped`) y no se contaban. Ahora **cada resumen cobra
exactamente lo de su grafo**, a cualquier profundidad dentro de él: `track_child_usage` (`run_use_case.rs`) cuenta
lo envuelto por un scope del mismo grafo en la fila de su `path` (`Fan>for_each#0`: dos scopes con el mismo id no
comparten fila) y deja afuera lo de una corrida anidada (`subgraph`, agente como tool): `run_subgraph` marca
`nested` cada `LlmUsage` que sale de una corrida hija, que lo cobra en su `subgraph-usage-summary`. **El
`usage-summary` raíz ya no trae los nodos de un `subgraph` hijo** (los sumaba, y si el hijo repetía un id del padre
mezclaba los tokens de los dos con un solo modelo); el modelo que dice un hijo no pisa el de una fila del padre.
`finish.usage` sigue siendo el total. Fila: `{"node_id":"fe#0","node_type":"llm_call","model":"…","provider":"…",`
`"provider_key_id":"…","prompt_tokens":…,…,"total_tokens":…}` (`provider_key_id` solo si el target lo tiene).
**Tests.** `tests/usage_counted_once.rs`: filas de `for_each`, un tool dentro de un `subgraph`, un agente como tool
cuyo nodo también es `agent` con otro modelo, dos `for_each` como tool y un hijo que repite el id del padre; toda
fila con tokens tiene `model`/`provider` y cada resumen suma lo de su grafo. Unitario de `row_usage_identity`.
Mutaciones: sin `nested`, fila por id en vez de `path`, modelo del hijo pisando al padre, sin `UsageIdentity` o sin
su clave: falla su caso. **ADP.** `<for_each>#N` y `<scope>>…` no son nodos del árbol: para cobrarlas hay que
estamparlas. **Estado.** done.
