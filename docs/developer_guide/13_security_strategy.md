# Credentials & Secrets Strategy in Colmena DAGs

## Overview

This document explains **all available methods** to securely pass credentials into a Colmena DAG, their trade-offs, and recommendations for different scenarios.

---

## Problem Statement

Colmena DAGs need to access external APIs (Amadeus, OpenAI, Anthropic, custom services) that require credentials:
- API keys (OpenAI, Gemini)
- OAuth2 tokens (Amadeus)
- Database credentials
- Bearer tokens, passwords, encryption keys

**The challenge:** How to get these credentials INTO the DAG without:
- ❌ Hardcoding them in the graph JSON
- ❌ Exposing them to LLM nodes
- ❌ Logging them to console/files
- ❌ Storing them unencrypted

---

## Available Strategies (TODAY)

### Strategy 1: Environment Variables (Simplest)

**How it works:**
```bash
# Before running the DAG
export AMADEUS_CLIENT_ID="ABC123"
export AMADEUS_CLIENT_SECRET="XYZ789"
export GEMINI_API_KEY="sk-..."
```

**In the graph JSON:**
```json
{
  "type": "http_request",
  "config": {
    "base_url": "https://api.amadeus.com/v1/security/oauth2",
    "endpoint": "/token",
    "body": "client_id=${AMADEUS_CLIENT_ID}&client_secret=${AMADEUS_CLIENT_SECRET}"
  }
}
```

**Node Types that Support This:**
- `http_request` — resolves `${VAR}` in: base_url, endpoint, headers, body
- `llm_call` — resolves `${VAR}` for `api_key` and `connection_url` (memory/history DB connection string)

**Flow:**
```
Process environment
  ↓
HTTP node: ${AMADEUS_CLIENT_ID} → "ABC123"
  ↓
Request body: client_id=ABC123
  ↓
Amadeus API returns: {access_token: "real_token_xyz"}
  ↓
Next node gets: access_token (real value in plaintext)
```

**Pros:**
- ✅ Simple, standard practice
- ✅ No database required
- ✅ Works with all node types
- ✅ Credentials stay in process memory, not in files/DB

**Cons:**
- ❌ Credentials in plaintext in process memory
- ❌ Process env vars visible in `ps aux`
- ❌ One misconfigured log statement exposes secrets
- ❌ LLM nodes see raw credentials if referenced in prompts
- ❌ Not suitable when LLM should NOT see credentials

**When to Use:**
- Development/testing environments
- When LLM nodes don't process the credentials
- When process security is guaranteed (Kubernetes, container isolation)

---

### Strategy 2: Trigger Webhook Payload + Secure Flag (RECOMMENDED FOR TESTING)

**How it works:**

1. **Caller sends credentials in HTTP POST body:**
```bash
curl -X POST http://localhost:3000/webhook \
  -H "Content-Type: application/json" \
  -d '{
    "client_id": "ABC123",
    "client_secret": "XYZ789"
  }'
```

2. **Trigger node passes payload to next nodes:**
```json
{
  "type": "trigger_webhook",
  "config": {
    "path": "/webhook"
  }
}
```

3. **HTTP node receives credentials and marks output as secure:**
```json
{
  "type": "http_request",
  "config": {
    "endpoint": "/token",
    "body": {
      "client_id": "${trigger.client_id}",
      "client_secret": "${trigger.client_secret}"
    },
    "secure": true
  }
}
```

4. **Secure Value Service:**
   - Response from API: `{access_token: "real_token_xyz"}`
   - After hashing: `{access_token: "<value_1>"}`
   - Stored in DB: `<value_1> → pgp_sym_encrypt(real_token_xyz)`

5. **LLM node SKIP injection (sees hashes only):**
```json
{
  "type": "llm_call",
  "inputs": {
    "user_message": "Token: ${get_amadeus_token.access_token}"
  }
}
```
Result: `"Token: <value_1>"` ← LLM NEVER sees real token

6. **HTTP node AUTO-INJECT (sees real values):**
```json
{
  "type": "http_request",
  "inputs": {
    "bearer_token": "${get_amadeus_token.access_token}"
  }
}
```
Result: auto-injected with `"Bearer real_token_xyz"` ← HTTP uses real token

**Flow Diagram:**
```
HTTP POST /webhook {client_id, client_secret}
  ↓
trigger node outputs: {client_id, client_secret}
  ↓
HTTP auth node (secure: true):
  • Calls Amadeus API
  • Gets {access_token: "real_token_xyz"}
  • Hashes to {access_token: "<value_1>"}
  • DB stores: <value_1> → AES(real_token_xyz)
  ↓
Output to next nodes: {access_token: "<value_1>"}
  ├→ LLM node: sees "<value_1>" (SAFE)
  └→ HTTP node: gets injected with "real_token_xyz" (WORKS)
  ↓
DAG ends: DELETE FROM secure_value_mappings (cleanup)
```

**Pros:**
- ✅ Credentials NEVER hardcoded in graph
- ✅ LLM nodes completely isolated from real credentials
- ✅ Non-LLM HTTP nodes work transparently with real values
- ✅ Values encrypted at rest (pgcrypto `pgp_sym_encrypt`, OpenPGP CFB — not AES-256-GCM, in PostgreSQL)
- ✅ Auto-cleanup on DAG completion
- ✅ Works with real APIs (Amadeus, OpenAI, etc.)

**Cons:**
- ❌ Credentials in HTTP request body (protected by TLS only)
- ❌ Requires PostgreSQL + secure_value_mappings table
- ❌ Requires SECURE_VALUES_KEY encryption key in environment
- ❌ Not suitable if caller is untrusted

