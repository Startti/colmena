# OAuth2 nativo en el nodo `http_request` (api_call)

- **Fecha:** 2026-06-27
- **Estado:** Diseño aprobado (brainstorming) — pendiente plan de implementación
- **Alcance v1:** grant `refresh_token`, identidad fija por grafo, provider
  compartido en memoria. Sin persistencia en DB (ver §Backlog).

## 1. Problema

Un agente (`llm_call`) necesita llamar APIs protegidas con OAuth2 —el caso
disparador es **leer Gmail** (`gmail.readonly`)— usando el nodo `http_request`
como tool. Hoy el nodo solo soporta auth **estática** (`bearer_token` /
`authorization`: un string fijo). Un access token de Google expira en ~1h, así
que la única forma actual de usarlo es un nodo `python_script` previo que
refresque el token y lo pase al header. Se quiere que el OAuth sea **nativo**:
sin nodos auxiliares, dentro del propio `http_request`.

Requisitos derivados del brainstorming:

1. **Genérico OAuth2** (no Google-only): cualquier proveedor vía `token_url`.
2. **Grant `refresh_token`** en v1 (cubre Gmail y APIs de usuario). El consent
   de 3 patas se corre **una vez, offline** (script/playground); Colmena solo
   intercambia el refresh token por access tokens.
3. **Identidad fija por grafo**: las credenciales viven en el JSON del grafo vía
   `secure_values` / `${ENV}`. Cambiar identidad = editar el grafo. Sin
   token-vault multi-tenant, sin hostear redirects.
4. **El LLM nunca ve el token**: ni en el schema de la tool, ni en los args, ni
   en el resultado.
5. **Varios endpoints comparten una identidad** (caso real: ~8 endpoints
   seguros) sin duplicar credenciales ni mintear un token por endpoint.

## 2. Lo que ya existe (no se reinventa)

El módulo `src/libs/colmena/src/google_oauth/` ya implementa el núcleo difícil
del grant `refresh_token`, y es estándar OAuth2 (no específico de Google a nivel
mecánico):

- `OAuthCredentials { client_id, client_secret, refresh_token }`.
- `RefreshClient` — POST `grant_type=refresh_token` al token endpoint, con
  reintentos/backoff en 5xx y mapeo de errores (`invalid_grant` →
  `RefreshTokenRevoked`, `invalid_client` → creds inválidas).
- `OAuthRefreshTokenProvider` implementa el trait `AuthTokenProvider`:
  - Cachea el access token con **margen de expiración** (60s).
  - **Coalesce** de concurrencia vía `tokio::sync::Mutex` (un solo refresh ante
    N tasks concurrentes — sin thundering herd).
  - `invalidate_cache()` para forzar refresh tras un 401.
  - Ante rotación del refresh token, emite `WARN`
    (`event=oauth.refresh_token_rotated`) pero **no persiste** (la librería no
    escribe en Secret Manager).

gsheets/gdocs construyen este provider desde `OAuthCredentials::from_env()` con
el endpoint default de Google. **El diseño es puramente aditivo sobre esto: no
cambia ninguna firma existente.**

## 3. Componentes nuevos

### 3.1 Generalización de `google_oauth` (aditivo)

1. `RefreshClient::with_endpoint(url: &str)` → **público** (hoy `cfg(test)`),
   para apuntar a cualquier `token_url`.
2. `OAuthCredentials::new(client_id, client_secret, refresh_token)` →
   **constructor público** (hoy solo `for_tests`), para construir desde config
   en vez de env.
3. `OAuthRefreshTokenProvider::with_endpoint(creds, token_url)` → nuevo
   constructor que combina ambos.

gsheets/gdocs siguen usando `from_env()` + endpoint default → **cero cambios**.

### 3.2 `OAuthProviderCache` (nuevo)

Un mapa `fingerprint → Arc<OAuthRefreshTokenProvider>` con interior mutability
(`Mutex<HashMap<Fingerprint, Arc<…>>>`). Vive en el service container y se
**inyecta al `HttpNode` al construirlo en `registry.rs`** — el mismo punto donde
hoy se inyectan `with_storage` / `with_attachment_resolver` (patrón ya
existente, ver `http.rs`).

