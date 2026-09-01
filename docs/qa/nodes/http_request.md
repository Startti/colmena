# QA — Nodo `http_request`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`  
Fuentes de doc revisadas:
- `docs/node_configurations.json` (§http_request, líneas 882–1023)
- `docs/node_as_tools_reference.json` (§http_request, líneas 159–358)
- `docs/agent_context/node_ports_reference.md` (líneas 58–135)
- `docs/developer_guide/25_web_nodes.md` (no cubre http_request; cubre tavily_client y api_explorer)

---

## 1) Config documentada NO soportada por el código

**Sin discrepancias detectadas.** Todos los campos documentados (`base_url`, `endpoint`, `method`, `headers`, `query_params`, `body`, `bearer_token`, `authorization`, `auth`, `max_file_size_bytes`, `max_parts`, `url_download_timeout_secs`, `allow_http_urls`, `secure`) están implementados en el código con los comportamientos y defaults descritos.

---

## 2) Código NO documentado

### A. `bearer_token` resuelve ${ENV_VAR}, pero doc dice `supports_env_vars: false`

**Hallazgo:**  
La doc en `node_configurations.json:938` declara:
```json
"bearer_token": {
  ...
  "supports_env_vars": false,
  ...
}
```

Pero el código en `http.rs:953` Y `http.rs:695` (línea de cierre de multipart) ambas llaman `Self::resolve_env_vars(token)`:
```rust
// http.rs:953 (regular execute path)
if let Some(token) = inputs.get("bearer_token").and_then(|v| v.as_str())
    .or_else(|| config.get("bearer_token").and_then(|v| v.as_str()))
{
    let token = Self::resolve_env_vars(token).map_err(...)?;
    request_builder = request_builder.header("Authorization", format!("Bearer {}", token));
}
```

Además, el ejemplo en `node_as_tools_reference.json:246` usa `"fixed": "${AMADEUS_TOKEN}"` en `bearer_token`, lo que implica que env var resolution es esperado.

**Impacto QA:**  
La doc es incorrecta. QA debe probar que `bearer_token` con `${ENV_VAR}` se resuelve correctamente.

---

### B. Stdout logging en multipart mode (líneas 713, 717, 1090)

**Hallazgo:**  
El código emite logs a stdout sin documentación:
```rust
// http.rs:713 (multipart mode)
println!("[HttpNode] → {method_str} {full_url} (multipart, {parts_count} parts)");
// http.rs:717
println!("[HttpNode] ← {status} ({full_url})");
// http.rs:903, 908 (regular mode)
println!("[HttpNode] → {} {}", method, url);
println!("[HttpNode] ← {} ({})", status, full_url_str);
// http.rs:1099
println!("[HttpNode] Response body is not JSON or is empty");
```

**Notas:**
- Los logs de request/response (`→`/`←` símbolos) son útiles para debugging, pero nunca aparecen en docs.
- La línea 1061 comenta "Never log body contents" (ok, respetado).
- Estos `println!` van a stderr (proceso host), no al stream SSE.

**Impacto QA:**  
Los logs ayudan a inspeccionar la traza de ejecución, pero no están contractualizados. QA debe saber que estos aparecen.

---

### C. Backward-compat alias `query_parameters` (línea 237)

**Hallazgo:**  
El código en `http.rs:237` mantiene backward compatibility:
```rust
const RESERVED_KEYS: [&'static str; 10] = [
    ...
    "query_params",     // correct key used throughout
    "query_parameters", // kept for backward compat
    ...
];
```

**Doc estado:**  
La doc en `node_configurations.json:1022` lista `query_parameters` en `reserved_input_keys`, pero el texto nunca lo menciona como alias aceptado. Esto es técnicamente correcto (el alias existe y está reservado), pero podría ser más explícito.

**Impacto QA:**  
Bajo. El alias funciona, se trata como campo reservado correctamente, no hay comportamiento sorpresa.

---

### D. OAuth (`auth` block) no soportado en multipart mode, error en línea 1034

**Hallazgo:**  
El código en `http.rs:1033–1036` rechaza multipart + OAuth:
```rust
if oauth_provider.is_some() {
    return Err(Box::new(std::io::Error::other(
        "http_request: native OAuth (`auth`) is not supported with multipart bodies in v1",
    )) as Box<dyn StdError + Send + Sync>);
}
```

