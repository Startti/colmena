import requests
import json
import sys

# Change port if needed
url = "http://localhost:3000/execute" # NOTE: Requires a DAG with a trigger listening on /execute 

payload = {"message": "Hello mock stream"}

print(f"Connecting to {url}...")
try:
    response = requests.post(url, json=payload, stream=True, headers={"Accept": "text/event-stream"})
    print(f"Status: {response.status_code}")

    if response.status_code != 200:
        print(response.text)
        sys.exit(1)

    for line in response.iter_lines():
        if line:
            decoded = line.decode('utf-8')
            # print(f"RAW: {decoded}")
            if decoded.startswith("data: "):
                try:
                    payload_str = decoded[6:]
                    # Server sends: data: {"type": "data", "value": {...}}
                    # OR data: [DONE] ?
                    # Currently strict implementation sends Vercel format:
                    # data: "token" (for text stream)
                    # OR data: {"type":...} (for complex events)
                    
                    # Wait, how did I implement the SSE formatting in api.rs?
                    # Let's check api.rs implementation from previous turn.
                    # It wraps the DAG event in `data: {"type": "data", "value": ...}` ?
                    # Or `data: JSON`.
                    
                    data = json.loads(payload_str)
                    
                    # Vercel protocol:
                    # if it is a string "...", it is a text part?
                    # No, Vercel protocol specifically says:
                    # "0:text_part" for text parts?
                    # Or Data Stream Protocol?
                    
                    # In api.rs, handling DagExecutionEvent:
                    # Event::default().json_data(...)
                    # This produces `data: {...}`
                    
                    # The content of {...} is the `DagExecutionEvent` json serialization.
                    # e.g. {"llm_token": {"node_id": "...", "token": "..."}}
                    
                    if "llm_token" in data:
                        token_event = data["llm_token"]
                        print(f"TOKEN: {token_event['token']}")
                    elif "node_start" in data:
                        print("Node Start")
                    elif "node_finish" in data:
                        print(f"Node Finish: {data['node_finish']['output']}")
                    elif "graph_finish" in data:
                         print("Graph Finish")
                    else:
                        print(f"Event: {data}")

                except Exception as e:
                    print(f"Error parsing: {decoded} -> {e}")
except Exception as e:
    print(f"Connection failed: {e}")