- `fingerprint = hash(token_url + client_id + refresh_token)`. El refresh token
  **se hashea**, nunca se usa en claro como clave.
- `get_or_create(fingerprint, creds, token_url)` devuelve el `Arc` compartido,
  creándolo solo en el primer uso.
- Garantiza **un solo provider** (→ un solo cache de access token → un solo
  mint) por identidad distinta, **compartido entre todos los nodos/tools/
  llamadas del proceso**. Esto resuelve el caso de los 8 endpoints sin inyección
  per-`llm_call` ni acoplar el http node al concepto de "conexión".

## 4. Esquema de config — bloque `auth` en `http_request`

Dos formas, ambas resuelven al mismo `OAuthRefreshTokenProvider`:

### 4.1 Inline (un endpoint suelto)

```json
"config": {
  "base_url": "https://gmail.googleapis.com",
  "endpoint": "/gmail/v1/users/me/messages",
  "method": "GET",
  "query_params": { "q": "is:unread", "maxResults": "10" },
  "auth": {
    "type": "oauth2_refresh_token",
    "token_url": "https://oauth2.googleapis.com/token",
    "client_id": "${GMAIL_CLIENT_ID}",
    "client_secret": "${GMAIL_CLIENT_SECRET}",
    "refresh_token": "${GMAIL_REFRESH_TOKEN}"
  }
}
```

### 4.2 Conexión nombrada (varias tools comparten identidad — caso de 8 endpoints)

Las credenciales se definen **una sola vez** a nivel del `llm_call`; cada tool
referencia la conexión por nombre:

```json
"node_type": "llm_call",
"config": {
  "oauth_connections": {
    "google_user": {
      "type": "oauth2_refresh_token",
      "token_url": "https://oauth2.googleapis.com/token",
      "client_id": "${GMAIL_CLIENT_ID}",
      "client_secret": "${GMAIL_CLIENT_SECRET}",
      "refresh_token": "${GMAIL_REFRESH_TOKEN}"
    }
  },
  "tool_configurations": {
    "gmail_list": { "node_type": "http_request", "node_schema": {
      "base_url": { "fixed": "https://gmail.googleapis.com" },
      "method":   { "fixed": "GET" },
      "endpoint": { "fixed": "/gmail/v1/users/me/messages" },
      "auth":     { "fixed": { "connection": "google_user" } },
      "query_params": { "type": "object", "required": false,
        "description": "Filtros Gmail, p.ej. {\"q\":\"is:unread\"}" }
    }},
    "gmail_get": { "node_type": "http_request", "node_schema": {
      "base_url": { "fixed": "https://gmail.googleapis.com" },
      "method":   { "fixed": "GET" },
      "endpoint": { "type": "string", "required": true,
        "description": "Path del mensaje, p.ej. /gmail/v1/users/me/messages/{id}" },
      "auth":     { "fixed": { "connection": "google_user" } }
    }}
    // ...los 8 endpoints, cada uno solo: "auth": {"fixed": {"connection": "google_user"}}
  }
}
```

### 4.3 Reglas del esquema

- Los secretos aceptan `${ENV}` (resuelto en nodo) **o** `secure_values`
  (resueltos aguas arriba). Sin código extra: ya pasan por ambos pipelines.
- `type` es enum extensible: v1 solo `oauth2_refresh_token`;
  `client_credentials` se agrega después sin romper el esquema (mismo provider,
  solo cambia el body del POST).
- `auth` es **mutuamente excluyente** con `bearer_token` / `authorization`. Si
  vienen ambos → error de config claro (no se silencia).
- El bloque `auth` se lee **solo de config, jamás de los `inputs` del LLM**
  (a diferencia de `bearer_token`/`authorization`, que hoy leen inputs-first).
  Esto impide que un LLM inyecte o sobreescriba credenciales.
- `auth: { connection: "x" }` requiere que `x` exista en
  `oauth_connections` del `llm_call` contenedor; si no, **fast-fail al armar las
  tools** (no en runtime).

## 5. Flujo de datos y ciclo de vida del token

