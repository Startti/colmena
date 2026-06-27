# OAuth2 nativo en `http_request` — Fase 1 (bloque `auth` inline + cache compartido)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Que el nodo `http_request` autentique nativamente con OAuth2 (grant `refresh_token`) sin nodo `python_script`, minteando y cacheando el access token internamente, y compartiendo un solo token entre todos los endpoints de la misma identidad.

**Architecture:** Reusa el `OAuthRefreshTokenProvider` existente (cache/coalesce/401 primitive). Generaliza sus constructores para aceptar `token_url` y creds desde config. Un `OAuthProviderCache` keyed por fingerprint de credenciales (inyectado al `HttpNode` en `registry.rs`, igual que `with_storage`) garantiza un solo provider por identidad. El nodo lee un bloque `auth` **solo de `config`** (nunca de `inputs` del LLM), mintea el token, lo inyecta como `Authorization: Bearer`, y reintenta una vez en 401.

**Tech Stack:** Rust, `reqwest`, `tokio`, `sha2` (ya en Cargo.toml), `wiremock` (dev), `async-trait`.

**Alcance:** Solo el bloque `auth` **inline**. La conexión nombrada (`oauth_connections` en `llm_call`) es Fase 2 (plan aparte) — innecesaria para la funcionalidad porque el cache por fingerprint ya comparte el token entre bloques inline con las mismas creds.

**Spec:** [`docs/superpowers/specs/2026-06-27-native-oauth-http-node-design.md`](../specs/2026-06-27-native-oauth-http-node-design.md)

---

## File Structure

- **Modificar** `src/libs/colmena/src/google_oauth/infrastructure/refresh_client.rs` — `with_endpoint` público.
- **Modificar** `src/libs/colmena/src/google_oauth/infrastructure/config.rs` — `OAuthCredentials::new` público.
- **Modificar** `src/libs/colmena/src/google_oauth/infrastructure/token_provider.rs` — `OAuthRefreshTokenProvider::with_endpoint` público.
- **Crear** `src/libs/colmena/src/google_oauth/infrastructure/provider_cache.rs` — `OAuthProviderCache` (fingerprint → `Arc<provider>`).
- **Modificar** `src/libs/colmena/src/google_oauth/infrastructure/mod.rs` — exportar `OAuthProviderCache`.
- **Crear** `src/libs/colmena/src/dag_engine/infrastructure/nodes/http_oauth.rs` — parseo/validación del bloque `auth` + helper `send_with_oauth_retry`.
- **Modificar** `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs` — campo `oauth_cache`, builder `with_oauth_cache`, integración en `execute`.
- **Modificar** `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs` — declarar `http_oauth`.
- **Modificar** `src/libs/colmena/src/dag_engine/infrastructure/registry.rs:112` — inyectar el cache.
- **Crear** `tests/graphs/external/gmail_oauth_read.json` — grafo E2E.
- **Modificar** docs: `25_web_nodes.md`, `node_configurations.json`, `node_as_tools_reference.json`.

---

## Task 1: `RefreshClient::with_endpoint` público

**Files:**
- Modify: `src/libs/colmena/src/google_oauth/infrastructure/refresh_client.rs`

- [ ] **Step 1: Escribir el test que falla**

Agregar al `mod tests` de `refresh_client.rs`:

```rust
#[tokio::test]
async fn with_endpoint_targets_custom_url() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"access_token":"ya29.custom","expires_in":3600,"token_type":"Bearer"}"#,
        ))
        .mount(&server)
        .await;
    let client = RefreshClient::with_endpoint(&server.uri());
    let creds = OAuthCredentials::for_tests("cid", "csec", "rt");
    let resp = client.refresh(&creds).await.expect("refresh ok");
    assert_eq!(resp.access_token.as_str(), "ya29.custom");
}
```

- [ ] **Step 2: Correr el test para verificar que falla**

Run: `cargo test --lib with_endpoint_targets_custom_url`
Expected: FAIL con `no function or associated item named 'with_endpoint' found`.

- [ ] **Step 3: Implementación mínima**

En `refresh_client.rs`, justo después de `pub fn new()`, agregar el constructor público (igual cuerpo que el `for_tests` pero con timeouts/retries de producción):

```rust
/// Production constructor pointing at a custom OAuth2 token endpoint.
/// Used by the http_request node's native OAuth to support any provider
/// (not just Google). Same timeouts/retries as `new()`.
pub fn with_endpoint(endpoint: &str) -> Self {
    Self {
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("reqwest builder should not fail with default opts"),
        endpoint: endpoint.to_string(),
        retry_delays: PRODUCTION_RETRY_DELAYS.to_vec(),
    }
}
```

- [ ] **Step 4: Correr el test**

