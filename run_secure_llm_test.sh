#!/bin/bash
# Test script for Secure Values with LLM streaming

set -e

echo "================================"
echo "🔐 Secure Values + LLM Test"
echo "================================"
echo ""

# Check environment
echo "📋 Checking environment variables..."
if [ -z "$DATABASE_URL" ]; then
    echo "❌ DATABASE_URL not set"
    exit 1
fi
echo "✅ DATABASE_URL set"

if [ -z "$OPENAI_API_KEY" ]; then
    echo "⚠️  OPENAI_API_KEY not set (needed for GPT-4)"
    echo "   You can still run the test, but LLM node will fail"
fi

if [ -z "$SECURE_VALUES_KEY" ]; then
    echo "⚠️  SECURE_VALUES_KEY not set, using default"
    export SECURE_VALUES_KEY="default-key-for-testing-only-32chars"
fi

echo ""
echo "📊 Database check..."
psql "$DATABASE_URL" -c "SELECT COUNT(*) as existing_mappings FROM secure_value_mappings" 2>/dev/null || echo "⚠️  Table doesn't exist yet (will be created)"

echo ""
echo "🚀 Running graph with streaming (HTTP Secure → LLM → Output)..."
echo ""
echo "Expected behavior:"
echo "  1. HTTP node returns with secret values"
echo "  2. Secure service hashes: secret_token → <value_1>, client_id → <value_2>"
echo "  3. LLM receives hashed values in the prompt"
echo "  4. LLM streams response (you should see hashes in the prompt, NOT real values)"
echo "  5. Database is cleaned up after"
echo ""
echo "---"
echo ""

# Run the test
cargo run --bin dag_engine -- run tests/graphs/security/http_secure_to_llm_test.json \
    --include-extra-info 2>&1 | tee /tmp/secure_llm_test.log

echo ""
echo "---"
echo ""
echo "📊 Checking database after execution..."
REMAINING=$(psql "$DATABASE_URL" -t -c "SELECT COUNT(*) FROM secure_value_mappings" 2>/dev/null || echo "error")
if [ "$REMAINING" = "0" ] || [ "$REMAINING" = " 0" ]; then
    echo "✅ Database cleanup worked! (0 remaining mappings)"
else
    echo "⚠️  Found $REMAINING remaining mappings (cleanup may not have run)"
fi

echo ""
echo "📄 Test output saved to: /tmp/secure_llm_test.log"
echo ""
echo "🔍 To verify secure values were used:"
echo "   grep -i '<value_' /tmp/secure_llm_test.log | head -5"
echo ""
echo "================================"
echo "Test Complete!"
echo "================================"
