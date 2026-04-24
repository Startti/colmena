# Web Nodes — `api_explorer` Implementation Plan (Spec C)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the `api_explorer` toolkit node exposing five LLM sub-tools — `load_spec`, `list_endpoints`, `search_endpoint`, `get_endpoint_details`, `build_http_request` — so an agent can take an OpenAPI 3.x or Swagger 2.0 URL at runtime, discover endpoints deterministically, and emit a validated `http_request`-shaped JSON config. Includes URL normalization for Git-forge blob URLs, a pure-Rust Swagger-2.0-to-OpenAPI-3.0 converter, an `ApiSpecPort` trait, a reqwest-backed `OpenApiAdapter` with ETag revalidation, and an `ApiSpecUseCase` with per-conversation LRU cache and fuzzy endpoint search.

**Architecture:** New files under `src/libs/colmena/src/web/`:

- `web/domain/api_spec_port.rs` — `ApiSpecPort` trait + value objects (`ParsedSpec`, `Endpoint`, `ParameterSpec`, `RequestBodySpec`, `ResponseSpec`, `SecurityScheme`, `SecurityRequirement`, `ApiKeyLocation`, `HttpMethod`, `SpecFetchResult`).
- `web/application/url_normalizer.rs` — pure-function Git-forge URL rewriter (GitHub blob/tree, GitLab `-/blob`, Bitbucket `src`).
- `web/application/swagger2_to_oas3.rs` — pure-Rust JSON-tree converter; Swagger 2.0 → OpenAPI 3.0.3.
- `web/application/api_spec_use_case.rs` — `ApiSpecUseCase` wrapping `ApiSpecPort` with an LRU `SpecCache` per conversation in a `SessionRegistry<SpecCache>`, plus fuzzy endpoint search (`nucleo-matcher`), endpoint detail formatting, and `build_http_request` validation/routing.
- `web/infrastructure/openapi_adapter.rs` — `OpenApiAdapter` implementing `ApiSpecPort`: URL normalization → reqwest GET with size/timeout limits → HTML-response detection → format/version detection → (Swagger 2.0 conversion if needed) → `oas3` parse → `ParsedSpec` mapping, with `If-None-Match`/`If-Modified-Since` revalidation.
- `dag_engine/infrastructure/nodes/api_explorer.rs` — `ApiExplorerNode` implementing `ToolkitNode`, dispatching on `__sub_tool` to five handlers.
- Registered in `HashMapNodeRegistry::new_with_secure_values` under `node_type = "api_explorer"`; subscribed to `ConversationLifecycleBus` for eager per-conversation cache cleanup.

**Tech Stack:** Rust (async/await + tokio), `reqwest` (already in `Cargo.toml`), `serde`/`serde_json` (preserve_order), `serde_yaml` (already present), `async-trait`, `thiserror`, `lru` (already present), `chrono` (already present), `mockall` (dev, already present), `wiremock` (dev, already 0.5; Plan A upgrades to 0.6 if not done yet), plus two new crates: `oas3` (OpenAPI 3.x parser) and `nucleo-matcher` (fuzzy matcher used by Helix editor).

**Design spec:** [docs/superpowers/specs/2026-04-23-web-nodes-c-api-explorer-design.md](../specs/2026-04-23-web-nodes-c-api-explorer-design.md)

**Depends on:**

- Plan 0 (`2026-04-23-web-nodes-0-unified-foundation.md`) — the `web/` module skeleton, `WebDomainError`, `ToolkitNode` trait, `SessionRegistry<T>`, `ConversationLifecycleBus`, and multi-sub-tool executor dispatch must exist first.
- Plan A (`2026-04-23-web-nodes-a-tavily-client.md`) is **recommended but not strictly required**. The shared `docs/developer_guide/25_web_nodes.md` skeleton comes from Plan 0 (Task 13). Plan A exercises the toolkit runtime end-to-end first, which de-risks this plan's bigger surface. If Plan A has not shipped, the cross-node graph in Task 18 can be skipped or converted to a standalone spec-only variant.

---

## Conventions for this plan

- All commits use: `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>`
- Run Rust tests with `cargo test --lib <module>` — the crate is named `colmena_dag_engine` (NOT `colmena`).
- Use the project venv for Python steps: `.venv/bin/pytest ...`. Source `.env` for graphs that hit real LLMs.
- After each task, run `cargo check --lib` before committing; it must pass.
- After tasks that touch `registry.rs` or the executor, also run `cargo test --lib registry dag_tool_executor` before committing.
- Every task ends with a commit. Don't batch.
- Fixture files live under `src/libs/colmena/tests/fixtures/specs/` — they are test-only assets loaded with `include_str!` or `std::fs::read_to_string` in unit tests.
- Live-network tests are **avoided entirely** in this plan — all fetching goes through `wiremock` (`reqwest` is pointed at the mock server) or through `std::fs` via a thin injection seam.
- When editing `Cargo.toml`, keep the existing alphabetical-ish grouping but don't renumber unrelated lines; minimal diff wins.

---

## Task 0: Verify Plan 0 pre-requisites and add `oas3` + `nucleo-matcher` deps

**Files:**
- Modify: `src/libs/colmena/Cargo.toml`

- [ ] **Step 1: Verify Plan 0 is merged**

Run:

```bash
cargo test --lib web::domain::errors
cargo test --lib web::domain::session
cargo test --lib web::domain::lifecycle
cargo test --lib toolkit_node
cargo test --lib dag_tool_executor::toolkit_runtime_tests
```

Expected: all five pass. If any fail, Plan 0 is not fully merged — stop and land it first.

- [ ] **Step 2: Add the two new runtime dependencies**

Edit `src/libs/colmena/Cargo.toml`. In `[dependencies]`, add the following two lines (grouped together; place them immediately after the existing `serde_yaml = "0.9"` line so related parser dependencies are colocated):

```toml
# OpenAPI 3.x parsing (api_explorer node, Spec C).
oas3 = { version = "0.21", features = ["yaml-spec"] }

# Unicode-aware fuzzy matcher (used by Helix editor) — endpoint search.
nucleo-matcher = "0.3"
```

> The `yaml-spec` feature teaches `oas3` to accept YAML input. We still do our own detection (see Task 6) but the feature provides the helper methods.

- [ ] **Step 3: Verify both crates resolve and the project still compiles**

Run: `cargo check --lib --tests 2>&1 | tail -20`
Expected: clean compile; `Cargo.lock` gains `oas3`, `nucleo-matcher`, and their transitive deps (notably `oas3` pulls in `semver`, `http`, `url`, and `rustc-hash` — all already in the dep graph or lightweight).

- [ ] **Step 4: Verify the existing tests still pass (nothing should have broken)**

Run: `cargo test --lib 2>&1 | tail -10`
Expected: existing suites still pass. If a transitive version bump breaks something, pin the offender rather than back out the two new crates.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
chore(web): add oas3 and nucleo-matcher deps for api_explorer

oas3 parses OpenAPI 3.x (with yaml-spec feature for YAML inputs).
nucleo-matcher powers fuzzy endpoint search. Both are pulled in now
so subsequent domain/application tasks can reference them without
churning Cargo.lock.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 1: URL normalizer module (pure function, table-driven tests)

**Files:**
- Create: `src/libs/colmena/src/web/application/url_normalizer.rs`
- Modify: `src/libs/colmena/src/web/application/mod.rs`

Pure function. No I/O. Rewrites common Git-forge "blob" URLs to their raw-content equivalents. The function is called from `OpenApiAdapter` before every fetch.

- [ ] **Step 1: Write the failing table-driven test**

Create `src/libs/colmena/src/web/application/url_normalizer.rs`:

```rust
//! Normalize Git-forge "blob" URLs to their raw-content equivalents.
//!
//! LLMs paste whatever URL the user gave them. For the top three public
//! forges we can rewrite the rendered-HTML URL to a URL that actually
//! serves the raw file. Unknown hosts and URLs that already point to raw
//! content pass through unchanged.

/// Result of normalizing a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedUrl {
    /// The URL that the adapter should actually GET.
    pub resolved: String,
    /// `true` if the input looked like a recognized forge and was rewritten.
    pub rewritten: bool,
}

/// Normalize well-known Git-forge "blob" URLs.
///
/// Rules:
/// - `github.com/{owner}/{repo}/blob/{ref}/{path}` → `raw.githubusercontent.com/{owner}/{repo}/{ref}/{path}`
/// - `github.com/{owner}/{repo}/tree/{ref}/{path}` → same as above (rarer; a tree URL that points at a file).
/// - `gitlab.com/{owner}/{repo}/-/blob/{ref}/{path}` → `gitlab.com/{owner}/{repo}/-/raw/{ref}/{path}`
/// - `bitbucket.org/{owner}/{repo}/src/{ref}/{path}` → `bitbucket.org/{owner}/{repo}/raw/{ref}/{path}`
///
/// Query strings and fragments are preserved.
pub fn normalize_forge_url(input: &str) -> NormalizedUrl {
    // GitHub blob / tree
    if let Some(rest) = input.strip_prefix("https://github.com/") {
        if let Some(rewritten) = rewrite_github(rest) {
            return NormalizedUrl { resolved: rewritten, rewritten: true };
        }
    }
    // GitLab -/blob
    if let Some(rest) = input.strip_prefix("https://gitlab.com/") {
        if let Some(rewritten) = rewrite_gitlab(rest) {
            return NormalizedUrl { resolved: rewritten, rewritten: true };
        }
    }
    // Bitbucket src
    if let Some(rest) = input.strip_prefix("https://bitbucket.org/") {
        if let Some(rewritten) = rewrite_bitbucket(rest) {
            return NormalizedUrl { resolved: rewritten, rewritten: true };
        }
    }
    NormalizedUrl { resolved: input.to_string(), rewritten: false }
}

fn rewrite_github(rest: &str) -> Option<String> {
    // {owner}/{repo}/{blob|tree}/{ref}/{path...}
    let (owner, rest) = rest.split_once('/')?;
    let (repo, rest) = rest.split_once('/')?;
    let (kind, rest) = rest.split_once('/')?;
    if kind != "blob" && kind != "tree" {
        return None;
    }
    let (git_ref, path) = rest.split_once('/')?;
    Some(format!(
        "https://raw.githubusercontent.com/{owner}/{repo}/{git_ref}/{path}"
    ))
}

fn rewrite_gitlab(rest: &str) -> Option<String> {
    // {owner}/{repo}/-/blob/{ref}/{path...}
    let (owner, rest) = rest.split_once('/')?;
    let (repo, rest) = rest.split_once('/')?;
    let rest = rest.strip_prefix("-/")?;
    let rest = rest.strip_prefix("blob/")?;
    let (git_ref, path) = rest.split_once('/')?;
    Some(format!(
        "https://gitlab.com/{owner}/{repo}/-/raw/{git_ref}/{path}"
    ))
}

fn rewrite_bitbucket(rest: &str) -> Option<String> {
    // {owner}/{repo}/src/{ref}/{path...}
    let (owner, rest) = rest.split_once('/')?;
    let (repo, rest) = rest.split_once('/')?;
    let rest = rest.strip_prefix("src/")?;
    let (git_ref, path) = rest.split_once('/')?;
    Some(format!(
        "https://bitbucket.org/{owner}/{repo}/raw/{git_ref}/{path}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_blob_url_is_rewritten_to_raw() {
        let n = normalize_forge_url(
            "https://github.com/OAI/OpenAPI-Specification/blob/main/examples/v3.0/petstore.yaml",
        );
        assert!(n.rewritten);
        assert_eq!(
            n.resolved,
            "https://raw.githubusercontent.com/OAI/OpenAPI-Specification/main/examples/v3.0/petstore.yaml"
        );
    }

    #[test]
    fn github_tree_url_is_rewritten_to_raw() {
        let n = normalize_forge_url(
            "https://github.com/amadeus4dev/amadeus-code-examples/tree/master/self-service/reference-data/airlines/get_airlines.yaml",
        );
        assert!(n.rewritten);
        assert!(n.resolved.starts_with("https://raw.githubusercontent.com/"));
    }

    #[test]
    fn gitlab_blob_url_is_rewritten_to_raw() {
        let n = normalize_forge_url(
            "https://gitlab.com/some/repo/-/blob/main/spec/openapi.yaml",
        );
        assert!(n.rewritten);
        assert_eq!(
            n.resolved,
            "https://gitlab.com/some/repo/-/raw/main/spec/openapi.yaml"
        );
    }

    #[test]
    fn bitbucket_src_url_is_rewritten_to_raw() {
        let n = normalize_forge_url(
            "https://bitbucket.org/team/repo/src/main/openapi.yaml",
        );
        assert!(n.rewritten);
        assert_eq!(
            n.resolved,
            "https://bitbucket.org/team/repo/raw/main/openapi.yaml"
        );
    }

    #[test]
    fn raw_url_passes_through() {
        let url = "https://raw.githubusercontent.com/foo/bar/main/openapi.yaml";
        let n = normalize_forge_url(url);
        assert!(!n.rewritten);
        assert_eq!(n.resolved, url);
    }

    #[test]
    fn non_forge_url_passes_through() {
        let url = "https://petstore3.swagger.io/api/v3/openapi.json";
        let n = normalize_forge_url(url);
        assert!(!n.rewritten);
        assert_eq!(n.resolved, url);
    }

    #[test]
    fn private_gitlab_host_passes_through() {
        // Only public gitlab.com is rewritten; self-hosted instances must be raw already.
        let url = "https://git.internal.example.com/foo/bar/-/blob/main/openapi.yaml";
        let n = normalize_forge_url(url);
        assert!(!n.rewritten);
        assert_eq!(n.resolved, url);
    }

    #[test]
    fn github_non_blob_url_passes_through() {
        // e.g. a releases page — we don't try to rewrite.
        let url = "https://github.com/OAI/OpenAPI-Specification/releases/tag/3.0.3";
        let n = normalize_forge_url(url);
        assert!(!n.rewritten);
        assert_eq!(n.resolved, url);
    }

    #[test]
    fn github_url_with_query_is_preserved_verbatim() {
        // Query strings are not stripped — URLs we rewrite include the full tail.
        let n = normalize_forge_url(
            "https://github.com/foo/bar/blob/main/openapi.yaml?raw=1",
        );
        assert!(n.rewritten);
        assert_eq!(
            n.resolved,
            "https://raw.githubusercontent.com/foo/bar/main/openapi.yaml?raw=1"
        );
    }

    #[test]
    fn github_url_without_file_path_passes_through() {
        // Missing a path after the ref — can't rewrite.
        let n = normalize_forge_url("https://github.com/foo/bar/blob/main");
        assert!(!n.rewritten);
    }
}
```

- [ ] **Step 2: Register the module**

Edit `src/libs/colmena/src/web/application/mod.rs`. Append:

```rust
pub mod url_normalizer;
```

(If the file doesn't yet exist or is empty, create it with that single line.)

- [ ] **Step 3: Run — expect PASS**

Run: `cargo test --lib web::application::url_normalizer`
Expected: 10 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/web/application/url_normalizer.rs src/libs/colmena/src/web/application/mod.rs
git commit -m "$(cat <<'EOF'
feat(web): add url_normalizer — Git-forge blob → raw rewriter

Pure-function rewriter for github.com, gitlab.com, and bitbucket.org
blob URLs. Unknown hosts and already-raw URLs pass through. Called by
OpenApiAdapter before every spec fetch so agents can paste the URL
the user gave them without knowing each forge's raw-content convention.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Swagger 2.0 → OpenAPI 3.0 converter — root structure

**Files:**
- Create: `src/libs/colmena/src/web/application/swagger2_to_oas3.rs`
- Modify: `src/libs/colmena/src/web/application/mod.rs`

Pure-Rust JSON-tree transformation. No network, no external tools. This task covers the "root envelope" rewrites; Task 3 covers per-operation rewrites (body / formData / collectionFormat).

The spec mapping table covered here:

| Swagger 2.0 | OpenAPI 3.0.3 |
|---|---|
| `"swagger": "2.0"` | `"openapi": "3.0.3"` |
| `host` + `basePath` + `schemes[]` | `servers: [{ url: "{scheme}://{host}{basePath}" }]` one entry per scheme |
| `definitions` | `components.schemas` |
| `parameters` (global) | `components.parameters` (refs updated) |
| `responses` (global) | `components.responses` |
| `securityDefinitions` | `components.securitySchemes` (**flattened: basic/apiKey/oauth2 types adjusted in Step 5**) |
| `$ref: "#/definitions/X"` | `$ref: "#/components/schemas/X"` (all occurrences) |
| `$ref: "#/parameters/X"` | `$ref: "#/components/parameters/X"` |
| `$ref: "#/responses/X"` | `$ref: "#/components/responses/X"` |

- [ ] **Step 1: Write the failing test — minimal valid Swagger 2.0 document**

Create `src/libs/colmena/src/web/application/swagger2_to_oas3.rs`:

```rust
//! Convert a Swagger 2.0 JSON/YAML tree to OpenAPI 3.0.3.
//!
//! Operates on `serde_json::Value` — if the input was YAML, the adapter
//! has already round-tripped it through `serde_yaml` → `serde_json::Value`.
//!
//! The converter is **lossy on purpose** for features OpenAPI 3.0 cannot
//! represent (e.g. `collectionFormat: tsv`): those cases raise an error
//! rather than silently degrading so the agent sees a clear failure.

use crate::web::domain::WebDomainError;
use serde_json::{json, Map, Value};

/// Convert a Swagger 2.0 `serde_json::Value` into an OpenAPI 3.0.3 `Value`.
///
/// Returns `Err(WebDomainError::Swagger2ConversionFailed)` if the document
/// uses a feature with no OAS 3.0 equivalent (e.g. `collectionFormat: tsv`).
pub fn convert_swagger2_to_openapi3(input: &Value) -> Result<Value, WebDomainError> {
    let root = input.as_object().ok_or_else(|| {
        WebDomainError::Swagger2ConversionFailed {
            reason: "root is not a JSON object".into(),
            unsupported_feature: None,
        }
    })?;

    let mut out = Map::new();
    out.insert("openapi".into(), Value::String("3.0.3".into()));

    if let Some(info) = root.get("info") {
        out.insert("info".into(), info.clone());
    } else {
        return Err(WebDomainError::Swagger2ConversionFailed {
            reason: "missing required `info` block".into(),
            unsupported_feature: None,
        });
    }

    // servers: one per scheme × (host + basePath)
    let servers = build_servers(root);
    if !servers.is_empty() {
        out.insert("servers".into(), Value::Array(servers));
    }

    // paths (deep-copied then ref-rewritten; operation-body conversion lives in Task 3)
    let paths = root.get("paths").cloned().unwrap_or(json!({}));
    let paths = rewrite_refs_recursive(paths);
    out.insert("paths".into(), paths);

    // components
    let mut components = Map::new();
    if let Some(defs) = root.get("definitions") {
        components.insert("schemas".into(), rewrite_refs_recursive(defs.clone()));
    }
    if let Some(global_params) = root.get("parameters") {
        components.insert("parameters".into(), rewrite_refs_recursive(global_params.clone()));
    }
    if let Some(global_responses) = root.get("responses") {
        components.insert("responses".into(), rewrite_refs_recursive(global_responses.clone()));
    }
    if let Some(sec_defs) = root.get("securityDefinitions") {
        components.insert("securitySchemes".into(), convert_security_definitions(sec_defs)?);
    }
    if !components.is_empty() {
        out.insert("components".into(), Value::Object(components));
    }

    // Security requirements at the root are shaped the same in 2.0 and 3.0.
    if let Some(sec) = root.get("security") {
        out.insert("security".into(), sec.clone());
    }

    // Tags are compatible.
    if let Some(tags) = root.get("tags") {
        out.insert("tags".into(), tags.clone());
    }

    Ok(Value::Object(out))
}

fn build_servers(root: &Map<String, Value>) -> Vec<Value> {
    let host = root.get("host").and_then(|v| v.as_str()).unwrap_or("");
    let base_path = root.get("basePath").and_then(|v| v.as_str()).unwrap_or("");
    let schemes: Vec<String> = root
        .get("schemes")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
        .unwrap_or_else(|| vec!["https".into()]);

    if host.is_empty() {
        return Vec::new();
    }

    schemes
        .into_iter()
        .map(|s| json!({ "url": format!("{s}://{host}{base_path}") }))
        .collect()
}

fn convert_security_definitions(sec_defs: &Value) -> Result<Value, WebDomainError> {
    let map = sec_defs.as_object().ok_or_else(|| {
        WebDomainError::Swagger2ConversionFailed {
            reason: "securityDefinitions is not an object".into(),
            unsupported_feature: None,
        }
    })?;

    let mut out = Map::new();
    for (name, scheme_val) in map {
        let scheme = scheme_val.as_object().ok_or_else(|| {
            WebDomainError::Swagger2ConversionFailed {
                reason: format!("security scheme '{name}' is not an object"),
                unsupported_feature: None,
            }
        })?;

        let ty = scheme.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let mut converted = Map::new();
        match ty {
            "basic" => {
                // 2.0 `type: basic` → 3.0 `type: http, scheme: basic`
                converted.insert("type".into(), Value::String("http".into()));
                converted.insert("scheme".into(), Value::String("basic".into()));
            }
            "apiKey" => {
                // 2.0 apiKey (in: header|query) → 3.0 identical shape.
                converted.insert("type".into(), Value::String("apiKey".into()));
                if let Some(v) = scheme.get("name") {
                    converted.insert("name".into(), v.clone());
                }
                if let Some(v) = scheme.get("in") {
                    converted.insert("in".into(), v.clone());
                }
            }
            "oauth2" => {
                // 2.0 flow-variants → 3.0 flows map. Minimal round-trip: copy
                // the fields, rename the flow.
                converted.insert("type".into(), Value::String("oauth2".into()));
                let flow_name = scheme
                    .get("flow")
                    .and_then(|v| v.as_str())
                    .unwrap_or("implicit");
                let mut flow_body = Map::new();
                for k in &["authorizationUrl", "tokenUrl", "refreshUrl", "scopes"] {
                    if let Some(v) = scheme.get(*k) {
                        flow_body.insert((*k).into(), v.clone());
                    }
                }
                let oas3_flow = match flow_name {
                    "implicit" => "implicit",
                    "password" => "password",
                    "application" => "clientCredentials",
                    "accessCode" => "authorizationCode",
                    other => {
                        return Err(WebDomainError::Swagger2ConversionFailed {
                            reason: format!("unknown oauth2 flow: {other}"),
                            unsupported_feature: Some("oauth2.flow".into()),
                        });
                    }
                };
                let mut flows = Map::new();
                flows.insert(oas3_flow.into(), Value::Object(flow_body));
                converted.insert("flows".into(), Value::Object(flows));
            }
            other => {
                return Err(WebDomainError::Swagger2ConversionFailed {
                    reason: format!("unsupported security scheme type: {other}"),
                    unsupported_feature: Some(format!("securityDefinitions.{name}.type")),
                });
            }
        }
        // `description` carries over for all types.
        if let Some(desc) = scheme.get("description") {
            converted.insert("description".into(), desc.clone());
        }
        out.insert(name.clone(), Value::Object(converted));
    }
    Ok(Value::Object(out))
}

/// Walk a JSON tree and rewrite `$ref` strings from 2.0 layout to 3.0 layout.
///
/// - `#/definitions/X`  → `#/components/schemas/X`
/// - `#/parameters/X`   → `#/components/parameters/X`
/// - `#/responses/X`    → `#/components/responses/X`
pub fn rewrite_refs_recursive(v: Value) -> Value {
    match v {
        Value::Object(mut map) => {
            if let Some(Value::String(r)) = map.get("$ref").cloned() {
                let rewritten = rewrite_single_ref(&r);
                map.insert("$ref".into(), Value::String(rewritten));
            }
            let keys: Vec<String> = map.keys().cloned().collect();
            for k in keys {
                if k == "$ref" {
                    continue;
                }
                if let Some(child) = map.remove(&k) {
                    map.insert(k, rewrite_refs_recursive(child));
                }
            }
            Value::Object(map)
        }
        Value::Array(arr) => {
            Value::Array(arr.into_iter().map(rewrite_refs_recursive).collect())
        }
        other => other,
    }
}

fn rewrite_single_ref(r: &str) -> String {
    if let Some(rest) = r.strip_prefix("#/definitions/") {
        format!("#/components/schemas/{rest}")
    } else if let Some(rest) = r.strip_prefix("#/parameters/") {
        format!("#/components/parameters/{rest}")
    } else if let Some(rest) = r.strip_prefix("#/responses/") {
        format!("#/components/responses/{rest}")
    } else {
        r.to_string()
    }
}

#[cfg(test)]
mod tests_root {
    use super::*;
    use serde_json::json;

    #[test]
    fn minimal_swagger2_produces_openapi_303() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "host": "api.example.com",
            "basePath": "/v1",
            "schemes": ["https"],
            "paths": {}
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert_eq!(out["openapi"], "3.0.3");
        assert_eq!(out["info"]["title"], "T");
        assert_eq!(out["servers"][0]["url"], "https://api.example.com/v1");
    }

    #[test]
    fn multiple_schemes_produce_multiple_servers() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "host": "api.example.com",
            "basePath": "",
            "schemes": ["https", "http"],
            "paths": {}
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert_eq!(out["servers"].as_array().unwrap().len(), 2);
        assert_eq!(out["servers"][0]["url"], "https://api.example.com");
        assert_eq!(out["servers"][1]["url"], "http://api.example.com");
    }

    #[test]
    fn missing_schemes_defaults_to_https() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "host": "api.example.com",
            "paths": {}
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert_eq!(out["servers"][0]["url"], "https://api.example.com");
    }

    #[test]
    fn missing_host_produces_no_servers() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {}
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert!(out.get("servers").is_none());
    }

    #[test]
    fn missing_info_is_conversion_error() {
        let input = json!({ "swagger": "2.0", "paths": {} });
        let err = convert_swagger2_to_openapi3(&input).unwrap_err();
        match err {
            WebDomainError::Swagger2ConversionFailed { reason, .. } => {
                assert!(reason.contains("info"));
            }
            other => panic!("expected Swagger2ConversionFailed, got {other:?}"),
        }
    }

    #[test]
    fn definitions_move_to_components_schemas() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {},
            "definitions": {
                "Pet": { "type": "object", "properties": { "id": { "type": "integer" } } }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert_eq!(
            out["components"]["schemas"]["Pet"]["type"],
            "object"
        );
    }

    #[test]
    fn refs_to_definitions_are_rewritten() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/pet": {
                    "get": {
                        "responses": {
                            "200": { "schema": { "$ref": "#/definitions/Pet" } }
                        }
                    }
                }
            },
            "definitions": { "Pet": { "type": "object" } }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let r = &out["paths"]["/pet"]["get"]["responses"]["200"]["schema"]["$ref"];
        assert_eq!(r, "#/components/schemas/Pet");
    }

    #[test]
    fn security_definitions_basic_becomes_http_basic() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {},
            "securityDefinitions": {
                "BasicAuth": { "type": "basic" }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert_eq!(out["components"]["securitySchemes"]["BasicAuth"]["type"], "http");
        assert_eq!(out["components"]["securitySchemes"]["BasicAuth"]["scheme"], "basic");
    }

    #[test]
    fn security_definitions_apikey_roundtrips() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {},
            "securityDefinitions": {
                "KeyHeader": { "type": "apiKey", "in": "header", "name": "X-API-Key" }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let ks = &out["components"]["securitySchemes"]["KeyHeader"];
        assert_eq!(ks["type"], "apiKey");
        assert_eq!(ks["in"], "header");
        assert_eq!(ks["name"], "X-API-Key");
    }

    #[test]
    fn security_definitions_oauth2_accesscode_becomes_authorization_code() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {},
            "securityDefinitions": {
                "OA": {
                    "type": "oauth2",
                    "flow": "accessCode",
                    "authorizationUrl": "https://example.com/oauth/authorize",
                    "tokenUrl": "https://example.com/oauth/token",
                    "scopes": { "read": "Read access" }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let oa = &out["components"]["securitySchemes"]["OA"];
        assert_eq!(oa["type"], "oauth2");
        assert!(oa["flows"]["authorizationCode"].is_object());
        assert_eq!(
            oa["flows"]["authorizationCode"]["tokenUrl"],
            "https://example.com/oauth/token"
        );
    }

    #[test]
    fn global_parameters_move_to_components_parameters() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {},
            "parameters": {
                "SkipParam": { "name": "skip", "in": "query", "type": "integer" }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert_eq!(
            out["components"]["parameters"]["SkipParam"]["name"],
            "skip"
        );
    }

    #[test]
    fn tags_carry_over() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {},
            "tags": [{ "name": "pets" }, { "name": "users" }]
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert_eq!(out["tags"][0]["name"], "pets");
        assert_eq!(out["tags"][1]["name"], "users");
    }
}
```

> **Note:** `WebDomainError::Swagger2ConversionFailed` is added next step if Plan 0 didn't already include it.

- [ ] **Step 2: Ensure `WebDomainError::Swagger2ConversionFailed` exists**

Open `src/libs/colmena/src/web/domain/errors.rs`. If the variant is not already present (Plan 0 introduced the enum but the exact list may differ), add this variant inside `pub enum WebDomainError { ... }`:

```rust
    /// A Swagger 2.0 document could not be converted to OpenAPI 3.0.3.
    /// The `unsupported_feature` field pinpoints the bit that tripped us up.
    #[error("swagger 2.0 conversion failed: {reason}")]
    Swagger2ConversionFailed {
        reason: String,
        unsupported_feature: Option<String>,
    },
