# Secure Values - Implementation Checklist

## 📋 Pre-Implementation

- [ ] Read `SECURE_VALUES_IMPLEMENTATION.md` (code is there)
- [ ] Review database schema in `SECURE_VALUES_DESIGN.md`
- [ ] Understand flow in `SECURE_VALUES_VISUAL_EXAMPLES.md`
- [ ] Set environment variables:
  ```bash
  export SECURE_VALUES_KEY="32-char-minimum-secret-key"
  export DATABASE_URL="postgres://..."
  ```

---

## 🔧 Phase 1: Create Files (Copy-Paste Code)

### File 1: Domain Trait
- [ ] Create: `src/dag_engine/domain/secure_value_repository.rs`
  - Copy trait from `SECURE_VALUES_IMPLEMENTATION.md`
  - Copy test module
- [ ] Estimated: 5 min

### File 2: Application Service
- [ ] Create: `src/dag_engine/application/secure_value_service.rs`
  - Copy `SecureValueService` struct from docs
  - Copy all methods (hash, inject, cleanup)
  - Copy unit tests
- [ ] Estimated: 10 min

### File 3: PostgreSQL Implementation
- [ ] Create: `src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs`
  - Copy `PostgresSecureValueRepository` implementation
  - Review SQL queries (especially pgcrypto calls)
- [ ] Estimated: 8 min

### File 4: Domain Module Exports
- [ ] Edit: `src/dag_engine/domain/mod.rs`
  - Add: `pub mod secure_value_repository;`
  - Add: `pub use secure_value_repository::SecureValueRepository;`
- [ ] Estimated: 1 min

### File 5: Application Module Exports
- [ ] Edit: `src/dag_engine/application/mod.rs`
  - Add: `pub mod secure_value_service;`
  - Add: `pub use secure_value_service::SecureValueService;`
- [ ] Estimated: 1 min

### File 6: Infrastructure Module Exports
- [ ] Edit: `src/dag_engine/infrastructure/persistence/mod.rs`
  - Add: `pub mod postgres_secure_value_repository;`
  - Add: `pub use postgres_secure_value_repository::PostgresSecureValueRepository;`
- [ ] Estimated: 1 min

### File 7: Integration into DagRunUseCase
- [ ] Edit: `src/dag_engine/application/run_use_case.rs`
  - Add imports (SecureValueService, PostgresSecureValueRepository)
  - Add field to struct
  - Update constructor
  - Modify `execute_node` method (3 places: inject, execute, hash)
  - Add cleanup at DAG end
- [ ] Estimated: 15 min (most complex)

**Subtotal Phase 1:** ~41 min

---

## 🏗️ Phase 2: Build & Migrate

### Build Check
- [ ] `cargo build`
  - Should compile with no errors
  - May have warnings (OK for now)
- [ ] Estimated: 2 min (first time: longer)

### Database Migrations
- [ ] Ensure PostgreSQL is running
- [ ] Run: `sqlx prepare --database-url "$DATABASE_URL"`
- [ ] This creates `sqlx-data.json` (git-ignore this for migrations)
- [ ] Run: `sqlx migrate add secure_values`
- [ ] Run: `sqlx migrate run`
- [ ] Verify: Query `secure_value_mappings` table exists
- [ ] Estimated: 3 min

### Check Compilation
- [ ] `cargo check` (faster than build)
- [ ] `cargo clippy` (linting)
  - Fix any obvious issues
- [ ] Estimated: 2 min

**Subtotal Phase 2:** ~7 min

---

## 🧪 Phase 3: Unit Tests

### Run Service Tests
- [ ] `cargo test secure_value_service:: -- --nocapture`
  - Should see 3 passing tests:
    - ✓ test_hash_output_with_secure_flag
    - ✓ test_hash_output_without_secure_flag
    - ✓ test_inject_secrets_restores_values
- [ ] Estimated: 2 min

### Debug Any Failures
- [ ] If tests fail:
  - Check imports in secure_value_service.rs
  - Verify MockSecureValueRepository is defined
  - Check tokio::test attribute on async tests
- [ ] Estimated: 5-10 min (if needed)

**Subtotal Phase 3:** ~5-12 min

---

## 🔗 Phase 4: Manual Testing

