"""
Example: Using DAG Engine with LLM

This example shows how to run a DAG that includes LLM nodes
with tool calling capabilities.
"""

import colmena
import json
import sys
import os

def main():
    # Check if API key is set
    if not os.getenv("OPENAI_API_KEY"):
        print("⚠️  Warning: OPENAI_API_KEY not set. LLM calls may fail.")
        print("   Set it in your .env file or environment variables.")
        print()
    
    # Path to the DAG file with LLM and tools
    dag_file = "tests/agent_with_tools.json"
    
    print(f"🤖 Running LLM DAG: {dag_file}")
    print("-" * 50)
    
    try:
        # Execute the DAG with LLM
        result_json = colmena.run_dag(dag_file)
        
        # Parse and display result
        result = json.loads(result_json)
        
        print("✅ LLM DAG execution completed!")
        print("\nFinal Output:")
        print(json.dumps(result, indent=2))
        
        return 0
    
    except colmena.DagException as e:
        print(f"❌ DAG execution failed: {e}", file=sys.stderr)
        return 1
    
    except Exception as e:
        print(f"❌ Unexpected error: {e}", file=sys.stderr)
        return 1

if __name__ == "__main__":
    sys.exit(main())
