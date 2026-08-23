# Quick Start: Testing Amadeus + Secure Values + Gemini LLM

## ⚡ 5-Minute Setup

### Step 1: Get Your API Keys

You need:
- **Amadeus** credentials (free sandbox at amadeus.com)
- **Gemini** API key (free tier at ai.google.dev)
- **PostgreSQL** database (or Docker)

### Step 2: Set Environment Variables

```bash
# Amadeus (sandbox or production)
export AMADEUS_CLIENT_ID="your_client_id_here"
export AMADEUS_CLIENT_SECRET="your_client_secret_here"

# Gemini API
export GEMINI_API_KEY="your_gemini_api_key_here"

# PostgreSQL (or use docker-compose to spin one up)
export DATABASE_URL="postgres://user:password@localhost:5432/colmena"

# Encryption key for secure values (IMPORTANT: keep this secret!)
export SECURE_VALUES_KEY="my-super-secret-32-character-minimum-key-abc123"
```

### Step 3: Setup PostgreSQL (if needed)

**Option A: Use existing local Postgres**
```bash
# Create database
psql -U postgres -c "CREATE DATABASE colmena;"

# Enable pgcrypto for encryption
psql -U postgres -d colmena -c "CREATE EXTENSION IF NOT EXISTS pgcrypto;"

# Run migrations to create secure_value_mappings table
sqlx migrate run --database-url "$DATABASE_URL"
```

**Option B: Docker**
```bash
docker run --name colmena-postgres \
  -e POSTGRES_PASSWORD=password \
  -p 5432:5432 \
  -d postgres:15

# Give it a second to start
sleep 2

# Setup
psql -h localhost -U postgres -c "CREATE DATABASE colmena;"
psql -h localhost -U postgres -d colmena -c "CREATE EXTENSION IF NOT EXISTS pgcrypto;"

# Set connection string
export DATABASE_URL="postgres://postgres:password@localhost:5432/colmena"
```

### Step 4: Run the Test Graph

```bash
cd /path/to/colmena

cargo run --bin dag_engine -- run tests/graphs/security/amadeus_secure_gemini_test.json
```

---

## 🔍 What Happens During Execution

```
Step 1: trigger_webhook
  ├─ Reads env vars: AMADEUS_CLIENT_ID, AMADEUS_CLIENT_SECRET
  └─ Outputs: {client_id: "ABC123", client_secret: "XYZ789"}

Step 2: get_amadeus_token (secure: true)
  ├─ Calls: POST https://api.amadeus.com/v1/security/oauth2/token
  ├─ Response: {access_token: "real_token_xyz..."}
  ├─ Hashing: {access_token: "<value_1>"}
  ├─ DB: INSERT secure_value_mappings(<value_1>, AES(real_token_xyz))
  └─ Outputs: {access_token: "<value_1>"}

Step 3: search_flights
  ├─ Input has: bearer_token = "<value_1>"
  ├─ Auto-inject: Lookup DB → "real_token_xyz"
  ├─ Calls: GET https://api.amadeus.com/v2/shopping/flight-offers
  │   Header: Authorization: Bearer real_token_xyz
  └─ Response: {data: [{flight info}]}

Step 4: analyze_with_gemini
  ├─ Input prompt: "Auth Token: <value_1>"  (NOT real token!)
  ├─ Calls: Gemini API
  ├─ Gemini SEES: "<value_1>" (opaque hash)
  ├─ Gemini NEVER SEES: "real_token_xyz"
  └─ Response: "Best option is flight ABC..."

Step 5: log_results
  ├─ Prints everything
  └─ Cleanup: DELETE FROM secure_value_mappings (auto-cleanup)
```

---

## ✅ Success Criteria

**You know it worked if you see:**

1. **Log output shows `<value_1>` NOT the real token:**
   ```
   Access Token Received: <value_1>
   (NOT: Access Token Received: sk_live_xyz...)
   ```

2. **Flight search succeeded (200 OK):**
   ```
   Search Flights Response:
   {data: [{id: "1", fare: {...}}, ...]}
   ```

3. **Gemini analyzed the flights:**
   ```
   Gemini Analysis:
   "Based on the flight data provided, I recommend..."
   ```

4. **Cleanup happened:**
   ```
   Database cleanup verified: 0 remaining mappings
   ```

---

## 🛠️ Troubleshooting

### "Missing environment variable AMADEUS_CLIENT_ID"
```bash
# Make sure to export, not just set:
export AMADEUS_CLIENT_ID="your_value"
echo $AMADEUS_CLIENT_ID  # Verify it's set
```

### "Connection refused: 5432 (PostgreSQL not running)"
```bash
# Start PostgreSQL:
docker run --name colmena-postgres \
  -e POSTGRES_PASSWORD=password \
  -p 5432:5432 \
  -d postgres:15

# Or if using local Postgres:
brew services start postgresql  # macOS
sudo systemctl start postgres   # Linux
```

### "Database 'colmena' does not exist"
```bash
psql -U postgres -c "CREATE DATABASE colmena;"
```

