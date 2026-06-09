# Propuesta: OAuth user-scoped flow para Google Docs (v1.1 item 1)

**Estado:** Propuesta — pendiente de aprobación para promover a spec.
**Fecha:** 2026-06-09
**Author:** daniel@startti.co
**Subsistema:** G (Google Docs) — extiende v1 (shipped 2026-06-08)
**Backlog ref:** `docs/BACKLOG.md` → "Subsystem G v1.1" → item 1

---

## 1. Problema

Hoy Subsystem G usa **Service Account (SA)** para autenticarse contra
Google Docs/Drive. Esto trae 3 limitaciones serias:

| Limitación | Síntoma observable |
|---|---|
| Docs creados por SA quedan owned por la SA | `storageQuotaExceeded` al crear en Drive personal — solo funciona en Workspace Shared Drives |
| Acceso requiere share explícito al email de la SA | Cada user debe compartir manualmente con `colmena-sa@…iam.gserviceaccount.com` |
| No funciona con cuentas Gmail personales | Quotas y permissions del SA, no del user |

**Goal de esta propuesta:** que el agente actúe **AS el usuario** —
docs creados quedan con owner = usuario; permisos = los del usuario;
quota = del usuario; cualquier Gmail funciona; cero shares manuales.

---

## 2. Arquitectura — boundaries

```
┌──────────────┐                    ┌──────────────┐                  ┌──────────────┐
│   Frontend   │  "Conectar Google" │  ADP backend │   user_id        │   Colmena    │
│  (ADP chat)  │ ─────────────────► │ (api/worker) │ ───────────────► │ (gdocs node) │
└──────┬───────┘                    └──────┬───────┘                  └──────┬───────┘
       │                                   │                                 │
       │  redirect a Google                │  guarda tokens                  │  lee tokens
       │                                   │  google_oauth_credentials       │  refresca si exp
       │                                   │  (encrypted en Postgres)        │  llama Google API
       ▼                                   ▼                                 ▼
   accounts.google.com               Postgres (adp_db_develop)        docs.googleapis.com
```

**Boundary clave:**
- **ADP** es dueño del baile OAuth (UI + callback HTTP + persistencia
  inicial). Cualquier cosa que requiera redirect del browser vive aquí.
- **Colmena** solo lee tokens del store por `user_id` y los usa para
  llamar Google API. No expone endpoints HTTP nuevos.
- **Postgres** es el bridge — tabla cifrada con la misma key
  (`SECURE_VALUES_KEY`) que ya usa secure_values.

---

## 3. Handshake OAuth (one-time per usuario)

### Paso 1 — Frontend dispara consent

ADP frontend (botón "Conectar Google" en settings o banner inline en
chat) redirige a:

```
https://accounts.google.com/o/oauth2/v2/auth
  ?client_id=<ADP_CLIENT_ID>.apps.googleusercontent.com
  &redirect_uri=https://api.adp.startti.ai/oauth/google/callback
  &response_type=code
  &scope=openid email
         https://www.googleapis.com/auth/documents
         https://www.googleapis.com/auth/drive.file
  &access_type=offline       ← CRÍTICO: pide refresh_token
  &prompt=consent            ← fuerza re-consent → garantiza refresh_token
  &state=<csrf_random>
```

### Paso 2 — Usuario consiente

Google redirige a:
```
https://api.adp.startti.ai/oauth/google/callback?code=4/0Aabc...&state=<csrf>
```

### Paso 3 — ADP backend canjea code por tokens

```
POST https://oauth2.googleapis.com/token
  code=<code>
  client_id=<ADP_CLIENT_ID>
  client_secret=<ADP_CLIENT_SECRET>
  redirect_uri=...
  grant_type=authorization_code
```

Respuesta:
```json
{
  "access_token": "ya29...",         // ~1h vida
  "refresh_token": "1//0g...",       // long-lived
  "expires_in": 3599,
  "scope": "...documents .../drive.file openid email",
  "id_token": "eyJ..."               // JWT con email del usuario
}
```

### Paso 4 — ADP persiste cifrado

```sql
INSERT INTO google_oauth_credentials (
  user_id, google_email,
  access_token_encrypted, refresh_token_encrypted,
  expires_at, scopes, connected_at
) VALUES (...)
ON CONFLICT (user_id) DO UPDATE SET ...;
```

Cifrado AES-256-GCM con `SECURE_VALUES_KEY` (ya disponible en Colmena
y ADP).

