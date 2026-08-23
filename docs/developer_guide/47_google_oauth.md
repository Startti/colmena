# 47 — Google OAuth (auth para gsheets + gdocs)

> **Cambio importante (2026-06-10):** colmena ya **no usa Service Account** para autenticar contra Google. La auth para `gsheets_*` y `gdocs_*` tools va por **OAuth user-scoped** sobre un user dedicado de Workspace (`agents@startti.co` en el deploy canónico). El refresh_token vive en Google Secret Manager y se monta como env var en el Cloud Run Job.

## TL;DR para el operador

1. **Una sola vez** (en tu laptop): corrés `colmena_oauth_setup`, hacés login como `agents@startti.co` en el browser, capturás un `refresh_token`.
2. Subís 3 secrets a Google Secret Manager: `client_id`, `client_secret`, `refresh_token`.
3. Actualizás `deploy_gcp.sh` del ADP worker para montar esos 3 secrets como env vars + setear `COLMENA_GOOGLE_SHARE_EMAIL=agents@startti.co`.
4. Deploy. El worker arranca y empieza a refrescar access_tokens automáticamente cada hora.

Después de eso, ninguna ventana de browser, ningún consent. Solo HTTP entre el worker y `oauth2.googleapis.com` cada ~1h.

## Setup paso a paso

### A. Crear el usuario `agents@startti.co` en Workspace

Ver la guía operativa que usaste durante el rollout. Resumen:
- License Workspace Business Standard.
- Password fuerte (en password manager).
- 2FA obligatorio (TOTP via Authy con cloud backup mínimo).
- NO admin role.

### B. Crear OAuth Client en GCP

1. console.cloud.google.com → `startti-dev` (o el project que use ADP worker).
2. APIs & Services → Library → habilitar Drive API + Docs API + Sheets API.
3. APIs & Services → OAuth consent screen → **Internal** type. App name "Colmena Agent". Authorized domain `startti.co`. Scopes:
   - `https://www.googleapis.com/auth/drive.file`
   - `https://www.googleapis.com/auth/documents`
   - `https://www.googleapis.com/auth/spreadsheets`
4. APIs & Services → Credentials → Create Credentials → OAuth Client ID → **Desktop app** → name "Colmena Agent OAuth Client" → descargar JSON.

### C. Obtener el refresh_token con `colmena_oauth_setup`

```bash
cargo run --release --bin colmena_oauth_setup -- \
    --client-secret ~/.colmena/oauth_client_secret.json
```

Pasos:
1. El CLI lee el JSON.
2. Abre tu browser default en la URL de consent.
3. Hacés login en el browser **como `agents@startti.co`** (NO como vos personalmente).
4. Aceptás el consent.
5. El browser redirige a `localhost:8080`.
6. El CLI captura el `code`, lo intercambia por un `refresh_token`, lo imprime en consola.

Output esperado:

```
════════════════════════════════════════════════════════════════════
  ✓ Consent successful. Refresh token (copy into Secret Manager):
════════════════════════════════════════════════════════════════════

1//0g_AbCdEfGh1234567890ZyXwVu...
```

### D. Subir las 3 credenciales a Secret Manager

```bash
PROJECT_ID=startti-dev   # ajustá

# client_id
echo -n "<el client_id que ves en el JSON>" | \
    gcloud secrets create colmena-oauth-client-id \
        --project=$PROJECT_ID --replication-policy=automatic --data-file=-

# client_secret
echo -n "<el client_secret del JSON>" | \
    gcloud secrets create colmena-oauth-client-secret \
        --project=$PROJECT_ID --replication-policy=automatic --data-file=-

# refresh_token (pegás el output del CLI, después Ctrl-D)
gcloud secrets create colmena-oauth-refresh-token \
    --project=$PROJECT_ID --replication-policy=automatic --data-file=-
```

**Inmediatamente después**: limpiá history del terminal (`history -c` en bash/zsh) y borrá el archivo local `~/.colmena/oauth_client_secret.json` si ya no lo necesitás.

### E. Otorgar acceso al runtime SA del worker