Run: `cargo test --lib with_endpoint_targets_custom_url`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/google_oauth/infrastructure/refresh_client.rs
git commit -m "feat(oauth): public RefreshClient::with_endpoint for custom token URLs"
```

---

## Task 2: `OAuthCredentials::new` público

**Files:**
- Modify: `src/libs/colmena/src/google_oauth/infrastructure/config.rs`

- [ ] **Step 1: Escribir el test que falla**

En el `mod tests` de `config.rs`:

```rust
#[test]
fn new_builds_credentials_directly() {
    let creds = OAuthCredentials::new("cid", "csec", "1//rt");
    assert_eq!(creds.client_id, "cid");
    assert_eq!(creds.client_secret, "csec");
    assert_eq!(creds.refresh_token.expose(), "1//rt");
}
```

- [ ] **Step 2: Correr el test para verificar que falla**

Run: `cargo test --lib new_builds_credentials_directly`
Expected: FAIL con `no function or associated item named 'new'`.

- [ ] **Step 3: Implementación mínima**

En `config.rs`, dentro de `impl OAuthCredentials`, antes del `#[cfg(test)] for_tests`, agregar:

```rust
/// Build credentials directly from config-supplied strings (used by the
/// http_request node's native OAuth, where creds come from the graph
/// config rather than env vars).
pub fn new(
    client_id: impl Into<String>,
    client_secret: impl Into<String>,
    refresh_token: impl Into<String>,
) -> Self {
    Self {
        client_id: client_id.into(),
        client_secret: client_secret.into(),
        refresh_token: RefreshTokenSecret::new(refresh_token),
    }
}
```

Y borrar el `#[cfg(test)] for_tests` reemplazando sus usos por `new` — **pero** otros tests (Task 1) usan `for_tests`. En vez de borrarlo, dejar `for_tests` delegando a `new`:

```rust
#[cfg(test)]
pub fn for_tests(client_id: &str, client_secret: &str, refresh_token: &str) -> Self {
    Self::new(client_id, client_secret, refresh_token)
}
```

- [ ] **Step 4: Correr el test**

Run: `cargo test --lib new_builds_credentials_directly`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/google_oauth/infrastructure/config.rs
git commit -m "feat(oauth): public OAuthCredentials::new for config-sourced creds"
```

---

## Task 3: `OAuthRefreshTokenProvider::with_endpoint` público

**Files:**
- Modify: `src/libs/colmena/src/google_oauth/infrastructure/token_provider.rs`

- [ ] **Step 1: Escribir el test que falla**

En el `mod tests` de `token_provider.rs`:

```rust
#[tokio::test]
async fn with_endpoint_mints_against_custom_url() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"access_token":"ya29.viaprovider","expires_in":3600,"token_type":"Bearer"}"#,
        ))
        .mount(&server)
        .await;
    let creds = OAuthCredentials::for_tests("cid", "csec", "rt");
    let provider = OAuthRefreshTokenProvider::with_endpoint(creds, &server.uri());
    let token = provider.get_bearer_token().await.expect("token ok");
    assert_eq!(token.as_str(), "ya29.viaprovider");
}
```

- [ ] **Step 2: Correr el test para verificar que falla**

Run: `cargo test --lib with_endpoint_mints_against_custom_url`
Expected: FAIL con `no function or associated item named 'with_endpoint'`.

- [ ] **Step 3: Implementación mínima**

En `token_provider.rs`, dentro de `impl OAuthRefreshTokenProvider`, después de `new`:

```rust
/// Build a provider whose refresh client targets a custom token endpoint.
/// Used by the http_request node's native OAuth (any provider via token_url).
pub fn with_endpoint(creds: OAuthCredentials, token_url: &str) -> Self {
    Self {
        creds,
        refresh_client: RefreshClient::with_endpoint(token_url),
        cache: Arc::new(Mutex::new(None)),
    }
}
```

- [ ] **Step 4: Correr el test**

Run: `cargo test --lib with_endpoint_mints_against_custom_url`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/google_oauth/infrastructure/token_provider.rs
git commit -m "feat(oauth): public OAuthRefreshTokenProvider::with_endpoint"
```

---

## Task 4: `OAuthProviderCache` (fingerprint → provider compartido)

**Files:**
- Create: `src/libs/colmena/src/google_oauth/infrastructure/provider_cache.rs`
- Modify: `src/libs/colmena/src/google_oauth/infrastructure/mod.rs`

- [ ] **Step 1: Escribir el test que falla**

