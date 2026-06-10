# Google OAuth User-Scoped Auth — Design Spec

**Date:** 2026-06-10
**Status:** Draft → pending user review
**Subsystem:** auth replacement for `gsheets` + `gdocs`
**Prior art:** Subsystem E (`gsheets`, 2026-06-05) and Subsystem G
(`gdocs`, 2026-06-08) currently authenticate via Service Account JSON
+ ADC. This spec replaces that with a user-scoped OAuth flow on a
dedicated Workspace identity (`agents@startti.co`).

---

## 1. Problem statement

The current auth model has three operational and security problems:

1. **Identity leak in user-facing surfaces.** When the agent edits a
   doc, the activity log records
   `colmena-sheets-tester@startti-dev.iam.gserviceaccount.com` as the
   editor. The email reveals the GCP project ID, the tool name, and
   the env tier. Sharing dialogs show the same string.

2. **No identity continuity across SA rotation.** Today, docs are
   shared directly with the SA email. Rotating the SA breaks every
   existing share — users would have to re-share every doc.

3. **Service Account auth model is wider than needed.** The SA can
   in principle act outside the scope of "edit docs shared with the
   agent". Reducing blast radius means moving to a per-user identity
   with narrowly-scoped consent.

The proposed change: replace the SA-based auth path inside `gsheets`
and `gdocs` with an **OAuth user-scoped flow** where the agent acts as
a regular Workspace user (`agents@startti.co`) via a long-lived
`refresh_token` obtained once through a consent flow.

---

## 2. Goals & non-goals

### Goals

- Agent edits show `agents@startti.co` in every user-facing surface
  (share dialog, activity log, notifications).
- Auth credentials carried as standard OAuth2 (`client_id`,
  `client_secret`, `refresh_token`) — no JWT signing, no JSON files
  with private keys in production.
- Drop-in replacement at the `gsheets` / `gdocs` auth-module boundary:
  the rest of those subsystems (dispatcher, HTTP client request
  shaping, application-layer use cases) is unchanged.
- One CLI utility (`colmena_oauth_setup`) performs the one-time
  consent flow and prints the resulting `refresh_token`. No
  hand-rolled browser dance.
- Production storage of credentials in Google Secret Manager, mounted
  by Cloud Run as env vars. No JSON files at runtime.

### Non-goals (v1)

- **Multi-user OAuth** — every user authorising their own consent
  screen. Future work; not needed for the "one shared agent identity"
  case we ship.
- **Domain-Wide Delegation** — explicitly rejected for risk
  asymmetry (key leak ⇒ ability to impersonate every user in the
  domain).
- **Dual mode** — supporting SA and OAuth simultaneously in
  production. We are doing a **hard cutover**: the SA auth path is
  removed from the production code path. SA constructors stay
  reachable only behind `#[cfg(test)]` so existing wiremock tests
  keep compiling without rewrites.
- **Backwards-compat shim for env vars.** The new env-var contract
  is documented; deploys that still set
  `GOOGLE_APPLICATION_CREDENTIALS` (and nothing else) will fail at
  worker boot with a clear error. This is intentional — surfaces
  the migration loudly instead of silently downgrading.
- **Storage layer changes.** The refresh_token lives in Secret
  Manager; we do NOT persist it in Postgres. The library never
  writes back a rotated refresh_token (see §7.4 for the contract).

---

## 3. Auth chain end-to-end

```
[CLI: colmena_oauth_setup]               (one-time, on operator's laptop)
    1. reads client_id + client_secret from a downloaded
       client_secret.json (artefact of GCP OAuth Client creation)
    2. opens browser at accounts.google.com/o/oauth2/v2/auth
       with response_type=code, scope=drive.file+documents+spreadsheets,
       redirect_uri=http://localhost:8080
    3. operator logs in as agents@startti.co, accepts consent
    4. browser redirects to localhost:8080 with ?code=...
    5. local HTTP server captures the code, POSTs to
       oauth2.googleapis.com/token with grant_type=authorization_code
    6. receives access_token + refresh_token
    7. prints refresh_token to stdout, exits

[Operator]
    8. uploads client_id, client_secret, refresh_token to
       Google Secret Manager (3 secrets)
    9. updates deploy_gcp.sh to mount those secrets as env vars
       on the ADP worker Cloud Run Job

[ADP Worker boot]
    10. starts with env vars set:
        COLMENA_GOOGLE_OAUTH_CLIENT_ID
        COLMENA_GOOGLE_OAUTH_CLIENT_SECRET
        COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN
        COLMENA_GOOGLE_SHARE_EMAIL=agents@startti.co
    11. worker starts colmena Engine
    12. agents in DAGs may use gsheets_* / gdocs_* tools

[Per gsheets / gdocs API call]
    13. colmena auth module checks in-memory cache
    14. if access_token cached AND now < expiry - 60s
        → return cached
    15. else POST oauth2.googleapis.com/token with
        grant_type=refresh_token, client_id, client_secret, refresh_token
        → receives access_token + expires_in
    16. cache access_token with expiry = now + expires_in
    17. add Authorization: Bearer <access_token> to request
    18. call Drive/Docs/Sheets API
    19. Google returns response; activity log records
        "edited by agents@startti.co"
```