**Doc estado:**  
La doc en `node_configurations.json:952` menciona: *"Not supported with multipart bodies in v1."*  
Esto es documentado correctamente.

**Impacto QA:**  
Nulo. Comportamiento esperado documentado.

---

### E. Agent session ID requirement for `$attachment:` in multipart (línea 781–787)

**Hallazgo:**  
El código rechaza multipart attachments si no hay `agent_session_id`:
```rust
// http.rs:781–787
let sid = agent_session_id.ok_or_else(|| -> Box<dyn StdError + Send + Sync> {
    format!(
        "AttachmentResolveError: body references \
         '$attachment:{storage_key}' but no agent_session_id \
         is available (resolver requires one)"
    ).into()
})?;
```

**Doc estado:**  
No documentado. La doc de `$attachment:` (línea 884: "any string value in the body that matches...") no menciona que multipart attachments requieren `agent_session_id`.

**Impacto QA:**  
Moderado. QA debe saber que multipart con `$attachment:` falla sin `--agent-session-id`.

---

## 3) Plan de pruebas QA

### **Caso T1: GET básico**

**Objetivo:**  
Verificar que GET sin autenticación funciona.

**Grafo mínimo:**
```json
{
  "nodes": {
    "request": {
      "type": "http_request",
      "config": {
        "base_url": "https://jsonplaceholder.typicode.com",
        "endpoint": "/posts/1",
        "method": "GET"
      }
    },
    "output": { "type": "output", "config": {} }
  },
  "edges": [{ "from": "request", "to": "output" }]
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run /tmp/test_http_get.json
```

**Resultado esperado:**
- HTTP 200
- `body` contiene objeto JSON con campos: `userId`, `id`, `title`, `body`
- `status`: 200

**Pass/Fail:**
- PASS si ambos campos (`status` y `body` JSON) presentes y correctos
- FAIL si status ≠ 200 o body null (response no-JSON)

---

### **Caso T2: POST con body JSON**

**Objetivo:**  
Verificar que POST con JSON body funciona y input body sobrescribe config body.

**Grafo mínimo:**
```json
{
  "nodes": {
    "input": {
      "type": "input",
      "config": {
        "data": {
          "userId": 1,
          "title": "Test Title",
          "body": "Test Body"
        }
      }
    },
    "request": {
      "type": "http_request",
      "config": {
        "base_url": "https://jsonplaceholder.typicode.com",
        "endpoint": "/posts",
        "method": "POST",
        "headers": { "Content-Type": "application/json" },
        "body": { "userId": 99, "title": "Default" }
      }
    },
    "output": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "input", "to": "request.body" },
    { "from": "request", "to": "output" }
  ]
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run /tmp/test_http_post.json
```

**Resultado esperado:**
- HTTP 201 (created)
- Response body es el objeto creado (echo del input)
- `body.userId` = 1 (del input, no config 99)
- `body.title` = "Test Title"

**Pass/Fail:**
- PASS si status 201 y body.userId = 1 (input override)
- FAIL si body.userId = 99 (config no fue sobrescrito)

---

### **Caso T3: Query params**

**Objetivo:**  
Verificar que query_params se agregan a la URL y que extra keys primitivas se convierten en query params.

**Grafo mínimo:**
```json
{
  "nodes": {
    "input": {
      "type": "input",
      "config": {
        "data": {
          "sortBy": "title",
          "limit": "5"
        }
      }
    },
    "request": {
      "type": "http_request",
      "config": {
        "base_url": "https://jsonplaceholder.typicode.com",
        "endpoint": "/posts",
        "method": "GET",
        "query_params": { "userId": "1" }
      }
    },
    "output": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "input", "to": "request" },
    { "from": "request", "to": "output" }
  ]
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run /tmp/test_http_params.json
```

**Resultado esperado:**
- HTTP 200
- URL debe incluir: `?userId=1&sortBy=title&limit=5`
- `body` es array de posts

**Pass/Fail:**
- PASS si response contiene array de posts (query params aceptados)
- FAIL si HTTP 400 (query params rechazados por servidor)

---

### **Caso T4: Headers personalización**

**Objetivo:**  
Verificar que headers from config + inputs se fusionan, con inputs sobrescribiendo config.

