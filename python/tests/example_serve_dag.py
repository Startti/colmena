"""
Example: Serving a DAG as an HTTP API

This example demonstrates how to use colmena.serve_dag() to start
an HTTP server that exposes webhook endpoints defined in a DAG.

To test:
1. Run this script
2. In another terminal, send a POST request to the webhook path declared in the
   graph ("/power"); the payload's "input" is raised to the 3rd power:
   curl -X POST http://localhost:3000/power -H "Content-Type: application/json" -d '{"input": 10}'
   # => 1000
"""

import colmena
import sys

def main():
    # Path to the DAG file with webhook triggers.
    # power_webhook.json: trigger_webhook "/power" -> exponential^3 -> log. No API keys needed.
    dag_file = "tests/graphs/basic/power_webhook.json"
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
