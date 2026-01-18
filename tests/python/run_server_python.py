import colmena
import os
import sys

# Path to the webhook DAG
JSON_PATH = os.path.join(os.path.dirname(__file__), "../dags/power_webhook.json")

def main():
    if not os.path.exists(JSON_PATH):
        print(f"Error: DAG file not found at {JSON_PATH}")
        sys.exit(1)

    print(f"🐍 Starting Python Colmena Server...")
    print(f"📂 DAG: {JSON_PATH}")
    print(f"🌍 Host: 127.0.0.1")
    print(f"zk Port: 8085")
    
    try:
        # This function blocks until the server is stopped
        colmena.serve_dag(JSON_PATH, host="127.0.0.1", port=8085)
    except Exception as e:
        print(f"❌ Error running server: {e}")
        sys.exit(1)

if __name__ == "__main__":
    main()