El runtime SA del Cloud Run **service** `colmena-worker` (y de `colmena-api`) está
definido en `deploy_gcp.sh` como `RUNTIME_SERVICE_ACCOUNT`. En el deploy canónico
de dev (2026-06-10) es `adp-backend-sa-develop@startti-dev.iam.gserviceaccount.com`.
Ajustá según tu env.

```bash
WORKER_SA=adp-backend-sa-develop@startti-dev.iam.gserviceaccount.com  # ajustá

for secret in colmena-oauth-client-id colmena-oauth-client-secret colmena-oauth-refresh-token; do
    gcloud secrets add-iam-policy-binding "$secret" \
        --member="serviceAccount:$WORKER_SA" \
        --role=roles/secretmanager.secretAccessor \
        --project=$PROJECT_ID
done
```

### F. Actualizar `deploy_gcp.sh` del ADP worker

Esto **ya está hecho** en ADP develop a partir del commit `09e90674` (2026-06-10).
Si vas a hacer un setup desde cero en otro project / dominio, mirá la documentación
operacional del script en
`apps/service/ia/platform/CICD.md`
(en el repo ADP, no en colmena).

Los cambios clave en el script son:

```bash
# 1) Default del share email (linea ~110 de deploy_gcp.sh):
COLMENA_GOOGLE_SHARE_EMAIL=${COLMENA_GOOGLE_SHARE_EMAIL:-"agents@startti.co"}

# 2) Bloque de secret refs (linea ~115 de deploy_gcp.sh):
COLMENA_OAUTH_SECRETS_REF="\
COLMENA_GOOGLE_OAUTH_CLIENT_ID=colmena-oauth-client-id:latest,\
COLMENA_GOOGLE_OAUTH_CLIENT_SECRET=colmena-oauth-client-secret:latest,\
COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN=colmena-oauth-refresh-token:latest"

# 3) Propagación literal vía build_env_vars() (linea ~178):
COLMENA_GOOGLE_SHARE_EMAIL agregado al for loop.

# 4) Mount en deploy de worker + api (líneas ~265 y ~285):
gcloud run deploy colmena-worker \
    ... \
    --set-secrets "$COLMENA_OAUTH_SECRETS_REF" \
    ...
gcloud run deploy colmena-api \
    ... \
    --set-secrets "$COLMENA_OAUTH_SECRETS_REF" \
    ...
```

Notar: son Cloud Run **services** (`gcloud run deploy`), NO Cloud Run Jobs
(`gcloud run jobs ...`). El único Cloud Run Job en el stack es
`attachment-gc`, que no usa gsheets/gdocs y por lo tanto NO necesita los
secrets OAuth.

### G. Smoke test post-deploy

1. Compartí un Google Doc / Sheet de prueba como **Editor** con `agents@startti.co`.
2. En dev, mandá un agent prompt que use `gsheets_read` o `gdocs_read_outline` contra ese doc.
3. Verificá:
   - El agent NO devuelve `PermissionDenied`.
   - El doc en activity log muestra "edited by agents@startti.co" (no la SA vieja).

## Auto-prompt de share email (primer turno)

Cuando un `llm_call` expone cualquier tool `gsheets_*` o `gdocs_*`, colmena auto-inyecta un bloque en el system message del LLM con instrucciones obligatorias para el primer turno:

1. Pedir el `doc_id` al usuario si no está en la conversación.
2. Decirle explícitamente que tiene que compartir el doc como Editor con el valor de `COLMENA_GOOGLE_SHARE_EMAIL` (en producción: `agents@startti.co`).

### Cómo se ve en producción

**Prompt del usuario** (sin doc ID):

> "agregale una fila a mi sheet con un producto nuevo"

**Respuesta del agente** (auto):

> Para agregar una fila a tu hoja de cálculo, necesito que me proporciones el **ID del documento**. Puedes encontrarlo en la URL de tu hoja de cálculo, que se verá algo así: `docs.google.com/spreadsheets/d/<ID>/edit`.
>
> Además, asegúrate de que el documento esté compartido como **Editor** con el correo `agents@startti.co` antes de continuar.

### Gating

El prelude se inyecta **solo cuando hay tools de Google Workspace en el catálogo del agente**. Si el agente no tiene `gsheets_*` ni `gdocs_*` enabled, no se gasta ningún token. La verificación está en `has_google_workspace_tools()`:

```rust
pub fn has_google_workspace_tools<I, S>(tool_names: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    tool_names.into_iter().any(|name| {
        let n = name.as_ref();
        n.starts_with("gsheets_") || n.starts_with("gdocs_")
    })
}
```

### Resolución del email mostrado

El email que aparece en el prelude se resuelve via `resolve_share_email()` con este orden de precedencia:

1. `COLMENA_GOOGLE_SHARE_EMAIL` env var (canónica desde 2026-06-10).
2. `COLMENA_GOOGLE_SA_EMAIL` env var (legacy — soportada para tests / local dev).
3. `client_email` field del JSON en `GOOGLE_APPLICATION_CREDENTIALS` (legacy).
4. `None` → fallback degradado: el prelude pide el doc ID y dice "compartilo con el service account configurado para este agente (pedile al operador la dirección si no la sabés)".

En el deploy de ADP esto se setea como literal en `deploy_gcp.sh`:

```bash
COLMENA_GOOGLE_SHARE_EMAIL=${COLMENA_GOOGLE_SHARE_EMAIL:-"agents@startti.co"}
```

### Por qué el wording es repetitivo

Si abrís [`google_workspace_prelude.rs`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/google_workspace_prelude.rs), vas a ver que el email aparece **dos veces** en el prelude (uno en la sección de "qué necesitás", otro en "reglas para el primer turno"). Esto es **intencional**. Sin la repetición, `gpt-4o-mini` con `temperature=0` tiende a comprimir la instrucción en un único mensaje tipo "dame el ID" y deja afuera el share. Pin del wording en tests: `prelude_with_email_repeats_email_for_mandatory_first_turn_instructions`.

## Env vars (referencia completa)

| Variable | Rol | Source |
|---|---|---|
| `COLMENA_GOOGLE_OAUTH_CLIENT_ID` | OAuth Client ID | Secret Manager |
| `COLMENA_GOOGLE_OAUTH_CLIENT_SECRET` | OAuth Client Secret | Secret Manager |
| `COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN` | Refresh token de `agents@startti.co` | Secret Manager |
| `COLMENA_GOOGLE_SHARE_EMAIL` | Email humano que el agent representa (`agents@startti.co`). Aparece en LLM prelude. | deploy_gcp.sh literal |
| `COLMENA_GSHEETS_SCOPES` (opcional) | Override de scopes de Sheets | deploy_gcp.sh |
| `COLMENA_GDOCS_SCOPES` (opcional) | Override de scopes de Docs | deploy_gcp.sh |

### Env vars legacy (deprecated)

| Variable | Estado |
|---|---|
| `GOOGLE_APPLICATION_CREDENTIALS` | **No leída en producción.** Solo persiste para tests locales que opcionalmente la usan; quitala de deploys productivos. |
| `COLMENA_GOOGLE_SA_EMAIL` | **Deprecated alias.** Si `COLMENA_GOOGLE_SHARE_EMAIL` no está set, el prelude cae a este como segunda opción (compat). Recomendado: solo `COLMENA_GOOGLE_SHARE_EMAIL` en deploys nuevos. |

## Error reference

Los 4 errores estructurados que produce el subsystem y qué hacer con cada uno:

### `OAuthError::RefreshTokenRevoked`

**Mensaje:** "OAuth refresh token revoked (Google returned invalid_grant). Re-run colmena_oauth_setup..."

**Causa típica:**
- `agents@startti.co` revocó manualmente el consent en https://myaccount.google.com/permissions.
- El refresh_token fue rotado por Google y la versión vieja eventualmente expiró.
- El refresh_token en Secret Manager está corrupto.

**Recovery (runbook):**
1. Volvé a correr `colmena_oauth_setup` en tu laptop (login otra vez como `agents@startti.co`).
2. Subí el nuevo refresh_token como una nueva versión del secret:
   ```bash
   gcloud secrets versions add colmena-oauth-refresh-token \
       --project=startti-dev --data-file=-
   ```
   (Pegás el nuevo token, después Ctrl-D.)
