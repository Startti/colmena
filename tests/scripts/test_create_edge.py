#!/usr/bin/env python3
"""Quick test: create two nodes and connect them with an edge via Socket.IO."""
import os
import time
import socketio

API_URL = os.environ["ADP_API_URL"]
SESSION_TOKEN = os.environ["ADP_SESSION_TOKEN"]
ENV_ID = os.environ["ADP_ENVIRONMENT_ID"]

sio = socketio.Client()

@sio.on("exception", namespace="/canvas")
def on_exception(data):
    print(f"❌ EXCEPTION: {data}")

@sio.on("edge_created", namespace="/canvas")
def on_edge_created(data):
    print(f"✅ Edge created: {data}")

@sio.on("node_created", namespace="/canvas")
def on_node_created(data):
    print(f"✅ Node created: {data.get('id', data)}")

@sio.on("canvas_state_loaded", namespace="/canvas")
def on_canvas_loaded(data):
    nodes = data.get("nodes", [])
    edges = data.get("edges", [])
    print(f"📋 Canvas loaded: {len(nodes)} nodes, {len(edges)} edges")
    for e in edges:
        print(f"   Edge: {e['source']}({e.get('sourceHandle')}) -> {e['target']}({e.get('targetHandle')}) animated={e.get('animated')} type={e.get('type')}")

sio.connect(
    API_URL,
    namespaces=["/canvas"],
    headers={"Cookie": f"__Secure-better-auth.session_token={SESSION_TOKEN}"},
)
print("Connected!")

# Load canvas first
print("\n--- Loading canvas ---")
sio.emit("load_canvas_state", {"environmentId": ENV_ID}, namespace="/canvas")
time.sleep(2)

# Create node 1
print("\n--- Creating node 1 (chatInput) ---")
sio.emit("create_node", {
    "environmentId": ENV_ID,
    "node": {
        "type": "chatInput",
        "category": "trigger",
        "position": {"x": 100, "y": 500},
        "data": {"label": "Python Test Input", "config": {"variableName": "test", "inputType": "Text"}},
    },
}, namespace="/canvas")
time.sleep(2)

# Create node 2
print("\n--- Creating node 2 (llmCall) ---")
sio.emit("create_node", {
    "environmentId": ENV_ID,
    "node": {
        "type": "llmCall",
        "category": "ai",
        "position": {"x": 400, "y": 500},
        "data": {"label": "Python Test LLM", "config": {"model": "gemini-2.5-flash", "temperature": 0.5}},
    },
}, namespace="/canvas")
time.sleep(2)

# Reload canvas to get node IDs
print("\n--- Reloading canvas to get IDs ---")

node_ids = []

@sio.on("canvas_state_loaded", namespace="/canvas")
def on_canvas_loaded2(data):
    nodes = data.get("nodes", [])
    for n in nodes:
        if n["data"]["label"] in ("Python Test Input", "Python Test LLM"):
            node_ids.append(n)
            print(f"   Found: {n['id']} ({n['data']['label']})")

sio.emit("load_canvas_state", {"environmentId": ENV_ID}, namespace="/canvas")
time.sleep(2)

if len(node_ids) >= 2:
    source_id = next(n["id"] for n in node_ids if n["type"] == "chatInput")
    target_id = next(n["id"] for n in node_ids if n["type"] == "llmCall")

    # Try different edge payloads
    print(f"\n--- Test 1: Minimal edge (source + target only) ---")
    sio.emit("create_edge", {
        "environmentId": ENV_ID,
        "edge": {
            "source": source_id,
            "target": target_id,
        },
    }, namespace="/canvas")
    time.sleep(2)

    print(f"\n--- Test 2: With handles ---")
    sio.emit("create_edge", {
        "environmentId": ENV_ID,
        "edge": {
            "source": source_id,
            "target": target_id,
            "sourceHandle": "right",
            "targetHandle": "left",
        },
    }, namespace="/canvas")
    time.sleep(2)

    print(f"\n--- Test 3: Full edge (all fields) ---")
    sio.emit("create_edge", {
        "environmentId": ENV_ID,
        "edge": {
            "source": source_id,
            "target": target_id,
            "sourceHandle": "right",
            "targetHandle": "left",
            "type": "default",
            "animated": True,
        },
    }, namespace="/canvas")
    time.sleep(2)

    print(f"\n--- Test 4: Flat (no edge wrapper) ---")
    sio.emit("create_edge", {
        "environmentId": ENV_ID,
        "source": source_id,
        "target": target_id,
        "sourceHandle": "right",
        "targetHandle": "left",
        "type": "default",
        "animated": True,
    }, namespace="/canvas")
    time.sleep(2)

else:
    print("❌ Could not find both test nodes")

sio.disconnect()
print("\nDone!")