---

## 4. Component architecture

### 4.1 New crate-internal module

`src/libs/colmena/src/google_oauth/` — shared auth used by both
`gsheets` and `gdocs`. New module instead of duplicating per
subsystem.

```
src/libs/colmena/src/google_oauth/
├── mod.rs                            # re-exports
├── domain/
│   ├── mod.rs
│   ├── types.rs                      # AccessToken, ExpiresAt, RefreshTokenSecret
│   ├── errors.rs                     # OAuthError enum
│   └── traits.rs                     # AuthTokenProvider async trait
└── infrastructure/
    ├── mod.rs
    ├── config.rs                     # OAuthCredentials::from_env()
    ├── token_cache.rs                # in-memory tokio::sync::Mutex<Option<CachedToken>>
    └── refresh_client.rs             # POST to oauth2.googleapis.com/token
```

The trait `AuthTokenProvider` returns a `Bearer` access_token. It is
implemented by `OAuthRefreshTokenProvider` (production) and (for tests
only) `StaticTokenProvider` so wiremock tests can preseed a bearer.

### 4.2 New CLI binary

`src/libs/colmena/src/bin/colmena_oauth_setup.rs` — small interactive
binary, ~150 LOC. Uses `webbrowser = "1"` (very small dep) to open the
URL; uses `axum` (already in tree) for the localhost callback server.
Self-contained; not invoked from production code.

### 4.3 Modified files

| File | Change |
|---|---|
| `src/libs/colmena/src/gsheets/infrastructure/auth.rs` | Replace SA JWT path with `OAuthRefreshTokenProvider`. Keep SA constructors `#[cfg(test)]`. |
| `src/libs/colmena/src/gsheets/infrastructure/config.rs` | Drop `credentials_path` from production reads. Replace by `oauth_credentials: OAuthCredentials`. |
| `src/libs/colmena/src/gsheets/infrastructure/http_client.rs` | Drop the `client_email` extraction (~10 lines). The `sa_email` field becomes `share_email: String` populated from `COLMENA_GOOGLE_SHARE_EMAIL`. |
| `src/libs/colmena/src/gdocs/infrastructure/auth.rs` | Same as gsheets. |
| `src/libs/colmena/src/gdocs/infrastructure/config.rs` | Same as gsheets. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/google_workspace_prelude.rs` | Extend resolution chain to honour `COLMENA_GOOGLE_SHARE_EMAIL` (the group/user email) BEFORE the SA chain. Old chain stays as fallback for the degraded path. |
| `src/libs/colmena/Cargo.toml` | Add `webbrowser = "1"` for the CLI binary. |
| `src/libs/colmena/src/lib.rs` | `pub mod google_oauth;`. |
| `docs/developer_guide/<next>_google_oauth.md` | New dev-guide section. |

### 4.4 Files deleted / dead-coded

No file deletions. The SA JWT signing path lives in
`gsheets/infrastructure/auth.rs` and `gdocs/infrastructure/auth.rs`.
Those modules retain a `#[cfg(test)]` constructor returning a
preseeded-token client for the existing wiremock unit tests. The
public `from_config` constructors switch over to `OAuthRefreshTokenProvider`.

---

## 5. Token lifecycle & caching

### 5.1 Cache shape

```rust
struct CachedToken {
    access_token: String,
    expires_at: chrono::DateTime<Utc>,
}

struct OAuthRefreshTokenProvider {
    creds: OAuthCredentials,           // client_id + client_secret + refresh_token
    http: reqwest::Client,              // shared with the rest of gsheets/gdocs HTTP
    cache: tokio::sync::Mutex<Option<CachedToken>>,
}
```

Cache scope: **per process**. Each ADP worker instance has its own
cache. Cloud Run typically runs 1 worker per container, so this is fine.

### 5.2 Refresh policy

- On every `get_bearer_token()` call: check cache.
- If `cache.is_some()` and `now() < expires_at - 60s` → return cached.
  - The 60s margin avoids "valid at check, expired at HTTP send" races.
- Else: acquire the mutex lock, double-check (defence against
  thundering herd from concurrent requests), refresh, store, return.

### 5.3 Refresh request

