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

### E. Otorgar acceso al worker SA

```bash
WORKER_SA=adp-worker@$PROJECT_ID.iam.gserviceaccount.com  # ajustá

for secret in colmena-oauth-client-id colmena-oauth-client-secret colmena-oauth-refresh-token; do
    gcloud secrets add-iam-policy-binding "$secret" \
        --member="serviceAccount:$WORKER_SA" \
        --role=roles/secretmanager.secretAccessor \
        --project=$PROJECT_ID
done
```

### F. Actualizar `deploy_gcp.sh` del ADP worker

En el repo ADP (`apps/service/ia/platform/deploy_gcp.sh`), agregar el bloque de `--update-secrets` + `--update-env-vars`. Ver `ADP_PRISMA_PENDING_TABLES.md` o la documentación interna del ADP repo para el shape exacto. Resumen:

```bash
gcloud run jobs update adp-worker \
    --update-secrets=\
COLMENA_GOOGLE_OAUTH_CLIENT_ID=colmena-oauth-client-id:latest,\
COLMENA_GOOGLE_OAUTH_CLIENT_SECRET=colmena-oauth-client-secret:latest,\
COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN=colmena-oauth-refresh-token:latest \
    --update-env-vars=COLMENA_GOOGLE_SHARE_EMAIL=agents@startti.co \
    --remove-secrets=GOOGLE_APPLICATION_CREDENTIALS \
    --remove-env-vars=COLMENA_GOOGLE_SA_EMAIL \
    --region=us-central1 \
    --project=$PROJECT_ID
```

`--remove-secrets` / `--remove-env-vars` son seguros si no estaban set (idempotente).

### G. Smoke test post-deploy

1. Compartí un Google Doc / Sheet de prueba como **Editor** con `agents@startti.co`.
2. En dev, mandá un agent prompt que use `gsheets_read` o `gdocs_read_outline` contra ese doc.
3. Verificá:
   - El agent NO devuelve `PermissionDenied`.
   - El doc en activity log muestra "edited by agents@startti.co" (no la SA vieja).

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
2. Subí el nuevo refresh_token a Secret Manager:
   ```bash
   gcloud secrets versions add colmena-oauth-refresh-token --data-file=-
   ```
3. Los workers automáticamente leen `latest` en su siguiente refresh — **no necesitás redeploy**.
4. Si Cloud Run cachea el secret (raro pero posible), forzá reinicio del job:
   ```bash
   gcloud run jobs execute adp-worker --region=us-central1
   ```

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

## Spec design

Para el design rationale completo, ver:
[`docs/superpowers/specs/2026-06-10-oauth-user-scoped-design.md`](../superpowers/specs/2026-06-10-oauth-user-scoped-design.md)

## Background

Migración shipped 2026-06-10. Item del BACKLOG "OAuth user-scoped flow" marcado SHIPPED.
