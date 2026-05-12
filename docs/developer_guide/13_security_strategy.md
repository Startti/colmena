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
- `llm_call` — resolves `${VAR}` for api_key field only

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
   - Stored in DB: `<value_1> → AES-256(real_token_xyz)`

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
- ✅ Values encrypted at rest (AES-256 in PostgreSQL)
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

The `secure_suspend` node pauses the DAG and presents the user with one or more questions (e.g., "Enter your API key"). Answers are encrypted with AES-256-GCM and stored in `secure_value_mappings`. The node returns only opaque handles (`<sv_name>`) — the LLM and all other nodes **never see the real value**. On DAG resume the handles flow through the graph and are auto-injected by `inject_secrets` at execution time.

The node can be used in two ways:

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

**Mode B — LLM tool via `tool_configurations`:**
```json
{
  "tool_configurations": {
    "collect_creds": {
      "name": "collect_creds",
      "node_type": "secure_suspend",
      "description": "Ask the user for API credentials. Call this before making authenticated requests.",
      "node_schema": {
        "secrets": {
          "fixed": [
            { "name": "api_key",    "question": "Please enter your API key" },
            { "name": "api_secret", "question": "Please enter your API secret" }
          ]
        }
      }
    }
  }
}
```
When the LLM calls this tool, `llm_call` detects `__colmena_status: SUSPENDED` in the tool result and propagates SUSPENDED upward, pausing the DAG. On resume the pending tool call is re-dispatched with the user's answer and the LLM loop continues. See the note on **LLM tool suspend propagation** below.

**End-to-end flow:**
```
User starts DAG (or LLM calls collect_creds tool)
  ↓
secure_suspend node: DAG pauses, user sees question(s)
  ↓
User provides answers (ID-keyed Q/A — keyed by each secret's `name`):
  --answer "Q[api_key]: Please enter your API key
            A[api_key]: VALUE
            Q[api_secret]: Please enter your API secret
            A[api_secret]: VALUE2"
  ↓
Values encrypted with AES-256 → secure_value_mappings
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
| Webhook + Secure | HTTP POST + DB | No | **NO** ✓ | **Yes** | AES-256 | Basic | **Yes** ✓ |
| Test Payload | Hardcode in JSON | **YES** ⚠️ | **YES** ⚠️ | No | No | No | **No** |
| DB Query (Future) | `store-secret` CLI | No | No | **Yes** | AES-256 | **Yes** ✓ | Future |
| LLM-Driven Auth | tool + secure:true | No | **NO** ✓ | **Yes** | AES-256 | Basic | **Yes** ✓ |
| `secure_suspend` | Human in loop | No | **NO** ✓ | **Yes** | AES-256 | Basic | **Yes** ✓ |

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
A: TLS encryption protects in-transit. HTTPS + authentication protects the endpoint. Database encryption (AES-256) protects at-rest.

**Q: How long are credentials stored in DB?**  
A: Automatically deleted when DAG completes (on `dag_end` trigger in `run_use_case.rs`). Fallback timeout: 1 hour.

**Q: Can I audit who accessed which credentials?**  
A: Currently no. Planned for Phase 2 (audit logging table).

**Q: What about API rate limits and secrets rotation?**  
A: Outside Colmena's scope. Manage via upstream service (Amadeus, OpenAI, etc.). Use webhook strategy so creds can be updated per-request.

**Q: Is `SECURE_VALUES_KEY` enough to encrypt credentials?**  
A: For MVP yes (AES-256 via PostgreSQL pgcrypto). For high-security scenarios, integrate with Vault or AWS Secrets Manager (Phase 3).

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

- `reqwest 0.11` con features `["json", "stream", "rustls-tls"]` y `default-features = false` — usa **rustls + webpki-roots** (bundle de CAs públicas embebido).
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

## References

- [Secure Values Design](SECURE_VALUES_DESIGN.md)
- [Secure Values Implementation](SECURE_VALUES_IMPLEMENTATION.md)
- [Secure Values Quick Reference](SECURE_VALUES_QUICK_REFERENCE.md)
- [NODE_CONNECTION_AND_DATA_FLOW.md](NODE_CONNECTION_AND_DATA_FLOW.md)
- [LLM Node Complete Guide](LLM_NODE_COMPLETE_GUIDE.md)
- [Connection Pool Management spec](../superpowers/specs/2026-04-20-connection-pool-management-design.md)

---

**Status:** Documentation Updated  
**Date:** 2026-05-08  
**Version:** 1.2  
**Changes:** Added Strategy 6 (`secure_suspend` — interactive credential collection as top-level DAG node or LLM tool). Documented `inject_secrets` now covers node `config` in addition to `inputs`. Added note on `llm_call` propagating `SUSPENDED` from tool results with replay-on-resume. Added note on `agent_session_id`-first lookup in `secure_value_mappings` for cross-session flows. Updated Comparison Matrix and Roadmap Phase 1. Previous: Strategy 5 (LLM-Driven Auth via secure tools), `secure=true` query-param fix, HttpNode body/query param docs.