**When to Use:**
- Testing real APIs with LLM integration
- Any scenario where LLM should NOT see raw credentials
- Production deployments with TLS and authenticated webhook endpoints
- Multi-tenant systems (each tenant's creds isolated via session_id)

**Example: amadeus_secure_gemini_test.json**
```json
{
  "type": "trigger_webhook",
  "config": {
    "path": "/amadeus-secure-test",
    "test_payload": {
      "client_id": "${AMADEUS_CLIENT_ID}",
      "client_secret": "${AMADEUS_CLIENT_SECRET}"
    }
  }
}
```
When running with `cargo run ... run amadeus_secure_gemini_test.json`, the trigger uses `test_payload` (Env vars resolved). When running via HTTP POST, trigger uses `__payload__` from the HTTP body.

---

### Strategy 3: Static Test Payload (LOCAL TESTING ONLY)

**How it works:**

Hardcode credentials directly in graph JSON using `trigger.config.test_payload`:

```json
{
  "type": "trigger_webhook",
  "config": {
    "path": "/amadeus-test",
    "test_payload": {
      "client_id": "ABC123_HARDCODED",
      "client_secret": "XYZ789_HARDCODED"
    }
  }
}
```

Run with:
```bash
cargo run --bin dag_engine -- run tests/graphs/security/amadeus_secure_gemini_test.json
```

**Pros:**
- ✅ Zero setup required
- ✅ No environment variables needed
- ✅ Works offline
- ✅ Great for demos and documentation

**Cons:**
- ❌ **NEVER use in production or commit to git**
- ❌ Credentials visible in plaintext in the file
- ❌ Risk of accidental exposure

**When to Use:**
- Local development/testing only
- Creating examples and documentation
- Quick proof-of-concept tests
- **NEVER** in production or with real API keys

**Note:** The `amadeus_secure_gemini_test.json` uses `test_payload` with `${ENV_VAR}` (env vars get resolved), NOT hardcoded values. This is the safe approach.

---

### Strategy 4: Database Query Node (NOT YET IMPLEMENTED)

**Planned Future Feature**

**How it would work:**

```bash
# Admin pre-loads credentials into DB
dag_engine store-secret \
  --name AMADEUS_CLIENT_ID \
  --value "ABC123" \
  --encrypted true

dag_engine store-secret \
  --name AMADEUS_CLIENT_SECRET \
  --value "XYZ789" \
  --encrypted true
```

**In the graph:**
```json
{
  "type": "db_query",
  "config": {
    "table": "credentials",
    "query": "SELECT value FROM credentials WHERE name = 'AMADEUS_CLIENT_ID'"
  }
}
```

**Result:** Credentials loaded from encrypted DB table

**Pros:**
- ✅ Credentials never in code, environment, or process memory
- ✅ Can be rotated without redeploying
- ✅ Full audit trail of access
- ✅ Encrypted at rest and in transit
- ✅ Per-environment credential isolation (dev vs prod)

**Cons:**
- ❌ Not implemented yet
- ❌ Adds DB dependency
- ❌ Requires CLI tool for credential management
- ❌ Complexity overhead

**When to Use (Future):**
- Production deployments with strict credential hygiene
- Multi-tenant systems
- Highly regulated environments (compliance, security audits)
- When rotation and audit logging are required

**Effort to Implement:**
- New `db_query` node type: ~2 hours
- CLI for credential management: ~2 hours
- Testing and documentation: ~2 hours
- **Total: ~1 day**

---

---

### Strategy 5: LLM-Driven Auth via Secure Tools (NEW — VALIDATED)

**How it works:**

Instead of a separate DAG node for auth, **the LLM decides when to authenticate** using an HTTP tool with `secure: true`. The `DagToolExecutor` applies `hash_output()` before returning the result to the LLM, ensuring the LLM only ever sees `<value_N>` placeholders.

**Key difference from Strategy 2:** Here the LLM orchestrates the auth flow (calls `get_amadeus_token` first, then uses the placeholder in `search_flights`). In Strategy 2, auth is a separate fixed DAG node.

**Flow:**
```
LLM receives task: "Find flights MAD→BCN"
  ↓
LLM calls tool: get_amadeus_token (fixed_config has secure: true)
  ↓
DagToolExecutor → HttpNode → POST oauth2/token → {access_token: "real_xyz"}
  ↓
hash_output() → {access_token: "<value_1>"} stored encrypted in DB
  ↓
LLM receives: {access_token: "<value_1>"}  ← never sees real token ✅
  ↓
LLM calls tool: search_flights with bearer_token: "<value_1>"
  ↓
inject_secrets() → bearer_token: "real_xyz" auto-injected
  ↓
HttpNode → GET /flight-offers with Authorization: Bearer real_xyz ✅
  ↓
LLM receives flight results and responds to user
```

**Validated graph:** `tests/graphs/agents/amadeus_llm_http_auth_experiment.json`

```bash
set -a && source .env && set +a
cargo run --bin dag_engine -- run tests/graphs/agents/amadeus_llm_http_auth_experiment.json
```

**Pros:**
- ✅ LLM can autonomously manage multi-step API auth flows
- ✅ Real tokens NEVER visible to LLM (hash_output applied in DagToolExecutor)
- ✅ Works with any OAuth2 API that the LLM knows how to sequence
- ✅ More flexible than fixed DAG auth nodes
- ✅ Requires only `DATABASE_URL` + `SECURE_VALUES_KEY` (same as Strategy 2)

**Cons:**
- ❌ LLM must be instructed to follow auth sequence (system_message matters)
- ❌ LLM could theoretically skip auth (needs max_iterations guard)
- ❌ Requires DB for secure value storage

**When to Use:**
- Agentic flows where the LLM orchestrates multi-step API interactions
- When auth tokens need to be managed dynamically by the agent
- Chatbots that call authenticated APIs on behalf of users

---

### Strategy 6: Interactive Collection via `secure_suspend` (CONVERSATION-DRIVEN)

**When to use it:**  
When the user is in the loop and must provide credentials interactively — e.g. the canvas-builder pattern where a meta-agent needs to collect API keys from the user before building and launching a new agent. Use this when credentials are not known upfront and cannot be injected via environment or webhook.

**How it works:**

The `secure_suspend` node pauses the DAG and presents the user with one or more questions (e.g., "Enter your API key"). Answers are encrypted with Postgres pgcrypto symmetric encryption (`pgp_sym_encrypt`/`pgp_sym_decrypt`, OpenPGP CFB — pgcrypto default cipher, **not** AES-256-GCM), keyed by `SECURE_VALUES_KEY`, and stored in `secure_value_mappings`. The node returns only opaque handles (`<sv_name>`) — the LLM and all other nodes **never see the real value**. On DAG resume the handles flow through the graph and are auto-injected by `inject_secrets` at execution time.

The node can be used in three ways:

**Mode A — Top-level DAG node:**
```json
{
  "collect_creds": {
    "type": "secure_suspend",
    "config": {
      "secrets": [
        { "name": "api_key",   "question": "Please enter your API key" },
        { "name": "api_secret","question": "Please enter your API secret" }
      ]
    }
  }
}
```
The DAG suspends until the user provides both values, then resumes. Downstream nodes receive `{ "api_key": "<sv_api_key>", "api_secret": "<sv_api_secret>" }`.

**Mode B — `secure_suspend_allowed: true` (recomendado):**

La forma más concisa de exponer `secure_suspend` como tool es el flag `secure_suspend_allowed` en la config del nodo `llm_call`. Al activarlo, el engine registra automáticamente una tool `ask_secret` con descripción canónica y `node_schema` listos para usar — no se necesita ninguna entrada en `tool_configurations`.

```json
{
  "agent": {
    "type": "llm_call",
    "config": {
      "provider": "google",
      "model": "gemini-2.5-flash",
      "api_key": "${GEMINI_API_KEY}",
      "connection_url": "${DATABASE_URL}",
      "secure_suspend_allowed": true,
      "system_message": "Eres un agente que recopila credenciales del usuario antes de hacer llamadas a APIs externas."
    }
  }
}
```

El LLM verá la tool `ask_secret` en su lista de herramientas disponibles. Al llamarla, el engine detecta `__colmena_status: SUSPENDED` en el resultado y propaga la suspensión hacia arriba, pausando el DAG. Al reanudar, la llamada pendiente se re-despacha con la respuesta del usuario y el loop del agente continúa normalmente.

> **Precedencia:** si `tool_configurations` ya contiene una entrada con `"node_type": "secure_suspend"`, el flag `secure_suspend_allowed` es un no-op — esa entrada tiene prioridad.

> **Convergencia:** ambos modos (B y C) convergen en el mismo mecanismo interno `apply_secure_suspend_tool_defaults`. El LLM ve un contrato idéntico en ambos casos.

**Mode C — `tool_configurations` explícito (para renombrar o co-ubicar overrides):**

Usa esta forma cuando necesites cambiar el nombre de la tool (p.ej. `ask_credentials`) o co-ubicar otras sobreescrituras de tool junto a `secure_suspend`:

```json
{
  "tool_configurations": {
    "ask_credentials": {
      "name": "ask_credentials",
      "node_type": "secure_suspend"
    }
  }
}
```

Los campos opcionales (`description`, `node_schema`, etc.) se rellenan con los defaults canónicos si se omiten, igual que en el Mode B. Ver [`docs/node_configurations.json`](../node_configurations.json) → `secure_suspend` para el schema completo.

Cuando el LLM llama a esta tool, `llm_call` detecta `__colmena_status: SUSPENDED` en el resultado y propaga la suspensión hacia arriba, pausando el DAG. Al reanudar, la llamada pendiente se re-despacha con la respuesta del usuario y el loop del agente continúa. Ver la nota **LLM tool suspend propagation** abajo.

**End-to-end flow:**
```
User starts DAG (or LLM calls ask_secret tool)
  ↓
secure_suspend node: DAG pauses, user sees question(s)
  ↓
User provides answers (ID-keyed Q/A — keyed by each secret's `name`):
  --answer "Q[api_key]: Please enter your API key
            A[api_key]: VALUE
            Q[api_secret]: Please enter your API secret
            A[api_secret]: VALUE2"
  ↓
Values encrypted with pgp_sym_encrypt → secure_value_mappings
  ↓
Node outputs: { api_key: "<sv_api_key>", api_secret: "<sv_api_secret>" }
  ↓
inject_secrets (runs on both inputs AND config)
  replaces handles with real values before node execution
  ↓
HTTP / downstream node calls API with real credentials ✅
LLM nodes see only "<sv_api_key>" ✅
```

#### Resume answer format

The `--answer` payload uses the canonical **ID-keyed Q/A format** shared with the `suspend` node:

```
Q[<name>]: <question echo>
A[<name>]: <answer body>
```

- `Q[<name>]:` and `A[<name>]:` are line-anchored literal prefixes. `<name>` is the secret's `name` from `config.secrets[i].name`.
- Order-independent: the parser binds by id, so the operator may answer secrets in any order.
- The text after `Q[<name>]:` is echoed for human readability only — the parser does not validate it. The LLM is free to rephrase or translate the question without breaking parsing.
- Multi-line answer bodies (PEM blocks, multi-line JSON) are preserved between `A[<name>]:` and the next prefix or end of input.
- ID character set: `[A-Za-z0-9_-]{1,64}` (already what `name` accepts).
- Each expected `name` must appear exactly once as `A[<name>]:`. Duplicates, missing names, unknown ids, or empty answers → parser error and resume fails loudly.
- `secure_suspend` only supports open questions in this iteration (no choice).
- For the classic `suspend` node, `config.id` is now **required** (no fallback to `__node_id`) — the same Q/A format applies, keyed by `config.id`.

Schema reference: [`docs/node_configurations.json`](../node_configurations.json) → `secure_suspend.resume_answer_format` and `suspend.resume_answer_format`.

**Pros:**
- ✅ No credentials need to be known at deploy time
- ✅ LLM never sees real credential values
- ✅ Works as top-level node or LLM tool
- ✅ Multi-secret batch collection in a single suspend round-trip
- ✅ Compatible with `agent_session_id`-first lookup (cross-session use case)

**Cons:**
- ❌ Requires a human in the loop (not suitable for fully automated pipelines)
- ❌ Requires PostgreSQL + `secure_value_mappings` table

**When to Use:**
- Canvas-builder / agent-spawning flows where the meta-agent must collect user secrets before configuring a new agent
- Any conversational DAG where credentials are not available in the environment
- Multi-tenant scenarios where each user provides their own credentials

**Spec:** [`docs/superpowers/specs/2026-05-07-secure-suspend-node-design.md`](../superpowers/specs/2026-05-07-secure-suspend-node-design.md)

---

### Note: `inject_secrets` Covers Both `inputs` and `config`

The engine runs `inject_secrets` on a node's **inputs AND config** before execution. This means `<sv_*>` handles placed directly in `node.config` fields (e.g., when a canvas-builder pre-populates a child node's config with a handle) are resolved to real values at execution time, without needing an edge to carry the value through inputs.