3. Los workers leen `:latest` del secret pero el valor SOLO se re-resuelve
   cuando el container reinicia (Cloud Run no recarga mounts en runtime).
   Forzá un rollout sin cambiar la imagen:
   ```bash
   gcloud run services update colmena-worker --region=us-central1 \
       --project=startti-dev --update-env-vars="OAUTH_RESYNC=$(date +%s)"
   ```
   El truco del env var con timestamp obliga a Cloud Run a crear una nueva
   revisión, que monta la versión `latest` del secret. Repetí para
   `colmena-api`.

Tiempo total: ~10 min.

### `OAuthError::ClientCredsInvalid`

**Mensaje:** "OAuth client credentials invalid: <Google's description>"

**Causa típica:**
- El OAuth Client ID o Client Secret en Secret Manager está vacío o corrupto.
- El OAuth Client fue **eliminado** desde la GCP console.
- Hay un typo en el secret value (whitespace, etc.).

**Recovery:**
1. Verificá que el OAuth Client siga existiendo en console.cloud.google.com → Credentials.
2. Re-uploadeá los secrets con los valores correctos del JSON original (que tenés en `~/.colmena/oauth_client_secret.json`).
3. Si lo borraste, creá uno nuevo + nuevo consent flow.

### `OAuthError::ConfigMissing(vars)`

**Mensaje:** "OAuth credentials missing from environment. Missing: [...]"

**Causa:** Una o más env vars OAuth no están seteadas en el worker. Lista exacta en el mensaje.

**Recovery:** revisá `deploy_gcp.sh` y confirmá que los 3 `--update-secrets` están presentes. El error te dice EXACTAMENTE cuáles faltan.

### `OAuthError::Transient`

**Mensaje:** "OAuth refresh transient failure (retries exhausted): <last error>"

**Causa típica:** Network blip, timeout, 5xx desde `oauth2.googleapis.com`. El client interno ya intentó 2 retries con backoff (1s, 2s) antes de surface-ar este error.