Crear `provider_cache.rs` con solo el `mod tests` y el `use` (la struct vendrá en Step 3). Para que compile el test primero, escribir el archivo completo en Step 3; aquí declarar el test que vivirá en él:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_creds_return_same_provider_arc() {
        let cache = OAuthProviderCache::new();
        let a = cache.get_or_create("https://t/token", "cid", "csec", "rt");
        let b = cache.get_or_create("https://t/token", "cid", "csec", "rt");
        assert!(Arc::ptr_eq(&a, &b), "same creds must share one provider");
    }

    #[test]
    fn different_creds_return_different_providers() {
        let cache = OAuthProviderCache::new();
        let a = cache.get_or_create("https://t/token", "cid", "csec", "rt1");
        let b = cache.get_or_create("https://t/token", "cid", "csec", "rt2");
        assert!(!Arc::ptr_eq(&a, &b), "different refresh tokens => different providers");
    }

    #[test]
    fn fingerprint_does_not_contain_plaintext_refresh_token() {
        let fp = OAuthProviderCache::fingerprint("https://t/token", "cid", "csec", "1//SECRET");
        assert!(!fp.contains("1//SECRET"), "fingerprint must hash, not embed, the refresh token");
    }
}
```

- [ ] **Step 2: Correr el test para verificar que falla**

Run: `cargo test --lib provider_cache`
Expected: FAIL al compilar (`OAuthProviderCache` no existe).

- [ ] **Step 3: Implementación mínima**

Escribir el archivo `provider_cache.rs` completo (el `mod tests` del Step 1 va al final):

```rust
//! Process-wide cache of `OAuthRefreshTokenProvider`s keyed by a hash of
//! the credentials. Guarantees that all http_request nodes/tool-calls
//! sharing one identity (same token_url + client_id + refresh_token) reuse
//! a single provider — hence a single access-token cache and a single mint.
//!
//! Injected into `HttpNode` at construction in `registry.rs`, same pattern
//! as `with_storage`.

use crate::google_oauth::infrastructure::{OAuthCredentials, OAuthRefreshTokenProvider};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Maps a credential fingerprint to a shared provider.
#[derive(Default)]
pub struct OAuthProviderCache {
    inner: Mutex<HashMap<String, Arc<OAuthRefreshTokenProvider>>>,
}

impl OAuthProviderCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// SHA-256 hex of the identity tuple. The refresh token is hashed, never
    /// embedded in clear — the key may appear in debug dumps of the map.
    pub fn fingerprint(token_url: &str, client_id: &str, client_secret: &str, refresh_token: &str) -> String {
        let mut h = Sha256::new();
        h.update(token_url.as_bytes());
        h.update([0u8]);
        h.update(client_id.as_bytes());
        h.update([0u8]);
        h.update(client_secret.as_bytes());
        h.update([0u8]);
        h.update(refresh_token.as_bytes());
        format!("{:x}", h.finalize())
    }

    /// Return the shared provider for these creds, creating it on first use.
    pub fn get_or_create(
        &self,
        token_url: &str,
        client_id: &str,
        client_secret: &str,
        refresh_token: &str,
    ) -> Arc<OAuthRefreshTokenProvider> {
        let fp = Self::fingerprint(token_url, client_id, client_secret, refresh_token);
        let mut guard = self.inner.lock().expect("oauth provider cache mutex poisoned");
        if let Some(p) = guard.get(&fp) {
            return p.clone();
        }
        let creds = OAuthCredentials::new(client_id, client_secret, refresh_token);
        let provider = Arc::new(OAuthRefreshTokenProvider::with_endpoint(creds, token_url));
        guard.insert(fp, provider.clone());
        provider
    }
}
```

- [ ] **Step 4: Declarar el módulo y exportar**

En `mod.rs` agregar `pub mod provider_cache;` y `pub use provider_cache::OAuthProviderCache;`.

- [ ] **Step 5: Correr los tests**

Run: `cargo test --lib provider_cache`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/google_oauth/infrastructure/provider_cache.rs \
        src/libs/colmena/src/google_oauth/infrastructure/mod.rs
git commit -m "feat(oauth): OAuthProviderCache keyed by credential fingerprint"
```

---

## Task 5: Parseo y validación del bloque `auth` (módulo `http_oauth`)

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http_oauth.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`

- [ ] **Step 1: Escribir el test que falla**

Crear `http_oauth.rs` con el `mod tests` (la impl viene en Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg(v: serde_json::Value) -> serde_json::Value { v }

    #[test]
    fn parses_valid_oauth_block() {
        let c = cfg(json!({
            "base_url": "https://api.example.com",
            "auth": {
                "type": "oauth2_refresh_token",
                "token_url": "https://oauth2.googleapis.com/token",
                "client_id": "cid", "client_secret": "csec", "refresh_token": "rt"
            }
        }));
        let spec = parse_oauth_auth(&c, &Default::default()).expect("ok").expect("some");
        assert_eq!(spec.token_url, "https://oauth2.googleapis.com/token");
        assert_eq!(spec.client_id, "cid");
    }

    #[test]
    fn none_when_no_auth_block() {
        let c = cfg(json!({ "base_url": "https://x" }));
        assert!(parse_oauth_auth(&c, &Default::default()).expect("ok").is_none());
    }

    #[test]
    fn rejects_missing_fields_listing_all() {
        let c = cfg(json!({ "auth": { "type": "oauth2_refresh_token" } }));
        let err = parse_oauth_auth(&c, &Default::default()).expect_err("missing");
        assert!(err.contains("token_url"));
        assert!(err.contains("client_id"));
        assert!(err.contains("client_secret"));
        assert!(err.contains("refresh_token"));
    }

    #[test]
    fn rejects_auth_plus_bearer_token() {
        let c = cfg(json!({
            "bearer_token": "abc",
            "auth": { "type": "oauth2_refresh_token", "token_url": "u",
                      "client_id": "c", "client_secret": "s", "refresh_token": "r" }
        }));
        let err = parse_oauth_auth(&c, &Default::default()).expect_err("mutually exclusive");
        assert!(err.to_lowercase().contains("mutually exclusive") || err.contains("bearer_token"));
    }

    #[test]
    fn rejects_base_url_from_inputs_when_auth_present() {
        let c = cfg(json!({
            "auth": { "type": "oauth2_refresh_token", "token_url": "u",
                      "client_id": "c", "client_secret": "s", "refresh_token": "r" }
        }));
        let mut inputs = std::collections::HashMap::new();
        inputs.insert("base_url".to_string(), json!("https://evil.com"));
        let err = parse_oauth_auth(&c, &inputs).expect_err("base_url from inputs blocked");
        assert!(err.contains("base_url"));
    }

    #[test]
    fn rejects_unknown_type() {
        let c = cfg(json!({ "auth": { "type": "client_credentials" } }));
        let err = parse_oauth_auth(&c, &Default::default()).expect_err("unknown type v1");
        assert!(err.contains("oauth2_refresh_token"));
    }
}
```