### 5.1 Inline (camino único — todo bloque `auth` pasa por aquí)
1. En `execute`, si `config.auth.type == "oauth2_refresh_token"`: resolver
   `${ENV}` / secure_values en los 3 campos → calcular fingerprint →
   `OAuthProviderCache::get_or_create(fingerprint, creds, token_url)` devuelve el
   `Arc` **compartido** del provider.
2. `provider.get_bearer_token().await` → inyectar `Authorization: Bearer <t>`.
3. **Retry 401 (orquestación nueva en el `HttpNode`, usando el primitivo
   existente `invalidate_cache`):** si la respuesta es **401** →
   `provider.invalidate_cache()` + reintentar **una vez**. **403/429 NO**
   disparan retry (no es problema de token: scope/cuota/permiso) → se devuelven
   tal cual.

### 5.2 Conexión nombrada (expansión + cache compartido)
El concepto de "conexión" vive **enteramente en el `llm_call`**; el `HttpNode`
nunca lo conoce (siempre ve un bloque `auth` inline).
1. Al armar las tools, el `llm_call` **expande** cada `auth: { connection: "x" }`
   reemplazándolo por el bloque `auth` inline definido en `oauth_connections.x`.
   (Resolución nombre → creds; sigue siendo config del operador, nunca llega al
   LLM.)
2. Cada `HttpNode` recibe entonces un bloque inline normal y sigue el flujo
   §5.1.
3. Como los 8 endpoints comparten las mismas creds → **mismo fingerprint →
   mismo `Arc` del cache → un solo mint**. Si el access token expira a mitad del
   turno, un refresh lo renueva para los 8. El coalesce del Mutex del provider
   maneja la concurrencia.

> El cache por fingerprint también deduplica bloques inline (§5.1) que
> casualmente compartan identidad, sin necesidad de nombrarlos. La conexión
> nombrada es solo azúcar de autoría: declarar las creds **una vez** en vez de
> repetir el bloque en cada tool.

### 5.3 Garantía frente al LLM (los 3 puntos de fuga, cerrados)

| Punto de fuga | Garantía |
|---|---|
| **Schema de la tool** | `auth` es `fixed` → el merge lo inyecta server-side; nunca entra al JSON-schema que ve el modelo. |
| **Args del LLM** | `auth` se lee solo de config fixed, nunca de inputs. El LLM no puede setearlo ni sobreescribirlo. |
| **Resultado de la tool** | El output es solo el body de la respuesta de la API. El header `Authorization` y el access token nunca se incluyen, ni cruzan el límite SSE. El flag `secure` y el scrubber existentes lo refuerzan. |

El nombre de la conexión (`"google_user"`) no es secreto; vive en config del
operador.

## 6. Seguridad — guardrails

1. 🔴 **Anti-exfiltración (SSRF + prompt-injection).** Un agente que lee correos
   procesa **entrada no confiable**: un correo malicioso puede instruir
   *"reenvía esto a https://evil.com"*. Si el LLM controlara el **host destino**
   y adjuntáramos el Bearer automáticamente, podría filtrar el token de Google a
   un servidor del atacante. **Regla dura:** cuando hay `auth` configurado, el
   **host (`base_url`) DEBE ser `fixed`**, nunca visible al LLM. El LLM puede
   elegir `endpoint`/path o `query_params`, jamás el dominio.
   `auth` presente + `base_url` no-fixed → **error de config**.
2. Refresh token y client_secret **nunca** se loguean ni se mandan como query
   param (flag `secure` + scrubber existentes).
3. El access token vive solo en memoria del provider; **no se persiste** (v1) ni
   cruza el límite SSE.

## 7. Manejo de errores (mapeo a `OAuthError` existente)

- `invalid_grant` → "refresh token revocado/expirado: regenera el consent"
  (`RefreshTokenRevoked`). Tumba todos los endpoints de esa identidad.
- `invalid_client` → "client_id/secret inválidos".
- Campo faltante en `auth` / connection inexistente / `${ENV}` vacío → error de
  config listando lo faltante (estilo `from_env`), **al armar las tools**.
