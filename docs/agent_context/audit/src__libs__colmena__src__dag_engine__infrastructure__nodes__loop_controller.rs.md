# src/libs/colmena/src/dag_engine/infrastructure/nodes/loop_controller.rs

**Layer:** infrastructure  
**Purpose:** Implements a `LoopControllerNode` that evaluates loop status and controls DAG iteration flow. Determines whether to continue (NEXT_TURN), suspend (SUSPENDED), or break (FINISHED) a loop based on input flags and state.

## Symbols

- `LoopControllerNode` (struct, pub) — Marker struct representing a loop control node in the DAG execution engine
- `Default::default` (impl) — Delegates to `Self::new()`
- `LoopControllerNode::new` (pub fn) — Creates a new instance of LoopControllerNode
- `ExecutableNode::execute` (async trait method) — Main execution logic: reads loop_status and suspend_flag from inputs/config, overrides to SUSPENDED if flag is true, outputs loop metadata with optional question or final_result payload
- `ExecutableNode::description` (trait method) — Returns node description: "Aggregates state and determines whether to continue the loop..."
- `ExecutableNode::default_input` (trait method) — Returns default input field name: "loop_status"
- `ExecutableNode::default_output` (trait method) — Returns default output field name: "output"
- `ExecutableNode::schema` (trait method) — Returns JSON schema documenting inputs (loop_status, suspend_flag, question, all_tasks) and outputs (__colmena_loop_status, question, final_result)

## File-level notes

- No dead code or unfinished work detected
- All imports are used (async_trait, serde_json, standard library error trait)
- Implements ExecutableNode trait completely with all required methods
- Safe by construction: `as_object_mut().unwrap()` calls on lines 56 and 65 are safe because `output_payload` is created as an object at line 48 and never reassigned
- Error handling is appropriate: returns `Result<Value, Box<dyn StdError + Send + Sync>>`
- Logic is straightforward: input/config fallback pattern consistent with other nodes, schema documents the contract clearly