**Spec:** [`docs/superpowers/specs/2026-05-07-inject-secrets-in-config-design.md`](../superpowers/specs/2026-05-07-inject-secrets-in-config-design.md)

---

### Note: LLM Tool Suspend Propagation

When `secure_suspend` (or any suspendable node) is used as an LLM tool via `tool_configurations`, the `llm_call` node propagates suspension correctly:

1. Tool returns `{ "__colmena_status": "SUSPENDED", ... }`.
2. `agent_service` detects SUSPENDED and short-circuits the tool loop.
3. `llm_call` emits `__colmena_status: SUSPENDED` to the DAG.
4. On resume (user provides `--answer`), the conversation is replayed from memory, the pending tool call is re-dispatched with the answer routed in, and the agent loop continues normally.

This enables the full `secure_suspend`-as-tool pattern without any special handling in the graph.

**Spec:** [`docs/superpowers/specs/2026-05-08-llm-call-tool-suspend-design.md`](../superpowers/specs/2026-05-08-llm-call-tool-suspend-design.md)

---

### Note: `agent_session_id`-First Lookup in `secure_value_mappings`

The `secure_value_mappings` table has an `agent_session_id TEXT` column. When an `agent_session_id` is set (via `--agent-session-id` CLI flag or the `ColmenaEngine` API), secret lookup uses it first and falls back to `session_id`. This mirrors the pattern already used by `llm_node_history` and `dag_runs`.

