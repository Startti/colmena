# src/libs/colmena/src/dag_engine/infrastructure/registry.rs

**Layer:** infrastructure  **Purpose:** Concrete registry adapter (HashMapNodeRegistry) that implements NodeRegistryPort, initializing and managing all 37+ available ExecutableNode and ToolkitNode types (math, debug, LLM, SQL, media generation, orchestrators, etc.) with dependency injection for storage, secure values, and attachment resolution.

## Symbols

### Main Types
- `HashMapNodeRegistry` (struct) — HashMap-backed concrete implementation of NodeRegistryPort; holds nodes and toolkit_nodes maps, plus optional subgraph_node and foreach_node references for dependency injection

### Constructors & Initialization
- `HashMapNodeRegistry::new()` (fn, pub) — Convenience constructor that calls new_with_secure_values with all optional dependencies set to None
- `HashMapNodeRegistry::new_with_secure_values()` (fn, pub) — Full constructor using Arc::new_cyclic that registers 37+ nodes, conditionally wires secure values (secure_suspend), storage (media nodes), and attachment resolution (multipart HTTP); takes repository_factory, sql_port_factory, task_memory_repo, secure_value_service, storage, and attachment_registry
- `HashMapNodeRegistry::set_subgraph_executor()` (fn, pub) — Injects SubGraphExecutorPort into subgraph_node's OnceLock; wires both subgraph and router nodes to use the same executor

### Port Implementation
- `HashMapNodeRegistry::get_node()` (fn) — NodeRegistryPort method; returns Arc-cloned ExecutableNode by node_type string or None
- `HashMapNodeRegistry::get_all_nodes()` (fn) — NodeRegistryPort method; returns shallow clone of entire nodes HashMap
- `HashMapNodeRegistry::get_toolkit_node()` (fn) — NodeRegistryPort method; returns Arc-cloned ToolkitNode by node_type string or None

### Test Helpers & Mocks
- `HashMapNodeRegistry::set_foreach_registry()` (fn, pub) — Injects NodeRegistryPort into for_each node's OnceLock; mirrors subgraph_executor pattern for list iteration dispatch
- `HashMapNodeRegistry::register_toolkit_node()` (fn, pub) — Test-only helper to register a ToolkitNode post-construction; silently no-ops if Arc is already shared
- `registry_tavily_tests::StubTaskMemory` (struct) — Mock DagTaskMemoryRepository used in tests; all async methods return Ok(()) or empty collections
- `registry_tavily_tests::build_registry()` (fn) — Creates a test registry with pool, repository factory, sql factory, and StubTaskMemory
- `registry_secure_suspend_tests::NoopRepo` (struct) — Mock SecureValueRepository returning None/false/0 for all operations
- `registry_secure_suspend_tests::build_registry_with_secure_values()` (fn) — Test registry builder that wires SecureValueService
- `media_tools_injection_tests::build_registry_with_storage()` (fn) — Test registry builder that wires LocalCacheStorageAdapter
- `media_tools_injection_tests::media_tool_configs()` (fn) — Constructs HashMap<String, ToolConfiguration> from hardcoded JSON matching multimedia_agent.json shape

### Test Modules
- `registry_tavily_tests` (mod) — Tests for tavily_client and output_parser registration; ensures both ExecutableNode and ToolkitNode trait impls are present
- `for_each_registration_tests` (mod) — Tests that for_each node is registered and set_foreach_registry correctly populates OnceLock
- `registry_api_explorer_tests` (mod) — Tests api_explorer as both ExecutableNode and ToolkitNode with 5 sub-tools
- `registry_secure_suspend_tests` (mod) — Tests conditional registration of secure_suspend (present only when SecureValueService wired; absent otherwise)
- `media_tools_injection_tests` (mod) — Comprehensive tests for media nodes (image_generation, image_edit, tts) availability, tool_configuration parsing, and LLM-visible parameter filtering

## File-level notes

- **Architecture**: Implements the Adapter pattern (ports & adapters). This is the infrastructure-layer concrete implementation of `NodeRegistryPort` from the application layer; the registry is injected as an Arc into LlmNode, OrchestratorNode, and ForEachNode via weak references to support lazy graph execution and HITL suspension/resume.

- **Dependency Injection**: Uses three patterns:
  1. **Constructor-time wiring** (new_with_secure_values): nodes that need repositories, factories, or secure services are instantiated with them at registry construction.
  2. **OnceLock-based lazy injection** (set_subgraph_executor, set_foreach_registry): subgraph and for_each nodes are constructed with empty OnceLocks, then populated after the registry is created (supports the cyclic Arc<Self> pattern).
  3. **Optional dependencies**: storage and attachment_registry gate media node registration; secure_value_service gates secure_suspend registration.

- **Node Count**: 37+ node types registered, including:
  - Debug/utility: mock_input, log, output
  - Math: add, subtract, multiply, divide, exponential
  - Timing: current_time
  - Triggers: trigger_webhook, input
  - Network: http_request, socketio_request
  - LLM: llm_call
  - Code: python_script
  - SQL: sql_query
  - Orchestration: orchestrator, planner, critic, reactor, router, subgraph, for_each, loop_controller
  - Search: tavily_client, api_explorer
  - Documents: document_create, document_edit, document_read
  - Media: image_generation, image_edit, tts (conditional on storage)
  - Utilities: suspend, secure_suspend (conditional on secure service), information_extraction, output_parser, task_memory_writer

- **Attachment Resolver**: Lines 66–83 build a composite `AttachmentStreamResolverImpl` only when both attachment_registry and storage are present, enabling `$attachment:<document_id>` placeholder resolution in HTTP multipart and LLM fallback paths. When either is absent, http_request falls back to direct storage_key lookup.

- **Media Node Conditionals**: Lines 284–320 register image_generation, tts, and image_edit only if storage adapter is provided. Each follows an identical pattern: storage check → optional secure values → optional attachment registry. This is correct but repetitive.

- **Licensing & Comments**: Comments are in Spanish (project policy for docs/) with English inline clarifications for complex patterns (Arc::new_cyclic, OnceLock, weak references). Spanish labels (e.g., "Registrar Nodos Matemáticos") are used throughout for consistency.

- **Test Coverage**: Four test modules (11 tests total) verify node registration, conditional logic (secure_suspend, media nodes), toolkit dispatch, and ToolConfiguration parsing. All pass; tests use helper builders to avoid repetition.

- **Immutability After Init**: Once `new_with_secure_values` completes and Arc is shared, the registry's nodes map is read-only. Later callers inject dependencies via `set_subgraph_executor()` and `set_foreach_registry()` into OnceLocks, which is safe under Arc sharing.