```

And inside `impl WebDomainError { pub fn is_llm_recoverable(&self) -> bool { match self { ... } } }`, add:

```rust
            Self::Swagger2ConversionFailed { .. } => true,
```

(The LLM can recover by falling back to `tavily_client` for that spec.)

If Plan 0 used a slightly different shape (e.g., `SpecParseError { kind, ... }` that could subsume this), align by using the existing variant instead and adjust `reason` / `unsupported_feature` callers accordingly; do not introduce duplication. The key requirement is that the converter has a typed "this document has no 3.0 equivalent" failure mode distinct from `InvalidConfig` or `Upstream`.

- [ ] **Step 3: Register the module**

Edit `src/libs/colmena/src/web/application/mod.rs`. Append:

```rust
pub mod swagger2_to_oas3;
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib web::application::swagger2_to_oas3::tests_root`
Expected: 13 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/web/application/swagger2_to_oas3.rs \
        src/libs/colmena/src/web/application/mod.rs \
        src/libs/colmena/src/web/domain/errors.rs
git commit -m "$(cat <<'EOF'
feat(web): Swagger 2.0 → OpenAPI 3.0 converter — root structure

Converts info, servers (host/basePath/schemes), definitions,
global parameters/responses, and securityDefinitions. Rewrites
#/definitions/X refs to #/components/schemas/X across the tree.
Adds Swagger2ConversionFailed error variant (LLM-recoverable) to
name features the 3.0 spec cannot represent. Operation-body and
formData conversion comes in the next task.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Swagger 2.0 → OpenAPI 3.0 converter — operation bodies + formData + collectionFormat

**Files:**
- Modify: `src/libs/colmena/src/web/application/swagger2_to_oas3.rs`

Covers:

| Swagger 2.0 | OpenAPI 3.0.3 |
|---|---|
| Operation-level `consumes`, `produces` | `requestBody.content.<type>` / `responses.<code>.content.<type>` |
| `in: body` parameter with `schema` | `requestBody: { content: { <consume>: { schema } } }` |
| `in: formData` parameters | `requestBody` with `application/x-www-form-urlencoded` or `multipart/form-data` |
| `type: file` (formData) | `type: string, format: binary` |
| `collectionFormat: csv` | `style: form, explode: false` |
| `collectionFormat: multi` | `style: form, explode: true` |
| `collectionFormat: ssv` | `style: spaceDelimited` |
| `collectionFormat: pipes` | `style: pipeDelimited` |
| `collectionFormat: tsv` | **ERROR** — `Swagger2ConversionFailed`, unsupported_feature="collectionFormat.tsv" |

- [ ] **Step 1: Write the failing tests**

Append to `src/libs/colmena/src/web/application/swagger2_to_oas3.rs` after the existing `tests_root` module:

```rust
#[cfg(test)]
mod tests_operations {
    use super::*;
    use serde_json::json;

    fn petstore_post_with_body() -> Value {
        json!({
            "swagger": "2.0",
            "info": { "title": "Pet", "version": "1" },
            "host": "api.example.com",
            "basePath": "/v2",
            "schemes": ["https"],
            "consumes": ["application/json"],
            "produces": ["application/json"],
            "paths": {
                "/pet": {
                    "post": {
                        "operationId": "addPet",
                        "parameters": [
                            {
                                "in": "body",
                                "name": "body",
                                "required": true,
                                "schema": { "$ref": "#/definitions/Pet" }
                            }
                        ],
                        "responses": {
                            "200": { "description": "ok", "schema": { "$ref": "#/definitions/Pet" } }
                        }
                    }
                }
            },
            "definitions": {
                "Pet": { "type": "object", "properties": { "id": { "type": "integer" } } }
            }
        })
    }