**Use case:** A meta-agent (session A) collects and persists credentials under `agent_session_id = "agent_X"`. A later invocation with a new ephemeral `session_id` but the same `--agent-session-id agent_X` can retrieve those credentials, enabling cross-session secret sharing without exposing values.

**Spec:** [`docs/superpowers/specs/2026-05-08-secure-values-agent-session-id-design.md`](../superpowers/specs/2026-05-08-secure-values-agent-session-id-design.md)

---

## Comparison Matrix

| Strategy | Setup | Credentials in Code? | LLM Sees Real Values? | DB Required? | Encryption? | Audit Trail? | Production Ready? |
|----------|-------|----------------------|----------------------|--------------|-------------|--------------|-------------------|
| Env Vars | `export VAR=...` | No | **YES** ⚠️ | No | No | No | Limited |
| Webhook + Secure | HTTP POST + DB | No | **NO** ✓ | **Yes** | pgcrypto | Basic | **Yes** ✓ |
| Test Payload | Hardcode in JSON | **YES** ⚠️ | **YES** ⚠️ | No | No | No | **No** |
| DB Query (Future) | `store-secret` CLI | No | No | **Yes** | pgcrypto | **Yes** ✓ | Future |
| LLM-Driven Auth | tool + secure:true | No | **NO** ✓ | **Yes** | pgcrypto | Basic | **Yes** ✓ |
| `secure_suspend` | Human in loop | No | **NO** ✓ | **Yes** | pgcrypto | Basic | **Yes** ✓ |

---

## Decision Flow Chart

```
Start: Need credentials in DAG?
  ↓
Is this local development/testing?
  ├─ YES → Use Environment Variables (Strategy 1)
  │         SIMPLE, no DB setup needed
  │
  └─ NO → Will LLM nodes see these credentials?
            ├─ YES → OK if LLM should see them → Use Env Vars (Strategy 1)
            │         (e.g., LLM analyzing public data)
            │
            └─ NO → LLM must NEVER see them
                     └─ Can you ensure TLS on webhook?
                        ├─ YES → Use Webhook + Secure (Strategy 2) ✓
                        │         RECOMMENDED FOR NOW
                        │
                        └─ MAYBE → Plan for DB Query (Strategy 4)
                                   Future: maximum security
```

---

## Real-World Example: Amadeus Flight Search

### Scenario: Web app needs to search flights via Amadeus without exposing token to Claude LLM

**Architecture:**

```
Web App → POST /amadeus-search
           {client_id: "ABC123", client_secret: "XYZ789"}
          ↓
  Colmena DAG (runs in backend)
  ├─ trigger: receives creds from POST body
  ├─ get_amadeus_token: calls Amadeus with creds (secure: true)
  │   Response: {access_token: "token_abc_xyz"}
  │   After hashing: {access_token: "<value_1>"}
  │   DB stores: <value_1> → AES(token_abc_xyz)
  │
  ├─ search_flights: calls Amadeus flight API
  │   Input: {bearer_token: "<value_1>"}
  │   Auto-injected: {bearer_token: "token_abc_xyz"}
  │   Response: {flights: [...]}
  │
  └─ analyze_with_claude: LLM analyzes flights
      Input: {token: "<value_1>", flights: [...]}
      Claude SEES: <value_1> (opaque hash)
      Claude NEVER SEES: token_abc_xyz
      ↓
      Claude output: "Best option is Flight XYZ..."
      ↓
      Web app gets analysis without security breach
```