**Grafo mínimo:**
```json
{
  "nodes": {
    "input": {
      "type": "input",
      "config": {
        "data": {
          "headers": { "X-Custom": "from-input" }
        }
      }
    },
    "request": {
      "type": "http_request",
      "config": {
        "base_url": "https://httpbin.org",
        "endpoint": "/headers",
        "method": "GET",
        "headers": {
          "X-Custom": "from-config",
          "X-Other": "value"
        }
      }
    },
    "output": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "input", "to": "request" },
    { "from": "request", "to": "output" }
  ]
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run /tmp/test_http_headers.json
```

**Resultado esperado:**
- HTTP 200
- Response JSON must contain: `headers: { "X-Custom": "from-input", "X-Other": "value", "User-Agent": "colmena-http-node/0.1" }`

**Pass/Fail:**
- PASS si `X-Custom` = "from-input" (input override)
- FAIL si `X-Custom` = "from-config" (input override failed)

---

### **Caso T5: Bearer token con env var**

**Objetivo:**  
Verificar que `bearer_token` resuelve `${ENV_VAR}` (hallazgo S2.A).

**Setup:**
```bash
export TEST_BEARER_TOKEN="test-token-12345"
```

**Grafo mínimo:**
```json
{
  "nodes": {
    "request": {
      "type": "http_request",
      "config": {
        "base_url": "https://httpbin.org",
        "endpoint": "/bearer",
        "method": "GET",
        "bearer_token": "${TEST_BEARER_TOKEN}"
      }
    },
    "output": { "type": "output", "config": {} }
  },
  "edges": [{ "from": "request", "to": "output" }]
}
```

**Comando:**
```bash
export TEST_BEARER_TOKEN="test-token-12345"
cargo run --bin dag_engine -- run /tmp/test_http_bearer.json
```

**Resultado esperado:**
- HTTP 200
- Response JSON: `{ "authenticated": true, "token": "test-token-12345" }`

**Pass/Fail:**
- PASS si authenticated=true (env var resolved + header sent)
- FAIL si authenticated=false (env var not resolved or header not sent)

---

### **Caso T6: Multipart con URL-sourced part**

**Objetivo:**  
Verificar que multipart mode funciona con URL parts (Content-Type: multipart/form-data).

**Grafo mínimo:**
```json
{
  "nodes": {
    "request": {
      "type": "http_request",
      "config": {
        "base_url": "https://httpbin.org",
        "endpoint": "/post",
        "method": "POST",
        "headers": { "Content-Type": "multipart/form-data" },
        "body": {
          "field1": "text value",
          "field2": "https://via.placeholder.com/150?text=test"
        }
      }
    },
    "output": { "type": "output", "config": {} }
  },
  "edges": [{ "from": "request", "to": "output" }]
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run /tmp/test_http_multipart_url.json
```

**Resultado esperado:**
- HTTP 200
- Response `form` object with: `field1` (text), `field2` (binary from URL)
- No HTTP errors on URL GET

**Pass/Fail:**
- PASS si status 200 y ambos fields presentes
- FAIL si HTTP 400 (multipart parsing error) o URL fetch failed (FileTooLarge / UrlValidationFailed)

---

### **Caso T7: Multipart max_parts limit**

**Objetivo:**  
Verificar que el nodo rechaza multipart body que excede `max_parts`.

**Grafo mínimo:**
```json
{
  "nodes": {
    "request": {
      "type": "http_request",
      "config": {
        "base_url": "https://httpbin.org",
        "endpoint": "/post",
        "method": "POST",
        "headers": { "Content-Type": "multipart/form-data" },
        "max_parts": 2,
        "body": {
          "p1": "text1",
          "p2": "text2",
          "p3": "text3"
        }
      }
    },
    "output": { "type": "output", "config": {} }
  },
  "edges": [{ "from": "request", "to": "output" }]
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run /tmp/test_http_maxparts.json
```

**Resultado esperado:**
- Error: "TooManyParts: body produced 3 parts, max is 2"
- DAG run fails with this error

**Pass/Fail:**
- PASS si error message matches "TooManyParts: ... max is 2"
- FAIL si request sent (limit not enforced)

---

### **Caso T8: Multipart + OAuth unsupported (v1)**

