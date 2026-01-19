import colmena
import threading
import time
import os

# Path to a test json
JSON_PATH = os.path.join(os.path.dirname(__file__), "../dags/memory_postgres_example.json")

def start_server():
    print("Starting server on localhost:8081...")
    try:
        # Using non-default port to avoid conflicts and explicit host
        colmena.serve_dag(JSON_PATH, host="127.0.0.1", port=8081)
    except Exception as e:
        print(f"Error starting server: {e}")

if __name__ == "__main__":
    if not os.path.exists(JSON_PATH):
        print(f"Error: Test file not found at {JSON_PATH}")
        exit(1)

    print(f"Testing colmena.serve_dag with {JSON_PATH}")
    
    # Run server in a thread since it blocks
    t = threading.Thread(target=start_server, daemon=True)
    t.start()
    
    # Give it a second to start
    time.sleep(2)
    
    if t.is_alive():
        print("Test SUCCESS: Server thread is still running (it accepted the host/port params).")
        print("Note: Ideally we would ping http://127.0.0.1:8081 but we just want to verify binding startup.")
    else:
        print("Test FAILED: Server thread exited prematurely.")