**Graph JSON:**
```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {"path": "/amadeus-search"}
    },
    "get_amadeus_token": {
      "type": "http_request",
      "config": {
        "endpoint": "/v1/security/oauth2/token",
        "secure": true,
        "body": {
          "client_id": "${trigger.client_id}",
          "client_secret": "${trigger.client_secret}"
        }
      }
    },
    "search_flights": {
      "type": "http_request",
      "config": {
        "endpoint": "/v2/shopping/flight-offers",
        "headers": {
          "Authorization": "Bearer ${get_amadeus_token.access_token}"
        }
      }
    },
    "analyze_with_claude": {
      "type": "llm_call",
      "config": {
        "provider": "anthropic",
        "api_key": "${ANTHROPIC_API_KEY}"
      },
      "inputs": {
        "user_message": "Analyze flights: ${search_flights.body}\nToken: ${get_amadeus_token.access_token}"
      }
    }
  }
}
```

**Execution:**
1. Web app calls: `POST /amadeus-search` with JSON body containing client credentials
2. Colmena receives creds in trigger node
3. HTTP node 1 calls Amadeus, gets `access_token: "real_token_xyz"`, hashes it to `<value_1>`
4. HTTP node 2 (search) auto-injects real token, calls Amadeus for flights
5. LLM node receives `<value_1>` in its prompt (NEVER sees real token)
6. LLM analyzes flights and returns recommendation
7. Database auto-cleans up `<value_1>` mapping after DAG completion

**Result:** ✅ Secure, transparent, works with LLM

---

## Security Best Practices

### DO ✅

- ✅ Use **Webhook + Secure** (Strategy 2) when LLM will process credential-adjacent data
- ✅ **Enable TLS** on webhook endpoints
- ✅ **Authenticate** webhook endpoints (API key, OAuth2, etc.)
- ✅ **Rotate** credentials regularly
- ✅ **Log** credential access (audit trail)
- ✅ **Limit DAG runtime** to prevent orphaned database entries
- ✅ **Use strong SECURE_VALUES_KEY** (32+ characters, random)
- ✅ **Store SECURE_VALUES_KEY** in secrets manager (not in .env file in git)

### DON'T ❌

- ❌ **Hardcode** credentials in graph JSON files
- ❌ **Commit** test payloads with real credentials to git
- ❌ **Log** HTTP request/response bodies containing credentials *(HttpNode now suppresses all body logging)*
- ❌ **Share** DAG files or screenshots containing `${ENV_VAR}` values
- ❌ **Use** Strategy 3 (test payload) in production
- ❌ **Expose** webhook endpoints without authentication
- ❌ **Trust** process environment variables alone (use Webhook + Secure instead)
- ❌ **Assume** HTTP-only TLS is sufficient (use certificate pinning for sensitive APIs)
- ❌ **Forget** to add internal flags to `reserved_keys` — any unknown primitive in `inputs` gets sent as query param to external APIs

---

## Environment Setup

### For Strategy 1: Environment Variables

```bash
export AMADEUS_CLIENT_ID="your_client_id"
export AMADEUS_CLIENT_SECRET="your_client_secret"
export GEMINI_API_KEY="sk-..."
export ANTHROPIC_API_KEY="sk-ant-..."
```

### For Strategy 2: Webhook + Secure

```bash
# Existing env vars
export AMADEUS_CLIENT_ID="..."
export AMADEUS_CLIENT_SECRET="..."
export GEMINI_API_KEY="..."

# Database setup
export DATABASE_URL="postgres://user:pass@localhost:5432/colmena"

# Encryption key for secure values (KEEP SECRET!)
export SECURE_VALUES_KEY="my-super-secret-32-character-minimum-key-12345"

# Pre-migration: enable pgcrypto
psql -d colmena -c "CREATE EXTENSION IF NOT EXISTS pgcrypto;"

# Create secure_value_mappings table (if not already done)
sqlx migrate run
```

### For Strategy 3: Test Payload

```bash
# NO special setup needed
# Just run: cargo run --bin dag_engine -- run tests/graphs/security/amadeus_secure_gemini_test.json
```

---

## Configuring Bearer Tokens with `node_schema` (Recommended)

When using HTTP nodes as LLM tools, the **modern approach** is to use `node_schema` with fixed bearer tokens:

```json
{
  "type": "llm_call",
  "config": {
    "provider": "google",
    "model": "gemini-2.5-flash",
    "api_key": "${GEMINI_API_KEY}",
    "enabled_tools": ["search_flights"],
    "tool_configurations": {
      "search_flights": {
        "name": "search_flights",
        "node_type": "http_request",
        "description": "Search for flight offers",
        "node_schema": {
          "base_url": {
            "type": "string",
            "fixed": "https://api.amadeus.com"
          },
          "endpoint": {
            "type": "string",
            "fixed": "/v2/shopping/flight-offers"
          },
          "method": {
            "type": "string",
            "fixed": "GET"
          },
          "bearer_token": {
            "type": "string",
            "fixed": "${context.amadeus_token}"
          },
          "query_params": {
            "type": "object",
            "properties": {
              "max": {
                "type": "string",
                "fixed": "5"
              },
              "originLocationCode": {
                "type": "string",
                "required": true,
                "description": "Origin IATA code (e.g., MAD)"
              },
              "destinationLocationCode": {
                "type": "string",
                "required": true,
                "description": "Destination IATA code (e.g., BCN)"
              },
              "departureDate": {
                "type": "string",
                "required": true,
                "description": "Date (YYYY-MM-DD)",
                "pattern": "^\\d{4}-\\d{2}-\\d{2}$"
              },
              "adults": {
                "type": "string",
                "required": true,
                "description": "Number of adults (1-9)"
              }
            }
          }
        }
      }
    }
  }
}
```