- [ ] **Step 2: Correr el test para verificar que falla**

Run: `cargo test --lib http_oauth`
Expected: FAIL al compilar (`parse_oauth_auth` / `OAuthAuthSpec` no existen).

- [ ] **Step 3: Implementación mínima**

Al inicio de `http_oauth.rs` (antes del `mod tests`):

```rust
//! Native OAuth2 (refresh_token grant) for the http_request node:
//! parsing + validation of the `auth` config block, and the 401-retry
//! send helper. The `auth` block is read ONLY from `config`, never from
//! the LLM's `inputs`.

use crate::dag_engine::domain::node::NodeInputs;
use serde_json::Value;

/// Resolved OAuth2 refresh-token auth, all `${ENV}` already expanded by
/// the caller before use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthAuthSpec {
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
}

/// Parse and validate the `auth` block from `config`.
/// Returns:
/// - `Ok(None)` when no `auth` block is present (non-OAuth request).
/// - `Ok(Some(spec))` when valid (raw values, still `${ENV}`-unresolved).
/// - `Err(msg)` on any validation failure (missing fields, wrong type,
///   mutual exclusion, or LLM-supplied base_url).
pub fn parse_oauth_auth(config: &Value, inputs: &NodeInputs) -> Result<Option<OAuthAuthSpec>, String> {
    let auth = match config.get("auth") {
        Some(a) => a,
        None => return Ok(None),
    };

    // Mutual exclusion with static auth.
    let has_static = config.get("bearer_token").is_some()
        || config.get("authorization").is_some()
        || inputs.contains_key("bearer_token")
        || inputs.contains_key("authorization");
    if has_static {
        return Err("`auth` is mutually exclusive with `bearer_token`/`authorization`".to_string());
    }

    // Anti-exfiltration guard: with `auth` present, the destination host
    // must be operator-fixed — the LLM must not supply `base_url` via inputs.
    if inputs.contains_key("base_url") {
        return Err(
            "`base_url` must be fixed in config when `auth` is set; \
             it cannot come from inputs (anti-exfiltration guard)"
                .to_string(),
        );
    }

    let ty = auth.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if ty != "oauth2_refresh_token" {
        return Err(format!(
            "unsupported auth.type '{ty}'; v1 supports only 'oauth2_refresh_token'"
        ));
    }

    let mut missing = Vec::new();
    let get = |k: &str| auth.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
    let token_url = get("token_url");
    let client_id = get("client_id");
    let client_secret = get("client_secret");
    let refresh_token = get("refresh_token");
    if token_url.is_none() { missing.push("token_url"); }
    if client_id.is_none() { missing.push("client_id"); }
    if client_secret.is_none() { missing.push("client_secret"); }
    if refresh_token.is_none() { missing.push("refresh_token"); }
    if !missing.is_empty() {
        return Err(format!("auth block missing required fields: {}", missing.join(", ")));
    }

    Ok(Some(OAuthAuthSpec {
        token_url: token_url.unwrap(),
        client_id: client_id.unwrap(),
        client_secret: client_secret.unwrap(),
        refresh_token: refresh_token.unwrap(),
    }))
}
```

- [ ] **Step 4: Declarar el módulo**

En `nodes/mod.rs` agregar (orden alfabético cerca de `http`): `pub mod http_oauth;`

- [ ] **Step 5: Correr los tests**

Run: `cargo test --lib http_oauth`
Expected: PASS (6 tests).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/http_oauth.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs
git commit -m "feat(http): parse + validate native OAuth2 auth block"
```

---

## Task 6: Helper `send_with_oauth_retry` (401 → invalidate → retry once)

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http_oauth.rs`

- [ ] **Step 1: Escribir el test que falla**

Agregar al `mod tests` de `http_oauth.rs`:

```rust
#[tokio::test]
async fn retries_once_on_401_with_fresh_token() {
    use wiremock::matchers::{method, path, header};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use crate::google_oauth::infrastructure::OAuthProviderCache;

    // Token endpoint: returns a different token each call so we can assert
    // the retry used a fresh one.
    let token_srv = MockServer::start().await;
    Mock::given(method("POST")).respond_with(
        ResponseTemplate::new(200).set_body_string(
            r#"{"access_token":"TOKEN_A","expires_in":3600,"token_type":"Bearer"}"#))
        .up_to_n_times(1).mount(&token_srv).await;
    Mock::given(method("POST")).respond_with(
        ResponseTemplate::new(200).set_body_string(
            r#"{"access_token":"TOKEN_B","expires_in":3600,"token_type":"Bearer"}"#))
        .mount(&token_srv).await;

    // API: 401 for TOKEN_A, 200 for TOKEN_B.
    let api = MockServer::start().await;
    Mock::given(method("GET")).and(path("/x")).and(header("Authorization", "Bearer TOKEN_A"))
        .respond_with(ResponseTemplate::new(401)).mount(&api).await;
    Mock::given(method("GET")).and(path("/x")).and(header("Authorization", "Bearer TOKEN_B"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok")).mount(&api).await;

    let cache = OAuthProviderCache::new();
    let provider = cache.get_or_create(&token_srv.uri(), "cid", "csec", "rt");
    let client = reqwest::Client::builder().http1_only().build().unwrap();
    let builder = client.get(format!("{}/x", api.uri()));
    let resp = send_with_oauth_retry(builder, provider).await.expect("ok");
    assert_eq!(resp.status().as_u16(), 200);
}
```

- [ ] **Step 2: Correr el test para verificar que falla**

Run: `cargo test --lib retries_once_on_401_with_fresh_token`
Expected: FAIL al compilar (`send_with_oauth_retry` no existe).

- [ ] **Step 3: Implementación mínima**

Agregar a `http_oauth.rs` (zona de impl, antes del `mod tests`):

```rust
use crate::google_oauth::infrastructure::OAuthRefreshTokenProvider;
use std::error::Error as StdError;
use std::sync::Arc;

/// Send `builder` (which must NOT already carry an Authorization header)
/// with a fresh Bearer from `provider`. On HTTP 401, invalidate the cached
/// token, mint a new one, and retry exactly once. 403/429/etc. pass through.
pub async fn send_with_oauth_retry(
    builder: reqwest::RequestBuilder,
    provider: Arc<OAuthRefreshTokenProvider>,
) -> Result<reqwest::Response, Box<dyn StdError + Send + Sync>> {
    let token = provider
        .get_bearer_token()
        .await
        .map_err(|e| Box::new(std::io::Error::other(format!("OAuth: {e}")))
            as Box<dyn StdError + Send + Sync>)?;

    let first = builder
        .try_clone()
        .ok_or_else(|| Box::new(std::io::Error::other(
            "http_request: OAuth requires a cloneable request (no streaming body)"))
            as Box<dyn StdError + Send + Sync>)?
        .header("Authorization", format!("Bearer {}", token.as_str()));
    let resp = first.send().await?;

    if resp.status().as_u16() == 401 {
        provider.invalidate_cache().await;
        let token2 = provider
            .get_bearer_token()
            .await
            .map_err(|e| Box::new(std::io::Error::other(format!("OAuth (retry): {e}")))
                as Box<dyn StdError + Send + Sync>)?;
        let second = builder.header("Authorization", format!("Bearer {}", token2.as_str()));
        return Ok(second.send().await?);
    }
    Ok(resp)
}
```

> Nota: `std::io::Error::other` requiere Rust ≥1.74; el toolchain está en 1.95 (ver `rust-toolchain.toml`). El `OAuthError` se rinde a string vía `Display`, que para `RefreshTokenRevoked`/`ClientCredsInvalid` ya trae mensaje accionable.

- [ ] **Step 4: Correr el test**

Run: `cargo test --lib retries_once_on_401_with_fresh_token`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/http_oauth.rs
git commit -m "feat(http): OAuth 401 invalidate+retry-once send helper"
```

---

## Task 7: Inyectar el cache en `HttpNode` y wirearlo en el registry

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs` (struct `HttpNode` ~31, `new`/builders ~224)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs:112`

- [ ] **Step 1: Escribir el test que falla**

Agregar al `mod tests` de `http.rs`:

```rust
#[test]
fn with_oauth_cache_sets_field() {
    use crate::google_oauth::infrastructure::OAuthProviderCache;
    let node = HttpNode::new().with_oauth_cache(std::sync::Arc::new(OAuthProviderCache::new()));
    assert!(node.oauth_cache.is_some());
}
```

- [ ] **Step 2: Correr el test para verificar que falla**

Run: `cargo test --lib with_oauth_cache_sets_field`
Expected: FAIL (campo/builder no existen).

- [ ] **Step 3: Implementación mínima**

En `http.rs`, agregar el campo al struct `HttpNode` (después de `attachment_resolver`):

```rust
    /// Shared OAuth provider cache (fingerprint → provider). When present,
    /// an `auth` block in config authenticates natively. Injected in registry.rs.
    oauth_cache: Option<Arc<crate::google_oauth::infrastructure::OAuthProviderCache>>,