**Recovery:**
- La siguiente API call que necesite un token automáticamente reintenta el refresh.
- Si persiste, chequeá que el worker tenga egress habilitado a `oauth2.googleapis.com`.
- Si es prolongado: incidente de Google (chequear https://www.google.com/appsstatus).

## Monitoring

Eventos estructurados que conviene alertear:

| Log event | Significado | Acción |
|---|---|---|
| `oauth.refresh_token_rotated` (WARN) | Google emitió un refresh_token nuevo en una response. Nosotros NO lo persistimos. El viejo sigue válido por un rato. | Monitor. Si seguido aparece `RefreshTokenRevoked`, re-correr consent. |
| `oauth.refresh_failed` (TBD) | Falló un refresh. | Alert si rate spike > X/min. |

## Runbook — revocación de emergencia

Si sospechás que el refresh_token se filtró:

1. Logueate como `agents@startti.co` (browser).
2. https://myaccount.google.com/permissions
3. Encontrá "Colmena Agent".
4. Click **Revoke**.
5. **Todos los access_token + refresh_token vivos se invalidan en segundos.**
6. El worker comenzará a fallar con `RefreshTokenRevoked` en su próximo refresh.
7. Seguí el runbook de "Recovery" arriba para emitir un refresh_token nuevo.

Total time to revoke + recover: ~10 min.

## Local dev

Devs que corren agents localmente contra Google necesitan las 3 env vars seteadas. Opciones:

**A. Usar el refresh_token compartido del equipo de dev.** Pedí al admin que te dé acceso al secret `colmena-oauth-refresh-token-dev` (Secret Manager); copialo a tu shell.

**B. Hacer tu propio consent flow contra tu cuenta personal de Google.** Útil para testing aislado. Corré `colmena_oauth_setup` con tu cuenta personal; los docs van a quedar atribuidos a vos (no a `agents@startti.co`).

Ejemplo `.env` local:

```bash
export COLMENA_GOOGLE_OAUTH_CLIENT_ID="..."
export COLMENA_GOOGLE_OAUTH_CLIENT_SECRET="..."
export COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN="..."
export COLMENA_GOOGLE_SHARE_EMAIL="vos@startti.co"  # o agents@startti.co
```

## Runbook — E2E local contra Google real (sin tocar prod)

Para verificar tools `gsheets_*` / `gdocs_*` localmente contra Google real,
exportá las credenciales OAuth del agente **en el env del proceso** y corré el
grafo. **Nunca** commitees valores reales: obtené los secrets de tu gestor de
secretos (Secret Manager u equivalente) e inyectalos en memoria — no a un
archivo versionado, no por `echo`, no con `set -x`.

Variables requeridas (los **valores** salen de tu gestor de secretos, NO de
este repo):

```bash
source .venv/bin/activate                 # ver "Gotcha: pandas" abajo
set -a; source .env; set +a               # GEMINI_API_KEY + DATABASE_URL
unset ANTHROPIC_BASE_URL                   # evita 404 si quedó exportada

# Identidad + credenciales OAuth del agente (valores desde tu secret manager):
export COLMENA_GOOGLE_SHARE_EMAIL=<agent-share-email>
export COLMENA_GOOGLE_OAUTH_CLIENT_ID=<...>
export COLMENA_GOOGLE_OAUTH_CLIENT_SECRET=<...>
export COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN=<...>

# Gotcha pandas: el binario release embebe el Python del sistema; el sandbox de
# gsheets_run_python necesita ver pandas con el MISMO ABI. Apuntá PYTHONPATH al
# site-packages del .venv de la versión que embebe el binario (ver gotcha abajo):
export PYTHONPATH="$PWD/.venv/lib/pythonX.Y/site-packages"

./target/release/dag_engine run tests/graphs/agents/gsheets_collision_envelope_e2e.json \
  --agent-session-id e2e_$(date +%s) --include-extra-info \
  > /tmp/colmena_e2e/<name>.sse 2>&1
```

**Reglas de seguridad (no negociables):** los valores de los secrets se
inyectan en memoria (idealmente por command-substitution desde el secret
manager, sin imprimir), viven solo en el env del proceso, y **NO se commitea
ningún valor real**. No `echo` del valor, no `set -x`, no escribir a `.env`.

**Gotcha: `ModuleNotFoundError: No module named 'pandas'`.** Si cada llamada a
`gsheets_run_python` / `crdt_doc_run_python` muere así, el `PYTHONPATH` no
apunta al `site-packages` del intérprete que embebe el binario. Confirmá la
versión con `otool -L target/release/dag_engine | grep -i python` (macOS) y usá
ese `pythonX.Y` en el path. Mismo class de bug que el pandas-en-worker
(CHANGELOG, "pandas no instalado en worker image").

**Graphs de referencia (collision envelope, QW1+QW3):**
- [`tests/graphs/agents/gsheets_collision_envelope_e2e.json`](../../tests/graphs/agents/gsheets_collision_envelope_e2e.json)
  — sheet creado por la app (prueba `last_modified` presente).
- [`tests/graphs/agents/gsheets_collision_envelope_existing_e2e.json`](../../tests/graphs/agents/gsheets_collision_envelope_existing_e2e.json)
  — sheet operator-shared (placeholder `<YOUR_SPREADSHEET_ID>`; prueba el caveat
  de scope: `last_modified` ausente).

**Caveat de scope `drive.file` (hallazgo E2E 2026-06-11).** El scope OAuth
actual (`spreadsheets` + `drive.file`) solo cubre operaciones Drive sobre
archivos que la app **creó o abrió**. Sobre un sheet que el usuario creó y
compartió con `agents@startti.co`, los métodos del **Sheets API** (R/W de
celdas) funcionan, pero `files.get` de **Drive** (e.g. `modifiedTime` que
alimenta `current_state.last_modified`) devuelve 403/404. Por eso
`last_modified` aparece en sheets creados por el agente y **no** en sheets
compartidos. Para cubrir sheets compartidos: agregar `drive.metadata.readonly`
al consent — ver BACKLOG "OAuth scope para last_modified en sheets compartidos".

## Spec design

Para el design rationale completo, ver:
[`docs/superpowers/specs/2026-06-10-oauth-user-scoped-design.md`](../superpowers/specs/2026-06-10-oauth-user-scoped-design.md)

## Background

Migración shipped 2026-06-10. Item del BACKLOG "OAuth user-scoped flow" marcado SHIPPED.