**How it works:**
1. `bearer_token` is marked `"fixed"` → LLM never sees the real value
2. When LLM calls the tool, it only provides `originLocationCode`, `destinationLocationCode`, etc.
3. The executor automatically injects `bearer_token: ${context.amadeus_token}` at runtime
4. If the token is marked as `secure: true` upstream, the LLM sees `<value_1>` (hash), but HTTP tools get the real token auto-injected
5. `query_params` with `required: true/false` tells LLM which parameters are mandatory

**Advantages:**
- ✅ Single source of truth for HTTP tool parameters
- ✅ LLM never sees real bearer tokens
- ✅ Pattern validation on dates (e.g., `^\\d{4}-\\d{2}-\\d{2}$`)
- ✅ Clear documentation of required vs optional parameters
- ✅ Automatic parameter merging into `query_params` container

---

## Testing the Secure Flow

### Test 1: Verify LLM Sees Hashes, Not Real Tokens

```bash
export AMADEUS_CLIENT_ID="ABC123"
export AMADEUS_CLIENT_SECRET="XYZ789"
export GEMINI_API_KEY="sk-..."
export DATABASE_URL="postgres://..."
export SECURE_VALUES_KEY="32-char-key..."

cargo run --bin dag_engine -- run tests/graphs/security/amadeus_secure_gemini_test.json
```

**Look for in output:**
```
✓ get_amadeus_token returns: {access_token: "<value_1>"}
✓ search_flights receives: bearer_token injected with real token
✓ analyze_with_gemini LLM prompt contains: "<value_1>" NOT "ABC123"
✓ Cleanup: 0 rows remaining in secure_value_mappings
```

### Test 2: Verify HTTP Node Gets Real Token (Auto-Injection)

Check the `search_flights` response. If the HTTP call succeeded with a 200 status, auto-injection worked and Amadeus received the real token.

### Test 3: Verify Database Encryption

```sql
SELECT * FROM secure_value_mappings;
-- Should show: encrypted_value as BYTEA (binary), NOT plaintext
```

---

## Roadmap

### Phase 1: NOW ✅
- Environment variables (Strategy 1)
- Webhook + Secure (Strategy 2)
- Test payloads (Strategy 3)
- LLM-Driven Auth via secure tools (Strategy 5)
- Interactive collection via `secure_suspend` — top-level node or LLM tool (Strategy 6)
- `inject_secrets` covers node `config` in addition to `inputs`
- `secure_value_mappings` `agent_session_id`-first lookup for cross-session flows

### Phase 2: SOON
- [ ] DB Query node (Strategy 4)
- [ ] CLI tool: `dag_engine store-secret`
- [ ] Audit logging table
- [ ] Credential rotation API

### Phase 3: FUTURE
- [ ] External secrets manager (Vault, AWS Secrets Manager)
- [ ] Key versioning and rotation
- [ ] Hardware security module (HSM) integration
- [ ] Zero-knowledge proofs for credentials

---

## FAQ

**Q: Which strategy should I use for production?**  
A: **Webhook + Secure (Strategy 2)** today, plan for **DB Query (Strategy 4)** when implemented.

**Q: Can I mix strategies?**  
A: Yes. E.g., use env vars for LLM API keys, use webhook payload for third-party API credentials, secure-flag the third-party response.

**Q: What if webhook is compromised?**  
A: TLS encryption protects in-transit. HTTPS + authentication protects the endpoint. Database encryption (pgcrypto `pgp_sym_encrypt`) protects at-rest.

**Q: How long are credentials stored in DB?**  
A: Sliding TTL of 24h, extended on each successful `decrypt` (see "Sliding TTL y outbound masking" below). Each run also triggers a bounded sweep — `cleanup_expired_for_run(session_id, agent_session_id)` in `secure_value_service.rs`, called from `run_use_case.rs` — that deletes only rows where `expires_at < NOW()` scoped to that session/agent_session; live (unexpired) rows survive across runs for multi-turn use.

**Q: Can I audit who accessed which credentials?**  
A: Currently no. Planned for Phase 2 (audit logging table).

**Q: What about API rate limits and secrets rotation?**  
A: Outside Colmena's scope. Manage via upstream service (Amadeus, OpenAI, etc.). Use webhook strategy so creds can be updated per-request.

**Q: Is `SECURE_VALUES_KEY` enough to encrypt credentials?**  
A: For MVP yes (pgcrypto `pgp_sym_encrypt`, OpenPGP CFB — not AES-256-GCM — via PostgreSQL). For high-security scenarios, integrate with Vault or AWS Secrets Manager (Phase 3).

> ⚠️ **CRITICAL — `SECURE_VALUES_KEY` is mandatory.** Since 2026-06-07 the
> Postgres secure-value backend **fails fast at startup** if the env var is
> unset or empty. Prior to that, the code silently fell back to the
> hardcoded string `"default-key"`, which let anyone with DB read access
> decrypt every stored secret. Export a random string of at least 32
> characters in **every** environment (dev, staging, prod) — typically via
> your secret manager — before instantiating the engine. There is no
> longer a way to "just try it" without a key.
>
> Tests that need to construct `PostgresSecureValueRepository` directly
> (without an ambient env var) should use the
> `new_with_key(pool, key)` constructor instead of `new(pool)`.

---

## Transport Security (TLS / SSL)

Este bloque resume el estado real del cifrado en tránsito para cada componente que abre sockets salientes.

### Postgres (`PgPoolRegistry`)

- sqlx 0.8 se compila con `runtime-tokio-rustls` — TLS disponible out-of-the-box, sin OpenSSL nativo.
- El pool crea conexiones con `.connect(url)`. El modo TLS se elige por query param:

  | `sslmode` | Qué hace |
  |-----------|----------|
  | omitido | `prefer`: intenta TLS, cae a plaintext |
  | `require` | Cifrado obligatorio, cert no validado |
  | `verify-ca` | Cifrado + valida CA (necesita `sslrootcert`) |
  | `verify-full` | `verify-ca` + valida hostname — **recomendado en producción** |

