"""
Example: Running a DAG from Python

This example demonstrates how to use colmena.run_dag() to execute
a DAG configuration file and get the results.
"""

import colmena
import json
import sys
from pathlib import Path

def main():
    # Path to the DAG file (relative to project root)
    dag_file = "tests/power.json"
    
    print(f"🚀 Running DAG: {dag_file}")
    print("-" * 50)
    
    try:
        # Execute the DAG
        result_json = colmena.run_dag(dag_file)
        
        # Parse the result
        result = json.loads(result_json)
        
        print("✅ DAG execution completed successfully!")
        print("\nResult:")
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