```http
POST https://oauth2.googleapis.com/token
Content-Type: application/x-www-form-urlencoded

grant_type=refresh_token
&refresh_token={refresh_token}
&client_id={client_id}
&client_secret={client_secret}
```

Response (200):

```json
{
  "access_token": "ya29.a0AfH6SMBs...",
  "expires_in": 3599,
  "scope": "https://www.googleapis.com/auth/drive.file ...",
  "token_type": "Bearer"
}
```

Non-200 handling:
- 400 `invalid_grant` → `OAuthError::RefreshTokenRevoked`. This means
  the refresh_token has been revoked or never was valid. The error
  bubbles to the dispatcher, which surfaces it as a tool error
  telling the agent the operator must re-run consent.
- 400 `invalid_client` → `OAuthError::ClientCredsInvalid`. Misconfigured
  secret. Same treatment.
- 5xx / network → `OAuthError::Transient`. Retry with backoff (1s/2s,
  max 2 attempts). After retries: bubble as transient.

### 5.4 Rotation contract

If the refresh response includes a NEW `refresh_token` field (Google
occasionally rotates), the library will:
- **Log a `WARN` line** with a structured event (`oauth.refresh_token_rotated`).
- **Continue using the OLD refresh_token** for subsequent calls (the old
  one stays valid for some time; Google's docs don't specify how long).
- **NOT persist the new token back to Secret Manager.** The library has
  no Secret Manager write access by contract; mutating storage out of
  scope.

Operational consequence: if Google ever fully rotates and invalidates
the old token, the next refresh after invalidation will fail with
`RefreshTokenRevoked` and the operator must re-run consent. Monitoring
for the `oauth.refresh_token_rotated` log event (alert) gives early
warning.

This is the intentional simplification of the spec. A future v2 could
push rotated tokens back to Secret Manager via a sidecar / job; not v1.

---

## 6. Config surface

### 6.1 Required env vars (production)

| Env var | Description | Source |
|---|---|---|
| `COLMENA_GOOGLE_OAUTH_CLIENT_ID` | OAuth Client ID from GCP console | Secret Manager: `colmena-oauth-client-id` |
| `COLMENA_GOOGLE_OAUTH_CLIENT_SECRET` | OAuth Client Secret | Secret Manager: `colmena-oauth-client-secret` |
| `COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN` | The refresh_token from consent flow | Secret Manager: `colmena-oauth-refresh-token` |
| `COLMENA_GOOGLE_SHARE_EMAIL` | The Workspace user email (`agents@startti.co`) the agent acts as | deploy_gcp.sh literal |

### 6.2 Removed env vars

| Env var | Status |
|---|---|
| `GOOGLE_APPLICATION_CREDENTIALS` | **REMOVED** from production. Workers that have it set but lack the OAuth vars will fail at boot with a clear error. |
| `COLMENA_GOOGLE_SA_EMAIL` | **REMOVED** from production. `COLMENA_GOOGLE_SHARE_EMAIL` replaces it. |

### 6.3 Validation behaviour

`OAuthCredentials::from_env()` returns `Err` if ANY required var is
missing or empty. The error message lists ALL missing vars, not just
the first — so operators see the full migration list in one boot
attempt.

