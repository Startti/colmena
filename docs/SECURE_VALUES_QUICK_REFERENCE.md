# Secure Values - Quick Reference

## TL;DR

**Feature:** HTTP nodes can mark outputs as `secure: true`. Real values get encrypted in DB, replaced with placeholders. LLM sees hashes, other nodes get real values auto-injected.

---

## How It Works

```
HTTP Node (secure: true)
    ↓ Output: {token: "sk_live_123"}
    ↓
Hash & Encrypt
    ↓ Output: {token: "<value_1>"}
    ↓ DB: <value_1> → encrypted(sk_live_123)
    ↓
Next Node?
    ├─ LLM → sees <value_1> (can't access token)
    └─ HTTP → gets token injected, sees "sk_live_123"
```

---

## JSON Configuration

### Enable Secure Mode

```json
{
  "type": "http",
  "config": {
    "base_url": "https://api.example.com",
    "endpoint": "/token",
    "method": "POST",
    "secure": true
  }
}
```

### Example: Amadeus Flight Search

```json
{
  "id": "fetch_amadeus_token",
  "type": "http",
  "config": {
    "base_url": "https://api.amadeus.com",
    "endpoint": "/v1/security/oauth2/token",
    "method": "POST",
    "secure": true,
    "body": {
      "client_id": "${AMADEUS_CLIENT_ID}",
      "client_secret": "${AMADEUS_CLIENT_SECRET}"
    }
  }
}
```

**What happens:**
1. HTTP node calls Amadeus API
2. Response: `{access_token: "ABC123XYZ"}`
3. With secure: true → becomes `{access_token: "<value_1>"}`
4. DB stores mapping: `<value_1> → encrypted(ABC123XYZ)`

---

## Node Behavior

| Node Type | Receives | Sees |
|-----------|----------|------|
| HTTP (after secure HTTP) | Injected | `access_token: "ABC123XYZ"` (real value) |
| LLM (after secure HTTP) | Hashed | `access_token: "<value_1>"` (placeholder) |
| Output (after secure HTTP) | Hashed | Same hashes |
| Python/Debug | Hashed | Same hashes |

---

## Environment Setup

```bash
# Required for encryption
export SECURE_VALUES_KEY="my-secret-key-32-chars-minimum"

# Database
export DATABASE_URL="postgres://user:pass@localhost/colmena"
```

**Encryption Key Requirements:**
- Minimum 32 characters
- Store in `.env` or secrets manager (NOT in git)
- Same key must decrypt values

---

## Testing

### Command Line

```bash
# Run a secure graph
cargo run --bin dag_engine -- run tests/graphs/security/http_secure_basic.json

# With session ID (useful for debugging)
cargo run --bin dag_engine -- run tests/graphs/security/http_secure_basic.json \
  --session-id "test_session_123"
```

### Unit Tests

```bash
# Run secure value service tests
cargo test secure_value_service::

# Run all tests including integration
cargo test -- --include-ignored
```

### Example Test Graph

File: `tests/graphs/security/http_secure_basic.json`

```json
{
  "nodes": [
    {
      "id": "secure_http",
      "type": "http",
      "config": {
        "base_url": "https://httpbin.org",
        "endpoint": "/post",
        "method": "POST",
        "secure": true,
        "body": {
          "secret": "my_api_key_12345",
          "username": "alice"
        }
      }
    },
    {
      "id": "use_secret",
      "type": "http",
      "config": {
        "base_url": "https://httpbin.org",
        "endpoint": "/headers",
        "method": "GET"
      },
      "inputs": {
        "bearer_token": "${secure_http.body.json.secret}"
      }
    },
    {
      "id": "llm_sees_hash",
      "type": "llm",
      "config": {
        "model": "gpt-4",
        "system": "You are a helpful assistant"
      },
      "inputs": {
        "user_message": "My secret is ${secure_http.body.json.secret}"
      }
    }
  ]
}
```

**Flow:**
1. `secure_http` → Returns with secret hashed: `{secret: "<value_1>"}`
2. `use_secret` → Gets injected: `{secret: "my_api_key_12345"}` ✅ Can use real secret
3. `llm_sees_hash` → Receives: `{secret: "<value_1>"}` ⛔ Never sees real secret

---

## Troubleshooting

### Placeholder Not Injecting

**Problem:** HTTP node after secure HTTP doesn't get real value
```
Expected: bearer_token: "ABC123"
Got: bearer_token: "<value_1>"
```

**Solution:** Ensure node type is NOT "llm" (only LLMs keep hashes)

### Encryption Key Missing

**Error:** `SECURE_VALUES_KEY env var not set`

**Fix:**
```bash
export SECURE_VALUES_KEY="your-32-char-minimum-key-here"
```

### DB Table Not Found

**Error:** `relation "secure_value_mappings" does not exist`

**Fix:** Run migrations
```bash
sqlx migrate run
```

