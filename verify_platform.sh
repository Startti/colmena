#!/bin/bash
set -e

# Cleanup on exit
trap 'kill $(jobs -p) 2>/dev/null' EXIT

echo "🚀 Starting Colmena Platform E2E Test..."

# 1. Start Redis
echo "📦 Starting Redis..."
docker-compose up -d redis
# Wait for Redis
echo "   Waiting for Redis..."
sleep 2

# 2. Start API (Background)
echo "🌐 Starting API Gateway..."
RUST_LOG=info cargo run --bin api &
API_PID=$!
sleep 5 # Wait for compilation and startup

# 3. Start Worker (Background)
echo "👷 Starting Worker..."
RUST_LOG=info cargo run --bin worker &
WORKER_PID=$!
sleep 5 # Wait for compilation and startup

# 4. Prepare Payload
# We embed the DAG JSON into the payload
DAG_JSON=$(cat tests/dags/python_simple_graph.json)
PAYLOAD=$(jq -n --argjson dag "$DAG_JSON" '{dag_json: $dag, inputs: {}}')

echo "📨 Sending Execution Request to API..."
RESPONSE=$(curl -s -X POST http://localhost:3000/api/v1/executions \
  -H "Content-Type: application/json" \
  -d "$PAYLOAD")

echo "   Response: $RESPONSE"

JOB_ID=$(echo $RESPONSE | jq -r .job_id)

if [ "$JOB_ID" == "null" ]; then
    echo "❌ Failed to enqueue job"
    exit 1
fi

echo "✅ Job Enqueued: $JOB_ID"
echo "🌐 Open 'test_stream.html' in your browser to test the Real-time Streaming!"
echo "⏳ Waiting for Worker to process (Check logs above)..."
sleep 5

echo "🎉 Test Completed. Press Ctrl+C to exit/cleanup."
wait