When `gsheets` or `gdocs` is instantiated and the env validation fails,
the failure surfaces to the LlmCallNode as an init error. The DAG run
aborts cleanly with a structured message ("Google OAuth credentials
missing — see deploy_gcp.sh §google_oauth_setup").

---

## 7. Production setup steps (operator)

Documented in detail in the implementation plan. Summary:

1. **One-time** in GCP console: create OAuth Client (Desktop type),
   download `client_secret.json`.
2. **One-time** on operator's laptop: `cargo run --bin
   colmena_oauth_setup -- --client-secret <path>`. Logs in as
   `agents@startti.co` in the browser, captures refresh_token.
3. **One-time** in `gcloud`: create 3 secrets in Secret Manager,
   upload values, grant `secretAccessor` to worker SA.
4. **Per deploy** in `deploy_gcp.sh`: bind the 3 secrets to env vars
   on the Cloud Run Job, set `COLMENA_GOOGLE_SHARE_EMAIL`.
5. **Per deploy** in `deploy_gcp.sh`: remove the old
   `GOOGLE_APPLICATION_CREDENTIALS` mount + `COLMENA_GOOGLE_SA_EMAIL`
   env var.
6. **Deploy** → smoke test against a real doc.

---

## 8. Security considerations

### 8.1 Threat model

The new threat surface revolves around the refresh_token. With it,
an attacker can act as `agents@startti.co` and access every doc that
user can access (which by `drive.file` scope is "docs created by the
app or explicitly shared with `agents@startti.co`").

Compared to the SA model:
- **SA private key leak** lets attacker act as the SA → access every
  doc the SA can access (same scope of impact).
- **refresh_token leak** lets attacker act as `agents@startti.co` →
  access every doc the OAuth grant covers (same scope of impact in
  practice, but limited to the consented scope set).

Net impact: **comparable**. The refresh_token can be revoked
instantly by the user at https://myaccount.google.com/permissions,
while the SA key requires regenerating in GCP — slightly faster
recovery.

### 8.2 Mitigations baked in

- `drive.file` scope (not `drive` full) limits blast radius even on
  full token compromise.
- Secret Manager auto-encrypts at rest + IAM-scoped access. The
  worker SA is the only IAM principal with read access.
- Worker SA itself has minimum needed roles (no Drive-direct, no
  Workspace admin).
- Access tokens are short-lived (~1 hour); attacker who intercepts
  one in transit gets 1 hour of access, not perpetual.

### 8.3 Audit & monitoring

- **Workspace audit log** records every action `agents@startti.co`
  takes on Drive. Anomaly detection (logins from new geo,
  unexpected API patterns) gives early warning of compromise.
- **GCP audit log** records every Secret Manager access. Worker SA
  reading the refresh_token at boot is the only expected pattern;
  any other access is anomalous.
- **App-level logging**: every `oauth.refresh_token_rotated`,
  `oauth.refresh_failed`, `oauth.consent_revoked` event logs
  structured to stdout (picked up by Cloud Logging). Alert rules can
  fire on `oauth.refresh_failed` rate spikes.

### 8.4 Revocation runbook

If suspicious activity detected on `agents@startti.co`:

1. Operator logs in as `agents@startti.co` (using stored password +
   2FA).
2. Goes to https://myaccount.google.com/permissions.
3. Finds "Colmena Agent" in the app list.
4. Clicks Revoke.
5. Every existing access_token + the refresh_token become invalid
   within seconds.
6. ADP worker's next API call fails with `invalid_grant`. The DAG
   tools surface a clear error.
7. Operator runs the consent flow again to issue a new refresh_token.
8. Updates Secret Manager.
9. Workers automatically pick up the new value at next refresh (no
   redeploy needed if using `:latest` secret version).

Total time to recover from a leak: ~10 minutes.

---

## 9. Migration story

This is a **hard cutover**. The user has confirmed there is no
gsheets/gdocs traffic in production today; only dev. Therefore:

1. We do NOT support dual mode (`AuthMode` enum, dispatch branching).
2. SA constructors stay in code for test compatibility, but **the
   production code path goes through OAuth exclusively** (and
   `#[cfg(not(test))]` guards make this explicit).
3. After this lands and ADP cuts over its deploy:
   - `colmena-sheets-tester@startti-dev.iam.gserviceaccount.com` can
     be deleted from GCP IAM (not blocking on us).
   - The SA JSON file can be deleted from the operator's machine.
4. Local dev: developers who run `cargo run --bin dag_engine -- run
   <graph>.json` with a graph that touches gsheets/gdocs need the
   3 OAuth env vars set (typically via `~/.env` or
   `direnv`). Documented in the dev guide.

---

## 10. Open questions

None at this time. The three decisions (scope, storage, migration)
were closed in pre-work:
- Scope: `drive.file` + `documents` + `spreadsheets`.
- Storage: Google Secret Manager.
- Migration: hard cutover.

---

## 11. Out of scope / future work

- **OAuth user-scoped flow for end-users** (not just one shared bot
  user) — each user authorises themselves; the agent acts as them.
  Useful for use cases like "edit MY personal doc". Significant
  rework; deferred.
- **Refresh token auto-rotation persistence** — pushing rotated
  tokens back to Secret Manager. Requires worker SA to have
  `secretmanager.secretVersionAdder` role plus a write path.
  Deferred to v2 if/when Google starts rotating aggressively.
- **Cleaning up the SA codepath entirely** — after a deprecation
  period, delete the `#[cfg(test)]` SA constructors and the inline
  JWT-signing helpers. Cosmetic; not v1.

---

## 12. References

- Google OAuth 2.0 for installed apps:
  https://developers.google.com/identity/protocols/oauth2/native-app
- Refresh token lifecycle:
  https://developers.google.com/identity/protocols/oauth2#expiration
- `drive.file` scope:
  https://developers.google.com/drive/api/guides/api-specific-auth
- Existing SA auth implementation: `gsheets/infrastructure/auth.rs`,
  `gdocs/infrastructure/auth.rs`.
- Existing prelude: `dag_engine/infrastructure/nodes/llm_synthetic_tools/google_workspace_prelude.rs`.