### Test Graph 1: Simple HTTP → Output
- [ ] Create: `tests/graphs/security/http_secure_basic.json`
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
          "body": {"secret": "api_key_12345"}
        }
      }
    ]
  }
  ```
- [ ] Run: `cargo run --bin dag_engine -- run tests/graphs/security/http_secure_basic.json`
- [ ] Verify:
  - ✓ No errors
  - ✓ Output contains `<value_*>` placeholders
  - ✓ Check logs for "Secure value persisted"
- [ ] Estimated: 5 min

### Test Graph 2: Secure HTTP → Next HTTP (Injection)
- [ ] Create: `tests/graphs/security/http_secure_to_http.json`
  ```json
  {
    "nodes": [
      {
        "id": "get_token",
        "type": "http",
        "config": {
          "endpoint": "/post",
          "secure": true,
          "body": {"token": "real_token_123"}
        }
      },
      {
        "id": "use_token",
        "type": "http",
        "inputs": {
          "bearer_token": "${get_token.body.json.token}"
        }
      }
    ]
  }
  ```
- [ ] Run the graph
- [ ] Verify:
  - ✓ First node output has `<value_1>`
  - ✓ Second node receives real token (check logs)
  - ✓ HTTP request succeeds with real token
- [ ] Estimated: 5 min

### Test Graph 3: Secure HTTP → LLM (LLM sees hash)
- [ ] Create: `tests/graphs/security/http_secure_to_llm.json`
  ```json
  {
    "nodes": [
      {
        "id": "secure_http",
        "type": "http",
        "config": {
          "endpoint": "/post",
          "secure": true,
          "body": {"api_key": "secret_xyz"}
        }
      },
      {
        "id": "llm_node",
        "type": "llm",
        "inputs": {
          "user_message": "My key is ${secure_http.body.json.api_key}"
        }
      }
    ]
  }
  ```
- [ ] Run the graph
- [ ] Verify:
  - ✓ LLM receives `<value_1>` (not real key)
  - ✓ Check LLM prompt shows hash
- [ ] Estimated: 5 min

### Test Graph 4: Real-World (Amadeus)
- [ ] Use: `tests/graphs/advanced/travel_agent_amadeus.json`
- [ ] Modify first HTTP node:
  ```json
  {
    "config": {
      "secure": true
    }
  }
  ```
- [ ] Run the full graph
- [ ] Verify:
  - ✓ Token from Amadeus is hashed
  - ✓ Second HTTP call gets token injected
  - ✓ LLM planning node sees hash
- [ ] Estimated: 10 min

**Subtotal Phase 4:** ~25 min

---

## 🗄️ Phase 5: Database Verification

### Inspect Secure Values Table
- [ ] Connect to PostgreSQL:
  ```bash
  psql $DATABASE_URL
  ```
- [ ] Run:
  ```sql
  SELECT session_id, hash_key, field_name, created_at 
  FROM secure_value_mappings 
  ORDER BY created_at DESC 
  LIMIT 5;
  ```
- [ ] Verify:
  - ✓ Rows exist after test runs
  - ✓ `hash_key` format is `<value_N>`
  - ✓ `encrypted_value` is BYTEA (not readable)
  - ✓ Sessions cleared after DAG ends
- [ ] Estimated: 3 min

### Check Cleanup
- [ ] Run a test graph
- [ ] Count rows: `SELECT COUNT(*) FROM secure_value_mappings;`
- [ ] Wait for DAG to complete
- [ ] Count rows again: should be zero
- [ ] Verify: cleanup happened automatically
- [ ] Estimated: 2 min

**Subtotal Phase 5:** ~5 min

---

## 📊 Phase 6: Full Test Suite

### Run All Tests
- [ ] `cargo test`
  - Watch for any failures
  - Should see:
    - ✓ secure_value_service tests (3)
    - ✓ Other existing tests
- [ ] Estimated: 5 min (first time: longer)

### Run with Feature Flags
- [ ] `cargo test --features python`
- [ ] `cargo test --features node`
- [ ] Ensure no breakage
- [ ] Estimated: 5 min

**Subtotal Phase 6:** ~10 min

---

## 📈 Phase 7: Performance Check

### Measure Overhead
- [ ] Run test graph with `time`:
  ```bash
  time cargo run --bin dag_engine -- run tests/graphs/security/http_secure_basic.json
  ```
- [ ] Note execution time
- [ ] Run same graph without secure flag, compare
- [ ] Expected overhead: < 50ms for typical payload
- [ ] Estimated: 3 min

### Load Test (Optional)
- [ ] Create graph with 10 secure HTTP nodes
- [ ] Run and measure
- [ ] Verify performance scales linearly
- [ ] Estimated: 5 min (if doing this)

**Subtotal Phase 7:** ~3-8 min

---

## 📚 Phase 8: Documentation

### Update CLAUDE.md
- [ ] Add to feature flags section:
  ```markdown
  - `secure_values` — Enables encryption of HTTP node outputs (default: enabled)
  ```
- [ ] Add example with amadeus + secure flag
- [ ] Estimated: 3 min

### Update README
- [ ] Add section: "Secure Values in HTTP Nodes"
- [ ] Link to docs
- [ ] Show quick example
- [ ] Estimated: 5 min

### Create Test Graph Documentation
- [ ] Add: `docs/graphs/security/README.md`
- [ ] List all security test graphs
- [ ] Explain what each tests
- [ ] Estimated: 5 min

**Subtotal Phase 8:** ~13 min

---

## ✅ Phase 9: Final Verification

### Code Review Checklist
- [ ] All files created/modified ✓
- [ ] No compilation errors ✓
- [ ] All tests passing ✓
- [ ] Database schema correct ✓
- [ ] Cleanup working ✓
- [ ] LLM isolation verified ✓
- [ ] Performance acceptable ✓
- [ ] Documentation updated ✓

### Git Commit
- [ ] `git status` (review changes)
- [ ] `git add src/dag_engine/{domain,application,infrastructure}/**`
- [ ] `git add tests/graphs/security/`
- [ ] `git add docs/SECURE_VALUES_*.md`
- [ ] `git commit -m "feat: implement secure values for HTTP nodes

  - Add secure flag to HTTP node config
  - Encrypt sensitive values in DB (AES-256)
  - Auto-inject for non-LLM nodes
  - Auto-cleanup on DAG termination
  - LLM nodes see only hashes
  
  Fixes #XXX (if relevant)"`
- [ ] Estimated: 2 min

**Subtotal Phase 9:** ~10 min

---

## 🎯 Total Time Estimate

| Phase | Time | Status |
|-------|------|--------|
| 1: Create Files | ~41 min | 🔧 Copy-paste from docs |
| 2: Build & Migrate | ~7 min | 🏗️ Compile & setup DB |
| 3: Unit Tests | ~5-12 min | 🧪 Run service tests |
| 4: Manual Testing | ~25 min | 🔗 4 test graphs |
| 5: DB Verification | ~5 min | 🗄️ Query & verify |
| 6: Full Test Suite | ~10 min | 📊 cargo test |
| 7: Performance | ~3-8 min | 📈 Measure overhead |
| 8: Documentation | ~13 min | 📚 Update docs |
| 9: Verification | ~10 min | ✅ Final checks |
| **TOTAL** | **~119-129 min** | **~2 hours** |

---

## 🚀 Quick Start (Experienced Dev)

If you've done this before:

1. Copy code from `SECURE_VALUES_IMPLEMENTATION.md` (15 min)
2. Build & migrate (5 min)
3. Run unit tests (2 min)
4. Test with amadeus graph (10 min)
5. Commit (2 min)

**Fast track:** ~34 min

---

## 🐛 Troubleshooting During Implementation

### Issue: Compilation error - "SecureValueRepository not found"
**Fix:** Check `src/dag_engine/domain/mod.rs` exports are added

### Issue: `pgp_sym_encrypt function not found`
**Fix:** Run `CREATE EXTENSION pgcrypto;` manually in psql

### Issue: `SECURE_VALUES_KEY env var not set`
**Fix:** Set environment variable before running:
```bash
export SECURE_VALUES_KEY="my-32-character-minimum-key"
```

### Issue: Tests fail with "connection refused"
**Fix:** Ensure PostgreSQL is running:
```bash
pg_isready  # Check status
```

### Issue: Build succeeds but tests fail
**Fix:** Run migrations:
```bash
sqlx migrate run
```

---

## ✨ Success Criteria

You're done when:

- [x] All 7 files created/modified
- [x] `cargo build` succeeds
- [x] `cargo test` shows 3 new passing tests
- [x] Test graphs execute without errors
- [x] Secure HTTP node output contains `<value_1>` placeholders
- [x] Next HTTP node gets real values auto-injected
- [x] LLM node sees only hashes
- [x] Database cleanup verified
- [x] Performance < 50ms overhead
- [x] Commit pushed

---

## 📞 If Stuck

1. **Compilation error?** → Check imports & exports in `mod.rs` files
2. **Runtime error?** → Check database connection & migrations
3. **Test failing?** → Check test graph JSON syntax
4. **Performance issue?** → Measure per-node overhead with timestamps
5. **Unclear requirement?** → Reread `SECURE_VALUES_DESIGN.md`

---

## Next Phase (After MVP)

Once Phase 1 is complete and tested:

- [ ] **Phase 2:** Add `secure_fields: ["token", "api_key"]` for granular control
- [ ] **Phase 3:** Background cleanup task (every 5 min)
- [ ] **Phase 4:** Audit logging (who accessed what secret)
- [ ] **Phase 5:** Integration with external secret managers

---

**Ready to start? → Open `SECURE_VALUES_IMPLEMENTATION.md` and start copying code!**