**Objetivo:**  
Verificar que multipart + auth block is rejected (hallazgo S2.D).

**Grafo mínimo:**
```json
{
  "nodes": {
    "request": {
      "type": "http_request",
      "config": {
        "base_url": "https://api.example.com",
        "endpoint": "/upload",
        "method": "POST",
        "headers": { "Content-Type": "multipart/form-data" },
        "body": { "file": "test.txt" },
        "auth": {
          "type": "oauth2_refresh_token",
          "token_url": "https://oauth2.googleapis.com/token",
          "client_id": "${CLIENT_ID}",
          "client_secret": "${CLIENT_SECRET}",
          "refresh_token": "${REFRESH_TOKEN}"
        }
      }
    },
    "output": { "type": "output", "config": {} }
  },
  "edges": [{ "from": "request", "to": "output" }]
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run /tmp/test_http_oauth_multipart.json
```

**Resultado esperado:**
- Error: "http_request: native OAuth (`auth`) is not supported with multipart bodies in v1"
- DAG run fails immediately (no network call)

**Pass/Fail:**
- PASS si error contains "not supported with multipart"
- FAIL si request sent or different error

---

### **Caso T9: Default output port is `body`**

**Objetivo:**  
Verificar que un nodo sin field selector recibe `body` por defecto.

**Grafo mínimo:**
```json
{
  "nodes": {
    "request": {
      "type": "http_request",
      "config": {
        "base_url": "https://jsonplaceholder.typicode.com",
        "endpoint": "/posts/1",
        "method": "GET"
      }
    },
    "log": {
      "type": "log",
      "config": {}
    },
    "output": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "request", "to": "log" },
    { "from": "log", "to": "output" }
  ]
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run /tmp/test_http_defaultport.json
```

**Resultado esperado:**
- Log node receives the parsed JSON body (not the full `{ status, body }` object)
- Output shows: `{ userId: 1, id: 1, title: "...", body: "..." }`

**Pass/Fail:**
- PASS si log shows JSON object (body only)
- FAIL si log shows `{ status: 200, body: {...} }` (full output)

---

### **Caso T10: Non-JSON response body → null**

**Objetivo:**  
Verificar que response body non-JSON se mapea a `null`.

**Grafo mínimo:**
```json
{
  "nodes": {
    "request": {
      "type": "http_request",
      "config": {
        "base_url": "https://httpbin.org",
        "endpoint": "/html",
        "method": "GET"
      }
    },
    "output": { "type": "output", "config": {} }
  },
  "edges": [{ "from": "request", "to": "output" }]
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run /tmp/test_http_nonjson.json
```

**Resultado esperado:**
- HTTP 200
- `status`: 200
- `body`: null (response is HTML, not JSON)
- Stderr message: "[HttpNode] Response body is not JSON or is empty"

**Pass/Fail:**
- PASS si body=null y status=200
- FAIL si body contains HTML string or error

---

### **Caso T11: Env var resolution in URL + headers**

**Objetivo:**  
Verificar que `${ENV_VAR}` se resuelve en base_url, endpoint, y header values.

**Setup:**
```bash
export MY_API_HOST="jsonplaceholder.typicode.com"
export MY_PATH="/posts"
export MY_HEADER_VALUE="custom-header-value"
```

**Grafo mínimo:**
```json
{
  "nodes": {
    "request": {
      "type": "http_request",
      "config": {
        "base_url": "https://${MY_API_HOST}",
        "endpoint": "${MY_PATH}/1",
        "method": "GET",
        "headers": {
          "X-Custom": "${MY_HEADER_VALUE}"
        }
      }
    },
    "output": { "type": "output", "config": {} }
  },
  "edges": [{ "from": "request", "to": "output" }]
}
```

**Comando:**
```bash
export MY_API_HOST="jsonplaceholder.typicode.com"
export MY_PATH="/posts"
export MY_HEADER_VALUE="custom-header-value"
cargo run --bin dag_engine -- run /tmp/test_http_envvars.json
```

**Resultado esperado:**
- HTTP 200
- URL resolves to: `https://jsonplaceholder.typicode.com/posts/1`
- Header `X-Custom` sent as: `custom-header-value`

**Pass/Fail:**
- PASS si HTTP 200 y response body is post object
- FAIL si HTTP error (URL not resolved or header not sent)