### Paso 5 — UI confirma

"Conectado como daniel@startti.co ✓"

---

## 4. Runtime — qué pasa en cada llamada del agente

Cuando el agente invoca, p.ej., `gdocs_create_document`:

```rust
// pseudocode en dag_tool_executor.rs
1. user_id = self.user_id;  // viene de ADP por (agent_session_id → user_id)
2. let creds = oauth_store.get(user_id).await?;
3. let creds = match creds {
       None => return ToolError {
           kind: "google_not_connected",
           message: "El usuario no ha conectado Google Docs todavía",
           action: { type: "show_connect_button", url: "/oauth/google/start" }
       },
       Some(c) if c.expires_at < now() + 60s
                  => oauth_store.refresh(user_id).await?,
       Some(c) => c,
   };
4. http_client.set_bearer(&creds.access_token);
5. dispatch_create_document(&http_client, args).await
```

### Refresh detalle

```
POST https://oauth2.googleapis.com/token
  refresh_token=<refresh_token>
  client_id=<ADP_CLIENT_ID>
  client_secret=<ADP_CLIENT_SECRET>
  grant_type=refresh_token
```

- Si Google rota el `refresh_token` → guardar el nuevo
- Si responde `invalid_grant` → user revocó → marcar `revoked_at`,
  devolver `GoogleAccessRevoked` accionable al LLM
- Race condition (2 agentes concurrent refrescan): lock via
  `SELECT ... FOR UPDATE` o re-check `expires_at` post-update

---

## 5. Schema Postgres (nuevo, en ADP)

```sql
CREATE TABLE google_oauth_credentials (
  user_id                  TEXT        NOT NULL,
  google_email             TEXT        NOT NULL,
  access_token_encrypted   BYTEA       NOT NULL,
  refresh_token_encrypted  BYTEA       NOT NULL,
  expires_at               TIMESTAMPTZ NOT NULL,
  scopes                   TEXT[]      NOT NULL,
  connected_at             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  last_refresh_at          TIMESTAMPTZ,
  revoked_at               TIMESTAMPTZ,                   -- null = vigente
  PRIMARY KEY (user_id)
);
CREATE INDEX ON google_oauth_credentials (google_email);
```

Misma convención que `gdocs_session_state`: SQL idempotente para
local + entrada en Prisma schema del lado de ADP.

---

## 6. Cambios en Colmena

### 6.1 Nuevo port en `gdocs/domain/traits.rs`

```rust
#[async_trait]
pub trait OAuthTokenStore: Send + Sync {
    async fn get(&self, user_id: &str) -> Result<Option<OAuthCredentials>, DocsError>;
    async fn put(&self, user_id: &str, creds: &OAuthCredentials) -> Result<(), DocsError>;
    async fn mark_revoked(&self, user_id: &str) -> Result<(), DocsError>;
}

pub struct OAuthCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub scopes: Vec<String>,
    pub google_email: String,
}
```

### 6.2 Nuevo adapter en `gdocs/infrastructure/oauth_store.rs`

- `PostgresOAuthStore` (cifrado vía `SECURE_VALUES_KEY` existente)
- `InMemoryOAuthStore` para tests (mockall-friendly)

### 6.3 Refactor de `gdocs/infrastructure/auth.rs`

Hoy `TokenCache` solo tiene un modo (SA via
`yup-oauth2::ServiceAccountAuthenticator`). Evolución:

```rust
pub enum AuthMode {
    ServiceAccount { path: PathBuf },
    UserOAuth { store: Arc<dyn OAuthTokenStore>, user_id: String },
    Hybrid {
        sa: ServiceAccountAuth,
        oauth_store: Arc<dyn OAuthTokenStore>,
        user_id: String,
    },
}
```

`TokenCache::token()` ramifica:
- `ServiceAccount` → flujo actual sin cambios (backward-compat)
- `UserOAuth` → lee del store; si expira refresca
- `Hybrid` → probar OAuth primero para `create_*`; SA para reads de
  shared drives (ver §8)

### 6.4 Config nuevo en `tool_configurations.gdocs`

```json
{
  "node_type": "gdocs",
  "auth_mode": "user_oauth",         // "service_account" | "user_oauth" | "hybrid"
  "fallback_to_sa": false            // solo aplica en hybrid
}
```

Default: `"service_account"` → graphs y deploys actuales NO cambian.