- `UrlKey::normalize` preserva query params, así que una URL con `sslmode=require` y otra sin él producen **pools separados** (aislamiento correcto).
- mTLS cliente no está expuesto por el registry (requeriría usar `PgConnectOptions` directamente).

### Nodo HTTP (`reqwest`)

- `reqwest 0.11` con features `["json", "stream", "multipart", "rustls-tls"]` y `default-features = false` — usa **rustls + webpki-roots** (bundle de CAs públicas embebido).
- El cliente se construye con `Client::builder().http1_only().build()` — sin flags de TLS personalizadas.
- En la práctica:
  - HTTPS contra APIs con CA pública: ✅ funciona.
  - Certificados self-signed o CA privada: ❌ falla. No hay opción actual para inyectar una CA.
  - mTLS cliente: ❌ no soportado.
- Recomendado: todo tráfico saliente del nodo HTTP debe ir por HTTPS. Los paths HTTP solo deberían aparecer en entornos locales/dev.

### Nodo Socket.IO (`rust_socketio`)

- `rust_socketio 0.6` — el scheme del URL decide el transporte:
  - `https://...` → Socket.IO sobre TLS.
  - `wss://...` → WebSocket Secure.
  - `http://...` / `ws://...` → sin cifrado.
- Internamente usa rustls (vía tungstenite) — mismo comportamiento que HTTP: solo CAs públicas.
- Config expone `transport: any|websocket|polling`, pero **ninguna opción TLS custom**.

### Brechas conocidas

| Capacidad | Postgres | HTTP | Socket.IO |
|-----------|:--------:|:----:|:---------:|
| TLS con CA pública | ✅ | ✅ | ✅ |
| `verify-full` hostname | ✅ | ✅ (default rustls) | ✅ |
| CA privada / self-signed | ✅ vía `sslrootcert` | ❌ | ❌ |
| mTLS (cliente con cert) | ❌ | ❌ | ❌ |

Si se necesita CA privada o mTLS en HTTP/Socket.IO hay que extender la config del nodo y crear un `Client` con `ClientBuilder::add_root_certificate(...)` o `Identity::from_pem(...)`. Para Postgres existe el camino vía `PgConnectOptions` (no expuesto hoy).

---

## Sliding TTL y outbound masking (desde 2026-05-11)

> **Spec:** [docs/superpowers/specs/2026-05-11-secure-values-sliding-ttl-design.md](../superpowers/specs/2026-05-11-secure-values-sliding-ttl-design.md)
> **Plan:** [docs/superpowers/plans/2026-05-11-secure-values-sliding-ttl.md](../superpowers/plans/2026-05-11-secure-values-sliding-ttl.md)

Cuatro garantías nuevas reemplazan al cleanup unconditional al final del run y blindan la superficie de secretos contra leaks via response bodies.

### 1. TTL deslizante de 24h, extendido en cada `decrypt`

El `expires_at` se setea a `NOW() + 24h` cuando se persiste un secreto y se **extiende otras 24h en cada uso** (`decrypt` exitoso). La extensión es atómica con el lookup vía `UPDATE … RETURNING`:

```sql
UPDATE secure_value_mappings
   SET expires_at = NOW() + INTERVAL '24 hours'
 WHERE handle = $1
   AND (session_id = $2 OR agent_session_id = $3)
   AND expires_at > NOW()
RETURNING decrypted_value;
```

`exists()` también filtra `expires_at > NOW()` pero **NO** extiende la ventana — es una precondición de chequeo, no un uso. Resultado: una conversación activa que se prolonga más de 24h no se queda sin credenciales abruptamente; el cap efectivo pasa de "24h desde el persist" a "24h desde el último uso".

Implementación: `src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs`. El TTL está hardcodeado como literal SQL `INTERVAL '24 hours'` repetido en las tres queries (persist, decrypt-with-agent, decrypt-with-session; líneas 91, 136, 152) — no hay una constante nombrada; configurabilidad vía env var queda explícitamente fuera de scope.

### 2. Cleanup periódico por expiración (no más barrido total)

Antes: al final de cada run, `run_use_case.rs` llamaba `cleanup(session_id)` que borraba **todas** las filas del `session_id` ephemeral, incluyendo las que se acababan de persistir en el mismo run para uso multi-turno. Esto rompía cross-run flows donde una conversación canvas-builder/A-B reusaba un token persistido en un turno anterior.

Ahora: `cleanup_expired_for_run(session_id, agent_session_id)` borra **solo filas expiradas** (`expires_at < NOW()`) scoped a `session_id` OR `agent_session_id` del run. Filas vivas sobreviven. Cada turno limpia su propio scope — patrón B3 (bounded per-run sweep) descripto en la sección "Decision" del spec.

Implementación: `src/libs/colmena/src/dag_engine/application/secure_value_service.rs` (método `cleanup_expired_for_run`) + el repo Postgres correspondiente. Engine call site: `src/libs/colmena/src/dag_engine/application/run_use_case.rs` (línea ~687 del antes; ahora invoca la versión bounded).

### 3. Handle hardening — sufijo random + min-length

**Antes** los handles eran `<sv_user>`, `<sv_pass>`, `<sv_token>` — predecibles. Un LLM que viera el formato una vez podría intentar inyectar `<sv_admin>` y, si existía en el scope, decrypt accidentalmente le devolvería el valor.

**Ahora** el `SecureValueService::persist_secret` genera un sufijo random de 8 hex chars al handle:

```rust
fn new_handle(name: &str) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();  // 32 hex
    let suffix: String = id.chars().take(8).collect();
    format!("<sv_{name}_{suffix}>")
}
```

