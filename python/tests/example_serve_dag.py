"""
Example: Serving a DAG as an HTTP API

This example demonstrates how to use colmena.serve_dag() to start
an HTTP server that exposes webhook endpoints defined in a DAG.

To test:
1. Run this script
2. In another terminal, send a POST request:
   curl -X POST http://localhost:3000/hello -H "Content-Type: application/json" -d '{"name": "World"}'
"""

import colmena
import sys

def main():
    # Path to the DAG file with webhook triggers
    dag_file = "tests/basic_webhook.json"
    port = 3000
    
    print(f"🌐 Starting DAG server: {dag_file}")
    print(f"📡 Listening on port: {port}")
    print("-" * 50)
    print("Press Ctrl+C to stop the server")
    print()
    
    try:
        # Start the HTTP server
        # Note: This is a blocking call - the server will run until interrupted
        colmena.serve_dag(dag_file, port=port)
        
    except KeyboardInterrupt:
        print("\n\n✋ Server stopped by user")
        return 0
    
    except colmena.DagException as e:
        print(f"❌ Server failed to start: {e}", file=sys.stderr)
        return 1
    
    except Exception as e:
        print(f"❌ Unexpected error: {e}", file=sys.stderr)
        return 1

if __name__ == "__main__":
    sys.exit(main())