    #[test]
    fn body_param_becomes_request_body() {
        let out = convert_swagger2_to_openapi3(&petstore_post_with_body()).unwrap();
        let op = &out["paths"]["/pet"]["post"];
        assert!(op["parameters"].as_array().map(|a| a.is_empty()).unwrap_or(true));
        let rb = &op["requestBody"];
        assert_eq!(rb["required"], true);
        assert_eq!(
            rb["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/Pet"
        );
    }

    #[test]
    fn response_schema_becomes_content_schema() {
        let out = convert_swagger2_to_openapi3(&petstore_post_with_body()).unwrap();
        let resp = &out["paths"]["/pet"]["post"]["responses"]["200"];
        assert!(resp["schema"].is_null() || resp.get("schema").is_none());
        assert_eq!(
            resp["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/Pet"
        );
        assert_eq!(resp["description"], "ok");
    }

    #[test]
    fn formdata_becomes_urlencoded_request_body() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/login": {
                    "post": {
                        "parameters": [
                            { "in": "formData", "name": "user",     "type": "string", "required": true },
                            { "in": "formData", "name": "password", "type": "string", "required": true }
                        ],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let rb = &out["paths"]["/login"]["post"]["requestBody"];
        let schema = &rb["content"]["application/x-www-form-urlencoded"]["schema"];
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["user"].is_object());
        assert!(schema["properties"]["password"].is_object());
        let req = schema["required"].as_array().unwrap();
        assert!(req.iter().any(|v| v == "user"));
        assert!(req.iter().any(|v| v == "password"));
    }

    #[test]
    fn formdata_with_file_becomes_multipart() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/upload": {
                    "post": {
                        "parameters": [
                            { "in": "formData", "name": "file", "type": "file", "required": true },
                            { "in": "formData", "name": "caption", "type": "string" }
                        ],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let rb = &out["paths"]["/upload"]["post"]["requestBody"];
        let schema = &rb["content"]["multipart/form-data"]["schema"];
        assert_eq!(schema["properties"]["file"]["type"], "string");
        assert_eq!(schema["properties"]["file"]["format"], "binary");
        assert_eq!(schema["properties"]["caption"]["type"], "string");
    }

    #[test]
    fn collection_format_csv_becomes_form_no_explode() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/things": {
                    "get": {
                        "parameters": [{
                            "in": "query", "name": "ids",
                            "type": "array", "items": { "type": "string" },
                            "collectionFormat": "csv"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let p = &out["paths"]["/things"]["get"]["parameters"][0];
        assert_eq!(p["style"], "form");
        assert_eq!(p["explode"], false);
        assert!(p.get("collectionFormat").is_none());
    }

    #[test]
    fn collection_format_multi_becomes_form_explode_true() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/things": {
                    "get": {
                        "parameters": [{
                            "in": "query", "name": "ids",
                            "type": "array", "items": { "type": "string" },
                            "collectionFormat": "multi"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let p = &out["paths"]["/things"]["get"]["parameters"][0];
        assert_eq!(p["style"], "form");
        assert_eq!(p["explode"], true);
    }

    #[test]
    fn collection_format_ssv_becomes_space_delimited() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/things": {
                    "get": {
                        "parameters": [{
                            "in": "query", "name": "ids",
                            "type": "array", "items": { "type": "string" },
                            "collectionFormat": "ssv"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let p = &out["paths"]["/things"]["get"]["parameters"][0];
        assert_eq!(p["style"], "spaceDelimited");
    }

    #[test]
    fn collection_format_pipes_becomes_pipe_delimited() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/things": {
                    "get": {
                        "parameters": [{
                            "in": "query", "name": "ids",
                            "type": "array", "items": { "type": "string" },
                            "collectionFormat": "pipes"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let p = &out["paths"]["/things"]["get"]["parameters"][0];
        assert_eq!(p["style"], "pipeDelimited");
    }

    #[test]
    fn collection_format_tsv_is_conversion_error() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/things": {
                    "get": {
                        "parameters": [{
                            "in": "query", "name": "ids",
                            "type": "array", "items": { "type": "string" },
                            "collectionFormat": "tsv"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let err = convert_swagger2_to_openapi3(&input).unwrap_err();
        match err {
            WebDomainError::Swagger2ConversionFailed { unsupported_feature, .. } => {
                assert_eq!(unsupported_feature.as_deref(), Some("collectionFormat.tsv"));
            }
            other => panic!("expected Swagger2ConversionFailed, got {other:?}"),
        }
    }

    #[test]
    fn operation_consumes_overrides_global_consumes() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "consumes": ["application/json"],
            "paths": {
                "/form": {
                    "post": {
                        "consumes": ["application/x-www-form-urlencoded"],
                        "parameters": [
                            { "in": "formData", "name": "x", "type": "string", "required": true }
                        ],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let rb = &out["paths"]["/form"]["post"]["requestBody"];
        assert!(rb["content"]["application/x-www-form-urlencoded"].is_object());
        assert!(rb.get("content").and_then(|c| c.get("application/json")).is_none());
    }
}
```

- [ ] **Step 2: Run and watch them FAIL**

Run: `cargo test --lib web::application::swagger2_to_oas3::tests_operations`
Expected: all 10 tests FAIL with assertion errors (the operation converter is not yet wired in).

- [ ] **Step 3: Wire the operation-level converter into `convert_swagger2_to_openapi3`**

Edit `convert_swagger2_to_openapi3` in the same file. Replace this line:

```rust
    let paths = root.get("paths").cloned().unwrap_or(json!({}));
    let paths = rewrite_refs_recursive(paths);
    out.insert("paths".into(), paths);
```

with:

```rust
    let global_consumes = root
        .get("consumes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let global_produces = root
        .get("produces")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let paths = root.get("paths").cloned().unwrap_or(json!({}));
    let paths = rewrite_refs_recursive(paths);
    let paths = convert_operations(paths, &global_consumes, &global_produces)?;
    out.insert("paths".into(), paths);
```

Then append the following helper functions to the module (just before `#[cfg(test)] mod tests_root`):

```rust
/// Walk each path → method → operation and rewrite 2.0-isms.
fn convert_operations(
    paths: Value,
    global_consumes: &[Value],
    global_produces: &[Value],
) -> Result<Value, WebDomainError> {
    let Value::Object(mut path_map) = paths else {
        return Ok(paths);
    };
    let path_keys: Vec<String> = path_map.keys().cloned().collect();
    for path_key in path_keys {
        let Some(path_item) = path_map.get_mut(&path_key) else { continue };
        let Value::Object(item_map) = path_item else { continue };
        let method_keys: Vec<String> = item_map.keys().cloned().collect();
        for method_key in method_keys {
            if !is_http_method(&method_key) {
                continue;
            }
            let Some(op_val) = item_map.get_mut(&method_key) else { continue };
            let Value::Object(op) = op_val else { continue };
            convert_single_operation(op, global_consumes, global_produces)?;
        }
    }
    Ok(Value::Object(path_map))
}

fn is_http_method(k: &str) -> bool {
    matches!(
        k.to_ascii_lowercase().as_str(),
        "get" | "put" | "post" | "delete" | "options" | "head" | "patch" | "trace"
    )
}

fn convert_single_operation(
    op: &mut Map<String, Value>,
    global_consumes: &[Value],
    global_produces: &[Value],
) -> Result<(), WebDomainError> {
    // Determine effective consumes / produces.
    let consumes: Vec<Value> = op
        .get("consumes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_else(|| global_consumes.to_vec());
    let produces: Vec<Value> = op
        .get("produces")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_else(|| global_produces.to_vec());
    op.remove("consumes");
    op.remove("produces");

    // Split parameters: body + formData vs the rest.
    let mut new_params: Vec<Value> = Vec::new();
    let mut body_param: Option<Value> = None;
    let mut form_data: Vec<Value> = Vec::new();

    if let Some(Value::Array(params)) = op.remove("parameters") {
        for p in params {
            let Value::Object(mut p_obj) = p else {
                new_params.push(p);
                continue;
            };
            let kind = p_obj.get("in").and_then(|v| v.as_str()).unwrap_or("");
            match kind {
                "body" => body_param = Some(Value::Object(p_obj)),
                "formData" => form_data.push(Value::Object(p_obj)),
                _ => {
                    convert_param_collection_format(&mut p_obj)?;
                    new_params.push(Value::Object(p_obj));
                }
            }
        }
    }

    if !new_params.is_empty() {
        op.insert("parameters".into(), Value::Array(new_params));
    }

    // body → requestBody
    if let Some(Value::Object(mut bp)) = body_param {
        let required = bp
            .remove("required")
            .unwrap_or(Value::Bool(false));
        let description = bp.remove("description");
        let schema = bp.remove("schema").unwrap_or(json!({}));

        let content_type = pick_first_content_type(&consumes, "application/json");
        let mut content = Map::new();
        let mut media = Map::new();
        media.insert("schema".into(), schema);
        content.insert(content_type, Value::Object(media));

        let mut rb = Map::new();
        rb.insert("required".into(), required);
        if let Some(d) = description {
            rb.insert("description".into(), d);
        }
        rb.insert("content".into(), Value::Object(content));
        op.insert("requestBody".into(), Value::Object(rb));
    }

    // formData → requestBody (urlencoded or multipart)
    if !form_data.is_empty() {
        let has_file = form_data.iter().any(|p| {
            p.get("type").and_then(|v| v.as_str()) == Some("file")
        });
        let media_type = if has_file {
            "multipart/form-data"
        } else {
            // Honor the operation's consumes if it names form-urlencoded; otherwise default.
            pick_first_consume_for_form(&consumes).unwrap_or("application/x-www-form-urlencoded")
        }
        .to_string();

        let mut properties = Map::new();
        let mut required_names: Vec<Value> = Vec::new();

        for p in form_data {
            let Value::Object(p_obj) = p else { continue };
            let Some(name) = p_obj.get("name").and_then(|v| v.as_str()).map(String::from) else {
                continue;
            };
            let mut prop = Map::new();
            if p_obj.get("type").and_then(|v| v.as_str()) == Some("file") {
                prop.insert("type".into(), Value::String("string".into()));
                prop.insert("format".into(), Value::String("binary".into()));
            } else if let Some(ty) = p_obj.get("type") {
                prop.insert("type".into(), ty.clone());
                if let Some(fmt) = p_obj.get("format") {
                    prop.insert("format".into(), fmt.clone());
                }
                if let Some(items) = p_obj.get("items") {
                    prop.insert("items".into(), items.clone());
                }
                if let Some(en) = p_obj.get("enum") {
                    prop.insert("enum".into(), en.clone());
                }
            }
            if let Some(desc) = p_obj.get("description") {
                prop.insert("description".into(), desc.clone());
            }
            if p_obj.get("required").and_then(|v| v.as_bool()).unwrap_or(false) {
                required_names.push(Value::String(name.clone()));
            }
            properties.insert(name, Value::Object(prop));
        }

        let mut schema = Map::new();
        schema.insert("type".into(), Value::String("object".into()));
        schema.insert("properties".into(), Value::Object(properties));
        if !required_names.is_empty() {
            schema.insert("required".into(), Value::Array(required_names));
        }

        let mut media = Map::new();
        media.insert("schema".into(), Value::Object(schema));

        let mut content = Map::new();
        content.insert(media_type, Value::Object(media));

        let mut rb = Map::new();
        rb.insert("required".into(), Value::Bool(true));
        rb.insert("content".into(), Value::Object(content));
        op.insert("requestBody".into(), Value::Object(rb));
    }

    // responses.<code>.schema → responses.<code>.content.<produce>.schema
    if let Some(Value::Object(ref mut responses)) = op.get_mut("responses") {
        let codes: Vec<String> = responses.keys().cloned().collect();
        for code in codes {
            let Some(Value::Object(ref mut resp)) = responses.get_mut(&code) else {
                continue;
            };
            if let Some(schema) = resp.remove("schema") {
                let content_type = pick_first_content_type(&produces, "application/json");
                let mut media = Map::new();
                media.insert("schema".into(), schema);
                let mut content = Map::new();
                content.insert(content_type, Value::Object(media));
                resp.insert("content".into(), Value::Object(content));
            }
        }
    }

    Ok(())
}

fn pick_first_content_type(list: &[Value], default: &str) -> String {
    list.iter()
        .find_map(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| default.to_string())
}

fn pick_first_consume_for_form(list: &[Value]) -> Option<&'static str> {
    for v in list {
        match v.as_str() {
            Some("application/x-www-form-urlencoded") => return Some("application/x-www-form-urlencoded"),
            Some("multipart/form-data") => return Some("multipart/form-data"),
            _ => {}
        }
    }
    None
}

/// Translate Swagger 2.0 `collectionFormat` on a parameter to OpenAPI 3.0
/// `style` + `explode`. Errors on `tsv` (no 3.0 equivalent).
fn convert_param_collection_format(p: &mut Map<String, Value>) -> Result<(), WebDomainError> {
    let Some(Value::String(cf)) = p.remove("collectionFormat") else {
        return Ok(());
    };
    match cf.as_str() {
        "csv" => {
            p.insert("style".into(), Value::String("form".into()));
            p.insert("explode".into(), Value::Bool(false));
        }
        "multi" => {
            p.insert("style".into(), Value::String("form".into()));
            p.insert("explode".into(), Value::Bool(true));
        }
        "ssv" => {
            p.insert("style".into(), Value::String("spaceDelimited".into()));
        }
        "pipes" => {
            p.insert("style".into(), Value::String("pipeDelimited".into()));
        }
        "tsv" => {
            return Err(WebDomainError::Swagger2ConversionFailed {
                reason: "collectionFormat: tsv has no OpenAPI 3.0 equivalent".into(),
                unsupported_feature: Some("collectionFormat.tsv".into()),
            });
        }
        other => {
            return Err(WebDomainError::Swagger2ConversionFailed {
                reason: format!("unknown collectionFormat: {other}"),
                unsupported_feature: Some(format!("collectionFormat.{other}")),
            });
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib web::application::swagger2_to_oas3`
Expected: all tests in `tests_root` (13) and `tests_operations` (10) pass — 23 total.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/web/application/swagger2_to_oas3.rs
git commit -m "$(cat <<'EOF'
feat(web): Swagger 2.0 → OpenAPI 3.0 converter — operations

Converts body parameters to requestBody (using consumes for content
type), formData parameters to application/x-www-form-urlencoded or
multipart/form-data (when any file is present), response schemas to
content.<produce>.schema, and maps collectionFormat (csv, multi, ssv,
pipes) to style+explode. tsv is rejected as Swagger2ConversionFailed.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Domain — `ApiSpecPort` trait + `ParsedSpec` value objects

**Files:**
- Create: `src/libs/colmena/src/web/domain/api_spec_port.rs`
- Modify: `src/libs/colmena/src/web/domain/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `src/libs/colmena/src/web/domain/api_spec_port.rs`:

```rust
//! `ApiSpecPort` — fetch-and-parse contract for OpenAPI 3.x / Swagger 2.0 specs.
//!
//! The port returns a format-normalized `ParsedSpec` domain value (the
//! adapter converts Swagger 2.0 internally). Domain code never sees raw
//! `oas3` or `serde_yaml` types.

use crate::web::domain::errors::WebDomainError;
use async_trait::async_trait;
use std::collections::HashMap;

#[async_trait]
pub trait ApiSpecPort: Send + Sync {
    /// Fetch the spec at `url`, revalidating against the optional cached
    /// ETag (or Last-Modified, if the adapter stored one) using a
    /// conditional GET. Returns `NotModified` when the server answered 304.
    async fn fetch_and_parse(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<SpecFetchResult, WebDomainError>;
}

#[derive(Debug, Clone)]
pub enum SpecFetchResult {
    Fresh {
        spec: ParsedSpec,
        etag: Option<String>,
        last_modified: Option<String>,
    },
    NotModified,
}

#[derive(Debug, Clone)]
pub struct ParsedSpec {
    /// URL actually fetched (post-normalization).
    pub resolved_url: String,
    /// URL as given by the agent (pre-normalization) — useful for error reporting.
    pub input_url: String,
    pub original_format: SpecFormat,
    pub internal_format: String, // always "openapi-3.x.y"
    pub title: String,
    pub version: String,
    pub description: Option<String>,
    pub servers: Vec<String>,
    pub endpoints: Vec<Endpoint>,
    pub security_schemes: HashMap<String, SecurityScheme>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecFormat {
    OpenApi3x,
    Swagger20,
}

impl SpecFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenApi3x => "openapi-3.x",
            Self::Swagger20 => "swagger-2.0",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Copy)]
pub enum HttpMethod {
    Get,
    Put,
    Post,
    Delete,
    Options,
    Head,
    Patch,
    Trace,
}

impl HttpMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
            Self::Post => "POST",
            Self::Delete => "DELETE",
            Self::Options => "OPTIONS",
            Self::Head => "HEAD",
            Self::Patch => "PATCH",
            Self::Trace => "TRACE",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "GET" => Some(Self::Get),
            "PUT" => Some(Self::Put),
            "POST" => Some(Self::Post),
            "DELETE" => Some(Self::Delete),
            "OPTIONS" => Some(Self::Options),
            "HEAD" => Some(Self::Head),
            "PATCH" => Some(Self::Patch),
            "TRACE" => Some(Self::Trace),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub operation_id: String,
    pub method: HttpMethod,
    pub path: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub path_params: Vec<ParameterSpec>,
    pub query_params: Vec<ParameterSpec>,
    pub header_params: Vec<ParameterSpec>,
    pub request_body: Option<RequestBodySpec>,
    pub responses: HashMap<String, ResponseSpec>,
    pub security: Vec<SecurityRequirement>,
}

#[derive(Debug, Clone)]
pub struct ParameterSpec {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
    pub param_type: ParamType,
    /// e.g. `form`, `spaceDelimited`, `pipeDelimited`. None for non-array params.
    pub style: Option<String>,
    pub explode: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamType {
    String,
    Integer,
    Number,
    Boolean,
    Array(Box<ParamType>),
    /// Shape may be opaque (`$ref` not inlined). We accept it and leave
    /// validation of structure to the user / server.
    Object,
    /// Unknown / unrecognized — treated as opaque string for HTTP purposes.
    Unknown,
}

#[derive(Debug, Clone)]
pub struct RequestBodySpec {
    /// Media type to use when encoding the body.
    pub content_type: String,
    pub required: bool,
    /// Raw JSON-Schema-ish object describing the body; preserved verbatim
    /// and exposed to the LLM so it can see `properties` / `required`.
    pub schema: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ResponseSpec {
    pub description: Option<String>,
    /// Map content_type → schema (verbatim JSON). Empty if no schema declared.
    pub content: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyLocation {
    Header,
    Query,
    Cookie,
}

#[derive(Debug, Clone)]
pub enum SecurityScheme {
    /// `type: http` — `scheme: bearer | basic | ...`
    Http {
        scheme: String,
        bearer_format: Option<String>,
    },
    ApiKey {
        name: String,
        location: ApiKeyLocation,
    },
    /// OAuth2 — Colmena expects a pre-obtained token supplied via a
    /// Secure Value reference; the scheme metadata is preserved for the
    /// LLM's benefit.
    OAuth2 {
        /// Flow name → metadata (auth/token URLs + scopes). Verbatim from spec.
        flows: serde_json::Value,
    },
    /// OpenID Connect — same contract as OAuth2 from the build-request POV.
    OpenIdConnect {
        openid_connect_url: String,
    },
}

#[derive(Debug, Clone)]
pub struct SecurityRequirement {
    pub scheme: String,
    pub scopes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn http_method_roundtrips_all_verbs() {
        for m in [
            HttpMethod::Get,
            HttpMethod::Put,
            HttpMethod::Post,
            HttpMethod::Delete,
            HttpMethod::Options,
            HttpMethod::Head,
            HttpMethod::Patch,
            HttpMethod::Trace,
        ] {
            assert_eq!(HttpMethod::parse(m.as_str()), Some(m));
        }
    }

    #[test]
    fn http_method_parse_is_case_insensitive() {
        assert_eq!(HttpMethod::parse("get"), Some(HttpMethod::Get));
        assert_eq!(HttpMethod::parse("Post"), Some(HttpMethod::Post));
    }

    #[test]
    fn http_method_parse_rejects_garbage() {
        assert!(HttpMethod::parse("FOO").is_none());
    }

    #[test]
    fn spec_format_string_is_stable() {
        assert_eq!(SpecFormat::OpenApi3x.as_str(), "openapi-3.x");
        assert_eq!(SpecFormat::Swagger20.as_str(), "swagger-2.0");
    }

    #[test]
    fn parsed_spec_clone_is_deep() {
        let p = ParsedSpec {
            resolved_url: "a".into(),
            input_url: "b".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "T".into(),
            version: "1".into(),
            description: None,
            servers: vec!["https://x".into()],
            endpoints: Vec::new(),
            security_schemes: HashMap::new(),
            tags: Vec::new(),
        };
        let c = p.clone();
        assert_eq!(c.title, "T");
        assert_eq!(c.servers, vec!["https://x".to_string()]);
    }
}
```

- [ ] **Step 2: Register the module**

Edit `src/libs/colmena/src/web/domain/mod.rs`. Append:

```rust
pub mod api_spec_port;
pub use api_spec_port::{
    ApiKeyLocation, ApiSpecPort, Endpoint, HttpMethod, ParamType, ParameterSpec, ParsedSpec,
    RequestBodySpec, ResponseSpec, SecurityRequirement, SecurityScheme, SpecFetchResult, SpecFormat,
};
```

- [ ] **Step 3: Run — expect PASS**

Run: `cargo test --lib web::domain::api_spec_port`
Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/web/domain/api_spec_port.rs src/libs/colmena/src/web/domain/mod.rs
git commit -m "$(cat <<'EOF'
feat(web): add ApiSpecPort trait + ParsedSpec value objects

Domain contract for fetch-and-parse of OpenAPI 3.x / Swagger 2.0
specs. ParsedSpec is the format-normalized shape the rest of the
system sees; Swagger 2.0 conversion lives in the adapter and is
invisible to domain/application code.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: `OpenApiAdapter` — fetch stage (URL normalization, size/timeout limits, HTML detection)

**Files:**
- Create: `src/libs/colmena/src/web/infrastructure/openapi_adapter.rs`
- Modify: `src/libs/colmena/src/web/infrastructure/mod.rs`

This task covers the download pipeline only: normalize URL → reqwest GET with size/timeout limits → HTML-response detection. Parsing (OpenAPI 3.x and Swagger 2.0) and ETag revalidation land in Tasks 6 and 7.

- [ ] **Step 1: Write the failing tests**

Create `src/libs/colmena/src/web/infrastructure/openapi_adapter.rs`:

```rust
//! Reqwest-backed implementation of `ApiSpecPort`.
//!
//! Pipeline (see Spec C §Architecture):
//!   1. Normalize URL (Git-forge blob → raw).
//!   2. GET with streamed body; abort if size > cap.
//!   3. Detect HTML-instead-of-spec responses.
//!   4. Detect JSON vs YAML.
//!   5. Detect OpenAPI 3.x vs Swagger 2.0 by root keys.
//!   6. Convert Swagger 2.0 → OpenAPI 3.0 if needed.
//!   7. Parse via `oas3` → map to `ParsedSpec`.
//!   8. Honor ETag / Last-Modified (304).
//!
//! This module contains Tasks 5–7's incremental slices. Tests in this
//! file are structured by pipeline stage so later tasks can add more
//! without breaking earlier ones.

use crate::web::application::url_normalizer::{normalize_forge_url, NormalizedUrl};
use crate::web::domain::{ApiSpecPort, SpecFetchResult, WebDomainError};
use async_trait::async_trait;
use std::time::Duration;

/// Caps and limits shared across the pipeline stages.
#[derive(Debug, Clone)]
pub struct OpenApiAdapterConfig {
    pub max_bytes: u64,
    pub timeout: Duration,
}

impl Default for OpenApiAdapterConfig {
    fn default() -> Self {
        Self {
            max_bytes: 10 * 1024 * 1024,
            timeout: Duration::from_secs(60),
        }
    }
}

pub struct OpenApiAdapter {
    client: reqwest::Client,
    config: OpenApiAdapterConfig,
}

impl OpenApiAdapter {
    pub fn new(config: OpenApiAdapterConfig) -> Result<Self, WebDomainError> {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .user_agent("colmena-api-explorer/0.1")
            .build()
            .map_err(|e| WebDomainError::AdapterInit(format!("reqwest client init: {e}")))?;
        Ok(Self { client, config })
    }

    /// Lower-level fetch stage. Returns the raw bytes plus response
    /// metadata (content-type, ETag, Last-Modified, final URL). Used by
    /// Task 6/7's parsing layer.
    pub(crate) async fn fetch_raw(
        &self,
        input_url: &str,
        if_none_match: Option<&str>,
        if_modified_since: Option<&str>,
    ) -> Result<FetchRawResult, WebDomainError> {
        let NormalizedUrl { resolved, rewritten: _ } = normalize_forge_url(input_url);

        let mut req = self.client.get(&resolved);
        if let Some(etag) = if_none_match {
            req = req.header("If-None-Match", etag);
        }
        if let Some(lm) = if_modified_since {
            req = req.header("If-Modified-Since", lm);
        }

        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                WebDomainError::Timeout { ms: self.config.timeout.as_millis() as u64 }
            } else {
                WebDomainError::Upstream {
                    status: None,
                    message: format!("fetch error: {e}"),
                }
            }
        })?;

        let status = resp.status();
        if status.as_u16() == 304 {
            return Ok(FetchRawResult::NotModified);
        }
        if !status.is_success() {
            return Err(WebDomainError::Upstream {
                status: Some(status.as_u16()),
                message: format!("HTTP {} from {resolved}", status.as_u16()),
            });
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let etag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let last_modified = resp
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        // Reject obvious HTML before spending bytes on download.
        if let Some(ct) = &content_type {
            if ct.starts_with("text/html") {
                return Err(WebDomainError::UnexpectedHtmlResponse {
                    url_given: input_url.to_string(),
                    resolved_url: resolved.clone(),
                });
            }
        }

        // Stream body with size cap. reqwest's content-length hint is best-effort;
        // we count bytes as they arrive and abort past the cap.
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| WebDomainError::Upstream {
                status: None,
                message: format!("stream error: {e}"),
            })?;
            if (buf.len() as u64) + (chunk.len() as u64) > self.config.max_bytes {
                return Err(WebDomainError::SpecTooLarge {
                    size_bytes: buf.len() as u64 + chunk.len() as u64,
                    limit_bytes: self.config.max_bytes,
                });
            }
            buf.extend_from_slice(&chunk);
        }

        // If content-type was absent, sniff body: HTML bodies usually start with '<'.
        if content_type.is_none() {
            let first = buf
                .iter()
                .copied()
                .find(|b| !b.is_ascii_whitespace());
            if matches!(first, Some(b'<')) {
                return Err(WebDomainError::UnexpectedHtmlResponse {
                    url_given: input_url.to_string(),
                    resolved_url: resolved.clone(),
                });
            }
        }

        Ok(FetchRawResult::Fresh {
            body: buf,
            content_type,
            etag,
            last_modified,
            resolved_url: resolved,
        })
    }
}

pub(crate) enum FetchRawResult {
    Fresh {
        body: Vec<u8>,
        content_type: Option<String>,
        etag: Option<String>,
        last_modified: Option<String>,
        resolved_url: String,
    },
    NotModified,
}

/// Temporary `ApiSpecPort` impl — Task 6 replaces the body with real parsing.
#[async_trait]
impl ApiSpecPort for OpenApiAdapter {
    async fn fetch_and_parse(
        &self,
        _url: &str,
        _etag: Option<&str>,
        _last_modified: Option<&str>,
    ) -> Result<SpecFetchResult, WebDomainError> {
        Err(WebDomainError::AdapterInit(
            "OpenApiAdapter::fetch_and_parse not yet implemented — Task 6 wires it up".into(),
        ))
    }
}

#[cfg(test)]
mod tests_fetch {
    use super::*;
    use wiremock::matchers::{header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn adapter_at(config: OpenApiAdapterConfig) -> OpenApiAdapter {
        OpenApiAdapter::new(config).unwrap()
    }

    fn small_yaml() -> &'static str {
        "openapi: 3.0.3\ninfo:\n  title: T\n  version: '1'\npaths: {}\n"
    }

    #[tokio::test]
    async fn fetch_raw_returns_yaml_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/openapi.yaml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/yaml")
                    .insert_header("ETag", "\"abc\"")
                    .set_body_string(small_yaml()),
            )
            .mount(&server)
            .await;

        let url = format!("{}/openapi.yaml", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig::default());
        let res = adapter.fetch_raw(&url, None, None).await.unwrap();
        match res {
            FetchRawResult::Fresh { body, content_type, etag, .. } => {
                assert!(std::str::from_utf8(&body).unwrap().contains("openapi: 3.0.3"));
                assert_eq!(content_type.as_deref(), Some("application/yaml"));
                assert_eq!(etag.as_deref(), Some("\"abc\""));
            }
            FetchRawResult::NotModified => panic!("expected Fresh"),
        }
    }

    #[tokio::test]
    async fn fetch_raw_rejects_html_by_content_type() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "text/html; charset=utf-8")
                    .set_body_string("<!DOCTYPE html><html>..."),
            )
            .mount(&server)
            .await;

        let url = format!("{}/", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig::default());
        let err = adapter.fetch_raw(&url, None, None).await.unwrap_err();
        match err {
            WebDomainError::UnexpectedHtmlResponse { resolved_url, .. } => {
                assert_eq!(resolved_url, url);
            }
            other => panic!("expected UnexpectedHtmlResponse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_raw_rejects_html_body_when_content_type_missing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string("   <!DOCTYPE html>..."))
            .mount(&server)
            .await;

        let url = format!("{}/", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig::default());
        let err = adapter.fetch_raw(&url, None, None).await.unwrap_err();
        assert!(matches!(err, WebDomainError::UnexpectedHtmlResponse { .. }));
    }

    #[tokio::test]
    async fn fetch_raw_enforces_size_cap() {
        let server = MockServer::start().await;
        let big = "x".repeat(10_000);
        Mock::given(method("GET"))
            .and(path("/big.yaml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/yaml")
                    .set_body_string(big),
            )
            .mount(&server)
            .await;

        let url = format!("{}/big.yaml", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig {
            max_bytes: 1024,
            ..OpenApiAdapterConfig::default()
        });
        let err = adapter.fetch_raw(&url, None, None).await.unwrap_err();
        match err {
            WebDomainError::SpecTooLarge { limit_bytes, .. } => {
                assert_eq!(limit_bytes, 1024);
            }
            other => panic!("expected SpecTooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_raw_propagates_if_none_match() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/openapi.yaml"))
            .and(header_exists("If-None-Match"))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;

        let url = format!("{}/openapi.yaml", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig::default());
        let res = adapter
            .fetch_raw(&url, Some("\"abc\""), None)
            .await
            .unwrap();
        assert!(matches!(res, FetchRawResult::NotModified));
    }

    #[tokio::test]
    async fn fetch_raw_maps_500_to_upstream_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/openapi.yaml"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let url = format!("{}/openapi.yaml", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig::default());
        let err = adapter.fetch_raw(&url, None, None).await.unwrap_err();
        match err {
            WebDomainError::Upstream { status: Some(500), .. } => {}
            other => panic!("expected Upstream(500), got {other:?}"),
        }
    }
}
```

> **Dependency check:** this test module uses `futures_util::StreamExt`. `reqwest` already pulls in `futures-util` transitively, but if this compile step fails on the unqualified import, add `futures-util = "0.3"` to `[dependencies]` of `src/libs/colmena/Cargo.toml` (the crate is already in the dep tree from `reqwest`/`tokio`; the top-level dep just exposes the feature gates you need). The import is kept inside the function so it does not pollute the module namespace.

- [ ] **Step 2: Ensure the error variants the adapter expects exist**

In `src/libs/colmena/src/web/domain/errors.rs`, confirm these variants exist (Plan 0 introduced `Timeout` and `Upstream`; this task also needs `UnexpectedHtmlResponse` and `SpecTooLarge`):

```rust
    #[error("URL returned HTML, not a spec: {resolved_url}")]
    UnexpectedHtmlResponse { url_given: String, resolved_url: String },

    #[error("spec too large: {size_bytes} bytes > {limit_bytes}")]
    SpecTooLarge { size_bytes: u64, limit_bytes: u64 },
```

Both are LLM-recoverable — extend `is_llm_recoverable()` accordingly:

```rust
            Self::UnexpectedHtmlResponse { .. } => true,
            Self::SpecTooLarge { .. } => false, // config-level / bounded retry impossible
```

- [ ] **Step 3: Register the module**

Edit `src/libs/colmena/src/web/infrastructure/mod.rs`. Append:

```rust
pub mod openapi_adapter;
pub use openapi_adapter::{OpenApiAdapter, OpenApiAdapterConfig};
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib web::infrastructure::openapi_adapter::tests_fetch`
Expected: 6 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/web/infrastructure/openapi_adapter.rs \
        src/libs/colmena/src/web/infrastructure/mod.rs \
        src/libs/colmena/src/web/domain/errors.rs
git commit -m "$(cat <<'EOF'
feat(web): OpenApiAdapter — fetch stage with size cap + HTML detection

Normalizes Git-forge URLs, GETs via reqwest with streaming byte cap,
rejects text/html responses (including when Content-Type is absent
but the body starts with '<'). Propagates If-None-Match → 304. The
full fetch_and_parse ApiSpecPort impl lands in Task 6.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `OpenApiAdapter` — parse OpenAPI 3.x → `ParsedSpec`

**Files:**
- Modify: `src/libs/colmena/src/web/infrastructure/openapi_adapter.rs`
- Create: `src/libs/colmena/tests/fixtures/specs/petstore-3.0.yaml` (fixture, see Step 5)

Implements the format/version detection + parsing for OpenAPI 3.x. Swagger 2.0 dispatch comes in Task 7.

- [ ] **Step 1: Write the failing tests (parse from a YAML fixture)**

Append to `src/libs/colmena/src/web/infrastructure/openapi_adapter.rs`:

```rust
#[cfg(test)]
mod tests_parse_openapi3 {
    use super::*;
    use crate::web::domain::{HttpMethod, SpecFormat};

    fn petstore_yaml() -> &'static str {
        include_str!("../../../tests/fixtures/specs/petstore-3.0.yaml")
    }

    #[test]
    fn detect_format_json_braces() {
        let b = b"  { \"openapi\": \"3.0.3\" }";
        assert!(matches!(
            super::super::openapi_adapter::detect_body_format(b),
            BodyFormat::Json
        ));
    }

    #[test]
    fn detect_format_yaml_fallback() {
        let b = b"openapi: 3.0.3\ninfo: ...";
        assert!(matches!(
            super::super::openapi_adapter::detect_body_format(b),
            BodyFormat::Yaml
        ));
    }

    #[test]
    fn detect_spec_kind_openapi_3x() {
        let v: serde_json::Value = serde_json::from_str(r#"{"openapi": "3.0.3"}"#).unwrap();
        assert_eq!(super::super::openapi_adapter::detect_spec_kind(&v).unwrap(), SpecFormat::OpenApi3x);
    }

    #[test]
    fn detect_spec_kind_swagger_2() {
        let v: serde_json::Value = serde_json::from_str(r#"{"swagger": "2.0"}"#).unwrap();
        assert_eq!(super::super::openapi_adapter::detect_spec_kind(&v).unwrap(), SpecFormat::Swagger20);
    }

    #[test]
    fn detect_spec_kind_rejects_asyncapi() {
        let v: serde_json::Value = serde_json::from_str(r#"{"asyncapi": "2.4.0"}"#).unwrap();
        let err = super::super::openapi_adapter::detect_spec_kind(&v).unwrap_err();
        assert!(matches!(err, WebDomainError::UnsupportedSpecFormat { .. }));
    }

    #[tokio::test]
    async fn parse_petstore_yaml_succeeds() {
        let body = petstore_yaml().as_bytes().to_vec();
        let parsed = super::super::openapi_adapter::parse_body_to_spec(
            &body,
            Some("application/yaml"),
            "https://example.test/petstore.yaml",
            "https://example.test/petstore.yaml",
        )
        .unwrap();
        assert_eq!(parsed.title, "Swagger Petstore");
        assert_eq!(parsed.original_format, SpecFormat::OpenApi3x);
        assert!(parsed.endpoints.len() >= 3);
        // find POST /pet
        let post_pet = parsed
            .endpoints
            .iter()
            .find(|e| e.method == HttpMethod::Post && e.path == "/pet")
            .expect("POST /pet should exist");
        assert!(post_pet.request_body.is_some());
        let rb = post_pet.request_body.as_ref().unwrap();
        assert!(rb.content_type.starts_with("application/"));
    }
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib web::infrastructure::openapi_adapter::tests_parse_openapi3 2>&1 | tail -20`
Expected: compile error — `detect_body_format`, `detect_spec_kind`, `parse_body_to_spec`, `BodyFormat` don't exist yet.

- [ ] **Step 3: Implement format detection + OpenAPI 3.x parsing**

Append to `src/libs/colmena/src/web/infrastructure/openapi_adapter.rs`, just below the `OpenApiAdapter` struct impl:

```rust
use crate::web::domain::{
    ApiKeyLocation, Endpoint, HttpMethod, ParamType, ParameterSpec, ParsedSpec, RequestBodySpec,
    ResponseSpec, SecurityRequirement, SecurityScheme, SpecFormat,
};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BodyFormat {
    Json,
    Yaml,
}

pub(crate) fn detect_body_format(body: &[u8]) -> BodyFormat {
    let first = body.iter().copied().find(|b| !b.is_ascii_whitespace());
    match first {
        Some(b'{') | Some(b'[') => BodyFormat::Json,
        _ => BodyFormat::Yaml,
    }
}

pub(crate) fn detect_spec_kind(v: &serde_json::Value) -> Result<SpecFormat, WebDomainError> {
    if let Some(openapi) = v.get("openapi").and_then(|x| x.as_str()) {
        if openapi.starts_with("3.") {
            return Ok(SpecFormat::OpenApi3x);
        }
        return Err(WebDomainError::UnsupportedSpecFormat {
            detected: format!("openapi {openapi}"),
        });
    }
    if let Some(swagger) = v.get("swagger").and_then(|x| x.as_str()) {
        if swagger.starts_with("2.") {
            return Ok(SpecFormat::Swagger20);
        }
        return Err(WebDomainError::UnsupportedSpecFormat {
            detected: format!("swagger {swagger}"),
        });
    }
    let hint = ["asyncapi", "raml", "info", "definitions"]
        .iter()
        .find(|k| v.get(*k).is_some())
        .copied()
        .unwrap_or("(none)");
    Err(WebDomainError::UnsupportedSpecFormat {
        detected: format!("root key '{hint}' not recognized as OpenAPI or Swagger"),
    })
}

/// Parse raw bytes of an OpenAPI 3.x document into `ParsedSpec`.
/// Swagger 2.0 dispatch is added in Task 7.
pub(crate) fn parse_body_to_spec(
    body: &[u8],
    content_type: Option<&str>,
    input_url: &str,
    resolved_url: &str,
) -> Result<ParsedSpec, WebDomainError> {
    let fmt = match content_type {
        Some(ct) if ct.contains("json") => BodyFormat::Json,
        Some(ct) if ct.contains("yaml") || ct.contains("yml") => BodyFormat::Yaml,
        _ => detect_body_format(body),
    };

    let as_value: serde_json::Value = match fmt {
        BodyFormat::Json => serde_json::from_slice(body).map_err(|e| {
            WebDomainError::SpecParseFailed {
                details: format!("json parse: {e}"),
            }
        })?,
        BodyFormat::Yaml => {
            let y: serde_yaml::Value = serde_yaml::from_slice(body).map_err(|e| {
                WebDomainError::SpecParseFailed {
                    details: format!("yaml parse: {e}"),
                }
            })?;
            serde_json::to_value(y).map_err(|e| WebDomainError::SpecParseFailed {
                details: format!("yaml→json: {e}"),
            })?
        }
    };

    let kind = detect_spec_kind(&as_value)?;
    match kind {
        SpecFormat::OpenApi3x => {
            parse_oas3_value(as_value, SpecFormat::OpenApi3x, input_url, resolved_url)
        }
        SpecFormat::Swagger20 => {
            // Task 7 fills this in; for now, error cleanly.
            Err(WebDomainError::AdapterInit(
                "Swagger 2.0 path pending Task 7".into(),
            ))
        }
    }
}

fn parse_oas3_value(
    v: serde_json::Value,
    original_format: SpecFormat,
    input_url: &str,
    resolved_url: &str,
) -> Result<ParsedSpec, WebDomainError> {
    let spec: oas3::OpenApiV3Spec =
        serde_json::from_value(v.clone()).map_err(|e| WebDomainError::SpecParseFailed {
            details: format!("oas3 decode: {e}"),
        })?;

    let title = spec.info.title.clone();
    let version = spec.info.version.clone();
    let description = spec.info.description.clone();
    let internal_format = format!("openapi-{}", spec.openapi);
    let tags = spec.tags.iter().map(|t| t.name.clone()).collect();
    let servers = spec
        .servers
        .iter()
        .map(|s| s.url.clone())
        .collect::<Vec<_>>();

    let security_schemes = extract_security_schemes(&v);
    let endpoints = extract_endpoints(&v)?;

    Ok(ParsedSpec {
        resolved_url: resolved_url.to_string(),
        input_url: input_url.to_string(),
        original_format,
        internal_format,
        title,
        version,
        description,
        servers,
        endpoints,
        security_schemes,
        tags,
    })
}

/// We walk the raw JSON rather than `oas3`'s typed view so refs we don't
/// inline still carry through — the LLM sees whatever the spec author
/// wrote, even if it's a `$ref` to a component we haven't resolved.
fn extract_endpoints(root: &serde_json::Value) -> Result<Vec<Endpoint>, WebDomainError> {
    let mut out: Vec<Endpoint> = Vec::new();
    let Some(paths) = root.get("paths").and_then(|v| v.as_object()) else {
        return Ok(out);
    };

    for (path, item) in paths {
        let Some(item_obj) = item.as_object() else {
            continue;
        };

        // Path-level parameters apply to every operation under the path.
        let path_level_params: Vec<serde_json::Value> = item_obj
            .get("parameters")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for (method_key, op_val) in item_obj {
            let Some(method) = HttpMethod::parse(method_key) else {
                continue;
            };
            let Some(op) = op_val.as_object() else {
                continue;
            };

            let operation_id = op
                .get("operationId")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| generate_operation_id(method, path));

            let summary = op.get("summary").and_then(|v| v.as_str()).map(String::from);
            let description = op
                .get("description")
                .and_then(|v| v.as_str())
                .map(String::from);
            let tags = op
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect())
                .unwrap_or_default();

            // Merge path-level + op-level parameters; op-level wins on name conflict.
            let mut combined: Vec<serde_json::Value> = path_level_params.clone();
            if let Some(Some(arr)) = op.get("parameters").map(|v| v.as_array()) {
                for p in arr {
                    combined.retain(|existing| {
                        existing.get("name") != p.get("name")
                            || existing.get("in") != p.get("in")
                    });
                    combined.push(p.clone());
                }
            }

            let mut path_params = Vec::new();
            let mut query_params = Vec::new();
            let mut header_params = Vec::new();
            for p in combined {
                let Some(p_obj) = p.as_object() else { continue };
                let spec = parameter_from_json(p_obj);
                match p_obj.get("in").and_then(|v| v.as_str()) {
                    Some("path") => path_params.push(spec),
                    Some("query") => query_params.push(spec),
                    Some("header") => header_params.push(spec),
                    _ => {} // cookie ignored for v1
                }
            }

            let request_body = op.get("requestBody").and_then(request_body_from_json);
            let responses = op
                .get("responses")
                .and_then(|v| v.as_object())
                .map(|map| {
                    let mut out = HashMap::new();
                    for (code, resp) in map {
                        out.insert(code.clone(), response_spec_from_json(resp));
                    }
                    out
                })
                .unwrap_or_default();
            let security = op
                .get("security")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().flat_map(security_requirements_from_array).collect())
                .unwrap_or_default();

            out.push(Endpoint {
                operation_id,
                method,
                path: path.clone(),
                summary,
                description,
                tags,
                path_params,
                query_params,
                header_params,
                request_body,
                responses,
                security,
            });
        }
    }

    Ok(out)
}

fn generate_operation_id(method: HttpMethod, path: &str) -> String {
    // Derive a stable ID when the author didn't provide one.
    // e.g. (POST, /pet/{petId}/uploadImage) → Post_pet_petId_uploadImage
    let cleaned: String = path
        .chars()
        .map(|c| if c == '/' || c == '{' || c == '}' || c == '-' { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim_matches('_');
    format!(
        "{}{}",
        match method {
            HttpMethod::Get => "Get_",
            HttpMethod::Put => "Put_",
            HttpMethod::Post => "Post_",
            HttpMethod::Delete => "Delete_",
            HttpMethod::Options => "Options_",
            HttpMethod::Head => "Head_",
            HttpMethod::Patch => "Patch_",
            HttpMethod::Trace => "Trace_",
        },
        cleaned
    )
}

fn parameter_from_json(p: &serde_json::Map<String, serde_json::Value>) -> ParameterSpec {
    let name = p
        .get("name")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_default();
    let description = p.get("description").and_then(|v| v.as_str()).map(String::from);
    let required = p.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
    let schema = p
        .get("schema")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let param_type = classify_schema(&schema);
    let style = p.get("style").and_then(|v| v.as_str()).map(String::from);
    let explode = p.get("explode").and_then(|v| v.as_bool());
    ParameterSpec {
        name,
        description,
        required,
        param_type,
        style,
        explode,
    }
}

fn classify_schema(schema: &serde_json::Value) -> ParamType {
    let Some(schema) = schema.as_object() else {
        return ParamType::Unknown;
    };
    match schema.get("type").and_then(|v| v.as_str()) {
        Some("string") => ParamType::String,
        Some("integer") => ParamType::Integer,
        Some("number") => ParamType::Number,
        Some("boolean") => ParamType::Boolean,
        Some("array") => {
            let items = schema.get("items").cloned().unwrap_or(serde_json::Value::Null);
            ParamType::Array(Box::new(classify_schema(&items)))
        }
        Some("object") => ParamType::Object,
        _ => {
            if schema.contains_key("$ref") {
                ParamType::Object
            } else {
                ParamType::Unknown
            }
        }
    }
}

fn request_body_from_json(rb: &serde_json::Value) -> Option<RequestBodySpec> {
    let rb_obj = rb.as_object()?;
    let required = rb_obj.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
    let content = rb_obj.get("content").and_then(|v| v.as_object())?;
    // Prefer JSON, fall back to form-urlencoded, multipart, then first key.
    let preferred = ["application/json", "application/x-www-form-urlencoded", "multipart/form-data"];
    let content_type = preferred
        .iter()
        .find(|k| content.contains_key(**k))
        .map(|s| s.to_string())
        .or_else(|| content.keys().next().cloned())?;
    let media = content.get(&content_type)?.as_object()?;
    let schema = media.get("schema").cloned().unwrap_or(serde_json::Value::Null);
    Some(RequestBodySpec {
        content_type,
        required,
        schema,
    })
}

fn response_spec_from_json(resp: &serde_json::Value) -> ResponseSpec {
    let description = resp
        .get("description")
        .and_then(|v| v.as_str())
        .map(String::from);
    let mut content = HashMap::new();
    if let Some(cmap) = resp.get("content").and_then(|v| v.as_object()) {
        for (ct, media) in cmap {
            if let Some(media_obj) = media.as_object() {
                if let Some(schema) = media_obj.get("schema") {
                    content.insert(ct.clone(), schema.clone());
                }
            }
        }
    }
    ResponseSpec { description, content }
}

fn security_requirements_from_array(v: &serde_json::Value) -> Vec<SecurityRequirement> {
    let Some(obj) = v.as_object() else {
        return Vec::new();
    };
    obj.iter()
        .map(|(scheme, scopes)| SecurityRequirement {
            scheme: scheme.clone(),
            scopes: scopes
                .as_array()
                .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                .unwrap_or_default(),
        })
        .collect()
}

fn extract_security_schemes(root: &serde_json::Value) -> HashMap<String, SecurityScheme> {
    let mut out = HashMap::new();
    let Some(comps) = root.get("components").and_then(|v| v.as_object()) else {
        return out;
    };
    let Some(ss) = comps.get("securitySchemes").and_then(|v| v.as_object()) else {
        return out;
    };
    for (name, raw) in ss {
        let Some(scheme_obj) = raw.as_object() else { continue };
        let ty = scheme_obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let scheme = match ty {
            "http" => SecurityScheme::Http {
                scheme: scheme_obj
                    .get("scheme")
                    .and_then(|v| v.as_str())
                    .unwrap_or("bearer")
                    .to_string(),
                bearer_format: scheme_obj
                    .get("bearerFormat")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            },
            "apiKey" => {
                let location = match scheme_obj.get("in").and_then(|v| v.as_str()).unwrap_or("header") {
                    "query" => ApiKeyLocation::Query,
                    "cookie" => ApiKeyLocation::Cookie,
                    _ => ApiKeyLocation::Header,
                };
                SecurityScheme::ApiKey {
                    name: scheme_obj
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    location,
                }
            }
            "oauth2" => SecurityScheme::OAuth2 {
                flows: scheme_obj
                    .get("flows")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            },
            "openIdConnect" => SecurityScheme::OpenIdConnect {
                openid_connect_url: scheme_obj
                    .get("openIdConnectUrl")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            },
            _ => continue,
        };
        out.insert(name.clone(), scheme);
    }
    out
}
```

- [ ] **Step 4: Ensure the remaining error variants exist**

In `src/libs/colmena/src/web/domain/errors.rs`, confirm these variants (add any that are missing):

```rust
    #[error("spec parse failed: {details}")]
    SpecParseFailed { details: String },

    #[error("unsupported spec format: {detected}")]
    UnsupportedSpecFormat { detected: String },
```

Both LLM-recoverable (the LLM can fall back to `tavily_client`):

```rust
            Self::SpecParseFailed { .. } => true,
            Self::UnsupportedSpecFormat { .. } => true,
```

- [ ] **Step 5: Add the petstore-3.0 fixture**

Create `src/libs/colmena/tests/fixtures/specs/petstore-3.0.yaml`:

```yaml
openapi: 3.0.3
info:
  title: Swagger Petstore
  description: A minimal petstore used by api_explorer tests.
  version: "1.0.0"
servers:
  - url: https://petstore3.swagger.io/api/v3
tags:
  - name: pet
  - name: store
paths:
  /pet:
    post:
      tags: [pet]
      summary: Add a new pet to the store
      operationId: addPet
      requestBody:
        description: Pet object
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/Pet'
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Pet'
      security:
        - BearerAuth: []
    get:
      tags: [pet]
      summary: List pets
      operationId: listPets
      parameters:
        - in: query
          name: limit
          required: false
          schema: { type: integer }
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: array
                items:
                  $ref: '#/components/schemas/Pet'
  /pet/{petId}:
    get:
      tags: [pet]
      summary: Get pet by ID
      operationId: getPetById
      parameters:
        - in: path
          name: petId
          required: true
          schema: { type: integer, format: int64 }
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Pet'
        '404':
          description: not found
components:
  schemas:
    Pet:
      type: object
      required: [name]
      properties:
        id:
          type: integer
          format: int64
        name:
          type: string
        status:
          type: string
          enum: [available, pending, sold]
  securitySchemes:
    BearerAuth:
      type: http
      scheme: bearer
      bearerFormat: JWT
```

- [ ] **Step 6: Wire the `ApiSpecPort` impl and run**

Replace the temporary `impl ApiSpecPort for OpenApiAdapter` (the stub added in Task 5) with the real one:

```rust
#[async_trait]
impl ApiSpecPort for OpenApiAdapter {
    async fn fetch_and_parse(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<SpecFetchResult, WebDomainError> {
        match self.fetch_raw(url, etag, last_modified).await? {
            FetchRawResult::NotModified => Ok(SpecFetchResult::NotModified),
            FetchRawResult::Fresh {
                body,
                content_type,
                etag,
                last_modified,
                resolved_url,
            } => {
                let spec = parse_body_to_spec(&body, content_type.as_deref(), url, &resolved_url)?;
                Ok(SpecFetchResult::Fresh {
                    spec,
                    etag,
                    last_modified,
                })
            }
        }
    }
}
```

- [ ] **Step 7: Run — expect PASS**

Run: `cargo test --lib web::infrastructure::openapi_adapter`
Expected: `tests_fetch` still passes (6 tests) and `tests_parse_openapi3` passes (6 tests) — 12 total.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/web/infrastructure/openapi_adapter.rs \
        src/libs/colmena/src/web/domain/errors.rs \
        src/libs/colmena/tests/fixtures/specs/petstore-3.0.yaml
git commit -m "$(cat <<'EOF'
feat(web): OpenApiAdapter parses OpenAPI 3.x into ParsedSpec

Detects JSON vs YAML by body sniff + content-type; routes OpenAPI 3.x
through oas3 then walks the raw JSON to collect endpoints with merged
path/op parameters, request body (preferring application/json), and
security scheme metadata. Adds petstore-3.0.yaml fixture. Swagger 2.0
branch still stubbed — Task 7.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `OpenApiAdapter` — Swagger 2.0 dispatch + revalidation round-trip

**Files:**
- Modify: `src/libs/colmena/src/web/infrastructure/openapi_adapter.rs`
- Create: `src/libs/colmena/tests/fixtures/specs/petstore-2.0.yaml`

- [ ] **Step 1: Add the Swagger 2.0 petstore fixture**

Create `src/libs/colmena/tests/fixtures/specs/petstore-2.0.yaml`:

```yaml
swagger: "2.0"
info:
  title: Swagger Petstore 2.0
  version: "1.0.0"
host: petstore.swagger.io
basePath: /v2
schemes:
  - https
consumes:
  - application/json
produces:
  - application/json
paths:
  /pet:
    post:
      operationId: addPet
      parameters:
        - in: body
          name: body
          required: true
          schema:
            $ref: '#/definitions/Pet'
      responses:
        '200':
          description: ok
          schema:
            $ref: '#/definitions/Pet'
      security:
        - ApiKeyAuth: []
  /pet/{petId}:
    get:
      operationId: getPetById
      parameters:
        - in: path
          name: petId
          required: true
          type: integer
          format: int64
      responses:
        '200':
          description: ok
          schema:
            $ref: '#/definitions/Pet'
definitions:
  Pet:
    type: object
    required:
      - name
    properties:
      id:
        type: integer
        format: int64
      name:
        type: string
securityDefinitions:
  ApiKeyAuth:
    type: apiKey
    name: X-API-Key
    in: header
```

- [ ] **Step 2: Write the failing tests**

Append to `src/libs/colmena/src/web/infrastructure/openapi_adapter.rs`:

```rust
#[cfg(test)]
mod tests_parse_swagger2 {
    use super::*;
    use crate::web::domain::{ApiKeyLocation, HttpMethod, SecurityScheme, SpecFormat};

    fn petstore2_yaml() -> &'static str {
        include_str!("../../../tests/fixtures/specs/petstore-2.0.yaml")
    }

    #[test]
    fn parse_swagger2_petstore_roundtrips() {
        let body = petstore2_yaml().as_bytes().to_vec();
        let parsed = super::super::openapi_adapter::parse_body_to_spec(
            &body,
            Some("application/yaml"),
            "https://example.test/petstore2.yaml",
            "https://example.test/petstore2.yaml",
        )
        .unwrap();

        assert_eq!(parsed.original_format, SpecFormat::Swagger20);
        assert!(parsed.internal_format.starts_with("openapi-3.0"));
        assert_eq!(parsed.title, "Swagger Petstore 2.0");
        assert_eq!(
            parsed.servers,
            vec!["https://petstore.swagger.io/v2".to_string()]
        );
        assert!(!parsed.endpoints.is_empty());

        // POST /pet exists, with a requestBody that came from the body parameter.
        let post_pet = parsed
            .endpoints
            .iter()
            .find(|e| e.method == HttpMethod::Post && e.path == "/pet")
            .expect("POST /pet");
        let rb = post_pet.request_body.as_ref().expect("request body");
        assert_eq!(rb.content_type, "application/json");

        // ApiKeyAuth security scheme survived the conversion.
        let sec = parsed.security_schemes.get("ApiKeyAuth").expect("ApiKeyAuth");
        match sec {
            SecurityScheme::ApiKey { name, location } => {
                assert_eq!(name, "X-API-Key");
                assert_eq!(*location, ApiKeyLocation::Header);
            }
            other => panic!("expected ApiKey, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_and_parse_uses_conditional_get_roundtrip() {
        use wiremock::matchers::{header_exists, method, path as wm_path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        // First GET → 200 + ETag.
        Mock::given(method("GET"))
            .and(wm_path("/ps.yaml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/yaml")
                    .insert_header("ETag", "\"v1\"")
                    .set_body_string(petstore2_yaml()),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;

        // Second GET with If-None-Match → 304.
        Mock::given(method("GET"))
            .and(wm_path("/ps.yaml"))
            .and(header_exists("If-None-Match"))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;

        let url = format!("{}/ps.yaml", server.uri());
        let adapter = OpenApiAdapter::new(OpenApiAdapterConfig::default()).unwrap();
        let first = adapter.fetch_and_parse(&url, None, None).await.unwrap();
        let etag = match first {
            crate::web::domain::SpecFetchResult::Fresh { etag, .. } => etag,
            _ => panic!("expected Fresh"),
        };
        assert_eq!(etag.as_deref(), Some("\"v1\""));

        let second = adapter
            .fetch_and_parse(&url, etag.as_deref(), None)
            .await
            .unwrap();
        assert!(matches!(
            second,
            crate::web::domain::SpecFetchResult::NotModified
        ));
    }
}
```

- [ ] **Step 3: Run — expect FAIL (first sub-test)**

Run: `cargo test --lib web::infrastructure::openapi_adapter::tests_parse_swagger2 2>&1 | tail -30`
Expected: `parse_swagger2_petstore_roundtrips` fails because the Swagger 2.0 branch in `parse_body_to_spec` returns `AdapterInit(...)`.

- [ ] **Step 4: Wire the Swagger 2.0 branch**

In `parse_body_to_spec` (added in Task 6), replace the `SpecFormat::Swagger20` arm:

```rust
        SpecFormat::Swagger20 => {
            Err(WebDomainError::AdapterInit(
                "Swagger 2.0 path pending Task 7".into(),
            ))
        }
```

with:

```rust
        SpecFormat::Swagger20 => {
            let converted = crate::web::application::swagger2_to_oas3::convert_swagger2_to_openapi3(
                &as_value,
            )?;
            parse_oas3_value(converted, SpecFormat::Swagger20, input_url, resolved_url)
        }
```

- [ ] **Step 5: Run — expect PASS**

Run: `cargo test --lib web::infrastructure::openapi_adapter`
Expected: all of `tests_fetch` (6), `tests_parse_openapi3` (6), and `tests_parse_swagger2` (2) pass — 14 total.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/web/infrastructure/openapi_adapter.rs \
        src/libs/colmena/tests/fixtures/specs/petstore-2.0.yaml
git commit -m "$(cat <<'EOF'
feat(web): OpenApiAdapter dispatches Swagger 2.0 through converter

Recognized 2.0 documents now flow Swagger-2.0 → convert to 3.0.3 JSON
→ oas3 → ParsedSpec. Adds an end-to-end wiremock test covering the
conditional-GET revalidation round-trip (200 with ETag, then 304 on
re-request with If-None-Match). Adds petstore-2.0.yaml fixture.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: `ApiSpecUseCase` — `SpecCache` + `fetch_spec`

**Files:**
- Create: `src/libs/colmena/src/web/application/api_spec_use_case.rs`
- Modify: `src/libs/colmena/src/web/application/mod.rs`

The use case owns per-conversation state through the shared `SessionRegistry<SpecCache>`. A `SpecCache` holds an LRU of parsed specs keyed by input URL. The node dispatches to this use case from each sub-tool handler.

- [ ] **Step 1: Write the failing tests**

Create `src/libs/colmena/src/web/application/api_spec_use_case.rs`:

```rust
//! `ApiSpecUseCase` — orchestrates fetch / cache / search / build for a
//! single conversation.

use crate::web::domain::{
    ApiSpecPort, ParsedSpec, SessionKey, SessionRegistry, SpecFetchResult, WebDomainError,
};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Tunables for the use case. Defaults match the Spec C "minimal config" values.
#[derive(Debug, Clone)]
pub struct ApiSpecUseCaseConfig {
    pub enable_cache: bool,
    pub cache_ttl: Duration,
    pub max_cached_specs: usize,
    pub fuzzy_match_threshold: f32,
    pub default_base_url_override: Option<String>,
}

impl Default for ApiSpecUseCaseConfig {
    fn default() -> Self {
        Self {
            enable_cache: true,
            cache_ttl: Duration::from_secs(86_400),
            max_cached_specs: 100,
            fuzzy_match_threshold: 0.6,
            default_base_url_override: None,
        }
    }
}

/// Per-conversation cache of parsed specs.
pub struct SpecCache {
    specs: Mutex<LruCache<String, CachedSpec>>,
}

impl SpecCache {
    pub fn new(max: usize) -> Self {
        Self {
            specs: Mutex::new(LruCache::new(
                NonZeroUsize::new(max.max(1)).unwrap(),
            )),
        }
    }
}

#[derive(Clone)]
pub struct CachedSpec {
    pub parsed: Arc<ParsedSpec>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub cached_at: Instant,
}

pub struct ApiSpecUseCase {
    port: Arc<dyn ApiSpecPort>,
    registry: Arc<SessionRegistry<Arc<SpecCache>>>,
    config: ApiSpecUseCaseConfig,
}

impl ApiSpecUseCase {
    pub fn new(
        port: Arc<dyn ApiSpecPort>,
        registry: Arc<SessionRegistry<Arc<SpecCache>>>,
        config: ApiSpecUseCaseConfig,
    ) -> Self {
        Self { port, registry, config }
    }

    /// Fetch-or-reuse a spec for a given conversation.
    ///
    /// * If `force_reload` is true or the cached entry is older than the
    ///   TTL, hit the port. A 304 response refreshes `cached_at` in place.
    /// * Always returns a `CachedSpec` (either the freshly-parsed one or
    ///   the reused one).
    pub async fn fetch_spec(
        &self,
        conversation_id: &str,
        input_url: &str,
        force_reload: bool,
    ) -> Result<CachedSpec, WebDomainError> {
        let key = SessionKey::new(conversation_id, "api_explorer");

        // Get-or-create the per-conversation cache.
        let cache = self
            .registry
            .with_entry(&key, |c| c.clone())
            .await
            .unwrap_or_else(|| {
                let fresh = Arc::new(SpecCache::new(self.config.max_cached_specs));
                // Ignoring return value: Some(old) never happens here because
                // we just read None above.
                let _ = tokio::task::block_in_place(|| ());
                fresh
            });
        // Insert if absent so future lookups share the Arc.
        self.registry.insert(key.clone(), cache.clone()).await;

        if self.config.enable_cache && !force_reload {
            if let Some(hit) = cache
                .specs
                .lock()
                .await
                .get(input_url)
                .cloned()
            {
                if hit.cached_at.elapsed() < self.config.cache_ttl {
                    // Fresh enough — skip revalidation to avoid network.
                    return Ok(hit);
                }
            }
        }

        // Either no cache entry, stale, or forced reload. Re-fetch.
        let previous = cache.specs.lock().await.get(input_url).cloned();
        let etag = previous.as_ref().and_then(|c| c.etag.clone());
        let last_modified = previous.as_ref().and_then(|c| c.last_modified.clone());

        match self
            .port
            .fetch_and_parse(input_url, etag.as_deref(), last_modified.as_deref())
            .await?
        {
            SpecFetchResult::Fresh {
                spec,
                etag,
                last_modified,
            } => {
                let entry = CachedSpec {
                    parsed: Arc::new(spec),
                    etag,
                    last_modified,
                    cached_at: Instant::now(),
                };
                cache.specs.lock().await.put(input_url.to_string(), entry.clone());
                Ok(entry)
            }
            SpecFetchResult::NotModified => {
                let mut prev = previous.ok_or_else(|| WebDomainError::AdapterInit(
                    "got 304 Not Modified without a cached spec".into(),
                ))?;
                prev.cached_at = Instant::now();
                cache
                    .specs
                    .lock()
                    .await
                    .put(input_url.to_string(), prev.clone());
                Ok(prev)
            }
        }
    }

    /// Public accessor for tests + later tasks that need the registry's
    /// Arc (e.g. lifecycle subscription).
    pub fn registry(&self) -> Arc<SessionRegistry<Arc<SpecCache>>> {
        self.registry.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::domain::{
        Endpoint, HttpMethod, ParsedSpec, SpecFormat, TtlConfig,
    };
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct CountingPort {
        calls: AtomicU32,
        respond_with: Mutex<Option<SpecFetchResult>>,
    }

    #[async_trait]
    impl ApiSpecPort for CountingPort {
        async fn fetch_and_parse(
            &self,
            _url: &str,
            _etag: Option<&str>,
            _lm: Option<&str>,
        ) -> Result<SpecFetchResult, WebDomainError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            // Return whatever was prepared; default to a fresh tiny spec.
            let prepared = self.respond_with.lock().await.take();
            if let Some(r) = prepared {
                return Ok(r);
            }
            Ok(SpecFetchResult::Fresh {
                spec: tiny_spec(),
                etag: Some("\"v1\"".into()),
                last_modified: None,
            })
        }
    }

    fn tiny_spec() -> ParsedSpec {
        ParsedSpec {
            resolved_url: "https://ex/s.yaml".into(),
            input_url: "https://ex/s.yaml".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "T".into(),
            version: "1".into(),
            description: None,
            servers: vec!["https://api.example.com".into()],
            endpoints: vec![Endpoint {
                operation_id: "getThing".into(),
                method: HttpMethod::Get,
                path: "/thing".into(),
                summary: None,
                description: None,
                tags: Vec::new(),
                path_params: Vec::new(),
                query_params: Vec::new(),
                header_params: Vec::new(),
                request_body: None,
                responses: HashMap::new(),
                security: Vec::new(),
            }],
            security_schemes: HashMap::new(),
            tags: Vec::new(),
        }
    }

    fn use_case_with(port: Arc<CountingPort>) -> ApiSpecUseCase {
        let registry: Arc<SessionRegistry<Arc<SpecCache>>> =
            SessionRegistry::new(TtlConfig::default());
        ApiSpecUseCase::new(port, registry, ApiSpecUseCaseConfig::default())
    }

    #[tokio::test]
    async fn first_fetch_hits_the_port() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        uc.fetch_spec("conv-1", "https://ex/s.yaml", false).await.unwrap();
        assert_eq!(port.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn second_fetch_within_ttl_does_not_hit_the_port() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        uc.fetch_spec("conv-1", "https://ex/s.yaml", false).await.unwrap();
        uc.fetch_spec("conv-1", "https://ex/s.yaml", false).await.unwrap();
        assert_eq!(port.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn force_reload_bypasses_cache() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        uc.fetch_spec("conv-1", "https://ex/s.yaml", false).await.unwrap();
        uc.fetch_spec("conv-1", "https://ex/s.yaml", true).await.unwrap();
        assert_eq!(port.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn different_conversations_have_separate_caches() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        uc.fetch_spec("conv-a", "https://ex/s.yaml", false).await.unwrap();
        uc.fetch_spec("conv-b", "https://ex/s.yaml", false).await.unwrap();
        assert_eq!(port.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn not_modified_refreshes_existing_cached_entry() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        uc.fetch_spec("conv-1", "https://ex/s.yaml", false).await.unwrap();
        *port.respond_with.lock().await = Some(SpecFetchResult::NotModified);
        let res = uc
            .fetch_spec("conv-1", "https://ex/s.yaml", true)
            .await
            .unwrap();
        assert_eq!(port.calls.load(Ordering::SeqCst), 2);
        assert_eq!(res.etag.as_deref(), Some("\"v1\""));
    }
}
```

- [ ] **Step 2: Register the module**

Edit `src/libs/colmena/src/web/application/mod.rs`. Append:

```rust
pub mod api_spec_use_case;
pub use api_spec_use_case::{ApiSpecUseCase, ApiSpecUseCaseConfig, CachedSpec, SpecCache};
```

- [ ] **Step 3: Run — expect PASS**

Run: `cargo test --lib web::application::api_spec_use_case`
Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/web/application/api_spec_use_case.rs \
        src/libs/colmena/src/web/application/mod.rs
git commit -m "$(cat <<'EOF'
feat(web): ApiSpecUseCase with SpecCache keyed by conversation

Per-conversation LRU of parsed specs lives in a shared
SessionRegistry<Arc<SpecCache>>. fetch_spec avoids network when the
in-memory entry is younger than cache_ttl; on 304 Not Modified the
existing entry is refreshed in place. Search / build_http_request
come in Tasks 9–11.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: `ApiSpecUseCase` — `list_endpoints` + `search_endpoint`

**Files:**
- Modify: `src/libs/colmena/src/web/application/api_spec_use_case.rs`

- [ ] **Step 1: Write the failing tests**

Append to `src/libs/colmena/src/web/application/api_spec_use_case.rs`:

```rust
#[cfg(test)]
mod tests_list_and_search {
    use super::*;
    use crate::web::domain::{Endpoint, HttpMethod, ParsedSpec, SpecFormat};
    use std::collections::HashMap;

    fn spec_with(endpoints: Vec<Endpoint>) -> ParsedSpec {
        ParsedSpec {
            resolved_url: "u".into(),
            input_url: "u".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "T".into(),
            version: "1".into(),
            description: None,
            servers: vec!["https://api.example.com".into()],
            endpoints,
            security_schemes: HashMap::new(),
            tags: vec!["pet".into(), "store".into()],
        }
    }

    fn ep(
        op: &str,
        method: HttpMethod,
        path: &str,
        summary: &str,
        tag: &str,
    ) -> Endpoint {
        Endpoint {
            operation_id: op.into(),
            method,
            path: path.into(),
            summary: Some(summary.into()),
            description: None,
            tags: vec![tag.into()],
            path_params: Vec::new(),
            query_params: Vec::new(),
            header_params: Vec::new(),
            request_body: None,
            responses: HashMap::new(),
            security: Vec::new(),
        }
    }

    fn sample() -> ParsedSpec {
        spec_with(vec![
            ep("listPets", HttpMethod::Get, "/pet", "List pets", "pet"),
            ep("addPet", HttpMethod::Post, "/pet", "Add a new pet", "pet"),
            ep("getPet", HttpMethod::Get, "/pet/{id}", "Get pet by ID", "pet"),
            ep("listStores", HttpMethod::Get, "/store", "List stores", "store"),
            ep("createSubscription", HttpMethod::Post, "/subscription", "Create a subscription", "billing"),
        ])
    }

    #[test]
    fn list_all_returns_all_endpoints() {
        let spec = sample();
        let page = super::super::api_spec_use_case::list_endpoints(&spec, None, 50, 0);
        assert_eq!(page.total, 5);
        assert_eq!(page.returned, 5);
        assert_eq!(page.endpoints.len(), 5);
    }

    #[test]
    fn list_paginates() {
        let spec = sample();
        let page = super::super::api_spec_use_case::list_endpoints(&spec, None, 2, 2);
        assert_eq!(page.total, 5);
        assert_eq!(page.returned, 2);
        assert_eq!(page.endpoints[0].operation_id, "getPet");
    }

    #[test]
    fn list_filters_by_tag() {
        let spec = sample();
        let page = super::super::api_spec_use_case::list_endpoints(&spec, Some("billing"), 50, 0);
        assert_eq!(page.total, 1);
        assert_eq!(page.endpoints[0].operation_id, "createSubscription");
    }

    #[test]
    fn search_finds_by_summary() {
        let spec = sample();
        let results = super::super::api_spec_use_case::search_endpoint(
            &spec,
            "create subscription",
            None,
            10,
            0.1,
        );
        assert!(!results.is_empty());
        assert_eq!(results[0].operation_id, "createSubscription");
    }

    #[test]
    fn search_filters_by_method() {
        let spec = sample();
        let results = super::super::api_spec_use_case::search_endpoint(
            &spec,
            "pet",
            Some("GET"),
            10,
            0.1,
        );
        for r in &results {
            assert_eq!(r.method, "GET");
        }
    }

    #[test]
    fn search_respects_threshold() {
        let spec = sample();
        let tight = super::super::api_spec_use_case::search_endpoint(
            &spec,
            "completely unrelated nonsense xyz",
            None,
            10,
            0.99,
        );
        assert!(tight.is_empty());
    }

    #[test]
    fn search_returns_top_n() {
        let spec = sample();
        let results = super::super::api_spec_use_case::search_endpoint(
            &spec,
            "pet",
            None,
            2,
            0.1,
        );
        assert!(results.len() <= 2);
    }
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib web::application::api_spec_use_case::tests_list_and_search 2>&1 | tail -10`
Expected: compile error — `list_endpoints`, `search_endpoint`, their result types not defined.

- [ ] **Step 3: Implement `list_endpoints` and `search_endpoint`**

Append to `src/libs/colmena/src/web/application/api_spec_use_case.rs`, before the `#[cfg(test)] mod tests` block:

```rust
use crate::web::domain::{Endpoint, HttpMethod};

#[derive(Debug, Clone)]
pub struct EndpointListPage {
    pub total: usize,
    pub returned: usize,
    pub offset: usize,
    pub endpoints: Vec<EndpointSummary>,
}

#[derive(Debug, Clone)]
pub struct EndpointSummary {
    pub operation_id: String,
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    pub tags: Vec<String>,
}

impl From<&Endpoint> for EndpointSummary {
    fn from(e: &Endpoint) -> Self {
        Self {
            operation_id: e.operation_id.clone(),
            method: e.method.as_str().to_string(),
            path: e.path.clone(),
            summary: e.summary.clone(),
            tags: e.tags.clone(),
        }
    }
}

pub fn list_endpoints(
    spec: &ParsedSpec,
    tag: Option<&str>,
    limit: usize,
    offset: usize,
) -> EndpointListPage {
    let filtered: Vec<&Endpoint> = spec
        .endpoints
        .iter()
        .filter(|e| match tag {
            None => true,
            Some(t) => e.tags.iter().any(|candidate| candidate == t),
        })
        .collect();
    let total = filtered.len();
    let slice: Vec<EndpointSummary> = filtered
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(EndpointSummary::from)
        .collect();
    let returned = slice.len();
    EndpointListPage {
        total,
        returned,
        offset,
        endpoints: slice,
    }
}

#[derive(Debug, Clone)]
pub struct EndpointSearchHit {
    pub operation_id: String,
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    pub score: f32,
    pub match_reason: String,
}

/// Fuzzy-search endpoints by a free-text query. The input is tokenized
/// and each token is scored independently against a concatenated
/// searchable string; the final score is the normalized sum.
pub fn search_endpoint(
    spec: &ParsedSpec,
    query: &str,
    method_filter: Option<&str>,
    max_results: usize,
    threshold: f32,
) -> Vec<EndpointSearchHit> {
    use nucleo_matcher::{Config as NucleoConfig, Matcher, Utf32Str};
    use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};

    let method_filter = method_filter.and_then(HttpMethod::parse);

    let mut matcher = Matcher::new(NucleoConfig::DEFAULT);

    let candidates: Vec<(&Endpoint, String)> = spec
        .endpoints
        .iter()
        .filter(|e| match method_filter {
            None => true,
            Some(m) => e.method == m,
        })
        .map(|e| {
            let mut haystack = String::new();
            haystack.push_str(&e.path);
            haystack.push(' ');
            haystack.push_str(&e.operation_id);
            haystack.push(' ');
            if let Some(s) = &e.summary {
                haystack.push_str(s);
                haystack.push(' ');
            }
            if let Some(d) = &e.description {
                haystack.push_str(d);
                haystack.push(' ');
            }
            for t in &e.tags {
                haystack.push_str(t);
                haystack.push(' ');
            }
            (e, haystack)
        })
        .collect();

    let pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );

    // Score each haystack with nucleo.
    let mut scored: Vec<(f32, &Endpoint, String)> = Vec::new();
    let mut hay_buf = Vec::new();
    for (ep, hay) in &candidates {
        hay_buf.clear();
        let hay_u32 = Utf32Str::new(hay, &mut hay_buf);
        if let Some(raw) = pattern.score(hay_u32, &mut matcher) {
            let normalized = (raw as f32) / (hay.chars().count().max(1) as f32 * 16.0);
            if normalized >= threshold {
                let reason = explain_match(query, ep);
                scored.push((normalized.min(1.0), ep, reason));
            }
        }
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(max_results)
        .map(|(score, ep, reason)| EndpointSearchHit {
            operation_id: ep.operation_id.clone(),
            method: ep.method.as_str().to_string(),
            path: ep.path.clone(),
            summary: ep.summary.clone(),
            score,
            match_reason: reason,
        })
        .collect()
}

fn explain_match(query: &str, ep: &Endpoint) -> String {
    let q = query.to_ascii_lowercase();
    let mut hits = Vec::new();
    if ep.path.to_ascii_lowercase().contains(&q) {
        hits.push(format!("path matches '{}'", q));
    }
    if let Some(s) = &ep.summary {
        if s.to_ascii_lowercase().contains(&q) {
            hits.push(format!("summary matches '{}'", q));
        }
    }
    if ep.operation_id.to_ascii_lowercase().contains(&q) {
        hits.push(format!("operation_id matches '{}'", q));
    }
    if hits.is_empty() {
        "fuzzy match across path/summary/description/tags".into()
    } else {
        hits.join("; ")
    }
}
```

> **Note on scoring:** nucleo returns a raw integer score proportional to match quality × haystack length. The `/16` divisor is an empirical normalizer that keeps typical scores in `[0, 1]`; adjust after fixture exploration if it fails to discriminate. The tests set a generous `0.1` threshold to keep them robust to tuning.

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib web::application::api_spec_use_case`
Expected: all tests across both test modules pass (5 from Task 8 + 7 here = 12 total).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/web/application/api_spec_use_case.rs
git commit -m "$(cat <<'EOF'
feat(web): list_endpoints + search_endpoint in ApiSpecUseCase

list_endpoints paginates and filters by tag. search_endpoint uses
nucleo-matcher fuzzy matching against a composed haystack (path,
operation_id, summary, description, tags) with a threshold and
max_results cap, and a human-readable match_reason for the LLM.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: `ApiSpecUseCase` — `get_endpoint_details`

**Files:**
- Modify: `src/libs/colmena/src/web/application/api_spec_use_case.rs`

Emits the detailed JSON object described in the spec (path/query/header parameters, request body schema, responses, security). Looks up by `operation_id`; returns `EndpointNotFound` with a fuzzy "did_you_mean" list on miss.

- [ ] **Step 1: Write the failing tests**

Append to `src/libs/colmena/src/web/application/api_spec_use_case.rs`:

```rust
#[cfg(test)]
mod tests_details {
    use super::*;
    use crate::web::domain::{
        Endpoint, HttpMethod, ParamType, ParameterSpec, ParsedSpec, RequestBodySpec,
        ResponseSpec, SpecFormat,
    };
    use serde_json::json;
    use std::collections::HashMap;

    fn sample() -> ParsedSpec {
        ParsedSpec {
            resolved_url: "u".into(),
            input_url: "u".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "T".into(),
            version: "1".into(),
            description: None,
            servers: vec!["https://api.example.com".into()],
            endpoints: vec![Endpoint {
                operation_id: "createSubscription".into(),
                method: HttpMethod::Post,
                path: "/v1/subscriptions".into(),
                summary: Some("Create a subscription".into()),
                description: None,
                tags: vec!["billing".into()],
                path_params: Vec::new(),
                query_params: vec![ParameterSpec {
                    name: "expand".into(),
                    description: Some("Fields to expand".into()),
                    required: false,
                    param_type: ParamType::Array(Box::new(ParamType::String)),
                    style: Some("form".into()),
                    explode: Some(false),
                }],
                header_params: Vec::new(),
                request_body: Some(RequestBodySpec {
                    content_type: "application/x-www-form-urlencoded".into(),
                    required: true,
                    schema: json!({
                        "type": "object",
                        "required": ["customer"],
                        "properties": {
                            "customer": { "type": "string" },
                            "items": { "type": "array" }
                        }
                    }),
                }),
                responses: {
                    let mut m = HashMap::new();
                    m.insert(
                        "200".to_string(),
                        ResponseSpec {
                            description: Some("Success".into()),
                            content: {
                                let mut c = HashMap::new();
                                c.insert(
                                    "application/json".into(),
                                    json!({ "$ref": "#/components/schemas/Subscription" }),
                                );
                                c
                            },
                        },
                    );
                    m
                },
                security: Vec::new(),
            }],
            security_schemes: HashMap::new(),
            tags: Vec::new(),
        }
    }

    #[test]
    fn happy_path_returns_expected_shape() {
        let spec = sample();
        let details = super::super::api_spec_use_case::get_endpoint_details(
            &spec,
            "createSubscription",
        )
        .unwrap();
        assert_eq!(details["operation_id"], "createSubscription");
        assert_eq!(details["method"], "POST");
        assert_eq!(details["path"], "/v1/subscriptions");
        assert_eq!(details["path_parameters"].as_array().unwrap().len(), 0);
        assert_eq!(details["query_parameters"][0]["name"], "expand");
        assert_eq!(details["query_parameters"][0]["type"], "array");
        assert_eq!(
            details["request_body"]["content_type"],
            "application/x-www-form-urlencoded"
        );
        assert_eq!(details["responses"]["200"]["description"], "Success");
    }

    #[test]
    fn missing_operation_suggests_candidates() {
        let spec = sample();
        let err = super::super::api_spec_use_case::get_endpoint_details(
            &spec,
            "createSubscrpition", // typo
        )
        .unwrap_err();
        match err {
            WebDomainError::EndpointNotFound { did_you_mean, .. } => {
                assert!(did_you_mean.iter().any(|s| s == "createSubscription"));
            }
            other => panic!("expected EndpointNotFound, got {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib web::application::api_spec_use_case::tests_details 2>&1 | tail -10`
Expected: compile error — `get_endpoint_details` undefined.

- [ ] **Step 3: Implement `get_endpoint_details`**

Add to `src/libs/colmena/src/web/application/api_spec_use_case.rs`:

```rust
use serde_json::{json, Value};

pub fn get_endpoint_details(
    spec: &ParsedSpec,
    operation_id: &str,
) -> Result<Value, WebDomainError> {
    let ep = match spec.endpoints.iter().find(|e| e.operation_id == operation_id) {
        Some(e) => e,
        None => {
            // Return top-3 fuzzy suggestions.
            let hits = search_endpoint(spec, operation_id, None, 3, 0.0);
            return Err(WebDomainError::EndpointNotFound {
                searched_for: operation_id.to_string(),
                did_you_mean: hits.into_iter().map(|h| h.operation_id).collect(),
            });
        }
    };

    fn params_to_json(params: &[ParameterSpec]) -> Value {
        Value::Array(
            params
                .iter()
                .map(|p| {
                    json!({
                        "name": p.name,
                        "type": param_type_str(&p.param_type),
                        "required": p.required,
                        "description": p.description,
                        "style": p.style,
                        "explode": p.explode,
                    })
                })
                .collect(),
        )
    }

    fn param_type_str(t: &ParamType) -> String {
        match t {
            ParamType::String => "string".into(),
            ParamType::Integer => "integer".into(),
            ParamType::Number => "number".into(),
            ParamType::Boolean => "boolean".into(),
            ParamType::Array(_) => "array".into(),
            ParamType::Object => "object".into(),
            ParamType::Unknown => "string".into(),
        }
    }

    let request_body = ep.request_body.as_ref().map(|rb| {
        json!({
            "content_type": rb.content_type,
            "required": rb.required,
            "schema": rb.schema,
        })
    });

    let responses = {
        let mut m = serde_json::Map::new();
        for (code, r) in &ep.responses {
            let content: serde_json::Map<String, Value> = r
                .content
                .iter()
                .map(|(ct, schema)| (ct.clone(), schema.clone()))
                .collect();
            m.insert(
                code.clone(),
                json!({
                    "description": r.description,
                    "content": Value::Object(content),
                }),
            );
        }
        Value::Object(m)
    };

    let security = Value::Array(
        ep.security
            .iter()
            .map(|s| {
                json!({ "scheme": s.scheme, "scopes": s.scopes })
            })
            .collect(),
    );

    Ok(json!({
        "operation_id": ep.operation_id,
        "method": ep.method.as_str(),
        "path": ep.path,
        "summary": ep.summary,
        "description": ep.description,
        "path_parameters": params_to_json(&ep.path_params),
        "query_parameters": params_to_json(&ep.query_params),
        "header_parameters": params_to_json(&ep.header_params),
        "request_body": request_body,
        "responses": responses,
        "security": security,
    }))
}
```

Add the required domain error variant in `src/libs/colmena/src/web/domain/errors.rs` (if Plan 0 did not include it already):

```rust
    #[error("endpoint not found: {searched_for}")]
    EndpointNotFound {
        searched_for: String,
        did_you_mean: Vec<String>,
    },
```

and make it LLM-recoverable:

```rust
            Self::EndpointNotFound { .. } => true,
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib web::application::api_spec_use_case`
Expected: 14 tests pass (5 from Task 8 + 7 from Task 9 + 2 here).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/web/application/api_spec_use_case.rs \
        src/libs/colmena/src/web/domain/errors.rs
git commit -m "$(cat <<'EOF'
feat(web): get_endpoint_details returns verbose endpoint JSON

Emits the shape documented in Spec C: operation_id, method, path,
parameters broken out by location, request body (content_type +
schema), responses keyed by status code, and security requirements.
Missing operation_id returns EndpointNotFound with a fuzzy
did_you_mean list.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: `ApiSpecUseCase` — `build_http_request`

**Files:**
- Modify: `src/libs/colmena/src/web/application/api_spec_use_case.rs`

The meat of the node. Given an endpoint + parameter map + optional secret ref, emit the JSON shape the `http_request` node consumes. Covers:

- Path parameter substitution.
- Query + header parameter routing with array `style` honored (CSV for `form,explode=false`; repeated keys for `form,explode=true`; space/pipe delimited).
- Body encoding per content-type:
  - `application/json` → serialized JSON object.
  - `application/x-www-form-urlencoded` → percent-encoded `a=1&b[0][c]=…` (Stripe bracket style).
  - `multipart/form-data` → emit the body fields as an object with a `__multipart: true` marker; the `http_request` node's multipart path already understands this shape.
- Missing required → `MissingRequiredParams` with hints.
- Wrong type → `InvalidParamType`.
- Missing auth → `MissingAuth` when security is declared and `auth_secret_ref` is absent.
- Auth header emission: `Authorization: Bearer ${SECURE:<ref>}` for Http Bearer, `Authorization: Basic ${SECURE:<ref>}` for Basic, `X-API-Key: ${SECURE:<ref>}` or `?api_key=${SECURE:<ref>}` for ApiKey, same as Bearer for OAuth2.

- [ ] **Step 1: Write the failing tests**

Append to `src/libs/colmena/src/web/application/api_spec_use_case.rs`:

```rust
#[cfg(test)]
mod tests_build_http_request {
    use super::*;
    use crate::web::domain::{
        ApiKeyLocation, Endpoint, HttpMethod, ParamType, ParameterSpec, ParsedSpec,
        RequestBodySpec, ResponseSpec, SecurityRequirement, SecurityScheme, SpecFormat,
    };
    use serde_json::json;
    use std::collections::HashMap;

    fn spec_stripe_like() -> ParsedSpec {
        let mut security_schemes = HashMap::new();
        security_schemes.insert(
            "BearerAuth".to_string(),
            SecurityScheme::Http {
                scheme: "bearer".into(),
                bearer_format: None,
            },
        );
        ParsedSpec {
            resolved_url: "u".into(),
            input_url: "u".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "Stripe".into(),
            version: "x".into(),
            description: None,
            servers: vec!["https://api.stripe.com".into()],
            endpoints: vec![Endpoint {
                operation_id: "PostSubscriptions".into(),
                method: HttpMethod::Post,
                path: "/v1/subscriptions".into(),
                summary: None,
                description: None,
                tags: Vec::new(),
                path_params: Vec::new(),
                query_params: Vec::new(),
                header_params: Vec::new(),
                request_body: Some(RequestBodySpec {
                    content_type: "application/x-www-form-urlencoded".into(),
                    required: true,
                    schema: json!({
                        "type": "object",
                        "required": ["customer"],
                        "properties": {
                            "customer": { "type": "string" },
                            "items[0][price]": { "type": "string" }
                        }
                    }),
                }),
                responses: HashMap::new(),
                security: vec![SecurityRequirement {
                    scheme: "BearerAuth".into(),
                    scopes: Vec::new(),
                }],
            }],
            security_schemes,
            tags: Vec::new(),
        }
    }

    fn spec_pet_by_id() -> ParsedSpec {
        ParsedSpec {
            resolved_url: "u".into(),
            input_url: "u".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "Pet".into(),
            version: "1".into(),
            description: None,
            servers: vec!["https://api.example.com".into()],
            endpoints: vec![Endpoint {
                operation_id: "getPetById".into(),
                method: HttpMethod::Get,
                path: "/pet/{petId}".into(),
                summary: None,
                description: None,
                tags: Vec::new(),
                path_params: vec![ParameterSpec {
                    name: "petId".into(),
                    description: None,
                    required: true,
                    param_type: ParamType::Integer,
                    style: None,
                    explode: None,
                }],
                query_params: Vec::new(),
                header_params: Vec::new(),
                request_body: None,
                responses: HashMap::new(),
                security: Vec::new(),
            }],
            security_schemes: HashMap::new(),
            tags: Vec::new(),
        }
    }

    fn spec_search_with_array_query() -> ParsedSpec {
        ParsedSpec {
            resolved_url: "u".into(),
            input_url: "u".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "S".into(),
            version: "1".into(),
            description: None,
            servers: vec!["https://api.example.com".into()],
            endpoints: vec![Endpoint {
                operation_id: "search".into(),
                method: HttpMethod::Get,
                path: "/items".into(),
                summary: None,
                description: None,
                tags: Vec::new(),
                path_params: Vec::new(),
                query_params: vec![ParameterSpec {
                    name: "ids".into(),
                    description: None,
                    required: false,
                    param_type: ParamType::Array(Box::new(ParamType::String)),
                    style: Some("form".into()),
                    explode: Some(false),
                }],
                header_params: Vec::new(),
                request_body: None,
                responses: HashMap::new(),
                security: Vec::new(),
            }],
            security_schemes: HashMap::new(),
            tags: Vec::new(),
        }
    }

    #[test]
    fn happy_path_stripe_post_subscriptions() {
        let spec = spec_stripe_like();
        let req = super::super::api_spec_use_case::build_http_request(
            &spec,
            "PostSubscriptions",
            &json!({
                "customer": "cus_ABC",
                "items[0][price]": "price_XYZ"
            }),
            Some("stripe_key"),
        )
        .unwrap();
        assert_eq!(req["url"], "https://api.stripe.com/v1/subscriptions");
        assert_eq!(req["method"], "POST");
        assert_eq!(
            req["headers"]["Authorization"],
            "Bearer ${SECURE:stripe_key}"
        );
        assert_eq!(
            req["headers"]["Content-Type"],
            "application/x-www-form-urlencoded"
        );
        let body = req["body"].as_str().unwrap();
        assert!(body.contains("customer=cus_ABC"));
        assert!(body.contains("items%5B0%5D%5Bprice%5D=price_XYZ"));
    }

    #[test]
    fn missing_required_body_field_returns_error() {
        let spec = spec_stripe_like();
        let err = super::super::api_spec_use_case::build_http_request(
            &spec,
            "PostSubscriptions",
            &json!({ "items[0][price]": "price_XYZ" }),
            Some("stripe_key"),
        )
        .unwrap_err();
        match err {
            WebDomainError::MissingRequiredParams { missing, .. } => {
                assert!(missing.contains(&"customer".to_string()));
            }
            other => panic!("expected MissingRequiredParams, got {other:?}"),
        }
    }

    #[test]
    fn missing_auth_when_endpoint_requires_it() {
        let spec = spec_stripe_like();
        let err = super::super::api_spec_use_case::build_http_request(
            &spec,
            "PostSubscriptions",
            &json!({ "customer": "cus_ABC" }),
            None,
        )
        .unwrap_err();
        assert!(matches!(err, WebDomainError::MissingAuth { .. }));
    }

    #[test]
    fn path_parameter_is_substituted() {
        let spec = spec_pet_by_id();
        let req = super::super::api_spec_use_case::build_http_request(
            &spec,
            "getPetById",
            &json!({ "petId": 42 }),
            None,
        )
        .unwrap();
        assert_eq!(req["url"], "https://api.example.com/pet/42");
        assert_eq!(req["method"], "GET");
    }

    #[test]
    fn missing_required_path_parameter_returns_error() {
        let spec = spec_pet_by_id();
        let err = super::super::api_spec_use_case::build_http_request(
            &spec,
            "getPetById",
            &json!({}),
            None,
        )
        .unwrap_err();
        assert!(matches!(err, WebDomainError::MissingRequiredParams { .. }));
    }

    #[test]
    fn integer_param_accepts_string_coercion() {
        let spec = spec_pet_by_id();
        let req = super::super::api_spec_use_case::build_http_request(
            &spec,
            "getPetById",
            &json!({ "petId": "42" }),
            None,
        )
        .unwrap();
        assert_eq!(req["url"], "https://api.example.com/pet/42");
    }

    #[test]
    fn integer_param_rejects_non_numeric_string() {
        let spec = spec_pet_by_id();
        let err = super::super::api_spec_use_case::build_http_request(
            &spec,
            "getPetById",
            &json!({ "petId": "not-a-number" }),
            None,
        )
        .unwrap_err();
        assert!(matches!(err, WebDomainError::InvalidParamType { .. }));
    }

    #[test]
    fn array_query_csv_serializes_comma_separated() {
        let spec = spec_search_with_array_query();
        let req = super::super::api_spec_use_case::build_http_request(
            &spec,
            "search",
            &json!({ "ids": ["a", "b", "c"] }),
            None,
        )
        .unwrap();
        assert_eq!(req["query_params"]["ids"], "a,b,c");
    }

    #[test]
    fn missing_auth_scheme_definition_is_error() {
        // Endpoint declares "GhostAuth" but the spec has no matching security scheme.
        let mut spec = spec_stripe_like();
        spec.security_schemes.clear();
        let err = super::super::api_spec_use_case::build_http_request(
            &spec,
            "PostSubscriptions",
            &json!({ "customer": "cus" }),
            Some("key"),
        )
        .unwrap_err();
        match err {
            WebDomainError::InvalidConfig(msg) => assert!(msg.contains("BearerAuth")),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn api_key_scheme_with_header_location() {
        let mut spec = spec_stripe_like();
        spec.security_schemes.clear();
        spec.security_schemes.insert(
            "KeyHdr".into(),
            SecurityScheme::ApiKey {
                name: "X-API-Key".into(),
                location: ApiKeyLocation::Header,
            },
        );
        spec.endpoints[0].security = vec![SecurityRequirement {
            scheme: "KeyHdr".into(),
            scopes: Vec::new(),
        }];
        let req = super::super::api_spec_use_case::build_http_request(
            &spec,
            "PostSubscriptions",
            &json!({ "customer": "cus_ABC" }),
            Some("my_key"),
        )
        .unwrap();
        assert_eq!(req["headers"]["X-API-Key"], "${SECURE:my_key}");
    }

    #[test]
    fn api_key_scheme_with_query_location() {
        let mut spec = spec_stripe_like();
        spec.security_schemes.clear();
        spec.security_schemes.insert(
            "KeyQ".into(),
            SecurityScheme::ApiKey {
                name: "api_key".into(),
                location: ApiKeyLocation::Query,
            },
        );
        spec.endpoints[0].security = vec![SecurityRequirement {
            scheme: "KeyQ".into(),
            scopes: Vec::new(),
        }];
        let req = super::super::api_spec_use_case::build_http_request(
            &spec,
            "PostSubscriptions",
            &json!({ "customer": "cus_ABC" }),
            Some("my_key"),
        )
        .unwrap();
        assert_eq!(req["query_params"]["api_key"], "${SECURE:my_key}");
    }
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib web::application::api_spec_use_case::tests_build_http_request 2>&1 | tail -10`
Expected: compile error — `build_http_request` and some error variants not yet present.

- [ ] **Step 3: Ensure the error variants exist**

In `src/libs/colmena/src/web/domain/errors.rs`, confirm these variants (add any missing):

```rust
    #[error("missing required parameters: {missing:?}")]
    MissingRequiredParams {
        missing: Vec<String>,
        hints: Option<String>,
    },

    #[error("invalid param type: {param} — expected {expected_type}, got {got}")]
    InvalidParamType {
        param: String,
        expected_type: String,
        got: String,
    },

    #[error("missing auth for scheme {scheme}")]
    MissingAuth { scheme: String, message: String },
```

All three LLM-recoverable:

```rust
            Self::MissingRequiredParams { .. } => true,
            Self::InvalidParamType { .. } => true,
            Self::MissingAuth { .. } => true,
```

- [ ] **Step 4: Implement `build_http_request`**

Append to `src/libs/colmena/src/web/application/api_spec_use_case.rs`:

```rust
use crate::web::domain::{ApiKeyLocation, SecurityScheme};

pub fn build_http_request(
    spec: &ParsedSpec,
    operation_id: &str,
    params: &Value,
    auth_secret_ref: Option<&str>,
) -> Result<Value, WebDomainError> {
    let ep = spec
        .endpoints
        .iter()
        .find(|e| e.operation_id == operation_id)
        .ok_or_else(|| WebDomainError::EndpointNotFound {
            searched_for: operation_id.to_string(),
            did_you_mean: search_endpoint(spec, operation_id, None, 3, 0.0)
                .into_iter()
                .map(|h| h.operation_id)
                .collect(),
        })?;

    let params_obj = params
        .as_object()
        .cloned()
        .unwrap_or_else(serde_json::Map::new);

    // ---- Path parameters ----
    let mut url_path = ep.path.clone();
    let mut missing: Vec<String> = Vec::new();
    for p in &ep.path_params {
        match params_obj.get(&p.name) {
            Some(v) => {
                let coerced = coerce_scalar(v, &p.param_type, &p.name)?;
                url_path = url_path.replace(&format!("{{{}}}", p.name), &coerced);
            }
            None if p.required => missing.push(p.name.clone()),
            None => {}
        }
    }

    // ---- Query parameters ----
    let mut query_params = serde_json::Map::new();
    for p in &ep.query_params {
        match params_obj.get(&p.name) {
            Some(v) => {
                query_params.insert(p.name.clone(), encode_param(v, p)?);
            }
            None if p.required => missing.push(p.name.clone()),
            None => {}
        }
    }

    // ---- Header parameters ----
    let mut headers = serde_json::Map::new();
    for p in &ep.header_params {
        match params_obj.get(&p.name) {
            Some(v) => {
                let coerced = coerce_scalar(v, &p.param_type, &p.name)?;
                headers.insert(p.name.clone(), Value::String(coerced));
            }
            None if p.required => missing.push(p.name.clone()),
            None => {}
        }
    }

    // ---- Body ----
    let mut body_value: Value = Value::Null;
    if let Some(rb) = &ep.request_body {
        let (body, body_missing) = build_body(rb, &params_obj)?;
        if rb.required {
            for m in body_missing {
                missing.push(m);
            }
        }
        body_value = body;
        headers.insert("Content-Type".into(), Value::String(rb.content_type.clone()));
    }

    if !missing.is_empty() {
        return Err(WebDomainError::MissingRequiredParams {
            missing,
            hints: Some(format!(
                "The listed parameters are required for {}. Check get_endpoint_details for descriptions.",
                ep.operation_id
            )),
        });
    }

    // ---- Security ----
    if !ep.security.is_empty() {
        let required = &ep.security[0]; // use the first security option
        let scheme = spec.security_schemes.get(&required.scheme).ok_or_else(|| {
            WebDomainError::InvalidConfig(format!(
                "endpoint declares security scheme '{}' but the spec has no matching entry in components.securitySchemes",
                required.scheme
            ))
        })?;
        let secret_ref = auth_secret_ref.ok_or_else(|| WebDomainError::MissingAuth {
            scheme: required.scheme.clone(),
            message: format!(
                "endpoint '{}' requires auth scheme '{}' but no auth_secret_ref was provided",
                ep.operation_id, required.scheme
            ),
        })?;
        apply_security_scheme(scheme, secret_ref, &mut headers, &mut query_params);
    }

    // ---- Base URL ----
    let base = spec
        .servers
        .first()
        .cloned()
        .ok_or_else(|| WebDomainError::InvalidConfig(
            "spec has no servers[]; set default_base_url_override in the node config".into(),
        ))?;
    let url = format!("{}{}", base.trim_end_matches('/'), url_path);

    Ok(json!({
        "url": url,
        "method": ep.method.as_str(),
        "headers": headers,
        "query_params": query_params,
        "body": body_value,
    }))
}

fn coerce_scalar(v: &Value, ty: &ParamType, name: &str) -> Result<String, WebDomainError> {
    match (v, ty) {
        (Value::String(s), ParamType::Integer) => {
            s.parse::<i64>()
                .map(|n| n.to_string())
                .map_err(|_| WebDomainError::InvalidParamType {
                    param: name.into(),
                    expected_type: "integer".into(),
                    got: format!("\"{s}\""),
                })
        }
        (Value::String(s), ParamType::Number) => {
            s.parse::<f64>()
                .map(|n| n.to_string())
                .map_err(|_| WebDomainError::InvalidParamType {
                    param: name.into(),
                    expected_type: "number".into(),
                    got: format!("\"{s}\""),
                })
        }
        (Value::Number(n), ParamType::Integer) => Ok(n.as_i64()
            .map(|i| i.to_string())
            .unwrap_or_else(|| n.to_string())),
        (Value::Number(n), ParamType::Number) => Ok(n.to_string()),
        (Value::Bool(b), ParamType::Boolean) => Ok(b.to_string()),
        (Value::String(s), ParamType::Boolean) => match s.as_str() {
            "true" | "false" => Ok(s.clone()),
            other => Err(WebDomainError::InvalidParamType {
                param: name.into(),
                expected_type: "boolean".into(),
                got: format!("\"{other}\""),
            }),
        },
        (Value::String(s), _) => Ok(s.clone()),
        (Value::Number(n), _) => Ok(n.to_string()),
        (Value::Bool(b), _) => Ok(b.to_string()),
        (Value::Null, _) => Err(WebDomainError::InvalidParamType {
            param: name.into(),
            expected_type: format!("{ty:?}"),
            got: "null".into(),
        }),
        (other, _) => Err(WebDomainError::InvalidParamType {
            param: name.into(),
            expected_type: format!("{ty:?}"),
            got: format!("{other:?}"),
        }),
    }
}

fn encode_param(v: &Value, p: &ParameterSpec) -> Result<Value, WebDomainError> {
    if let ParamType::Array(inner) = &p.param_type {
        let arr = v.as_array().ok_or_else(|| WebDomainError::InvalidParamType {
            param: p.name.clone(),
            expected_type: "array".into(),
            got: format!("{v:?}"),
        })?;
        let strs: Result<Vec<String>, WebDomainError> = arr
            .iter()
            .map(|e| coerce_scalar(e, inner, &p.name))
            .collect();
        let strs = strs?;
        let style = p.style.as_deref().unwrap_or("form");
        let explode = p.explode.unwrap_or(true);
        let encoded = match (style, explode) {
            ("form", false) => strs.join(","),
            ("form", true) => strs.join(","), // explode=true is consumed by the http_request node as array; keep CSV as a safe default for transport
            ("spaceDelimited", _) => strs.join(" "),
            ("pipeDelimited", _) => strs.join("|"),
            _ => strs.join(","),
        };
        Ok(Value::String(encoded))
    } else {
        Ok(Value::String(coerce_scalar(v, &p.param_type, &p.name)?))
    }
}

fn build_body(
    rb: &RequestBodySpec,
    params: &serde_json::Map<String, Value>,
) -> Result<(Value, Vec<String>), WebDomainError> {
    match rb.content_type.as_str() {
        "application/json" => Ok(build_body_json(rb, params)),
        "application/x-www-form-urlencoded" => Ok(build_body_form_urlencoded(rb, params)),
        "multipart/form-data" => Ok(build_body_multipart(rb, params)),
        other => Err(WebDomainError::InvalidConfig(format!(
            "unsupported request body content_type: {other}"
        ))),
    }
}

fn required_fields_from_schema(schema: &Value) -> Vec<String> {
    schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn schema_property_names(schema: &Value) -> Vec<String> {
    schema
        .get("properties")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

fn build_body_json(
    rb: &RequestBodySpec,
    params: &serde_json::Map<String, Value>,
) -> (Value, Vec<String>) {
    let required = required_fields_from_schema(&rb.schema);
    let mut missing = Vec::new();
    for name in &required {
        if !params.contains_key(name) {
            missing.push(name.clone());
        }
    }
    let props = schema_property_names(&rb.schema);
    let mut body = serde_json::Map::new();
    for name in props {
        if let Some(v) = params.get(&name) {
            body.insert(name, v.clone());
        }
    }
    // Any extra params not in the schema are still allowed (many specs are
    // partial); include them verbatim.
    for (k, v) in params {
        if !body.contains_key(k) {
            body.insert(k.clone(), v.clone());
        }
    }
    (Value::Object(body), missing)
}

fn build_body_form_urlencoded(
    rb: &RequestBodySpec,
    params: &serde_json::Map<String, Value>,
) -> (Value, Vec<String>) {
    let required = required_fields_from_schema(&rb.schema);
    let mut missing = Vec::new();
    for name in &required {
        if !params.contains_key(name) {
            missing.push(name.clone());
        }
    }
    let pairs: Vec<String> = params
        .iter()
        .map(|(k, v)| {
            let vs = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            format!("{}={}", percent_encode(k), percent_encode(&vs))
        })
        .collect();
    (Value::String(pairs.join("&")), missing)
}

fn build_body_multipart(
    rb: &RequestBodySpec,
    params: &serde_json::Map<String, Value>,
) -> (Value, Vec<String>) {
    let required = required_fields_from_schema(&rb.schema);
    let mut missing = Vec::new();
    for name in &required {
        if !params.contains_key(name) {
            missing.push(name.clone());
        }
    }
    let mut out = params.clone();
    out.insert("__multipart".into(), Value::Bool(true));
    (Value::Object(out), missing)
}

fn percent_encode(s: &str) -> String {
    // RFC3986 unreserved: A-Z a-z 0-9 - . _ ~
    // application/x-www-form-urlencoded also permits +/= but we stay strict
    // and percent-encode everything else.
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        let c = *b as char;
        if c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' || c == '~' {
            out.push(c);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn apply_security_scheme(
    scheme: &SecurityScheme,
    secret_ref: &str,
    headers: &mut serde_json::Map<String, Value>,
    query_params: &mut serde_json::Map<String, Value>,
) {
    let placeholder = format!("${{SECURE:{secret_ref}}}");
    match scheme {
        SecurityScheme::Http { scheme: s, .. } => {
            let prefix = if s.eq_ignore_ascii_case("basic") { "Basic" } else { "Bearer" };
            headers.insert(
                "Authorization".into(),
                Value::String(format!("{prefix} {placeholder}")),
            );
        }
        SecurityScheme::ApiKey { name, location } => match location {
            ApiKeyLocation::Header => {
                headers.insert(name.clone(), Value::String(placeholder));
            }
            ApiKeyLocation::Query => {
                query_params.insert(name.clone(), Value::String(placeholder));
            }
            ApiKeyLocation::Cookie => {
                headers.insert(
                    "Cookie".into(),
                    Value::String(format!("{name}={placeholder}")),
                );
            }
        },
        SecurityScheme::OAuth2 { .. } | SecurityScheme::OpenIdConnect { .. } => {
            headers.insert(
                "Authorization".into(),
                Value::String(format!("Bearer {placeholder}")),
            );
        }
    }
}
```

- [ ] **Step 5: Run — expect PASS**

Run: `cargo test --lib web::application::api_spec_use_case`
Expected: 25 tests pass across all four `tests*` modules (5 + 7 + 2 + 11).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/web/application/api_spec_use_case.rs \
        src/libs/colmena/src/web/domain/errors.rs
git commit -m "$(cat <<'EOF'
feat(web): build_http_request routes params + applies auth

Substitutes path params with type coercion, encodes query arrays per
style/explode, composes the request body per content-type (JSON,
x-www-form-urlencoded with Stripe bracket notation, multipart via
__multipart marker). Emits Authorization / X-API-Key / query api_key
with \${SECURE:<ref>} placeholders that the http_request node
resolves at execute time. Missing required / invalid type / missing
auth all surface as LLM-recoverable domain errors.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Node skeleton — `ApiExplorerNode` + `sub_tool_catalog` (all five sub-tools)

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`

This task delivers the node struct, the `ConversationLifecycleSubscriber` impl, `sub_tool_catalog` returning all five sub-tool definitions, and a dispatch stub. Handlers are added in Tasks 13-15.

- [ ] **Step 1: Write the failing catalog test**

Create `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`:

```rust
//! `api_explorer` toolkit node. Exposes five LLM sub-tools — `load_spec`,
//! `list_endpoints`, `search_endpoint`, `get_endpoint_details`,
//! `build_http_request` — over a cached OpenAPI 3.x / Swagger 2.0 spec.
//!
//! Spec: docs/superpowers/specs/2026-04-23-web-nodes-c-api-explorer-design.md
//!
//! The node holds a single [`ApiSpecUseCase`] plus its shared
//! [`SessionRegistry`] so per-conversation spec caches survive across
//! sub-tool calls. It subscribes to [`ConversationLifecycleBus`] so the
//! registry is evicted eagerly when a conversation closes.

use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::domain::toolkit_node::{SubToolDefinition, ToolkitNode, SUB_TOOL_INPUT_KEY};
use crate::llm::domain::ParameterProperty;
use crate::web::application::api_spec_use_case::{
    ApiSpecUseCase, ApiSpecUseCaseConfig, SpecCache,
};
use crate::web::domain::api_spec_port::ApiSpecPort;
use crate::web::domain::lifecycle::ConversationLifecycleSubscriber;
use crate::web::domain::session_registry::{SessionRegistry, TtlConfig};
use crate::web::infrastructure::openapi_adapter::OpenApiAdapter;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::Arc;
use std::time::Duration;

/// `api_explorer` node.
///
/// Construction happens once at registry build time. The adapter is
/// stateless (no API key) so a single `ApiSpecUseCase` is shared across
/// calls. The per-conversation cache lives inside the
/// `SessionRegistry<Arc<SpecCache>>` owned by the use case.
pub struct ApiExplorerNode {
    use_case: Arc<ApiSpecUseCase>,
    registry: Arc<SessionRegistry<Arc<SpecCache>>>,
    secure_values: Option<Arc<SecureValueService>>,
}

impl ApiExplorerNode {
    pub fn new() -> Self {
        let port: Arc<dyn ApiSpecPort> = Arc::new(OpenApiAdapter::default());
        let registry = Arc::new(SessionRegistry::<Arc<SpecCache>>::new(
            "api_explorer_specs",
            TtlConfig::default(),
        ));
        let cfg = ApiSpecUseCaseConfig::default();
        let use_case = Arc::new(ApiSpecUseCase::new(port, registry.clone(), cfg));
        Self {
            use_case,
            registry,
            secure_values: None,
        }
    }

    /// Build a node with a custom port — used by tests that inject a
    /// `CountingPort` or similar. Not part of the public API.
    #[cfg(test)]
    pub(crate) fn new_with_port(port: Arc<dyn ApiSpecPort>) -> Self {
        let registry = Arc::new(SessionRegistry::<Arc<SpecCache>>::new(
            "api_explorer_specs",
            TtlConfig::default(),
        ));
        let cfg = ApiSpecUseCaseConfig::default();
        let use_case = Arc::new(ApiSpecUseCase::new(port, registry.clone(), cfg));
        Self {
            use_case,
            registry,
            secure_values: None,
        }
    }

    pub fn with_secure_values(mut self, svc: Arc<SecureValueService>) -> Self {
        self.secure_values = Some(svc);
        self
    }

    /// Registry handle so the registrar can subscribe the node to a
    /// `ConversationLifecycleBus`.
    pub fn registry(&self) -> Arc<SessionRegistry<Arc<SpecCache>>> {
        self.registry.clone()
    }

    /// Extract `conversation_id` from node inputs. Toolkit executor passes
    /// this through from the llm_call parent. Falls back to "default" so
    /// the node remains usable when the engine does not supply one.
    fn extract_conversation_id(inputs: &NodeInputs) -> String {
        inputs
            .get("conversation_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| "default".into())
    }

    /// Helper — read a required string field from the LLM's argument map.
    /// Returns a structured LLM-recoverable error JSON on miss.
    pub(crate) fn require_str<'a>(inputs: &'a NodeInputs, key: &str) -> Result<&'a str, Value> {
        match inputs.get(key).and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => Ok(s),
            _ => Err(json!({
                "error": "invalid_input",
                "missing": key,
                "message": format!("`{key}` is required (string)")
            })),
        }
    }
}

impl Default for ApiExplorerNode {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConversationLifecycleSubscriber for ApiExplorerNode {
    async fn on_conversation_closed(&self, conversation_id: &str) {
        self.registry.evict_conversation(conversation_id).await;
    }
}

#[async_trait]
impl ExecutableNode for ApiExplorerNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let sub = inputs
            .get(SUB_TOOL_INPUT_KEY)
            .and_then(|v| v.as_str())
            .ok_or("api_explorer: missing __sub_tool")?;
        Err(format!("api_explorer: sub_tool '{sub}' not implemented yet").into())
    }

    fn schema(&self) -> Value {
        json!({
            "inputs": { "__sub_tool": "string" },
            "outputs": { "output": "any" },
            "config": {
                "enable_cache": "bool (default true)",
                "cache_ttl_seconds": "u64 (default 86400)",
                "max_cached_specs": "u64 (default 100)",
                "session_idle_ttl_seconds": "u64 (default 900)",
                "session_max_lifetime_seconds": "u64 (default 3600)",
                "max_spec_size_bytes": "u64 (default 10 MiB)",
                "spec_download_timeout_seconds": "u64 (default 60)",
                "default_base_url_override": "string | null",
                "fuzzy_match_threshold": "f32 (default 0.6)",
                "retry_policy": { "max_attempts": "u32", "initial_backoff_ms": "u64" }
            }
        })
    }

    fn description(&self) -> Option<&str> {
        Some(
            "OpenAPI / Swagger 2.0 discovery + request builder. Exposes five sub-tools: \
             load_spec, list_endpoints, search_endpoint, get_endpoint_details, \
             build_http_request. Output of build_http_request is ready-to-execute input \
             for the http_request node.",
        )
    }
}

impl ToolkitNode for ApiExplorerNode {
    fn sub_tool_catalog(&self, _config: &Value) -> Vec<SubToolDefinition> {
        vec![
            load_spec_sub_tool(),
            list_endpoints_sub_tool(),
            search_endpoint_sub_tool(),
            get_endpoint_details_sub_tool(),
            build_http_request_sub_tool(),
        ]
    }
}

fn load_spec_sub_tool() -> SubToolDefinition {
    let mut props = HashMap::new();
    props.insert(
        "url".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "Absolute URL of an OpenAPI 3.x or Swagger 2.0 JSON/YAML file. \
                Git-forge blob URLs (github.com/.../blob/..., gitlab.com/.../-/blob/..., \
                bitbucket.org/.../src/...) are accepted and auto-rewritten to raw."
                .into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "force_reload".into(),
        ParameterProperty {
            property_type: "boolean".into(),
            description: "If true, bypass cache and re-download. Default false.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    SubToolDefinition {
        name: "load_spec",
        description: "Download and parse an OpenAPI 3.x or Swagger 2.0 specification from a URL. \
            Must be called before any other api_explorer tool. The parsed spec is cached for \
            the conversation so subsequent tools are fast. Returns a summary of what the spec \
            contains. You can paste Git-forge URLs — the node rewrites them to the raw-content \
            URL automatically; use `resolved_url` in the result to see what was actually fetched. \
            Swagger 2.0 documents are converted internally to OpenAPI 3.0 so all subsequent tools \
            behave identically. If the download returns HTML (usually because a Git-forge blob \
            URL could not be normalized), you get a clear error suggesting the raw URL format."
            .into(),
        properties: props,
        required: vec!["url".into()],
    }
}

fn list_endpoints_sub_tool() -> SubToolDefinition {
    let mut props = HashMap::new();
    props.insert(
        "spec_url".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "The URL of the previously-loaded spec (the `spec_url_input` you \
                passed to load_spec)."
                .into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "tag".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "Filter by tag (e.g., \"Subscriptions\").".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "limit".into(),
        ParameterProperty {
            property_type: "integer".into(),
            description: "Page size. Default 50, max 200.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "offset".into(),
        ParameterProperty {
            property_type: "integer".into(),
            description: "Pagination offset. Default 0.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    SubToolDefinition {
        name: "list_endpoints",
        description: "List all endpoints in a previously loaded spec. Prefer `search_endpoint` \
            unless you want to browse by category. Results are paginated. If you do not know \
            which tags exist, call `load_spec` first — its result lists them."
            .into(),
        properties: props,
        required: vec!["spec_url".into()],
    }
}

fn search_endpoint_sub_tool() -> SubToolDefinition {
    let mut props = HashMap::new();
    props.insert(
        "spec_url".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "The URL of the previously-loaded spec.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "query".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "Free-text query, e.g. \"create subscription\", \"list customers\".".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "method".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "Filter by HTTP method.".into(),
            enum_values: Some(vec![
                "GET".into(),
                "POST".into(),
                "PUT".into(),
                "PATCH".into(),
                "DELETE".into(),
            ]),
            pattern: None,
        },
    );
    props.insert(
        "max_results".into(),
        ParameterProperty {
            property_type: "integer".into(),
            description: "Default 10, max 50.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    SubToolDefinition {
        name: "search_endpoint",
        description: "Find endpoints by keyword. Matches against path, summary, description, \
            operation_id, and tags. Uses fuzzy matching so typos and reordered words still work. \
            Returns the best ranked matches with relevance scores. Prefer this over \
            `list_endpoints` when you have any idea what you are looking for."
            .into(),
        properties: props,
        required: vec!["spec_url".into(), "query".into()],
    }
}

fn get_endpoint_details_sub_tool() -> SubToolDefinition {
    let mut props = HashMap::new();
    props.insert(
        "spec_url".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "The URL of the previously-loaded spec.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "operation_id".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "The operation id from `search_endpoint` or `list_endpoints`.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    SubToolDefinition {
        name: "get_endpoint_details",
        description: "Retrieve the full specification of a single endpoint: parameters (path, \
            query, headers), request body schema, response schemas, and required auth. Call this \
            before `build_http_request` if you need to know what arguments are required. If the \
            operation_id is wrong, the result includes a `did_you_mean` list of the nearest \
            matches so you can retry."
            .into(),
        properties: props,
        required: vec!["spec_url".into(), "operation_id".into()],
    }
}

fn build_http_request_sub_tool() -> SubToolDefinition {
    let mut props = HashMap::new();
    props.insert(
        "spec_url".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "The URL of the previously-loaded spec.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "operation_id".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "The operation id from `search_endpoint` or `list_endpoints`.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "params".into(),
        ParameterProperty {
            property_type: "object".into(),
            description: "A flat map of parameter values. Path params, query params, header \
                params, and body fields are all resolved from the same map. The node routes each \
                to the right location based on the spec."
                .into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "auth_secret_ref".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "Name of a Secure Value containing the token / API key. Required if \
                the endpoint declares auth. The name ends up as a `${SECURE:<name>}` placeholder \
                in the returned headers, which the http_request node resolves at execute time."
                .into(),
            enum_values: None,
            pattern: None,
        },
    );
    SubToolDefinition {
        name: "build_http_request",
        description: "Build a validated HTTP-request configuration for a specific endpoint. \
            The output is a JSON object in the exact shape the `http_request` node accepts — \
            pass it as the input to an `http_request` call to execute. Missing required \
            parameters or wrong types return an error with hints; do not invent values to make \
            the error go away — the hint tells you exactly what to ask the user for."
            .into(),
        properties: props,
        required: vec!["spec_url".into(), "operation_id".into(), "params".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_all_five_sub_tools() {
        let node = ApiExplorerNode::new();
        let cat = node.sub_tool_catalog(&json!({}));
        assert_eq!(cat.len(), 5);
        let names: Vec<&str> = cat.iter().map(|s| s.name).collect();
        for expected in [
            "load_spec",
            "list_endpoints",
            "search_endpoint",
            "get_endpoint_details",
            "build_http_request",
        ] {
            assert!(names.contains(&expected), "missing sub-tool {expected}");
        }
    }

    #[test]
    fn load_spec_requires_url() {
        let node = ApiExplorerNode::new();
        let cat = node.sub_tool_catalog(&json!({}));
        let s = cat.iter().find(|s| s.name == "load_spec").unwrap();
        assert!(s.required.contains(&"url".to_string()));
    }

    #[test]
    fn build_http_request_requires_three_fields() {
        let node = ApiExplorerNode::new();
        let cat = node.sub_tool_catalog(&json!({}));
        let s = cat.iter().find(|s| s.name == "build_http_request").unwrap();
        for k in ["spec_url", "operation_id", "params"] {
            assert!(s.required.contains(&k.to_string()), "missing required {k}");
        }
    }

    #[test]
    fn search_endpoint_exposes_method_enum() {
        let node = ApiExplorerNode::new();
        let cat = node.sub_tool_catalog(&json!({}));
        let s = cat.iter().find(|s| s.name == "search_endpoint").unwrap();
        let method = s.properties.get("method").unwrap();
        let evs = method.enum_values.as_ref().unwrap();
        for m in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
            assert!(evs.iter().any(|e| e == m));
        }
    }

    #[test]
    fn extract_conversation_id_falls_back_to_default() {
        let inputs: NodeInputs = HashMap::new();
        assert_eq!(ApiExplorerNode::extract_conversation_id(&inputs), "default");
        let mut inputs2: NodeInputs = HashMap::new();
        inputs2.insert("conversation_id".into(), json!("c-42"));
        assert_eq!(ApiExplorerNode::extract_conversation_id(&inputs2), "c-42");
    }

    #[tokio::test]
    async fn dispatch_stub_errors_until_handlers_land() {
        let node = ApiExplorerNode::new();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        let mut state = json!({});
        let err = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not implemented"));
    }
}
```

- [ ] **Step 2: Register the module**

Edit `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`. Append:

```rust
pub mod api_explorer;
```

- [ ] **Step 3: Run — expect PASS**

Run: `cargo test --lib api_explorer`
Expected: 6 tests pass (5 catalog-shape + 1 dispatch stub).

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs
git commit -m "$(cat <<'EOF'
feat(nodes): add ApiExplorerNode skeleton with 5 sub-tool catalog

Declares load_spec, list_endpoints, search_endpoint,
get_endpoint_details, and build_http_request with rich descriptions
that drive LLM selection accuracy. Node owns one shared
ApiSpecUseCase + SessionRegistry<Arc<SpecCache>> so per-conversation
caches survive across calls. ConversationLifecycleSubscriber impl
evicts cache entries when a conversation closes. execute() still
returns NotImplemented — Tasks 13-15 add the handlers.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: `ApiExplorerNode::execute` — `load_spec` handler

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `api_explorer.rs`:

```rust
    use crate::web::domain::api_spec_port::{
        ApiSpecPort, Endpoint, HttpMethod, ParsedSpec, SecurityRequirement, SecurityScheme,
        SpecFetchResult,
    };
    use crate::web::domain::errors::WebDomainError;
    use std::sync::Mutex;

    /// Minimal stub port returning a hand-built ParsedSpec.
    struct FakePort {
        calls: Mutex<u32>,
        spec: ParsedSpec,
    }

    #[async_trait::async_trait]
    impl ApiSpecPort for FakePort {
        async fn fetch_and_parse(
            &self,
            url: &str,
            _etag: Option<&str>,
            _last_modified: Option<&str>,
        ) -> Result<SpecFetchResult, WebDomainError> {
            *self.calls.lock().unwrap() += 1;
            let mut s = self.spec.clone();
            s.url = url.to_string();
            Ok(SpecFetchResult::Fresh {
                spec: s,
                etag: Some("W/\"v1\"".into()),
                last_modified: None,
                resolved_url: url.to_string(),
            })
        }
    }

    fn fake_parsed_spec() -> ParsedSpec {
        ParsedSpec {
            url: "".into(),
            title: "Petstore".into(),
            version: "1.0.0".into(),
            openapi_version: "3.0.3".into(),
            original_format: "openapi-3.x".into(),
            description: Some("Sample API.".into()),
            servers: vec!["https://petstore.example.com".into()],
            endpoints: vec![Endpoint {
                operation_id: "listPets".into(),
                method: HttpMethod::Get,
                path: "/pets".into(),
                summary: Some("List pets".into()),
                description: None,
                tags: vec!["pets".into()],
                path_params: vec![],
                query_params: vec![],
                header_params: vec![],
                request_body: None,
                responses: Default::default(),
                security: vec![SecurityRequirement {
                    scheme: "ApiKeyAuth".into(),
                    scopes: vec![],
                }],
            }],
            security_schemes: {
                let mut m = HashMap::new();
                m.insert(
                    "ApiKeyAuth".into(),
                    SecurityScheme::ApiKey {
                        name: "X-API-Key".into(),
                        location: crate::web::domain::api_spec_port::ApiKeyLocation::Header,
                    },
                );
                m
            },
            tags: vec!["pets".into()],
        }
    }

    fn node_with_fake_port() -> (Arc<FakePort>, ApiExplorerNode) {
        let port = Arc::new(FakePort {
            calls: Mutex::new(0),
            spec: fake_parsed_spec(),
        });
        let node = ApiExplorerNode::new_with_port(port.clone() as Arc<dyn ApiSpecPort>);
        (port, node)
    }

    #[tokio::test]
    async fn load_spec_returns_summary_with_resolved_url() {
        let (port, node) = node_with_fake_port();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        inputs.insert("url".into(), json!("https://example.com/petstore.yaml"));
        inputs.insert("conversation_id".into(), json!("c-1"));
        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();

        assert_eq!(
            out.get("spec_url_input").and_then(|v| v.as_str()),
            Some("https://example.com/petstore.yaml")
        );
        assert_eq!(
            out.get("resolved_url").and_then(|v| v.as_str()),
            Some("https://example.com/petstore.yaml")
        );
        assert_eq!(out.get("title").and_then(|v| v.as_str()), Some("Petstore"));
        assert_eq!(out.get("endpoints_count").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(out.get("cached").and_then(|v| v.as_bool()), Some(false));
        let schemes = out
            .get("security_schemes")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(schemes[0].as_str(), Some("ApiKeyAuth"));
        assert_eq!(*port.calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn load_spec_caches_within_conversation() {
        let (port, node) = node_with_fake_port();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        inputs.insert("url".into(), json!("https://example.com/petstore.yaml"));
        inputs.insert("conversation_id".into(), json!("c-cache"));
        let mut state = json!({});

        let first = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(first.get("cached").and_then(|v| v.as_bool()), Some(false));

        let second = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(second.get("cached").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            *port.calls.lock().unwrap(),
            1,
            "second call must hit cache, not the port"
        );
    }

    #[tokio::test]
    async fn load_spec_force_reload_bypasses_cache() {
        let (port, node) = node_with_fake_port();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        inputs.insert("url".into(), json!("https://example.com/petstore.yaml"));
        inputs.insert("conversation_id".into(), json!("c-force"));
        let mut state = json!({});

        node.execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        inputs.insert("force_reload".into(), json!(true));
        node.execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(*port.calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn load_spec_missing_url_returns_invalid_input() {
        let (_port, node) = node_with_fake_port();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        inputs.insert("conversation_id".into(), json!("c-x"));
        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("error").and_then(|v| v.as_str()),
            Some("invalid_input")
        );
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib api_explorer::tests::load_spec_returns_summary_with_resolved_url`
Expected: FAIL — `execute()` still returns the "not implemented" error; `FakePort` will not compile yet if `ApiSpecPort::fetch_and_parse` does not carry `resolved_url` on `SpecFetchResult::Fresh`. That field was added in Task 4; if the Task-4 variant does not yet carry it, go back and add it now — it is required for the spec's `load_spec` return shape.

- [ ] **Step 3: Implement `handle_load_spec` + dispatch wiring**

Edit `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`. Replace the stub `execute` body with a full dispatch, and add the handler:

```rust
#[async_trait]
impl ExecutableNode for ApiExplorerNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let sub = inputs
            .get(SUB_TOOL_INPUT_KEY)
            .and_then(|v| v.as_str())
            .ok_or("api_explorer: missing __sub_tool")?;
        let conversation_id = Self::extract_conversation_id(inputs);

        match sub {
            "load_spec" => self.handle_load_spec(inputs, config, &conversation_id).await,
            "list_endpoints" => Err("api_explorer: list_endpoints not implemented yet".into()),
            "search_endpoint" => Err("api_explorer: search_endpoint not implemented yet".into()),
            "get_endpoint_details" => {
                Err("api_explorer: get_endpoint_details not implemented yet".into())
            }
            "build_http_request" => {
                Err("api_explorer: build_http_request not implemented yet".into())
            }
            other => Ok(json!({
                "error": "unknown_sub_tool",
                "sub_tool": other,
                "message": "Use one of: load_spec, list_endpoints, search_endpoint, get_endpoint_details, build_http_request"
            })),
        }
    }

    // schema() and description() remain as before.
    fn schema(&self) -> Value {
        // ... (unchanged — keep the body from Task 12)
        unimplemented!("replace with the json! block from Task 12 verbatim")
    }
    fn description(&self) -> Option<&str> {
        // ... (unchanged — keep the string from Task 12)
        unimplemented!("replace with the string from Task 12 verbatim")
    }
}
```

> **Editor note:** Keep the existing `schema()` / `description()` bodies from Task 12 — only `execute()` changes here. The placeholder `unimplemented!` lines above are to highlight what NOT to change, not to paste literally.

Add the handler method inside `impl ApiExplorerNode` (below `require_str`):

```rust
    async fn handle_load_spec(
        &self,
        inputs: &NodeInputs,
        _config: &Value,
        conversation_id: &str,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let url = match Self::require_str(inputs, "url") {
            Ok(u) => u.to_string(),
            Err(v) => return Ok(v),
        };
        let force_reload = inputs
            .get("force_reload")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        match self
            .use_case
            .fetch_spec(conversation_id, &url, force_reload)
            .await
        {
            Ok(result) => Ok(json!({
                "spec_url_input": url,
                "resolved_url": result.resolved_url,
                "original_format": result.parsed.original_format,
                "internal_format": result.parsed.openapi_version,
                "title": result.parsed.title,
                "version": result.parsed.version,
                "description": result.parsed.description,
                "server_url": result.parsed.servers.first().cloned().unwrap_or_default(),
                "endpoints_count": result.parsed.endpoints.len(),
                "tags": result.parsed.tags,
                "security_schemes": result
                    .parsed
                    .security_schemes
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>(),
                "cached": result.was_cached
            })),
            Err(e) => Ok(format_spec_error(e)),
        }
    }
```

Add the shared error-to-LLM helper at module scope, just below the `SubToolDefinition` helpers (this helper is used by every handler):

```rust
fn format_spec_error(e: crate::web::domain::errors::WebDomainError) -> Value {
    use crate::web::domain::errors::WebDomainError::*;
    match e {
        SpecParseError { details } => json!({
            "error": "spec_parse_failed",
            "details": details,
            "message": "Spec could not be parsed as OpenAPI 3.x or Swagger 2.0."
        }),
        UnexpectedHtmlResponse { url_given, resolved_url } => json!({
            "error": "unexpected_html_response",
            "url_given": url_given,
            "resolved_url": resolved_url,
            "message": "URL returned HTML. If this is a Git forge blob URL for a lesser-known host, use the raw content URL instead."
        }),
        Swagger2ConversionFailed { reason, unsupported_feature } => json!({
            "error": "swagger2_conversion_failed",
            "reason": reason,
            "unsupported_feature": unsupported_feature,
            "message": "This Swagger 2.0 spec uses a feature the converter does not handle. Fall back to reading docs with web__fetch."
        }),
        UnsupportedSpecFormat { detected } => json!({
            "error": "unsupported_spec_format",
            "detected": detected,
            "message": "api_explorer supports OpenAPI 3.x and Swagger 2.0 only."
        }),
        EndpointNotFound { searched_for, did_you_mean } => json!({
            "error": "endpoint_not_found",
            "searched_for": searched_for,
            "did_you_mean": did_you_mean
        }),
        MissingRequiredParams { missing, hints } => json!({
            "error": "missing_required_params",
            "missing": missing,
            "hints": hints
        }),
        InvalidParamType { param, expected_type, got } => json!({
            "error": "invalid_param_type",
            "param": param,
            "expected_type": expected_type,
            "got": got
        }),
        MissingAuth { scheme, message } => json!({
            "error": "missing_auth",
            "scheme": scheme,
            "message": message
        }),
        SpecTooLarge { size_bytes, limit_bytes } => json!({
            "error": "spec_too_large",
            "size_bytes": size_bytes,
            "limit_bytes": limit_bytes
        }),
        Timeout { ms } => json!({
            "error": "fetch_failed",
            "reason": "timeout",
            "ms": ms,
            "retryable": true
        }),
        Upstream { status, body } => json!({
            "error": "fetch_failed",
            "status": status,
            "retryable": status >= 500,
            "message": body
        }),
        SpecNotLoaded => json!({
            "error": "spec_not_loaded",
            "message": "Call load_spec(url) first."
        }),
        InvalidConfig { message } => json!({
            "error": "invalid_config",
            "message": message
        }),
        other => json!({
            "error": "web_error",
            "message": other.to_string()
        }),
    }
}
```

> **Ensure** the `use_case.fetch_spec` signature from Task 8 returns a result that carries `resolved_url` and a `was_cached` flag alongside the `parsed` spec. If the Task-8 return type is only `Arc<ParsedSpec>`, widen it now to a small struct:
>
> ```rust
> pub struct FetchedSpec {
>     pub parsed: Arc<ParsedSpec>,
>     pub resolved_url: String,
>     pub was_cached: bool,
> }
> ```
>
> and update the existing Task-8 tests to read these fields.

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib api_explorer`
Expected: 10 tests pass (6 from Task 12 + 4 here).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs \
        src/libs/colmena/src/web/application/api_spec_use_case.rs \
        src/libs/colmena/src/web/domain/api_spec_port.rs
git commit -m "$(cat <<'EOF'
feat(nodes): implement api_explorer::load_spec handler

Fetches through ApiSpecUseCase with per-conversation cache. Returns
the spec summary shape the LLM expects (resolved_url, original vs
internal format, endpoints_count, tags, security_schemes, cached).
Force-reload bypasses the cache. Missing/empty URL returns a
structured invalid_input error. format_spec_error centralises the
domain-error→LLM-JSON mapping that the remaining sub-tools also use.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: `ApiExplorerNode::execute` — `list_endpoints` + `search_endpoint` handlers

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`

- [ ] **Step 1: Write the failing tests**

Append to `#[cfg(test)] mod tests`:

```rust
    #[tokio::test]
    async fn list_endpoints_returns_paginated_summary() {
        let (_port, node) = node_with_fake_port();
        // Pre-load.
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-list"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut list: NodeInputs = HashMap::new();
        list.insert(SUB_TOOL_INPUT_KEY.into(), json!("list_endpoints"));
        list.insert("spec_url".into(), json!("https://x/spec.yaml"));
        list.insert("conversation_id".into(), json!("c-list"));
        let out = node
            .execute(&list, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(out.get("total").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(out.get("returned").and_then(|v| v.as_u64()), Some(1));
        let eps = out.get("endpoints").and_then(|v| v.as_array()).unwrap();
        assert_eq!(
            eps[0].get("operation_id").and_then(|v| v.as_str()),
            Some("listPets")
        );
        assert_eq!(
            eps[0].get("method").and_then(|v| v.as_str()),
            Some("GET")
        );
    }

    #[tokio::test]
    async fn list_endpoints_on_unloaded_spec_returns_spec_not_loaded() {
        let (_port, node) = node_with_fake_port();
        let mut list: NodeInputs = HashMap::new();
        list.insert(SUB_TOOL_INPUT_KEY.into(), json!("list_endpoints"));
        list.insert("spec_url".into(), json!("https://never/loaded.yaml"));
        list.insert("conversation_id".into(), json!("c-unloaded"));
        let mut state = json!({});
        let out = node
            .execute(&list, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("error").and_then(|v| v.as_str()),
            Some("spec_not_loaded")
        );
    }

    #[tokio::test]
    async fn search_endpoint_ranks_by_fuzzy_score() {
        let (_port, node) = node_with_fake_port();
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-search"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut search: NodeInputs = HashMap::new();
        search.insert(SUB_TOOL_INPUT_KEY.into(), json!("search_endpoint"));
        search.insert("spec_url".into(), json!("https://x/spec.yaml"));
        search.insert("conversation_id".into(), json!("c-search"));
        search.insert("query".into(), json!("list pets"));
        let out = node
            .execute(&search, &json!({}), &mut state, None)
            .await
            .unwrap();
        let results = out.get("results").and_then(|v| v.as_array()).unwrap();
        assert!(!results.is_empty());
        assert_eq!(
            results[0].get("operation_id").and_then(|v| v.as_str()),
            Some("listPets")
        );
        assert!(results[0].get("score").and_then(|v| v.as_f64()).is_some());
    }

    #[tokio::test]
    async fn search_endpoint_filters_by_method() {
        let (_port, node) = node_with_fake_port();
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-method"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut search: NodeInputs = HashMap::new();
        search.insert(SUB_TOOL_INPUT_KEY.into(), json!("search_endpoint"));
        search.insert("spec_url".into(), json!("https://x/spec.yaml"));
        search.insert("conversation_id".into(), json!("c-method"));
        search.insert("query".into(), json!("pets"));
        search.insert("method".into(), json!("POST"));
        let out = node
            .execute(&search, &json!({}), &mut state, None)
            .await
            .unwrap();
        let results = out.get("results").and_then(|v| v.as_array()).unwrap();
        assert!(results.is_empty(), "no POST /pets in fake spec");
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib api_explorer::tests::list_endpoints_returns_paginated_summary`
Expected: FAIL — dispatch returns "not implemented".

- [ ] **Step 3: Implement both handlers**

Edit `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`. Replace the two matching arms in `execute()`:

```rust
            "list_endpoints" => self
                .handle_list_endpoints(inputs, &conversation_id)
                .await,
            "search_endpoint" => self
                .handle_search_endpoint(inputs, &conversation_id)
                .await,
```

Add the handlers in `impl ApiExplorerNode`:

```rust
    async fn handle_list_endpoints(
        &self,
        inputs: &NodeInputs,
        conversation_id: &str,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let spec_url = match Self::require_str(inputs, "spec_url") {
            Ok(u) => u.to_string(),
            Err(v) => return Ok(v),
        };
        let tag = inputs
            .get("tag")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let limit = inputs
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50)
            .clamp(1, 200) as usize;
        let offset = inputs
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        match self
            .use_case
            .list_endpoints(conversation_id, &spec_url, tag.as_deref(), limit, offset)
            .await
        {
            Ok(page) => Ok(json!({
                "total": page.total,
                "returned": page.endpoints.len(),
                "offset": offset,
                "endpoints": page
                    .endpoints
                    .iter()
                    .map(|e| json!({
                        "operation_id": e.operation_id,
                        "method": e.method.as_str(),
                        "path": e.path,
                        "summary": e.summary,
                        "tags": e.tags
                    }))
                    .collect::<Vec<_>>()
            })),
            Err(e) => Ok(format_spec_error(e)),
        }
    }

    async fn handle_search_endpoint(
        &self,
        inputs: &NodeInputs,
        conversation_id: &str,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let spec_url = match Self::require_str(inputs, "spec_url") {
            Ok(u) => u.to_string(),
            Err(v) => return Ok(v),
        };
        let query = match Self::require_str(inputs, "query") {
            Ok(q) => q.to_string(),
            Err(v) => return Ok(v),
        };
        let method_filter = inputs
            .get("method")
            .and_then(|v| v.as_str())
            .map(str::to_ascii_uppercase);
        let max_results = inputs
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .clamp(1, 50) as usize;

        match self
            .use_case
            .search_endpoint(
                conversation_id,
                &spec_url,
                &query,
                method_filter.as_deref(),
                max_results,
            )
            .await
        {
            Ok(results) => Ok(json!({
                "query": query,
                "results": results
                    .into_iter()
                    .map(|r| json!({
                        "operation_id": r.operation_id,
                        "method": r.method.as_str(),
                        "path": r.path,
                        "summary": r.summary,
                        "score": r.score,
                        "match_reason": r.match_reason
                    }))
                    .collect::<Vec<_>>()
            })),
            Err(e) => Ok(format_spec_error(e)),
        }
    }
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib api_explorer`
Expected: 14 tests pass (10 prior + 4 here).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs
git commit -m "$(cat <<'EOF'
feat(nodes): implement api_explorer list_endpoints + search_endpoint

Both sub-tools dispatch to ApiSpecUseCase and shape the result into
the JSON envelope the LLM expects. list_endpoints paginates with
limit (max 200) and offset. search_endpoint accepts an optional
method filter and a max_results cap (max 50). Missing spec in the
per-conversation cache surfaces as spec_not_loaded so the LLM knows
to call load_spec first.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 15: `ApiExplorerNode::execute` — `get_endpoint_details` + `build_http_request` handlers

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`

- [ ] **Step 1: Write the failing tests**

Append to `#[cfg(test)] mod tests`:

```rust
    #[tokio::test]
    async fn get_endpoint_details_returns_structured_json() {
        let (_port, node) = node_with_fake_port();
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-det"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut det: NodeInputs = HashMap::new();
        det.insert(SUB_TOOL_INPUT_KEY.into(), json!("get_endpoint_details"));
        det.insert("spec_url".into(), json!("https://x/spec.yaml"));
        det.insert("conversation_id".into(), json!("c-det"));
        det.insert("operation_id".into(), json!("listPets"));
        let out = node
            .execute(&det, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("operation_id").and_then(|v| v.as_str()),
            Some("listPets")
        );
        assert_eq!(out.get("method").and_then(|v| v.as_str()), Some("GET"));
        assert_eq!(out.get("path").and_then(|v| v.as_str()), Some("/pets"));
    }

    #[tokio::test]
    async fn get_endpoint_details_miss_returns_did_you_mean() {
        let (_port, node) = node_with_fake_port();
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-miss"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut det: NodeInputs = HashMap::new();
        det.insert(SUB_TOOL_INPUT_KEY.into(), json!("get_endpoint_details"));
        det.insert("spec_url".into(), json!("https://x/spec.yaml"));
        det.insert("conversation_id".into(), json!("c-miss"));
        det.insert("operation_id".into(), json!("listPet"));
        let out = node
            .execute(&det, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("error").and_then(|v| v.as_str()),
            Some("endpoint_not_found")
        );
        let dym = out.get("did_you_mean").and_then(|v| v.as_array()).unwrap();
        assert!(dym.iter().any(|v| v.as_str() == Some("listPets")));
    }

    #[tokio::test]
    async fn build_http_request_emits_ready_to_execute_config() {
        let (_port, node) = node_with_fake_port();
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-build"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut build: NodeInputs = HashMap::new();
        build.insert(SUB_TOOL_INPUT_KEY.into(), json!("build_http_request"));
        build.insert("spec_url".into(), json!("https://x/spec.yaml"));
        build.insert("conversation_id".into(), json!("c-build"));
        build.insert("operation_id".into(), json!("listPets"));
        build.insert("params".into(), json!({}));
        build.insert("auth_secret_ref".into(), json!("my_key"));
        let out = node
            .execute(&build, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(out.get("method").and_then(|v| v.as_str()), Some("GET"));
        assert_eq!(
            out.get("url").and_then(|v| v.as_str()),
            Some("https://petstore.example.com/pets")
        );
        let headers = out.get("headers").and_then(|v| v.as_object()).unwrap();
        assert_eq!(
            headers.get("X-API-Key").and_then(|v| v.as_str()),
            Some("${SECURE:my_key}")
        );
    }

    #[tokio::test]
    async fn build_http_request_missing_auth_returns_structured_error() {
        let (_port, node) = node_with_fake_port();
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-auth"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut build: NodeInputs = HashMap::new();
        build.insert(SUB_TOOL_INPUT_KEY.into(), json!("build_http_request"));
        build.insert("spec_url".into(), json!("https://x/spec.yaml"));
        build.insert("conversation_id".into(), json!("c-auth"));
        build.insert("operation_id".into(), json!("listPets"));
        build.insert("params".into(), json!({}));
        // No auth_secret_ref.
        let out = node
            .execute(&build, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("error").and_then(|v| v.as_str()),
            Some("missing_auth")
        );
    }

    #[tokio::test]
    async fn build_http_request_params_not_object_returns_invalid_input() {
        let (_port, node) = node_with_fake_port();
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-bad-params"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut build: NodeInputs = HashMap::new();
        build.insert(SUB_TOOL_INPUT_KEY.into(), json!("build_http_request"));
        build.insert("spec_url".into(), json!("https://x/spec.yaml"));
        build.insert("conversation_id".into(), json!("c-bad-params"));
        build.insert("operation_id".into(), json!("listPets"));
        build.insert("params".into(), json!("not-an-object"));
        let out = node
            .execute(&build, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("error").and_then(|v| v.as_str()),
            Some("invalid_input")
        );
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib api_explorer::tests::get_endpoint_details_returns_structured_json`
Expected: FAIL.

- [ ] **Step 3: Implement both handlers**

Edit `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`. Replace the two remaining matching arms in `execute()`:

```rust
            "get_endpoint_details" => self
                .handle_get_endpoint_details(inputs, &conversation_id)
                .await,
            "build_http_request" => self
                .handle_build_http_request(inputs, &conversation_id)
                .await,
```

Add:

```rust
    async fn handle_get_endpoint_details(
        &self,
        inputs: &NodeInputs,
        conversation_id: &str,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let spec_url = match Self::require_str(inputs, "spec_url") {
            Ok(u) => u.to_string(),
            Err(v) => return Ok(v),
        };
        let operation_id = match Self::require_str(inputs, "operation_id") {
            Ok(o) => o.to_string(),
            Err(v) => return Ok(v),
        };

        match self
            .use_case
            .get_endpoint_details(conversation_id, &spec_url, &operation_id)
            .await
        {
            Ok(details) => Ok(details),
            Err(e) => Ok(format_spec_error(e)),
        }
    }

    async fn handle_build_http_request(
        &self,
        inputs: &NodeInputs,
        conversation_id: &str,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let spec_url = match Self::require_str(inputs, "spec_url") {
            Ok(u) => u.to_string(),
            Err(v) => return Ok(v),
        };
        let operation_id = match Self::require_str(inputs, "operation_id") {
            Ok(o) => o.to_string(),
            Err(v) => return Ok(v),
        };
        let params = match inputs.get("params") {
            Some(v) if v.is_object() => v.clone(),
            _ => {
                return Ok(json!({
                    "error": "invalid_input",
                    "missing": "params",
                    "message": "`params` must be a JSON object mapping parameter names to values"
                }));
            }
        };
        let auth_secret_ref = inputs
            .get("auth_secret_ref")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        match self
            .use_case
            .build_http_request(
                conversation_id,
                &spec_url,
                &operation_id,
                &params,
                auth_secret_ref.as_deref(),
            )
            .await
        {
            Ok(request_value) => Ok(request_value),
            Err(e) => Ok(format_spec_error(e)),
        }
    }
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib api_explorer`
Expected: 19 tests pass (14 prior + 5 here).

- [ ] **Step 5: Run the full lib test suite to catch regressions**

Run: `cargo test --lib 2>&1 | tail -30`
Expected: all green; no unrelated breakage from adding the node.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs
git commit -m "$(cat <<'EOF'
feat(nodes): implement api_explorer get_endpoint_details + build_http_request

All five sub-tools are now wired. get_endpoint_details returns the
spec-verbatim endpoint structure (path/query/header params, request
body schema, responses, security); miss returns did_you_mean.
build_http_request emits the http_request-node-shaped JSON with
\${SECURE:<ref>} placeholders for auth headers. Missing required
params, type mismatches, and missing auth all surface as
LLM-recoverable JSON errors via format_spec_error.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 16: Register `ApiExplorerNode` in `HashMapNodeRegistry` + subscribe to `ConversationLifecycleBus`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Modify: `src/libs/colmena/src/dag_engine/application/run_use_case.rs` (if not already wired by Plan 0 Task 12)

The node must be reachable via `node_type = "api_explorer"` for both the `ExecutableNode` lookup and the `ToolkitNode` lookup. Unlike `tavily_client`, `api_explorer` is stateful — its `SessionRegistry<Arc<SpecCache>>` needs `ConversationLifecycleBus` notification so closed conversations do not leak cached specs.

- [ ] **Step 1: Write the failing test**

Append (or create) a test module in `registry.rs`:

```rust
#[cfg(test)]
mod registry_api_explorer_tests {
    use super::*;
    use crate::dag_engine::domain::toolkit_node::ToolkitNode;
    use std::sync::Arc;

    // Reuse whichever build_registry helper already exists in this file
    // (the Tavily test from Plan A's Task 12 introduced one). If none,
    // copy the helper from `registry_tavily_tests`.

    fn registry_for_tests() -> Arc<HashMapNodeRegistry> {
        registry_tavily_tests::build_registry()
    }

    #[test]
    fn api_explorer_registered_as_executable_node() {
        let reg = registry_for_tests();
        let node = reg.get_node("api_explorer");
        assert!(node.is_some(), "api_explorer must be registered");
    }

    #[test]
    fn api_explorer_registered_as_toolkit_node_with_five_sub_tools() {
        let reg = registry_for_tests();
        let tk = reg.get_toolkit_node("api_explorer");
        assert!(tk.is_some(), "api_explorer must be registered as ToolkitNode");
        let cat = tk.unwrap().sub_tool_catalog(&serde_json::json!({}));
        assert_eq!(cat.len(), 5);
    }
}
```

> **Note:** if `registry_tavily_tests::build_registry` is private, re-export it with `pub(super) fn build_registry()` in that module. If Plan A has not yet shipped, copy the helper verbatim from Plan A's Task 12.

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib registry_api_explorer_tests`
Expected: FAIL — `api_explorer` not registered.

- [ ] **Step 3: Register the node + wire lifecycle**

Edit `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`. Locate the section that registers Tavily (from Plan A Task 12). Immediately **below** that, add an analogous block for `api_explorer`, plus insert it into `toolkit_nodes`:

```rust
            // --- Register api_explorer ---
            let api_explorer = {
                use crate::dag_engine::infrastructure::nodes::api_explorer::ApiExplorerNode;
                let n = ApiExplorerNode::new();
                if let Some(svc) = secure_value_service.clone() {
                    Arc::new(n.with_secure_values(svc))
                } else {
                    Arc::new(n)
                }
            };
            nodes.insert(
                "api_explorer".to_string(),
                api_explorer.clone() as Arc<dyn ExecutableNode>,
            );
            toolkit_nodes.insert(
                "api_explorer".to_string(),
                api_explorer.clone()
                    as Arc<dyn crate::dag_engine::domain::toolkit_node::ToolkitNode>,
            );

            // Store a weak for lifecycle subscription later (see Step 4).
            let api_explorer_lifecycle = api_explorer.clone();
```

At the bottom of `new_with_secure_values`, **after** the closure that builds `Self`, add a short async subscription helper. The registry struct already exists; expose the subscriber wiring via a new public method:

```rust
impl HashMapNodeRegistry {
    /// Subscribe every lifecycle-aware node in this registry to the shared bus.
    /// Called by the run orchestrator once during engine initialisation.
    pub async fn subscribe_lifecycle(
        self: &Arc<Self>,
        bus: &crate::web::domain::lifecycle::ConversationLifecycleBus,
    ) {
        if let Some(node) = self.nodes.get("api_explorer") {
            // The ApiExplorerNode is both an ExecutableNode and a
            // ConversationLifecycleSubscriber; downcast via Any is brittle,
            // so we construct a small tower: the registry holds an explicit
            // `Arc<dyn ConversationLifecycleSubscriber>` set up at build time.
            // Prefer reading `lifecycle_subscribers` that you populate in
            // `new_with_secure_values`.
            let _ = node;
        }
        for sub in &self.lifecycle_subscribers {
            bus.subscribe(sub.clone()).await;
        }
    }
}
```

And extend the struct:

```rust
pub struct HashMapNodeRegistry {
    nodes: HashMap<String, Arc<dyn ExecutableNode>>,
    toolkit_nodes: HashMap<
        String,
        Arc<dyn crate::dag_engine::domain::toolkit_node::ToolkitNode>,
    >,
    subgraph_node: Option<Arc<SubGraphNode>>,
    lifecycle_subscribers:
        Vec<Arc<dyn crate::web::domain::lifecycle::ConversationLifecycleSubscriber>>,
}
```

Populate `lifecycle_subscribers` in the `Self { .. }` construction:

```rust
            let lifecycle_subscribers: Vec<
                Arc<dyn crate::web::domain::lifecycle::ConversationLifecycleSubscriber>,
            > = vec![api_explorer_lifecycle.clone()
                as Arc<dyn crate::web::domain::lifecycle::ConversationLifecycleSubscriber>];

            Self {
                nodes,
                toolkit_nodes,
                subgraph_node: Some(sub_node),
                lifecycle_subscribers,
            }
```

- [ ] **Step 4: Wire the subscription in the run orchestrator**

Edit `src/libs/colmena/src/dag_engine/application/run_use_case.rs`. In the place where the `RunUseCase` (or equivalent) is constructed with both the registry and the `ConversationLifecycleBus`, add the one-shot subscription just after both are available:

```rust
        if let (Some(bus), reg) = (self.conversation_lifecycle.clone(), self.registry.clone()) {
            reg.subscribe_lifecycle(&bus).await;
        }
```

> **Where exactly:** this belongs in whatever startup path the DAG engine already has — typically `new()` or `build()` on `RunUseCase`. If the lifecycle bus is created lazily when the first HTTP request arrives, move the subscription there. Do not make subscription lazy-per-call; it is a single setup.

- [ ] **Step 5: Run — expect PASS**

Run: `cargo test --lib registry_api_explorer_tests`
Expected: 2 tests pass.

Run: `cargo check --lib`
Expected: clean build, no warnings introduced in `registry.rs`.

- [ ] **Step 6: Run the full lib test suite**

Run: `cargo test --lib 2>&1 | tail -20`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/registry.rs \
        src/libs/colmena/src/dag_engine/application/run_use_case.rs
git commit -m "$(cat <<'EOF'
feat(registry): register api_explorer + subscribe to lifecycle bus

The node is inserted into both \`nodes\` and \`toolkit_nodes\` so it
can be used standalone or via llm_call.tool_configurations. A new
\`lifecycle_subscribers\` field holds the subscriber side of
api_explorer; the run orchestrator subscribes every entry to the
shared ConversationLifecycleBus once at setup, so per-conversation
spec caches are evicted eagerly when a conversation closes.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 17: End-to-end test graph — `api_explorer_petstore.json`

**Files:**
- Create: `tests/graphs/web/api_explorer_petstore.json`

- [ ] **Step 1: Ensure the directory exists**

Run:

```bash
mkdir -p tests/graphs/web
ls tests/graphs/web
```

Expected: lists whichever files Plan A may have already placed there. An empty directory is also fine.

- [ ] **Step 2: Write the graph**

Create `tests/graphs/web/api_explorer_petstore.json`:

```json
{
  "id": "api-explorer-petstore",
  "name": "api_explorer_petstore",
  "description": "LLM loads the Swagger Petstore spec, searches for the 'add pet' endpoint, inspects its details, and builds an http_request config.",
  "nodes": [
    {
      "id": "ask",
      "type": "input",
      "config": {
        "default": "Using the Petstore OpenAPI 3.0 spec at https://raw.githubusercontent.com/OAI/OpenAPI-Specification/main/examples/v3.0/petstore.yaml, build a request to add a new pet named 'Rex' (id 42, status 'available'). Return only the final http_request JSON config you would execute."
      }
    },
    {
      "id": "agent",
      "type": "llm_call",
      "config": {
        "provider": "anthropic",
        "model": "claude-opus-4-7",
        "api_key": "${ANTHROPIC_API_KEY}",
        "system_message": "You are an integration agent. Use apis__load_spec to load an OpenAPI URL, apis__search_endpoint to find what you need, apis__get_endpoint_details to check argument shapes, and apis__build_http_request to construct a valid call. Never guess parameter names — always call apis__get_endpoint_details first.",
        "tool_configurations": {
          "apis": {
            "name": "apis",
            "description": "OpenAPI spec discovery + request builder",
            "node_type": "api_explorer",
            "node_config": {
              "fuzzy_match_threshold": 0.3
            },
            "expose_sub_tools": "all"
          }
        }
      }
    },
    { "id": "sink", "type": "output", "config": {} }
  ],
  "edges": [
    { "from": "ask", "to": "agent", "field": "prompt" },
    { "from": "agent", "to": "sink" }
  ]
}
```

- [ ] **Step 3: Dry-run check (graph-loading only, no live LLM call required)**

Run:

```bash
cargo run --bin dag_engine -- run tests/graphs/web/api_explorer_petstore.json --include-extra-info 2>&1 | tail -40
```

Expected: the engine parses the graph and registers the `api_explorer` toolkit with `apis__load_spec`, `apis__list_endpoints`, `apis__search_endpoint`, `apis__get_endpoint_details`, `apis__build_http_request` exposed to the LLM. If `ANTHROPIC_API_KEY` is unset the run fails with a clear adapter-init message — that is fine; the intent here is graph-loading verification.

- [ ] **Step 4: Commit**

```bash
git add tests/graphs/web/api_explorer_petstore.json
git commit -m "$(cat <<'EOF'
test(graphs): add api_explorer_petstore end-to-end DAG

Wires an llm_call node to the api_explorer toolkit with
expose_sub_tools="all". Drives the LLM through load_spec →
search_endpoint → get_endpoint_details → build_http_request against
the canonical OpenAPI 3.0 Petstore example. Used by the Python / TS
smoke tests and the final verification task.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 18: End-to-end test graph — `api_explorer_amadeus_swagger2.json` (URL normalization + Swagger 2.0 round-trip)

**Files:**
- Create: `tests/graphs/web/api_explorer_amadeus_swagger2.json`

This graph exercises the URL normalizer (GitHub blob URL) and the Swagger 2.0 → OpenAPI 3.0 converter in one flow. It is the user's concrete motivating case.

- [ ] **Step 1: Write the graph**

Create `tests/graphs/web/api_explorer_amadeus_swagger2.json`:

```json
{
  "id": "api-explorer-amadeus-swagger2",
  "name": "api_explorer_amadeus_swagger2",
  "description": "LLM loads the Amadeus Airline-Code-Lookup Swagger 2.0 spec via its GitHub blob URL, verifies it was rewritten to raw.githubusercontent.com and converted to OpenAPI 3.0.3, then builds a GET /v1/reference-data/airlines call.",
  "nodes": [
    {
      "id": "ask",
      "type": "input",
      "config": {
        "default": "Load the Amadeus Airline Code Lookup spec from https://github.com/amadeus4dev/amadeus-open-api-specification/blob/main/spec/json/AirlineCodeLookup_v1_swagger_specification.json, then build a request to look up the airline with IATA code 'BA'. Return only the final http_request JSON config you would execute."
      }
    },
    {
      "id": "agent",
      "type": "llm_call",
      "config": {
        "provider": "anthropic",
        "model": "claude-opus-4-7",
        "api_key": "${ANTHROPIC_API_KEY}",
        "system_message": "You are an integration agent. Use apis__load_spec to load an OpenAPI or Swagger URL — Git-forge blob URLs are auto-rewritten for you; read the `resolved_url` and `original_format` in the result to confirm. Use apis__search_endpoint → apis__get_endpoint_details → apis__build_http_request to construct the call. The Amadeus API uses Bearer auth (scheme: Bearer); pass auth_secret_ref=\"amadeus_token\" in build_http_request.",
        "tool_configurations": {
          "apis": {
            "name": "apis",
            "description": "OpenAPI + Swagger 2.0 discovery",
            "node_type": "api_explorer",
            "node_config": {
              "fuzzy_match_threshold": 0.3
            },
            "expose_sub_tools": "all"
          }
        }
      }
    },
    { "id": "sink", "type": "output", "config": {} }
  ],
  "edges": [
    { "from": "ask", "to": "agent", "field": "prompt" },
    { "from": "agent", "to": "sink" }
  ]
}
```

- [ ] **Step 2: Dry-run check**

Run:

```bash
cargo run --bin dag_engine -- run tests/graphs/web/api_explorer_amadeus_swagger2.json --include-extra-info 2>&1 | tail -40
```

Expected: graph loads cleanly. If `ANTHROPIC_API_KEY` is set, the LLM runs `apis__load_spec` with the blob URL; the node rewrites it to `raw.githubusercontent.com`, fetches, detects `swagger: "2.0"`, converts to 3.0.3, and the result's `resolved_url` starts with `https://raw.githubusercontent.com/amadeus4dev/`, `original_format == "swagger-2.0"`, `internal_format == "openapi-3.0.3"`. The rest of the flow should succeed with the dummy `amadeus_token` secure value (the `${SECURE:amadeus_token}` placeholder passes through to the http_request node, which would fail downstream — that is expected; the test verifies the build_http_request step, not the downstream call).

- [ ] **Step 3: Commit**

```bash
git add tests/graphs/web/api_explorer_amadeus_swagger2.json
git commit -m "$(cat <<'EOF'
test(graphs): add api_explorer_amadeus_swagger2 DAG

Exercises the two riskiest paths end-to-end: (1) Git-forge URL
rewriting (github.com/.../blob/... → raw.githubusercontent.com/...)
and (2) Swagger 2.0 → OpenAPI 3.0.3 conversion. The LLM is instructed
to verify the rewrite by inspecting \`resolved_url\` and the
conversion by inspecting \`original_format\` / \`internal_format\` on
the load_spec result. Amadeus AirlineCodeLookup is the motivating
real-world case.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 19: Python smoke test — construct + drive the node through a mock LLM

**Files:**
- Create: `python/tests/test_api_explorer_smoke.py`

- [ ] **Step 1: Confirm the Python bindings are built**

Run:

```bash
.venv/bin/maturin develop 2>&1 | tail -5
```

Expected: a successful rebuild with the new `api_explorer` node compiled in.

- [ ] **Step 2: Write the smoke test**

Create `python/tests/test_api_explorer_smoke.py`:

```python
"""Smoke test for the api_explorer toolkit node via PyO3 bindings.

Validates that:
  1. The node registers under node_type="api_explorer".
  2. A minimal DAG referencing the node parses and validates without errors.
  3. sub_tool_catalog surfaces the five expected sub-tools through the
     toolkit-node registry helper.

No live network; uses a mock LLM backend if available, otherwise stops
at validation.
"""
import json
import os
import pytest

import colmena  # PyO3 binding

GRAPH_PATH = os.path.join(
    os.path.dirname(__file__),
    "..",
    "..",
    "tests",
    "graphs",
    "web",
    "api_explorer_petstore.json",
)


def test_graph_loads_and_validates():
    with open(os.path.abspath(GRAPH_PATH), encoding="utf-8") as f:
        graph = json.load(f)
    # The binding exposes `validate_graph(dict) -> None | raises` in
    # shared/graph_validator.rs — reuse it to avoid needing a live LLM.
    colmena.validate_graph(graph)


def test_api_explorer_node_registered():
    registry = colmena.default_registry()
    node_types = registry.node_types()
    assert "api_explorer" in node_types, (
        f"api_explorer missing from registry; got {sorted(node_types)}"
    )


def test_api_explorer_catalog_has_five_sub_tools():
    registry = colmena.default_registry()
    catalog = registry.toolkit_catalog("api_explorer", {})
    names = sorted(entry["name"] for entry in catalog)
    assert names == [
        "build_http_request",
        "get_endpoint_details",
        "list_endpoints",
        "load_spec",
        "search_endpoint",
    ]
```

> **If `colmena.validate_graph`, `colmena.default_registry`, or `registry.toolkit_catalog` are not already exposed**, add the thin wrappers in `src/libs/colmena/src/python_bindings/mod.rs` — the existing bindings already expose similar helpers (grep `#[pyfunction]` / `#[pyclass]` there). A ~30-line change suffices: a `#[pyfunction] fn validate_graph(py_dict: &PyDict) -> PyResult<()>` that pushes through to the Rust graph validator, plus a `Registry` PyClass wrapping the default `HashMapNodeRegistry` with `node_types()` and `toolkit_catalog(node_type, config)` methods.
>
> Keep these bindings minimal — they exist for smoke testing only and are not part of the public Python SDK surface in this milestone.

- [ ] **Step 3: Run the smoke test**

Run:

```bash
.venv/bin/pytest python/tests/test_api_explorer_smoke.py -v
```

Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add python/tests/test_api_explorer_smoke.py \
        src/libs/colmena/src/python_bindings/mod.rs
git commit -m "$(cat <<'EOF'
test(python): add api_explorer smoke test

Validates the graph in tests/graphs/web/api_explorer_petstore.json via
PyO3 bindings, checks api_explorer is registered, and asserts the
sub-tool catalog has exactly the five expected entries. Adds minimal
Python helpers (validate_graph, default_registry, toolkit_catalog) to
support this without needing a live LLM backend.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 20: Documentation updates

**Files:**
- Modify: `docs/node_configurations.json`
- Modify: `docs/agent_context/node_ports_reference.md`
- Modify: `docs/developer_guide/25_web_nodes.md` (from Plan 0 Task 13)

- [ ] **Step 1: Add `api_explorer` to the canonical config schema**

Edit `docs/node_configurations.json`. Find the top-level JSON object listing node types. Append a new entry (preserve key ordering: existing node entries are alphabetical; place `api_explorer` after any existing `a*` node and before `b*` nodes):

```json
  "api_explorer": {
    "description": "OpenAPI 3.x / Swagger 2.0 discovery + validated http_request builder. Toolkit node with 5 sub-tools.",
    "config": {
      "enable_cache": {
        "type": "boolean",
        "default": true,
        "description": "Cache parsed specs per conversation."
      },
      "cache_ttl_seconds": {
        "type": "integer",
        "default": 86400,
        "description": "Cached spec lifetime in seconds (default 24 h)."
      },
      "max_cached_specs": {
        "type": "integer",
        "default": 100,
        "description": "Per-conversation LRU cache capacity."
      },
      "session_idle_ttl_seconds": {
        "type": "integer",
        "default": 900,
        "description": "Evict spec caches whose conversation has been idle this long."
      },
      "session_max_lifetime_seconds": {
        "type": "integer",
        "default": 3600,
        "description": "Absolute conversation cache lifetime."
      },
      "max_spec_size_bytes": {
        "type": "integer",
        "default": 10485760,
        "description": "Abort download if the response exceeds this many bytes (10 MiB default)."
      },
      "spec_download_timeout_seconds": {
        "type": "integer",
        "default": 60,
        "description": "HTTP timeout for spec downloads."
      },
      "default_base_url_override": {
        "type": "string",
        "default": null,
        "description": "Base URL used when the spec's servers[] is empty."
      },
      "fuzzy_match_threshold": {
        "type": "number",
        "default": 0.6,
        "description": "Minimum normalized score for search_endpoint matches."
      },
      "retry_policy": {
        "type": "object",
        "description": "Retry policy for spec download failures.",
        "default": { "max_attempts": 3, "initial_backoff_ms": 500 }
      }
    },
    "sub_tools": [
      "load_spec",
      "list_endpoints",
      "search_endpoint",
      "get_endpoint_details",
      "build_http_request"
    ]
  },
```

- [ ] **Step 2: Add ports / outputs documentation**

Edit `docs/agent_context/node_ports_reference.md`. Find the section listing node types (look for an `## HTTP Nodes` or similar existing heading). Insert a new section:

```markdown
## `api_explorer`

**Node type:** `api_explorer`

**Inputs:**

| Key | Type | Required | Source | Description |
|---|---|---|---|---|
| `__sub_tool` | string | yes | toolkit executor | One of: `load_spec`, `list_endpoints`, `search_endpoint`, `get_endpoint_details`, `build_http_request`. Injected automatically when the node is used through `llm_call.tool_configurations`. |
| `conversation_id` | string | no | toolkit executor | Used to scope the spec cache per conversation. Defaults to `"default"` if absent. |
| `url` | string | yes for `load_spec` | LLM | Absolute URL of the spec. |
| `force_reload` | boolean | no | LLM | `load_spec` bypasses cache when true. |
| `spec_url` | string | yes for the other four sub-tools | LLM | Previously-loaded spec. |
| `tag` | string | no | LLM | `list_endpoints` tag filter. |
| `limit` / `offset` | integer | no | LLM | `list_endpoints` pagination. |
| `query` | string | yes for `search_endpoint` | LLM | Free-text query. |
| `method` | string | no | LLM | `search_endpoint` method filter. |
| `max_results` | integer | no | LLM | `search_endpoint` cap (default 10, max 50). |
| `operation_id` | string | yes for `get_endpoint_details` / `build_http_request` | LLM | |
| `params` | object | yes for `build_http_request` | LLM | Flat param map; node routes to path / query / header / body. |
| `auth_secret_ref` | string | sometimes | LLM | Required when the endpoint declares auth. |

**Outputs (`output` field):** per-sub-tool JSON envelope — see
[Spec C](../superpowers/specs/2026-04-23-web-nodes-c-api-explorer-design.md) for the exact shapes.

**Lifecycle:** The node holds a `SessionRegistry<Arc<SpecCache>>` scoped by `conversation_id`. The DAG run orchestrator subscribes the node to the `ConversationLifecycleBus`, so cached specs for a closed conversation are evicted immediately rather than waiting for TTL.
```

- [ ] **Step 3: Flesh out the `api_explorer` section of the web-nodes developer guide**

Edit `docs/developer_guide/25_web_nodes.md`. Replace the placeholder line `- [api_explorer](#api_explorer) — populated by Spec C.` with a full section (add it after the existing `## tavily_client` section from Plan A):

```markdown
## api_explorer

Toolkit node that lets an LLM discover endpoints in an **OpenAPI 3.x** or **Swagger 2.0** specification and deterministically build a valid `http_request` configuration. Swagger 2.0 documents are transparently converted to OpenAPI 3.0.3 inside the adapter so downstream code sees a single model.

### Five sub-tools

| Sub-tool | Purpose |
|---|---|
| `load_spec` | Download + parse + cache a spec. Must be called first. |
| `list_endpoints` | Paginated browse by tag. |
| `search_endpoint` | Fuzzy keyword search (path + summary + op_id + tags + description). |
| `get_endpoint_details` | Per-endpoint parameters, request body, responses, security. |
| `build_http_request` | Emit a JSON object shaped like the `http_request` node's input, with `${SECURE:<ref>}` placeholders for auth headers. |

### Configuration reference

See the canonical schema in `docs/node_configurations.json` (`api_explorer` key). Key tuning knobs:

- **`cache_ttl_seconds`** — how long a parsed spec stays fresh (default 24 h); revalidated via ETag/If-None-Match, so hits that are still fresh cost only a HEAD-like 304.
- **`max_spec_size_bytes`** — guards against giant specs (default 10 MiB). The adapter aborts mid-stream if the limit is crossed.
- **`fuzzy_match_threshold`** — raise to be stricter about `search_endpoint` matches; lower for recall.

### URL normalization

The adapter rewrites these Git-forge patterns before issuing the HTTP request:

| Input pattern | Rewritten to |
|---|---|
| `github.com/{o}/{r}/blob/{ref}/{p}` | `raw.githubusercontent.com/{o}/{r}/{ref}/{p}` |
| `github.com/{o}/{r}/tree/{ref}/{p}` | same as above |
| `gitlab.com/{o}/{r}/-/blob/{ref}/{p}` | `gitlab.com/{o}/{r}/-/raw/{ref}/{p}` |
| `bitbucket.org/{o}/{r}/src/{ref}/{p}` | `bitbucket.org/{o}/{r}/raw/{ref}/{p}` |

Unknown hosts pass through unchanged. The LLM sees both `spec_url_input` (what it gave you) and `resolved_url` (what was fetched) in the `load_spec` result. If a URL returns HTML (forge that was not rewritten), the node returns `unexpected_html_response` with a suggestion to use the raw URL directly.

### Swagger 2.0 → OpenAPI 3.0 conversion

All conversion happens in pure Rust (`swagger2_to_oas3.rs`). The mapping rules:

| Swagger 2.0 | OpenAPI 3.0.3 |
|---|---|
| `swagger: "2.0"` | `openapi: "3.0.3"` |
| `host` + `basePath` + `schemes[]` | `servers: [{ url: "{scheme}://{host}{basePath}" }]` (one per scheme) |
| `definitions` | `components.schemas` |
| `securityDefinitions` | `components.securitySchemes` |
| body param + `consumes` | `requestBody.content` |
| formData params | `multipart/form-data` or `x-www-form-urlencoded` |
| `collectionFormat: csv` | `style: form, explode: false` |
| `collectionFormat: multi` | `style: form, explode: true` |
| `collectionFormat: ssv` | `style: spaceDelimited` |
| `collectionFormat: pipes` | `style: pipeDelimited` |
| `collectionFormat: tsv` | **error** — no 3.0 equivalent, falls back to `tavily_client` + manual. |

### build_http_request — output contract

The returned JSON matches the `http_request` node's input exactly:

```json
{
  "url": "...",
  "method": "GET|POST|...",
  "headers": { ... },
  "query_params": { ... },
  "body": "...raw string..." | { ... } | { "__multipart": true, "fields": { ... } }
}
```

Auth headers use the `${SECURE:<ref>}` placeholder resolved by the downstream `http_request` node. Plaintext secrets never enter the LLM's view.

### Per-conversation caching and lifecycle

Spec caches are scoped per `conversation_id` through the shared `SessionRegistry<Arc<SpecCache>>`. The DAG run orchestrator subscribes each `api_explorer` instance to the `ConversationLifecycleBus`, so caches for a closed conversation are evicted immediately rather than waiting for TTL.

### Reference

Spec: [2026-04-23-web-nodes-c-api-explorer-design.md](../superpowers/specs/2026-04-23-web-nodes-c-api-explorer-design.md).
Implementation: `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`, `src/libs/colmena/src/web/application/api_spec_use_case.rs`, `src/libs/colmena/src/web/infrastructure/openapi_adapter.rs`, `src/libs/colmena/src/web/infrastructure/swagger2_to_oas3.rs`, `src/libs/colmena/src/web/infrastructure/url_normalizer.rs`.
```

- [ ] **Step 4: Quick doc-parity check**

Run:

```bash
grep -c "api_explorer" docs/node_configurations.json
grep -c "api_explorer" docs/agent_context/node_ports_reference.md
grep -c "api_explorer" docs/developer_guide/25_web_nodes.md
```

Expected: each returns ≥ 1 (typically 1, 5, and 10+ respectively). Verify the JSON parses cleanly:

```bash
python3 -c "import json; json.load(open('docs/node_configurations.json'))" && echo OK
```

Expected: prints `OK`.

- [ ] **Step 5: Commit**

```bash
git add docs/node_configurations.json \
        docs/agent_context/node_ports_reference.md \
        docs/developer_guide/25_web_nodes.md
git commit -m "$(cat <<'EOF'
docs: document api_explorer node (config, ports, developer guide)

Canonical schema in docs/node_configurations.json documents every
config field with defaults. docs/agent_context/node_ports_reference.md
adds the inputs table (what the toolkit executor injects vs. what the
LLM supplies per sub-tool). docs/developer_guide/25_web_nodes.md
gains the full api_explorer section covering the five sub-tools,
config knobs, URL normalization, Swagger 2.0 conversion table,
build_http_request output contract, and lifecycle model.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 21: Final verification

**Files:**
- None (read-only)

- [ ] **Step 1: Full library test suite**

Run:

```bash
cargo test --lib 2>&1 | tail -30
```

Expected: all green. No warnings introduced by this plan's code.

- [ ] **Step 2: Clippy**

Run:

```bash
cargo clippy --lib --all-features -- -D warnings 2>&1 | tail -30
```

Expected: no warnings. If `clippy` complains about `dead_code` or `clippy::too_many_arguments` on the new use-case methods, fix properly — do **not** add `#[allow]` unless you are suppressing a genuinely false positive (document with a one-line comment explaining why).

- [ ] **Step 3: Formatter**

Run:

```bash
cargo fmt --all -- --check
```

Expected: no diff. If there is a diff, run `cargo fmt --all` and commit with:

```bash
git add -u && git commit -m "style: cargo fmt"
```

- [ ] **Step 4: Focused targeted tests**

Run each of the task-focused suites one more time:

```bash
cargo test --lib url_normalizer
cargo test --lib swagger2_to_oas3
cargo test --lib openapi_adapter
cargo test --lib api_spec_use_case
cargo test --lib api_explorer
cargo test --lib registry_api_explorer_tests
```

Expected: every suite green.

- [ ] **Step 5: Graph-loading sanity check**

Run:

```bash
cargo run --bin dag_engine -- run tests/graphs/web/api_explorer_petstore.json --include-extra-info 2>&1 | tail -20
cargo run --bin dag_engine -- run tests/graphs/web/api_explorer_amadeus_swagger2.json --include-extra-info 2>&1 | tail -20
```

Expected: each parses the graph, registers the `api_explorer` toolkit with five sub-tools, and starts execution. Without live API keys these runs terminate with an adapter-init or LLM-provider error — that is fine for the structural check.

- [ ] **Step 6: Python smoke**

Run:

```bash
.venv/bin/maturin develop 2>&1 | tail -5 && \
  .venv/bin/pytest python/tests/test_api_explorer_smoke.py -v
```

Expected: all three smoke tests pass.

- [ ] **Step 7: Documentation cross-references**

Spot-check that every new code path is referenced in at least one of: `docs/node_configurations.json`, `docs/agent_context/node_ports_reference.md`, `docs/developer_guide/25_web_nodes.md`.

Run:

```bash
grep -rn "api_explorer" docs | wc -l
```

Expected: ≥ 15 (typically more, covering schema + ports + guide + any cross-links in the unified Web Nodes section).

- [ ] **Step 8: Final summary commit (no-op if the tree is clean)**

If any of Steps 1-7 required follow-up edits that were not yet committed, group them into a single verification commit:

```bash
git status
# Only if changes remain:
git commit -am "$(cat <<'EOF'
chore(web): final verification fixes for api_explorer

Catches residual clippy / fmt / doc-parity fixes surfaced by the
final verification pass. No behaviour change.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

Otherwise no commit is needed.

---

<!-- END-OF-PLAN-MARKER -->