```

En `impl HttpNode`, en `new()` agregar `oauth_cache: None,` al inicializador, y agregar el builder tras `with_attachment_resolver`:

```rust
    /// Wire the shared OAuth provider cache so config `auth` blocks
    /// authenticate via the refresh_token grant.
    pub fn with_oauth_cache(
        mut self,
        cache: Arc<crate::google_oauth::infrastructure::OAuthProviderCache>,
    ) -> Self {
        self.oauth_cache = Some(cache);
        self
    }
```

- [ ] **Step 4: Wirear en el registry**

En `registry.rs`, dentro de `Arc::new_cyclic`, antes de `let mut http_node = HttpNode::new();` crear el cache una vez, y encadenarlo:

```rust
            // Native OAuth: one shared provider cache for all http_request
            // usages (and tool-calls) in this engine. Keyed by credential
            // fingerprint so the same identity mints one token.
            let oauth_cache = Arc::new(
                crate::google_oauth::infrastructure::OAuthProviderCache::new(),
            );

            let mut http_node = HttpNode::new().with_oauth_cache(oauth_cache);
```

(Las dos líneas `if let Some(st)... with_storage` / `with_attachment_resolver` siguen igual, encadenadas sobre `http_node`.)

- [ ] **Step 5: Correr el test**

Run: `cargo test --lib with_oauth_cache_sets_field`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs \
        src/libs/colmena/src/dag_engine/infrastructure/registry.rs
git commit -m "feat(http): inject shared OAuthProviderCache into HttpNode"
```

---

## Task 8: Integrar OAuth en `HttpNode::execute`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs` (bloque de auth ~824-846 y el send ~984)

- [ ] **Step 1: Escribir el test de integración que falla**

Agregar al `mod tests` de `http.rs` (usa wiremock para token + API, ejercita `execute` completo):

```rust
#[tokio::test]
async fn execute_authenticates_with_oauth_block() {
    use wiremock::matchers::{method, path, header};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use crate::google_oauth::infrastructure::OAuthProviderCache;
    use std::collections::HashMap;

    let token_srv = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_string(
        r#"{"access_token":"ya29.exec","expires_in":3600,"token_type":"Bearer"}"#))
        .mount(&token_srv).await;

    let api = MockServer::start().await;
    Mock::given(method("GET")).and(path("/data")).and(header("Authorization", "Bearer ya29.exec"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .mount(&api).await;

    let node = HttpNode::new().with_oauth_cache(std::sync::Arc::new(OAuthProviderCache::new()));
    let config = serde_json::json!({
        "base_url": api.uri(),
        "endpoint": "/data",
        "method": "GET",
        "auth": {
            "type": "oauth2_refresh_token",
            "token_url": token_srv.uri() + "/token-not-used-path",
            "client_id": "cid", "client_secret": "csec", "refresh_token": "rt"
        }
    });
    // token_url must point at the mock root (POST matches any path on the server):
    let config = {
        let mut c = config;
        c["auth"]["token_url"] = serde_json::Value::String(token_srv.uri());
        c
    };
    let inputs: HashMap<String, serde_json::Value> = HashMap::new();
    let mut state = serde_json::Value::Null;
    let out = node.execute(&inputs, &config, &mut state, None).await.expect("ok");
    assert_eq!(out["status"], 200);
    assert_eq!(out["body"]["ok"], true);
}

#[tokio::test]
async fn execute_rejects_auth_plus_bearer_token() {
    use std::collections::HashMap;
    let node = HttpNode::new().with_oauth_cache(
        std::sync::Arc::new(crate::google_oauth::infrastructure::OAuthProviderCache::new()));
    let config = serde_json::json!({
        "base_url": "https://x", "endpoint": "/y", "bearer_token": "static",
        "auth": { "type": "oauth2_refresh_token", "token_url": "u",
                  "client_id": "c", "client_secret": "s", "refresh_token": "r" }
    });
    let inputs: HashMap<String, serde_json::Value> = HashMap::new();
    let mut state = serde_json::Value::Null;
    let err = node.execute(&inputs, &config, &mut state, None).await.expect_err("mutually exclusive");
    assert!(format!("{err}").contains("mutually exclusive"));
}
```

- [ ] **Step 2: Correr el test para verificar que falla**

Run: `cargo test --lib execute_authenticates_with_oauth_block`
Expected: FAIL (el `auth` block se ignora hoy → 401/sin header, o el segundo test no detecta el conflicto).

- [ ] **Step 3: Implementación — insertar la rama OAuth**

En `http.rs`, en el bloque de auth (el que hoy hace `bearer_token`/`authorization`, ~824-846 en `execute`), envolver la lógica:

Primero, justo antes del bloque de `bearer_token`, parsear y resolver el `auth`:

```rust
        // --- Native OAuth2 (refresh_token grant) ---
        // Parse the `auth` block (config-only). Validation includes mutual
        // exclusion with bearer_token/authorization and the base_url-from-inputs
        // guard, so do it BEFORE the static-auth handling below.
        let oauth_spec = crate::dag_engine::infrastructure::nodes::http_oauth::parse_oauth_auth(
            config, inputs,
        )
        .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
            as Box<dyn StdError + Send + Sync>)?;

        let oauth_provider = if let Some(spec) = oauth_spec {
            let cache = self.oauth_cache.as_ref().ok_or_else(|| {
                Box::new(std::io::Error::other(
                    "http_request: `auth` block set but no OAuthProviderCache wired"))
                    as Box<dyn StdError + Send + Sync>
            })?;
            // Resolve ${ENV} in each secret field.
            let token_url = Self::resolve_env_vars(&spec.token_url).map_err(|e| {
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                    as Box<dyn StdError + Send + Sync>})?;
            let client_id = Self::resolve_env_vars(&spec.client_id).map_err(|e| {
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                    as Box<dyn StdError + Send + Sync>})?;
            let client_secret = Self::resolve_env_vars(&spec.client_secret).map_err(|e| {
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                    as Box<dyn StdError + Send + Sync>})?;
            let refresh_token = Self::resolve_env_vars(&spec.refresh_token).map_err(|e| {
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                    as Box<dyn StdError + Send + Sync>})?;
            Some(cache.get_or_create(&token_url, &client_id, &client_secret, &refresh_token))
        } else {
            None
        };
```

Luego, dejar los bloques de `bearer_token` / `authorization` **solo cuando NO hay OAuth** — envolverlos en `if oauth_provider.is_none() { ... }`. (Como `parse_oauth_auth` ya rechaza el combo, en la práctica nunca coexisten, pero el guard evita doble header.)

Finalmente, reemplazar el envío `let response = request_builder.send().await?;` (~984) por:

```rust
        let response = if let Some(provider) = oauth_provider {
            crate::dag_engine::infrastructure::nodes::http_oauth::send_with_oauth_retry(
                request_builder, provider,
            )
            .await?
        } else {
            request_builder.send().await?
        };
```

- [ ] **Step 4: Correr los tests**

Run: `cargo test --lib execute_authenticates_with_oauth_block execute_rejects_auth_plus_bearer_token`
Expected: PASS (2 tests).

- [ ] **Step 5: Correr toda la suite del nodo http + oauth**

Run: `cargo test --lib http`
Expected: PASS (sin regresiones en los tests existentes de http).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs
git commit -m "feat(http): native OAuth2 auth in http_request execute (refresh_token grant)"
```

---

## Task 9: Grafo E2E + verificación contra Gmail real

**Files:**
- Create: `tests/graphs/external/gmail_oauth_read.json`

- [ ] **Step 1: Crear el grafo**

`gmail_oauth_read.json` — `llm_call` con una tool `gmail_list` (http_request) con `auth` inline fijo. Usa `${GMAIL_CLIENT_ID}` / `${GMAIL_CLIENT_SECRET}` / `${GMAIL_REFRESH_TOKEN}`:

```json
{
  "nodes": [
    {
      "id": "agent",
      "node_type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "system_prompt": "Eres un asistente que lee el correo del usuario. Usa la herramienta gmail_list para buscar correos y resume lo que encuentres.",
        "tool_configurations": {
          "gmail_list": {
            "node_type": "http_request",
            "description": "Lista correos del buzón del usuario. Devuelve ids de mensajes que coinciden con la búsqueda.",
            "node_schema": {
              "base_url": { "fixed": "https://gmail.googleapis.com" },
              "method":   { "fixed": "GET" },
              "endpoint": { "fixed": "/gmail/v1/users/me/messages" },
              "auth":     { "fixed": {
                "type": "oauth2_refresh_token",
                "token_url": "https://oauth2.googleapis.com/token",
                "client_id": "${GMAIL_CLIENT_ID}",
                "client_secret": "${GMAIL_CLIENT_SECRET}",
                "refresh_token": "${GMAIL_REFRESH_TOKEN}"
              }},
              "query_params": { "type": "object", "required": false,
                "description": "Filtros Gmail, p.ej. {\"q\": \"is:unread\", \"maxResults\": \"5\"}" }
            }
          }
        }
      }
    }
  ],
  "edges": []
}
```

- [ ] **Step 2: Verificar que todos los `node_type` están registrados**

Run: `grep -E '"(llm_call|http_request)"' src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
Expected: ambos aparecen (`llm_call`, `http_request`).

- [ ] **Step 3: Inyectar creds reales desde Secret Manager y correr (NO commitear valores)**

Obtener el refresh token `gmail.readonly` una vez vía OAuth Playground (ver spec §10), guardarlo en Secret Manager (`startti-dev`), y exportarlo en memoria. Luego:

