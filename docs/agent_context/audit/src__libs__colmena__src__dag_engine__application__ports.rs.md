# src/libs/colmena/src/dag_engine/application/ports.rs

**Layer:** application  **Purpose:** Defines two port traits (`NodeRegistryPort` and `SubGraphExecutorPort`) that decouple the application layer from infrastructure implementations; prevents circular dependencies between node execution and the DAG orchestrator.

## Symbols

- `NodeRegistryPort` (trait, pub) — Port trait that provides access to node implementations by type name; infrastructure implementations retrieve concrete ExecutableNode instances.
- `NodeRegistryPort::get_node` (fn, pub) — Returns an ExecutableNode arc for a given node_type string, or None if not found.
- `NodeRegistryPort::get_all_nodes` (fn, pub) — Returns a HashMap of all registered node type strings to their ExecutableNode arc implementations.
- `NodeRegistryPort::get_toolkit_node` (fn, pub) — Returns a ToolkitNode arc if registered as one; defaults to None for backward compatibility.
- `SubGraphExecutorPort` (trait, pub, async) — Async port trait that defines execution logic for SubGraph nodes to run their internal child graphs; prevents circular dependency between Node layer and DagRunUseCase.
- `SubGraphExecutorPort::run_subgraph` (fn, async, pub) — Executes a subgraph from scratch with session, config, optional observer, and optional parent/agent context; returns result value or DagError.
- `SubGraphExecutorPort::resume_subgraph` (fn, async, pub) — Resumes a previously suspended subgraph with an HITL answer; requires session, answer, optional observer and agent context.
- `SubGraphExecutorPort::find_child_session_id_for_resume` (fn, async, pub) — Finds the SUSPENDED child run by matching parent_session_id and node path; single-leaf design with path arg reserved for future multi-child parallelism.

## File-level notes

- **Design:** Clean hexagonal architecture—two focused port traits with no implementation leakage; application layer defines what it needs, infrastructure implements.
- **Backward compatibility:** `get_toolkit_node` default impl (returns None) allows existing NodeRegistryPort implementations to remain unchanged when ToolkitNode support was added.
- **Language mix:** Doc comments are Spanish; inline explanation within `run_subgraph` comment mixes Spanish+English (expected per project conventions—docs are Spanish, code comments are English).
- **Async trait:** Proper `async_trait` macro usage on `SubGraphExecutorPort`; all async methods correctly declared.
- **No issues identified:** No dead code, TODOs, stubs, or missing error handling at boundaries.