### 6.5 Propagar `user_id` end-to-end

ADP pasa `user_id` igual que hoy pasa `agent_session_id`. En el worker:

```rust
EngineConfig {
    agent_session_id: Some("agent_demo_001"),
    user_id: Some("user_42"),         // NUEVO
    ...
}
```

Llega a `dag_tool_executor` por la misma cadena que `agent_session_id`.

---

## 7. Errores accionables al LLM

Nuevos variants en `DocsError`:

```rust
pub enum DocsError {
    // … variants actuales …

    GoogleNotConnected {
        user_id: String,
        connect_url: String,
    },
    GoogleAccessRevoked {
        user_id: String,
        reconnect_url: String,
    },
    GoogleScopeMissing {
        needed: Vec<String>,
        have: Vec<String>,
        reconnect_url: String,
    },
}
```

Cada uno se serializa a tool result con `action: {type, url}` para que
el LLM responda al user en lenguaje natural:

> "Necesito que conectes Google Docs primero. Hacé clic aquí: [link]"

---

## 8. Modo Hybrid (recomendado para v1.1)

**Idea:** SA sigue siendo útil para reads sobre Shared Drives
corporativos (un user puede no estar conectado pero el agente igual
tiene acceso via SA). OAuth se usa siempre para `create_*` (sino
`storageQuotaExceeded`).

**Decisión por llamada:**

```rust
fn pick_auth(tool: &str, ctx: &Ctx) -> AuthMode {
    // 1. create_* → SIEMPRE OAuth (sino storageQuotaExceeded)
    if tool.starts_with("gdocs_create") {
        return AuthMode::UserOAuth { ... };
    }
    // 2. User conectado → preferir OAuth (acceso natural)
    if ctx.user_connected() {
        return AuthMode::UserOAuth { ... };
    }
    // 3. SA configurado y user no conectado → SA (acceso a shared drives)
    if ctx.sa_available() {
        return AuthMode::ServiceAccount { ... };
    }
    // 4. Sin opciones → GoogleNotConnected
    return AuthMode::Fail;
}
```

**Soft fallback:** si OAuth devuelve 404 (user no tiene acceso) y
`fallback_to_sa: true`, reintentar con SA. Útil para "hey Claude, abrí
este doc que me compartieron desde Workspace".

---

## 9. Edge cases que el diseño cubre

| Caso | Handling |
|---|---|
| Dos agentes concurrent refrescan token | DB row lock `SELECT FOR UPDATE`; segundo lee el ya-refrescado |
| User revoca en accounts.google.com | `invalid_grant` en refresh → `mark_revoked` → próxima llamada devuelve `GoogleAccessRevoked` |
| Token leak | Cifrado con `SECURE_VALUES_KEY`; rotación de key invalida tokens forzando reconnect |
| User en múltiples agent_sessions | Tokens por `user_id`, no por session → 1 conexión sirve todas las chats |
| Workspace admin fuerza re-consent cada X días | Misma ruta que revocación → reconnect button |
| User cambia de cuenta Google | `connected_at` se sobrescribe; reusar misma row con nuevo `google_email` |
| Scope insuficiente para tool nueva | `GoogleScopeMissing` → consent screen con scopes ampliados |
| 1 user, varias orgs Google | v1.1 solo soporta 1 cuenta/user; multi-cuenta queda como v1.2 |
| User borra `refresh_token` de Google manualmente | Próximo refresh `invalid_grant` → revoked → reconnect |

---

## 10. Trabajo del lado de ADP

1. **Crear OAuth Client en Google Cloud Console** (proyecto ADP):
   - Tipo: Web application
   - Authorized redirect URI: `https://api.adp.startti.ai/oauth/google/callback`
   - Authorized JS origins: `https://chat.adp.startti.ai`
   - Guardar `CLIENT_ID` y `CLIENT_SECRET` como secretos en GCP
     Secret Manager → inyectar a Cloud Run vía `--set-secrets`
2. **Endpoints en `apps/service/ia/platform/api/`**:
   - `GET /oauth/google/start` → genera CSRF state, redirige a Google
   - `GET /oauth/google/callback` → exchange code → persist en
     `google_oauth_credentials`
   - `POST /oauth/google/disconnect` → marca `revoked_at` + opcional
     revoke en Google (`POST oauth2.googleapis.com/revoke`)
