import requests
import json
import sseclient
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
import time
import os

# 1. Mock Weather API Server
class WeatherHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-type', 'application/json')
        self.end_headers()
        self.wfile.write(json.dumps({"weather": "Sunny", "temp": 25}).encode())
    
    def log_message(self, format, *args):
        pass

def run_weather_server():
    server = HTTPServer(('localhost', 8085), WeatherHandler)
    print("🌤️ Weather server running on 8085...")
    server.serve_forever()

threading.Thread(target=run_weather_server, daemon=True).start()

# 2. Execute DAG and Stream
COLMENA_URL = "http://localhost:3000/execute"

# Payload is just the prompt now
payload = {"prompt": "What is the weather in Paris?"}

print(f"\n🚀 Sending request to {COLMENA_URL}...")
response = requests.post(COLMENA_URL, json=payload, stream=True, headers={'Accept': 'text/event-stream'})

client = sseclient.SSEClient(response)

tool_call_seen = False
usage_seen = False
content_received = ""

print("\n📡 Listening for SSE events...")
for event in client.events():

    
    print(f"Event: {event.event}, Data: {event.data}")
    
    # Simple check for success logic to keep test passing
    if event.event == "message":
        try:
            payload = json.loads(event.data)
            if "value" in payload and "event" in payload["value"]:
                evt = payload["value"]["event"]
                if evt == "llm_tool_call":
                    tool_call_seen = True
                elif evt == "llm_usage":
                    usage_seen = True
        except:
            pass

print(f"\n\n✅ Verification Results:")
print(f"Tool Call Seen: {tool_call_seen}")
print(f"Usage Seen: {usage_seen}")

if tool_call_seen and usage_seen:
    print("🎉 SUCCESS: Streamed tool calls and usage!")
    exit(0)
else:
    print("❌ FAILURE: Missing expected events.")
    exit(1)
