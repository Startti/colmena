# Secure Values Test Graphs

Test graphs for the Secure Values feature in HTTP nodes.

## Test Cases

### 1. `http_secure_basic.json` - Simple Secure HTTP
**Purpose:** Test that HTTP node with `secure: true` hashes all output values.

**What happens:**
1. HTTP node makes POST request to httpbin.org with secret data
2. Response is processed:
   - Without secure flag: values pass through as-is
   - With secure flag: all body values are replaced with hashes like `<value_1>`
3. Output node displays the hashed response

**Expected behavior:**
- ✓ Output should contain placeholders like `<value_1>`, `<value_2>`, `<value_3>`
- ✓ Real values (my_api_key_12345, alice, etc.) should NOT appear in output
- ✓ Database should contain encrypted mappings
- ✓ Database should be cleaned up after DAG completes

### 2. `http_secure_to_http_inject.json` - Token Injection
**Purpose:** Test that non-LLM nodes get real values auto-injected.

**What happens:**
1. First HTTP node (`get_token`):
   - Makes secure request
   - Output: `{body: {json: {token: "<value_1>", client_id: "<value_2>"}}}`
   - Stores encrypted mappings in DB

2. Second HTTP node (`use_token_in_header`):
   - Receives input: `{bearer_token: "<value_1>"}`
   - **Injection phase**: Non-LLM node → lookup DB → replace with real token
   - Makes actual HTTP request with real token: `X-Auth-Token: sk_live_abc123xyz`

3. Output node displays success

**Expected behavior:**
- ✓ First node output contains hashes
- ✓ Second node actually uses real token in HTTP request
- ✓ Second HTTP call succeeds (200 OK) with real token
- ✓ Database cleaned up after completion

## Running Tests

### Test 1: Simple secure HTTP
```bash
cargo run --bin dag_engine -- run tests/graphs/security/http_secure_basic.json
```

Expected output will show:
- HTTP request made to httpbin.org
- Response contains `<value_1>`, `<value_2>`, `<value_3>` placeholders
- No real secret values visible

### Test 2: Token injection
```bash
cargo run --bin dag_engine -- run tests/graphs/security/http_secure_to_http_inject.json
```

Expected output will show:
- First HTTP call: response with hashes
- Second HTTP call: successful with injected real token
- Database cleanup completed

## Verification

### Check Encrypted Values in Database

```bash
psql $DATABASE_URL

SELECT session_id, hash_key, field_name, created_at 
FROM secure_value_mappings 
ORDER BY created_at DESC;
```

After DAG completes, this should return 0 rows (cleanup worked).

### Check During Execution

If you want to inspect mappings before cleanup, add a debug statement or modify cleanup to happen later.

## Environment Variables Required

```bash
export DATABASE_URL="postgres://user:pass@localhost/colmena"
export SECURE_VALUES_KEY="your-32-character-minimum-encryption-key"
```

## Implementation Details

### Secure HTTP Output Flow
1. HTTP node executes → returns `{status: 200, body: {...}}`
2. `SecureValueService.hash_output()` processes:
   - Skips `status` field
   - Hashes all values in `body`: string → `<value_N>`
   - Persists to `secure_value_mappings` table (encrypted)
3. Output to next nodes: hashes only

### Auto-Injection Flow
1. Next node is scheduled for execution
2. **Before execution**:
   - If node_type ≠ "llm": `inject_secrets()`
   - Scan inputs for placeholders: `<value_N>`
   - Query DB: decrypt real value
   - Replace placeholder with real value
3. Execute node with real values

### LLM Node Protection
- LLM nodes skip injection phase
- LLM always receives hashes: `<value_1>`, `<value_2>`, etc.
- LLM can reference them but never sees actual secrets
- If LLM generates subsequent HTTP calls referencing hashes, they get injected

## Troubleshooting

**Issue:** Tests fail with "pgcrypto not enabled"
```bash
# Fix: Enable pgcrypto extension
psql $DATABASE_URL
CREATE EXTENSION pgcrypto;
```

**Issue:** Tests fail with "Encryption key not set"
```bash
# Fix: Set environment variable
export SECURE_VALUES_KEY="min-32-chars-encryption-key"
```

**Issue:** Database values not being hashed
- Check that `secure: true` is in node config
- Check that values are strings/numbers (not objects/arrays at root)

---

For more details see: `docs/SECURE_VALUES_VISUAL_EXAMPLES.md`