3. **Frontend ADP**:
   - Botón "Conectar Google" en settings del user
   - Banner inline en chat cuando agent devuelve `GoogleNotConnected`
4. **DB migration**:
   - Agregar tabla `google_oauth_credentials` al schema Prisma
   - `prisma migrate deploy` (recordar: nunca `migrate dev`)
5. **Worker** (`apps/service/ia/platform/worker/`):
   - Pasar `user_id` a `EngineConfig` (lo tiene del JWT del request)
   - Inyectar `PostgresOAuthStore` en construcción del engine
6. **`deploy_gcp.sh`**:
   - Agregar las 2 vars nuevas: `GOOGLE_OAUTH_CLIENT_ID`,
     `GOOGLE_OAUTH_CLIENT_SECRET` (Secret Manager refs)

---

## 11. Plan de implementación sugerido (v1.1.0)

1. **Domain** — `OAuthTokenStore` trait + `OAuthCredentials` struct +
   nuevos `DocsError` variants
2. **Infra** — `PostgresOAuthStore` + `InMemoryOAuthStore` + cifrado
   con `SECURE_VALUES_KEY`
3. **Auth refactor** — `AuthMode` enum + `TokenCache` ramificado
4. **Migration SQL** — `google_oauth_credentials` table
5. **Tool config** — `auth_mode` + `fallback_to_sa` en `gdocs` schema
6. **Dispatcher** — propagar `user_id` end-to-end (desde
   `EngineConfig`)
7. **Unit tests** — refresh logic, revoked, scope mismatch, race
   conditions
8. **Integration test** (`#[ignore]`) — real OAuth contra cuenta de
   prueba
9. **ADP doc** — `ADP_OAUTH_GOOGLE_INTEGRATION.md` raíz con endpoints +
   migration + ENV vars (igual que `ADP_PRISMA_PENDING_TABLES.md`)
10. **Dev guide** — actualizar `45_gdocs.md` §Auth con los 3 modos
11. **Backward compat** — SA sigue funcionando idéntico
    (default `auth_mode: "service_account"`)

**Costo estimado:** ~3-4 días de subagent-driven development en Colmena
+ ~1-2 días ADP (endpoints + frontend + deploy script).
**Bloqueador externo:** ADP creando el OAuth Client en GCP Console
(~30 min).

---

## 12. Cosas que esta propuesta NO cubre (parking lot)

- **Multi-cuenta por user** (ej. trabajo + personal): v1.2
- **OAuth para Gmail / Calendar / otras Google APIs**: fuera de scope
- **Server-side credentials reuse entre agentes** (ej. cuenta de
  "Colmena Bot" compartida): no — cada user con su propia conexión
- **Revocation push de Google → Colmena** (webhooks): no — detectamos
  via `invalid_grant` lazy. Suficiente.
- **UI propia de OAuth (sin Google's consent screen)**: no, usamos la
  estándar — más segura y familiar al user.

---

## 13. Decisiones abiertas (necesitan input antes de promover a spec)

1. ¿`auth_mode` default global vs per-tool-configuration? Hoy propongo
   per-tool-config; alternativa: ENV var `COLMENA_GDOCS_AUTH_MODE`.
2. ¿Mostrar a la UI el email Google conectado o solo "conectado ✓"?
   Recomendado: mostrar email para que el user sepa qué cuenta uso.
3. ¿Permitir scope `drive` además de `drive.file`? Con `drive.file` el
   user solo da acceso a docs que abre con la app; con `drive` damos
   acceso a TODO su Drive. Recomendado: solo `drive.file` por privacidad.
4. ¿Qué pasa con `gdocs_session_state` (revisión post-write) cuando
   cambia el `user_id` de una sesión? Recomendado: keyed por
   `(agent_session_id, document_id)` igual que hoy — el `user_id` solo
   afecta auth, no el guard.

---

## Referencias

- Spec v1: `docs/superpowers/specs/2026-06-08-google-docs-design.md`
- Plan v1: `docs/superpowers/plans/2026-06-08-google-docs.md`
- Dev guide: `docs/developer_guide/45_gdocs.md`
- Backlog: `docs/BACKLOG.md` → "Subsystem G v1.1"
- ADP integration doc actual: `ADP_PRISMA_PENDING_TABLES.md` (root)
- Google OAuth 2.0 docs:
  https://developers.google.com/identity/protocols/oauth2/web-server
- Google Drive scopes:
  https://developers.google.com/drive/api/guides/api-specific-auth
