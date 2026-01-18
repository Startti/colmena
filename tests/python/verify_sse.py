import requests
import json
import time
import sys

# URL of the server
URL = "http://localhost:3000/execute" # Modify as needed based on your running DAG

def main():
    print(f"📡 Connecting to {URL} with SSE...")
    
    headers = {
        "Content-Type": "application/json",
        "Accept": "text/event-stream",
        "x-vercel-ai-ui-message-stream": "v1"
    }
    
    payload = {"base_num": 4} # 4^3 = 64

    try:
        response = requests.post(URL, json=payload, headers=headers, stream=True)
        response.raise_for_status()
        
        print("✅ Connection established. Listening for events...\n")

        for line in response.iter_lines():
            if line:
                decoded_line = line.decode('utf-8')
                print(f"RAW: {decoded_line}")
                
                if decoded_line.startswith("data:"):
                    json_str = decoded_line[5:].strip()
                    try:
                        data = json.loads(json_str)
                        # Pretty print the data/value part
                        if data.get("type") == "data":
                            print(f"🔹 EVENT: {json.dumps(data['value'], indent=2)}")
                    except json.JSONDecodeError:
                        print("⚠️ Could not decode JSON")
                        
    except Exception as e:
        print(f"❌ Error: {e}")
        sys.exit(1)

if __name__ == "__main__":
    time.sleep(2) # Give server time to start if manually run
    main()
