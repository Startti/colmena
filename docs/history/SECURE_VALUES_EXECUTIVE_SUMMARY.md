# Secure Values for HTTP Nodes - Executive Summary

## What's the Problem?

Currently, when an HTTP node returns a secret (API token, credential, password), that value flows through the DAG to **LLM nodes**. LLMs see the real secret in their context window.

**Risks:**
- ❌ LLM logs expose tokens (insecure deployment)
- ❌ Token accidentally appears in fine-tuning data
- ❌ Prompt injection attacks can extract credentials
- ❌ Third-party LLM providers see your secrets

**Example:**
```
HTTP: POST /auth → {token: "sk_live_abc123"}
      ↓
LLM sees: "Your token is sk_live_abc123"
      ↓
❌ Token visible in logs, context, fine-tuning
```

---

## Solution: Secure Flag

Add a `secure: true` flag to HTTP nodes. When enabled:

```json
{
  "type": "http",
  "config": {
    "endpoint": "/token",
    "secure": true    ← NEW FLAG
  }
}
```

**Result:**

| Component | Sees |
|-----------|------|
| HTTP node output | `{token: "<value_1>"}` (placeholder) |
| LLM node | `<value_1>` (can't access real token) |
| Next HTTP node | `sk_live_abc123` (real token, auto-injected) |
| Database | `encrypted(sk_live_abc123)` (encrypted at rest) |

---

## How It Works (Simple Flow)

```
1. HTTP Node Executes
   Output: {token: "sk_live_abc123"}

2. Secure Value Service (NEW)
   Hashes: "sk_live_abc123" → "<value_1>"
   Encrypts & stores in DB
   Returns: {token: "<value_1>"}

3. LLM Node Receives
   Input: {token: "<value_1>"}
   ✓ Never sees real token

4. Another HTTP Node Needs Token
   Input has: {bearer_token: "<value_1>"}
   Service injects: "sk_live_abc123"
   HTTP executes with real token
   ✓ Transparent injection

5. DAG Ends
   Automatic cleanup: DELETE all mappings
   ✓ No secrets left in DB
```

---

## Key Features

✅ **Transparent:** No code changes needed for HTTP nodes that consume the token  
✅ **Automatic:** Injection happens automatically before non-LLM node execution  
✅ **Encrypted:** Values encrypted at rest (AES-256)  
✅ **Cleanup:** Auto-delete when DAG completes  
✅ **LLM-Safe:** LLMs see only hashes, can't extract secrets  
✅ **Simple:** Single `secure: true` flag (no config needed)  

---

## Architecture (Quick)

**Three layers:**

```
Application Layer
├─ DagRunUseCase (orchestrator)
│  ├─ Before HTTP execution: hash & encrypt
│  └─ Before non-LLM execution: inject real values

Domain Layer
├─ SecureValueRepository (trait)
│  └─ persist(), decrypt(), cleanup()
└─ SecureValueService (business logic)
   ├─ hash_output()
   ├─ inject_secrets()
   └─ cleanup()

Infrastructure Layer
└─ PostgresSecureValueRepository
   └─ AES-256 encryption via pgcrypto
```

**Database:**

```
secure_value_mappings table:
├─ session_id (which DAG run)
├─ hash_key (e.g., <value_1>)
├─ encrypted_value (AES-encrypted real value)
├─ field_name (for audit)
└─ expires_at (1-hour timeout safety net)
```

---

## Implementation Status

### ✅ Complete
- ✅ Full architecture designed
- ✅ Data flows documented
- ✅ Database schema finalized
- ✅ All code written (ready to implement)
- ✅ Testing strategy defined
- ✅ Security considerations reviewed

### 📋 Ready to Start
- [ ] Create domain trait (SecureValueRepository)
- [ ] Create service (SecureValueService)
- [ ] Create PostgreSQL implementation
- [ ] Integrate with DagRunUseCase
- [ ] Run migrations
- [ ] Test with amadeus graph

**Effort:** 4-6 hours | **Complexity:** Medium (mostly DB + Rust integration)

---

## Configuration Example

### Before (Insecure)

```json
{
  "nodes": [
    {
      "id": "auth",
      "type": "http",
      "config": {
        "endpoint": "/token",
        "body": {"api_key": "${API_KEY}"}
      }
    },
    {
      "id": "agent",
      "type": "llm"
    }
  ]
}
```

**Problem:** LLM sees the real token from auth node.

### After (Secure)

```json
{
  "nodes": [
    {
      "id": "auth",
      "type": "http",
      "config": {
        "endpoint": "/token",
        "secure": true,  ← ADD THIS
        "body": {"api_key": "${API_KEY}"}
      }
    },
    {
      "id": "agent",
      "type": "llm"
    }
  ]
}
```

**Solution:** LLM sees `<value_1>` instead of real token.

---

## Real-World Example: Amadeus Flight Search

```json
{
  "nodes": [
    {
      "id": "get_amadeus_token",
      "type": "http",
      "config": {
        "endpoint": "/v1/security/oauth2/token",
        "secure": true,  ← PROTECTS TOKEN
        "body": {
          "client_id": "${AMADEUS_CLIENT_ID}",
          "client_secret": "${AMADEUS_CLIENT_SECRET}"
        }
      }
    },
    {
      "id": "search_flights",
      "type": "http",
      "inputs": {
        "bearer_token": "${get_amadeus_token.body.access_token}"
        ← Auto-injected with real token
      }
    },
    {
      "id": "book_trip",
      "type": "llm"
      ← Sees: <value_1> (NOT the real token)
    }
  ]
}
```

**Execution:**
1. `get_amadeus_token` returns `{access_token: "real_token_xyz"}`
2. Service hashes → `{access_token: "<value_1>"}`
3. `search_flights` node gets real token auto-injected
4. `book_trip` LLM sees `<value_1>` (safe!)

---

## Testing

### Unit Tests
```bash
cargo test secure_value_service::
```

### Integration Test
```bash
cargo run --bin dag_engine -- run tests/graphs/security/http_secure_basic.json
```

### Real-World Test
```bash
# Existing test graph, just add "secure": true
cargo run --bin dag_engine -- run tests/graphs/advanced/amadeus_secure.json
```

---

## Roadmap

### Phase 1: MVP (4-6 hours)
- [x] Design
- [ ] Implementation
- [ ] Testing
- Includes: `secure: true` (all-or-nothing)

### Phase 2: Enhancement (2-3 hours)
- [ ] Granular `secure_fields: ["token", "api_key"]`
- [ ] Background cleanup task
- [ ] Audit logging
- [ ] Metrics

### Phase 3: Advanced (Future)
- [ ] External secrets manager (Vault, AWS Secrets Manager)
- [ ] Key rotation
- [ ] Field path tracking

---

## Environment Setup

```bash
# Required
export SECURE_VALUES_KEY="32-character-minimum-encryption-key"
export DATABASE_URL="postgres://user:pass@localhost/colmena"

# Migration
sqlx migrate run
```

---

## Security Guarantees

| Threat | Mitigation |
|--------|------------|
| **Tokens in logs** | Real values never serialized, only hashes |
| **Fine-tuning exposure** | LLM sees only hashes (`<value_1>`) |
| **Prompt injection** | Hash is opaque, can't be reversed |
| **DB breach** | Values encrypted with AES-256 |
| **Orphaned secrets** | Auto-cleanup + 1-hour timeout |

---

## Files to Review

**For Implementation:**
1. `docs/SECURE_VALUES_IMPLEMENTATION.md` — All code, step-by-step
2. `docs/SECURE_VALUES_DESIGN.md` — Architecture & theory
3. `docs/SECURE_VALUES_QUICK_REFERENCE.md` — Patterns & lookup

**For Examples:**
- `docs/SECURE_VALUES_VISUAL_EXAMPLES.md` — 6 real scenarios with diagrams

**In Memory:**
- `SECURE_VALUES_SUMMARY.md` — Quick status & decisions

---

## Decision Summary

| Decision | Why |
|----------|-----|
| **Secure flag on config** | Simple, obvious, no hidden behavior |
| **PostgreSQL pgcrypto** | Built-in, audited, no external deps |
| **Hash all values** | Simpler MVP (v2 can be granular) |
| **Auto-inject for non-LLM** | Transparent, no manual steps |
| **Auto-cleanup on DAG end** | Safest, compliant with secret retention |
| **No selective mode v1** | Keeps MVP simple, can add later |

---

## Next Steps

1. **Review** → Read `SECURE_VALUES_IMPLEMENTATION.md`
2. **Create** → Add 7 files (domain, service, repository, etc.)
3. **Test** → Run with amadeus graph + `"secure": true`
4. **Deploy** → Set env vars, run migrations
5. **Iterate** → Phase 2 with granular fields

---

## Questions?

- **"How does it work?"** → SECURE_VALUES_DESIGN.md (architecture section)
- **"Show me examples"** → SECURE_VALUES_VISUAL_EXAMPLES.md (6 scenarios)
- **"Ready to code?"** → SECURE_VALUES_IMPLEMENTATION.md (all code included)
- **"Quick lookup?"** → SECURE_VALUES_QUICK_REFERENCE.md (patterns, troubleshooting)

**Status:** ✅ Ready to implement. Design complete, all code written.