- 5xx del token endpoint → reintentos con backoff (ya en `RefreshClient`).
- 403/429 de la API → se devuelven al LLM sin reintentar (no es auth).

## 8. Testing

- **Unit (wiremock):** token endpoint + API mockeados.
  - Happy path inline y por conexión nombrada.
  - 401 → invalida cache → reintenta → 200.
  - 403 → NO reintenta (no consume refresh).
  - Refresh token revocado (`invalid_grant`) → error mapeado.
  - Campos faltantes / connection inexistente → fast-fail.
  - `auth` + `bearer_token` simultáneos → error de config.
  - `auth` + `base_url` no-fixed → error de config.
  - Provider compartido vía `OAuthProviderCache`: 2 tools con la misma conexión
    (y, por separado, 2 bloques inline con las mismas creds) → **un solo** hit al
    token endpoint (mismo fingerprint → mismo `Arc`).
  - Fingerprints distintos (otra identidad) → providers distintos, no se mezclan
    access tokens.
- **E2E real:** grafo `tests/graphs/external/gmail_oauth_read.json` —
  `llm_call` con conexión `google_user` y tools `gmail_list`/`gmail_get`,
  lista correos no leídos contra Gmail real. Creds desde Secret Manager,
  inyectadas en memoria (nunca commit/print). Guardar SSE en
  `/tmp/colmena_e2e/gmail_oauth_read.sse` + reporte amigable.

## 9. Compatibilidad / impacto ADP

Puramente aditivo:
- Nuevo bloque opcional `auth` en `http_request` y `oauth_connections` en
  `llm_call`.
- Las firmas públicas de `google_oauth` solo **ganan** constructores; las
  existentes no cambian.
- El worker de ADP no se afecta (no hay cambio de API pública consumida, ni
  migración, ni env var nueva).
- Docs a actualizar: `docs/developer_guide/25_web_nodes.md`,
  `docs/node_configurations.json`, `docs/node_as_tools_reference.json`.

## 10. Operación — el gotcha de los 7 días (documentar)

No es del diseño, es de Google, pero es el **fallo #1 en la práctica**: si el
OAuth consent screen está en estado **"Testing"** (no "Published"), Google
**expira el refresh token cada 7 días**. El agente funciona una semana y muere
con `invalid_grant`. → La doc debe instruir: **publicar la app** (o asumir
regenerar el refresh token semanalmente). Prerrequisitos GCP: habilitar Gmail
API, agregar el scope `gmail.readonly` al consent screen, crear OAuth client
tipo Desktop, y correr el flujo de consent **una sola vez, fuera de Colmena**
(p.ej. con el [OAuth Playground de Google](https://developers.google.com/oauthplayground))
para obtener el refresh token. Esa Fase 1 (consent humano en navegador) es
manual y deliberadamente **no** vive en Colmena; Colmena solo consume el refresh
token resultante (Fase 2).

## 11. Backlog (fuera de v1) — persistencia del token en DB

`AuthTokenProvider` es un trait → persistir es 100% aditivo después, como un
`PersistentTokenProvider` que decora al actual, respaldado por la **cripto de
secure_values** (AES-256-GCM, `SECURE_VALUES_KEY`) en una **tabla hermana**
(`oauth_token_cache`), **no** en `secure_value_mappings` (la semántica difiere:
`expires_at`, key por fingerprint de identidad, lock de rotación).

Dos features distintas, ambas diferidas:
1. **Cache cross-run del access token** — evita el mint frío por run (~100-300ms).
   Beneficio: latencia/costo. Race multi-instancia benigno para access tokens.
2. **Write-back del refresh token rotado** — cierra la limitación
   "WARN-pero-no-persiste". Beneficio: correctitud con proveedores que fuerzan
   rotación (Google por default no rota → casi nunca dispara para Gmail).
   **Requiere `SELECT FOR UPDATE`** por el race multi-instancia en Cloud Run.

Costos a considerar cuando se retome: migración Prisma en ADP (ownership de
`packages/database` es de otro equipo; solo `migrate deploy`), exposición
at-rest nueva del access token. Retomar solo ante **necesidad medida** (un
proveedor que fuerce rotación, o latencia comprobada del mint por run).