```bash
set -a; source .env; set +a   # OPENAI/ANTHROPIC/GEMINI keys
export GMAIL_CLIENT_ID="$(gcloud secrets versions access latest --secret=GMAIL_OAUTH_CLIENT_ID --project=startti-dev)"
export GMAIL_CLIENT_SECRET="$(gcloud secrets versions access latest --secret=GMAIL_OAUTH_CLIENT_SECRET --project=startti-dev)"
export GMAIL_REFRESH_TOKEN="$(gcloud secrets versions access latest --secret=GMAIL_OAUTH_REFRESH_TOKEN --project=startti-dev)"
mkdir -p /tmp/colmena_e2e
unset COLMENA_LOCAL
cargo run --bin dag_engine -- run tests/graphs/external/gmail_oauth_read.json \
  --agent-session-id gmail_oauth_e2e_001 2>&1 | tee /tmp/colmena_e2e/gmail_oauth_read.sse
```

Expected: el agente llama `gmail_list`, recibe `{"messages":[...]}` de Gmail (status 200), y resume. En los logs NO debe aparecer el access token ni el refresh token.

- [ ] **Step 4: Reporte amigable**

Presentar al usuario: prompt, payload clave (cuántos mensajes), tokens, resumen. No pegar el SSE completo en el chat.

- [ ] **Step 5: Commit del grafo (sin secretos)**

```bash
git add tests/graphs/external/gmail_oauth_read.json
git commit -m "test(http): E2E graph reading Gmail via native OAuth2"
```

---

## Task 10: Documentación

**Files:**
- Modify: `docs/developer_guide/25_web_nodes.md`
- Modify: `docs/node_configurations.json`
- Modify: `docs/node_as_tools_reference.json`

- [ ] **Step 1: `25_web_nodes.md` — sección "OAuth2 nativo"**

Agregar una sección documentando el bloque `auth` (esquema 4.1 del spec), las reglas (config-only, mutual exclusion, host fijo), el comportamiento de retry (401 sí, 403/429 no), y el gotcha operativo de los 7 días (spec §10). Incluir el ejemplo `node_schema+fixed` para uso como tool y la tabla de garantías frente al LLM (spec §5.3).

- [ ] **Step 2: `node_configurations.json` — campo `auth` en `http_request`**

Agregar `auth` (objeto opcional) al schema de `http_request` con sus subcampos `type`, `token_url`, `client_id`, `client_secret`, `refresh_token` y nota "config-only; mutually exclusive with bearer_token/authorization".

- [ ] **Step 3: `node_as_tools_reference.json` — ejemplo OAuth**

Agregar bajo `http_request` un ejemplo de `node_schema` con `auth` como campo `fixed`, señalando que va en `node_schema+fixed` (nunca visible al LLM).

- [ ] **Step 4: Commit**

```bash
git add docs/developer_guide/25_web_nodes.md docs/node_configurations.json docs/node_as_tools_reference.json
git commit -m "docs: native OAuth2 in http_request node"
```

---

## Task 11: Verificación final pre-push

- [ ] **Step 1: Suite completa (como CI)**

Run: `cargo test --verbose 2>&1 | tail -30`
Expected: todos los tests pasan (unit + integration + doctests). `--lib` solo no basta (ver CLAUDE.md / memoria).

- [ ] **Step 2: Clippy + fmt**

Run: `cargo clippy --all-targets 2>&1 | tail -20 && cargo fmt --check`
Expected: sin warnings (el crate tiene `warnings = "deny"`), fmt limpio.

- [ ] **Step 3: Sweep de impacto ADP (aditivo, debería estar limpio)**

Confirmar que no se cambió ninguna firma pública existente de `google_oauth` ni de `ExecutableNode`. Solo se agregaron constructores/campos. ADP worker no se afecta (ver spec §9).

---

## Self-Review (cobertura del spec)

- §3.1 generalización google_oauth → Tasks 1-3. ✅
- §3.2 OAuthProviderCache → Task 4. ✅
- §4.1 esquema auth inline → Task 5 (parseo) + Task 9 (grafo). ✅
- §4.3 reglas (config-only, mutual exclusion, type enum) → Task 5. ✅
- §5.1 flujo inline + fingerprint → Tasks 7-8. ✅
- §5.3 garantías frente al LLM → Task 9 (node_schema fixed) + Task 10 (docs). ✅
- §6.1 anti-exfiltración (base_url fijo) → Task 5 (`rejects_base_url_from_inputs_when_auth_present`). ✅
- §6.2/§6.3 secretos no logueados / token solo en memoria → reusa `RefreshTokenSecret` redaction + no se persiste (Task 4/8). ✅
- §7 manejo de errores → Task 5 (config) + Task 6 (`OAuthError` Display). ✅
- §8 testing → Tasks 1-9 (unit wiremock) + Task 9 (E2E). ✅
- §9 compat ADP → Task 11 Step 3. ✅
- §10 gotcha 7 días → Task 10 Step 1. ✅
- §11 Backlog (persistencia DB) → fuera de alcance (ya en BACKLOG.md). ✅
- §4.2 / §5.2 conexión nombrada → **Fase 2** (plan aparte; no requerida funcionalmente).