### pgcrypto Not Enabled

**Error:** `function pgp_sym_encrypt does not exist`

**Fix:** Manually enable in DB
```sql
CREATE EXTENSION IF NOT EXISTS pgcrypto;
```

---

## Security Notes

| What | How | Why |
|------|-----|-----|
| **Encryption** | PostgreSQL pgcrypto (AES-256) | Industry standard |
| **Storage** | Database BYTEA column | Encrypted at rest |
| **In-memory** | Real values only in RAM during injection | Never logged/serialized |
| **LLM Isolation** | Hashes only sent to LLM | Models can't extract secrets |
| **Cleanup** | Auto-delete on DAG end | No orphaned secrets |

---

## Database Schema

```sql
CREATE TABLE secure_value_mappings (
    id UUID PRIMARY KEY,
    session_id VARCHAR(255),      -- Which DAG run
    source_node_id VARCHAR(255),  -- Which HTTP node
    hash_key VARCHAR(255),         -- e.g., <value_1>
    encrypted_value BYTEA,         -- AES-256 encrypted
    field_name VARCHAR(255),       -- For auditing
    created_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,        -- 1-hour fallback cleanup
    UNIQUE(session_id, hash_key)
);
```

---

## Common Patterns

### Pattern 1: Token from API → Use in Headers

```json
{
  "nodes": [
    {
      "id": "auth",
      "type": "http",
      "config": {
        "endpoint": "/auth",
        "secure": true,
        "body": {"api_key": "${MY_API_KEY}"}
      }
    },
    {
      "id": "use_token",
      "type": "http",
      "config": {
        "endpoint": "/data",
        "headers": {
          "Authorization": "Bearer ${auth.body.token}"
        }
      }
    }
  ]
}
```

**What happens:**
- `auth` returns hashed: `{token: "<value_1>"}`
- `use_token` gets injected: `{token: "real_token_abc"}` in header

### Pattern 2: Credentials from API → Show to LLM (Safe)

```json
{
  "nodes": [
    {
      "id": "fetch_creds",
      "type": "http",
      "config": {
        "endpoint": "/credentials",
        "secure": true
      }
    },
    {
      "id": "agent",
      "type": "llm",
      "inputs": {
        "user_message": "Use credentials: ${fetch_creds.body.api_key}"
      }
    }
  ]
}
```

**What happens:**
- LLM sees: `api_key: "<value_1>"` (never sees real credential)
- If agent needs to use credential, it references `<value_1>`, which gets injected in subsequent HTTP calls

---

## Files & Locations

### Source Code
- **Domain:** `src/dag_engine/domain/secure_value_repository.rs`
- **Service:** `src/dag_engine/application/secure_value_service.rs`
- **Infrastructure:** `src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs`
- **Integration:** `src/dag_engine/application/run_use_case.rs` (modified)

### Tests
- **Unit:** Inside `secure_value_service.rs` (`#[cfg(test)]`)
- **Integration:** `tests/secure_values_integration.rs`
- **Graphs:** `tests/graphs/security/*.json`

### Docs
- **Architecture:** `docs/SECURE_VALUES_DESIGN.md` (detailed)
- **Implementation:** `docs/SECURE_VALUES_IMPLEMENTATION.md` (step-by-step)
- **This Reference:** `docs/SECURE_VALUES_QUICK_REFERENCE.md`

---

## Performance

### Overhead

| Operation | Cost | When |
|-----------|------|------|
| Hash output | O(n) JSON traversal | After HTTP execution |
| Inject secrets | O(n) JSON traversal | Before non-LLM execution |
| Persist | 1 DB write per value | After hash |
| Decrypt | 1 DB query per placeholder | Before execution |
| Cleanup | 1 DELETE statement | DAG end |

**Typical impact:** < 10ms per node for most payloads

### Optimization Tips

1. **Only use `secure: true` on HTTP nodes that return secrets**
2. **Skip for public APIs** (no sensitive data)
3. **Batch multiple values** (already handled by service)

---

## Roadmap

### ✅ Phase 1 (MVP)
- [x] Hash all values when `secure: true`
- [x] Inject for non-LLM nodes
- [x] Auto-cleanup on DAG end
- [x] pgcrypto encryption

### 📋 Phase 2 (Enhancement)
- [ ] `secure_fields: ["token", "api_key"]` (granular)
- [ ] Background cleanup task
- [ ] Audit logging (who accessed what)
- [ ] Metrics (secret usage)

### 🔮 Phase 3 (Advanced)
- [ ] Recursive field path tracking
- [ ] Secret rotation
- [ ] External secrets manager integration (Vault, AWS Secrets Manager)
- [ ] Key versioning

---

## Questions?

See:
- **How it works:** `SECURE_VALUES_DESIGN.md`
- **Build it:** `SECURE_VALUES_IMPLEMENTATION.md`
- **Code:** Source files listed above