### "Relation 'secure_value_mappings' does not exist"
```bash
# Run migrations:
sqlx migrate run --database-url "$DATABASE_URL"

# Or manually create the table:
psql -d colmena << 'EOF'
CREATE TABLE secure_value_mappings (
    id UUID PRIMARY KEY,
    session_id VARCHAR(255),
    source_node_id VARCHAR(255),
    hash_key VARCHAR(255),
    encrypted_value BYTEA,
    field_name VARCHAR(255),
    created_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    UNIQUE(session_id, hash_key)
);
EOF
```

### "401 Unauthorized from Amadeus"
- Double-check your `AMADEUS_CLIENT_ID` and `AMADEUS_CLIENT_SECRET`
- Make sure you're using sandbox credentials (not production)
- Check that your Amadeus account has API access enabled

### "401 Unauthorized from Gemini"
- Verify `GEMINI_API_KEY` is correct
- Check that your Google Cloud project has the Generative AI API enabled
- Try a simple request: `curl -X POST https://generativelanguage.googleapis.com/... -H "x-goog-api-key: $GEMINI_API_KEY"`

### "Cargo build fails"
```bash
cargo clean
cargo build --bin dag_engine

# If still failing:
cargo test --lib dag_engine
```

---

## 📊 Understanding the Output

### Normal Output (Success)
```
=== TOKEN ANALYSIS ===
Access Token Received: <value_1>
(Above should be <value_N> format, NOT a real token)

=== FLIGHT SEARCH RESULTS ===
{
  "data": [
    {
      "id": "1",
      "source": "GDS",
      "instantTicketingRequired": false,
      "nonHomogeneous": false,
      "oneWay": false,
      "lastTicketingDate": "2026-05-01",
      "numberOfBookableSeats": 4,
      "itineraries": [...],
      "price": {
        "total": "150.00",
        "base": "100.00",
        ...
      }
    },
    ...
  ]
}

=== GEMINI ANALYSIS ===
Based on the flight data provided, I recommend Flight #1 for the following reasons:
1. Most competitive price at €150.00
2. Single connection with reasonable layover time
3. Departure at 08:00 AM (good for business travel)
...

=== SECURITY VERIFICATION ===
✓ HTTP node (search_flights) used real token via auto-injection
✓ LLM node (analyze_with_gemini) sees only secure hash
✓ Database stores encrypted mapping: <value_1> → pgp_sym_encrypt(real_token)
```

---

## 🔐 Verify Security (Advanced)

### Check that LLM actually received the hash

Look at Colmena logs or add `verbose: true` to the LLM node:
```json
{
  "type": "llm_call",
  "config": {
    "provider": "google",
    "verbose": true
  }
}
```

This will print the actual prompt sent to Gemini. Should show:
```
Prompt sent to Gemini:
"...Auth Token (secure): <value_1>..."
```

NOT:
```
Prompt sent to Gemini:
"...Auth Token (secure): sk_live_xyz..."  ❌ WRONG!
```

### Check database encryption

```bash
psql -d colmena

# See encrypted values (should be binary/unreadable)
SELECT hash_key, encrypted_value FROM secure_value_mappings;

# Output:
 hash_key |         encrypted_value
----------+-------------------------------------
 <value_1> | \x1c2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b...

# The encrypted_value is binary (BYTEA), not plaintext ✓
```

---

## 📚 Next Steps

After this test passes:

1. **Explore more patterns:**
   - [Security Strategy](../developer_guide/13_security_strategy.md) — All 4 strategies explained
   - [Data Flow Guide](../developer_guide/16_data_flow_guide.md) — How data flows between nodes

2. **Plan for production:**
   - Implement Strategy 4: DB Query node for credential management
   - Setup webhook authentication
   - Configure credential rotation
   - Enable audit logging

3. **Run more tests:**
   - Try other secure graphs: `tests/graphs/security/*.json`
   - Build your own graph with secure values
   - Test with different LLM providers (Anthropic, OpenAI, etc.)

---

## 🚀 Single Command Test

If you want to test EVERYTHING in one command (after setting env vars):

```bash
# Start Postgres
docker run --rm -d --name colmena-test-db \
  -e POSTGRES_PASSWORD=password \
  -p 5432:5432 \
  postgres:15

sleep 3

# Setup
PGPASSWORD=password psql -h localhost -U postgres -c "CREATE DATABASE colmena;"
PGPASSWORD=password psql -h localhost -U postgres -d colmena -c "CREATE EXTENSION IF NOT EXISTS pgcrypto;"

# Run test
export DATABASE_URL="postgres://postgres:password@localhost:5432/colmena"
cargo run --bin dag_engine -- run tests/graphs/security/amadeus_secure_gemini_test.json

# Cleanup
docker stop colmena-test-db
```

---

## Questions?

- **"Is my setup correct?"** → Check all env vars are set: `echo $AMADEUS_CLIENT_ID $GEMINI_API_KEY $DATABASE_URL`
- **"What went wrong?"** → Run with `RUST_LOG=debug` for verbose output
- **"How does secure hashing work?"** → Read [Secure Values — diseño](../dds/SECURE_VALUES_DISEÑO.md)
- **"Can I use this in production?"** → Yes, but read [Security Strategy](../developer_guide/13_security_strategy.md) first

---

**Status:** Ready to test  
**Date:** 2026-04-04