Ejemplos:
- Viejo: `<sv_user>`
- Nuevo: `<sv_user_4f3a2b9c>`

Adicionalmente, **`secure_suspend` rechaza valores con menos de 4 caracteres**:

```rust
const MIN_SECRET_VALUE_LEN: usize = 4;
// ... después de parse_qa_response ...
if value.chars().count() < MIN_SECRET_VALUE_LEN {
    return Err(format!(
        "secure_suspend: value for secret '{}' is too short (min 4 chars). \
         Short values cause unsafe outbound masking — please supply ≥4 chars.",
        secret.name
    ).into());
}
```

El mínimo de 4 chars permite PINs estándar pero rechaza strings de alta colisión (`"on"`, `"ok"`, `"42"`). Es un prerequisito de la garantía #4 — substring matching sobre valores muy cortos causaría sobre-enmascarado patológico en response bodies.

Implementación: `src/libs/colmena/src/dag_engine/application/secure_value_service.rs::persist_secret` (handle generation), `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs` (min-length check).

**Compatibilidad backward.** Los handles viejos (`<sv_user>` sin sufijo) **siguen resolviendo** porque el lookup es match exacto sobre `hash_key` — no hay parsing del formato. No hay backfill ni flag de versión. Conversaciones en curso siguen funcionando sin tocar nada.

### 4. Outbound masking en `DagToolExecutor`

**El leak vector.** Cuando un nodo consume un secreto (decrypted por `inject_secrets` antes de ejecutar), su response puede echo el valor al LLM. Ejemplo: un login HTTP recibe `username = <sv_user_4f3a2b9c>`, lo resuelve a `"alice"`, llama al endpoint, y la respuesta es `{"token": "abc", "username": "alice"}` — `"alice"` era secreto y ahora va de vuelta al LLM verbatim.

**El choke point.** Una pasada de masking en `DagToolExecutor::execute_inner`, antes de retornar al `agent_service`. Funciona porque CADA tool result pasa por ese punto, incluyendo errors:

```rust
// Inside DagToolExecutor::execute_inner, after node execution:
let applied = self.secure_value_service
    .inject_secrets(&mut node_inputs, session_id, agent_session_id)
    .await?;  // applied: HashMap<decrypted_value, handle>

let result = node.execute(...).await;
// MASK every tool response — Ok and Err paths, same mask_outbound() for both.
// Err is wrapped into a Value::String, masked, then unwrapped back into an error.
match result {
    Ok(mut v) => { self.secure_value_service.mask_outbound(&mut v, &applied); Ok(v) }
    Err(e) => {
        let mut err_value = Value::String(e.to_string());
        self.secure_value_service.mask_outbound(&mut err_value, &applied);
        Err(err_value.as_str().unwrap_or("").to_string().into())
    }
}
```

**Cómo funciona el masking.** Walk recursivo de la `serde_json::Value`. Para cada JSON string, reemplazar cada substring que coincida con un valor decrypted por su handle. Reemplazos aplicados **longest-key-first** para evitar leaks parciales cuando dos secretos comparten prefix.

```rust
pub fn mask_outbound(&self, value: &mut serde_json::Value, mapping: &HashMap<String, String>) {
    let mut sorted: Vec<&String> = mapping.keys().collect();
    sorted.sort_by_key(|k| std::cmp::Reverse(k.len()));  // longest first
    // ... recursive walk replacing substrings ...
}
```

**Diff en la API de `inject_secrets`.** Cambió de retornar `Result<()>` a `Result<HashMap<String, String>>` (mapeo de `decrypted_value → handle`). Callers que no necesitan el mapeo (ej. `run_use_case` para nodos non-LLM) simplemente ignoran el return.

Implementación:
- `src/libs/colmena/src/dag_engine/application/secure_value_service.rs` — nueva firma de `inject_secrets` + `mask_outbound`.
- `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs::execute_inner` — call site del masking.

### Consecuencia para graph authors

Nada cambia en tu graph JSON. Los handles que ves en los logs son levemente más largos (con sufijo), las respuestas de tools que contenían el valor decrypted ahora muestran el handle en su lugar, y los secretos que no se usan en 24h se borran solos sin afectar los activos.

Si pasaste handles viejos (`<sv_*>` sin sufijo) en conversation history persistida antes del 2026-05-11, siguen resolviendo. Si tu test depende de handles deterministas (ej. snapshot de output), tenés que actualizar el snapshot a la nueva forma con sufijo.

---

## References

- [Secure Values — diseño](../dds/SECURE_VALUES_DISEÑO.md)
- [Data Flow Guide](16_data_flow_guide.md)
- [LLM Deep Dive](14_llm_deep_dive.md)
- [Connection Pool Management spec](../superpowers/specs/2026-04-20-connection-pool-management-design.md)

---

**Status:** Documentation Updated  
**Date:** 2026-05-18  
**Version:** 1.3  
**Changes:** Added section "Sliding TTL y outbound masking (desde 2026-05-11)" covering the four guarantees introduced by spec `2026-05-11-secure-values-sliding-ttl-design.md`: (1) sliding 24h TTL extended on `decrypt`, (2) per-run cleanup of expired rows replacing the unconditional run-end sweep, (3) handle hardening with 8-hex random suffix + min-length 4 chars, (4) outbound masking of tool responses in `DagToolExecutor::execute_inner`. Previous (v1.2, 2026-05-08): Added Strategy 6 (`secure_suspend` — interactive credential collection as top-level DAG node or LLM tool). Documented `inject_secrets` now covers node `config` in addition to `inputs`. Added note on `llm_call` propagating `SUSPENDED` from tool results with replay-on-resume. Added note on `agent_session_id`-first lookup in `secure_value_mappings` for cross-session flows. Updated Comparison Matrix and Roadmap Phase 1. Previous: Strategy 5 (LLM-Driven Auth via secure tools), `secure=true` query-param fix, HttpNode body/query param docs.
