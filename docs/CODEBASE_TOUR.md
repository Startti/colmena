# Codebase Tour

> **Audience:** developers who want to contribute to the colmena engine — add a node type, fix a bug in the ReAct loop, extend a port, or add an LLM provider. If you just want to use Colmena as a library or author graphs, start with [`00_architecture_overview.md`](./developer_guide/00_architecture_overview.md) and follow the [graph-author path](./ONBOARDING.md#path-3).
>
> **How to read this:** the tour follows the hexagonal layers — domain → application → infrastructure — within each module. Files are listed by responsibility, not alphabetically. Every "key file" reference was verified by opening that file.

---

## Repo layout (10,000-foot view)

```
src/libs/colmena/src/
├── lib.rs                  — crate root; re-exports llm::*, feature-gates bindings
├── main.rs                 — thin shim (unused in library mode)
│
├── dag_engine/             — DAG execution engine (25+ node types, CLI, HTTP server)
│   ├── domain/             — ExecutableNode trait, Graph, NodeEvent, state ports
│   ├── application/        — DagRunUseCase (topological executor), ports, services
│   ├── infrastructure/     — HashMapNodeRegistry, all node impls, SQL infra, SSE
│   ├── engine.rs           — ColmenaEngine (process-wide entry point)
│   ├── api.rs              — run_dag / serve_dag functions (HTTP + CLI)
│   ├── sse_mapper.rs       — converts DagExecutionEvent → SSE wire format
│   └── verbose.rs          — colmena_log! macro + COLMENA_VERBOSE flag
│
├── llm/                    — multi-provider LLM abstraction + ReAct loop
│   ├── domain/             — LlmRepository trait, messages, tools, memory, attachments
│   ├── application/        — AgentService (ReAct), LlmCallUseCase, AttachmentCatalog
│   └── infrastructure/     — OpenAI/Anthropic/Gemini/Mock adapters, persistence, files
│
├── skills/                 — markdown skill packages loaded on-demand
│   ├── domain/             — Skill, SkillRepository trait, SkillConfig
│   └── infrastructure/     — BuiltinSkillRepository, FilesystemSkillRepository, composite
│
├── storage/                — artifact storage abstraction (images, audio)
│   ├── domain/             — OutputStorageRepository trait, StorageError
│   └── infrastructure/     — LocalCache, LocalHttp, HttpCallback adapters
│
├── documents/              — Word/Excel document library
│   ├── domain/             — IR (Intermediate Representation), patch, ports
│   ├── application/        — DocumentRuntime, use cases (apply_patch, create, read…)
│   └── infrastructure/     — renderers (xlsx, docx), storage (local/GCS), validators
│
├── web/                    — web/HTTP toolkit nodes (api_explorer, tavily_client)
│   ├── domain/             — ApiSpecPort, SearchPort, SessionRegistry, errors
│   ├── application/        — ApiSpecUseCase, SearchUseCase, Swagger2→OAS3 converter
│   └── infrastructure/     — OpenAPIAdapter, TavilyAdapter
│
├── shared/                 — cross-cutting helpers (no domain/application layers)
│   └── infrastructure/     — ConfigResolver (API keys), ServiceContainer (DI)
│
├── python_bindings/        — PyO3 bindings (feature = "python")
│   └── mod.rs              — ColmenaLlm, PyLlmStream, run_dag, serve_dag
│
├── node_bindings/          — napi-rs bindings (feature = "node")
│   └── mod.rs              — ColmenaLlm, runDag, servedag (Node.js types)
│
└── attachment_gc/          — standalone GC binary (TTL cleanup)
    └── main.rs             — attachment_gc binary entry point
```

---

## The hexagonal pattern

Colmena applies the **Ports & Adapters (Hexagonal) architecture** to every module. Each module has three layers:

- **Domain** — pure Rust: value objects, error enums, and *trait definitions* (the ports). Zero infrastructure imports. No `reqwest`, no `sqlx`, no `tokio::fs`. This is where you define *what* a capability is.
- **Application** — orchestrates domain types. Calls trait objects (ports) without knowing which adapter is behind them. Use cases live here: `DagRunUseCase`, `AgentService`, `LlmCallUseCase`.
- **Infrastructure** — adapters: concrete structs that implement the domain traits. One file per external dependency (OpenAI, Postgres, local filesystem). These are the only places where `reqwest`, `sqlx`, or `tokio::fs` appear.

The rule: *domain never imports infrastructure*. Infrastructure imports domain. Application imports both, but only through trait objects.

For the full design rationale, see [`docs/dds/ARQUITECTURA_HEXAGONAL_GUIA.md`](./dds/ARQUITECTURA_HEXAGONAL_GUIA.md).

---

## Module-by-module tour

### `dag_engine/` — DAG execution engine

The core of the system. Everything that takes a `graph.json`, resolves its topology, and runs nodes in dependency order lives here.

**Layer breakdown:**

- `dag_engine/domain/`
  - `node.rs` — **`ExecutableNode` trait** (the single most important contract in the codebase). Every runnable node implements it. Also defines `NodeInputs = HashMap<String, Value>`.
  - `initializable_node.rs` — optional `InitializableNode` trait. Nodes that need a one-time setup before their first `execute()` (e.g. `sql_query` bootstrapping pools) implement this. The engine checks for it via downcast.
  - `graph.rs` — `Graph` struct: `nodes: HashMap<id, NodeConfig>`, `edges: Vec<Edge>`, and three optional root-level fields for temporal/geographic context (`timezone`, `location`, `locale`).
  - `tool_configuration.rs` — `ToolConfiguration` and `NodeSchema` types. Three tool-config strategies documented inline: `node_schema` (recommended), `$DYNAMIC` placeholders, deprecated fallback.
  - `toolkit_node.rs` — `ToolkitNode` trait for nodes that expose *multiple* LLM sub-tools via the `__sub_tool` dispatch key.
  - `state.rs` — `DagRunStatus`, `DagStateRepository` port (suspend/resume state), `DagTaskMemoryRepository`.
  - `observer.rs` — `ExecutionObserver` trait. Nodes call `observer.on_event(NodeEvent)` for SSE.
  - `events.rs` — `DagExecutionEvent` enum: `NodeStart`, `NodeFinish`, `LlmToken`, `LlmToolCall`, `LlmUsage`, etc. The wire format for SSE.
  - `sql_ports.rs` — port traits for the SQL node (`SqlConnectionPort`, `StaticValidatorPort`, `LlmCriticPort`).
  - `sql_permissions.rs` — `SqlPermissions` value object (presets + deny-lists + allowed schemas).
  - `secure_value_repository.rs` — port for encrypting/looking up secure values.
  - `error.rs` — `DagError` enum (thiserror).

- `dag_engine/application/`
  - `run_use_case.rs` — **`DagRunUseCase`**: the topological execution loop. Uses a `VecDeque` to dequeue nodes when all their upstream inputs are resolved. Handles suspend/resume. Emits `DagExecutionEvent`s.
  - `ports.rs` — `NodeRegistryPort` (look up a node by type) and `SubGraphExecutorPort` (recursive DAG execution for the `subgraph` node).
  - `secure_value_service.rs` — AES-256-GCM encryption/decryption wrapper for secure values.
  - `sql_execution_service.rs` — orchestrates the SQL validation pipeline (static AST → optional LLM critic → execute).

- `dag_engine/infrastructure/`
  - `registry.rs` — **`HashMapNodeRegistry`**: the concrete `NodeRegistryPort`. Instantiates and registers all 25+ node types in its `new()` constructor. **The first file to look at when adding a new node.**
  - `dag_tool_executor.rs` — **`DagToolExecutor`**: implements `ToolExecutor`. The bridge between an LLM tool call and the actual DAG node execution. Three merge strategies (node_schema → $DYNAMIC → deprecated). Handles secret injection and output hashing.
  - `nodes/` — one file per node type (see table below).
  - `nodes/llm_synthetic_tools/` — synthetic tools that live inside the LLM node (not backed by DAG nodes): `describe_tool`, `load_skill`, `load_attachment`, document tools, lazy tool catalog.
  - `nodes/util/` — shared helpers across node implementations. `attachment_id.rs` builds stable `document_id` values for generated artifacts.
  - `persistence/` — `PostgresDagStateRepository` (suspend/resume), `PostgresSecureValueRepository`.
  - `pool_registry/` — `PgPoolRegistry`: one `PgPool` per unique connection URL, shared across nodes and jobs.
  - `sql_ast.rs` — shared AST helpers built on the `sqlparser` crate. All SQL structural analysis goes through here (no regex heuristics elsewhere).
  - `sql_static_validator.rs` — AST-based allow/deny checks before any query runs.
  - `sql_llm_critic.rs` — optional LLM-based second pass for ambiguous queries.
  - `sql_pool_adapter.rs` — concrete `SqlConnectionPort`: executes queries against a `PgPool`.
  - `sql_port_factory.rs` — builds the full SQL port stack (static validator + optional LLM critic + pool) from graph config.
  - `sql_function_registry.rs` — tracks registered SQL functions for the sandbox schema.
  - `sse_mapper.rs` — stateful `DagExecutionEvent → SSE JSON` mapper. Used by CLI, HTTP handler, and tests.
  - `verbose.rs` — `colmena_log!` macro gated by `COLMENA_VERBOSE=1`.

- `dag_engine/engine.rs` — **`ColmenaEngine`**: process-wide singleton. Owns the `PgPoolRegistry`, state repo, secure value repo, skill repository, node registry, and `DagRunUseCase`. Every consumer (CLI, HTTP, Python bindings, Node bindings) builds one.

- `dag_engine/api.rs` — `run_dag()` / `serve_dag()` free functions. `run_dag` builds an engine, parses the JSON, and runs to completion. `serve_dag` starts an Axum HTTP server.

**Node implementations in `infrastructure/nodes/`:**

| File | Node type(s) | Notes |
|------|-------------|-------|
| `math.rs` | `add`, `subtract`, `multiply`, `divide` | Simplest nodes — good copy template |
| `trigger.rs` | `trigger` | Entry point for graph execution |
| `input.rs` | `input` | Injects the `inject_payload` value |
| `output.rs` | `output` | Passes through final output |
| `debug.rs` | `debug` | Logs inputs and passes through |
| `current_time.rs` | `current_time` | ISO 8601 timestamp node |
| `llm.rs` | `llm_call` | Full LLM node: builds AgentService, wires tools, synthetic tools, skills, lazy loading |
| `http.rs` | `http_request` | HTTP client, multipart, `$attachment:` placeholder |
| `sql.rs` | `sql_query` | SQL execution with validation pipeline, RLS, sandbox, `InitializableNode` |
| `python_node.rs` | `python_script` | Runs Python via PyO3, sandbox AST whitelist |
| `socketio.rs` | `socketio_request` | Socket.IO client, ack + wait-event modes |
| `orchestrator.rs` | `orchestrator` | Multi-phase planner + critic loop |
| `planner.rs` | `planner` | LLM planning step for orchestrator |
| `reactor.rs` | `reactor` | Phase executor for orchestrator |
| `critic.rs` | `critic` | LLM quality evaluator for orchestrator |
| `suspend.rs` | `suspend` | Human-in-the-loop pause with Q/A resume |
| `secure_suspend.rs` | `secure_suspend` | Like suspend but collects secrets (never logs them) |
| `subgraph.rs` | `subgraph` | Recursively runs a nested graph |
| `extraction.rs` | `extraction` | Structured data extraction via LLM |
| `loop_controller.rs` | `loop_controller` | DAG looping primitive |
| `task_memory_writer.rs` | `task_memory_writer` | Writes to `DagTaskMemoryRepository` |
| `image_generation.rs` | `image_generation` | OpenAI / Vertex Imagen 4 image synthesis |
| `image_edit.rs` | `image_edit` | OpenAI image edit (multipart) |
| `tts.rs` | `tts` | TTS via OpenAI / ElevenLabs / Google Gemini |
| `api_explorer.rs` | `api_explorer` | OpenAPI toolkit (5 sub-tools) |
| `tavily_client.rs` | `tavily_client` | Web search toolkit |
| `echo_toolkit.rs` | `echo_toolkit` | Dev/testing toolkit |
| `document_nodes.rs` | `document_create`, `document_edit`, `document_read` | Office document nodes |
| `qa_response_parser.rs` | (shared) | Parses the ID-keyed `Q[id]: A[id]:` resume format |

**Key files to know:**

- `dag_engine/domain/node.rs` — the `ExecutableNode` trait. Start here before touching any node.
- `dag_engine/infrastructure/registry.rs` — where all nodes are registered. Adding a node = adding a line here.
- `dag_engine/infrastructure/dag_tool_executor.rs` — how LLM tool calls become node executions. If tool calling is broken, this is your first stop.
- `dag_engine/domain/tool_configuration.rs` — the three config strategies (`node_schema`, `$DYNAMIC`, deprecated). Heavily documented inline.
- `dag_engine/application/run_use_case.rs` — the topological execution loop. If a node isn't running or the resume logic is broken, it's here.
- `dag_engine/engine.rs` — the `ColmenaEngine` entry point. If the engine isn't wiring up correctly (pools, repos, nodes), look here.

**Common contribution patterns:**

- **Add a new node type** → copy `math.rs` structure → implement `ExecutableNode` → register in `registry.rs`. Full walkthrough: [`12_dag_engine_guide.md`](./developer_guide/12_dag_engine_guide.md).
- **Debug a tool call lifecycle** → `dag_tool_executor.rs::execute_inner` (entry point) → `generate_tool_definition` (schema build) → `tool_configuration.rs::parse_node_schema` (schema parsing). Full flow: [`22_tool_execution_flow.md`](./developer_guide/22_tool_execution_flow.md).
- **Fix suspend/resume** → `run_use_case.rs` (DAG-level) → `suspend.rs` (node-level) → `qa_response_parser.rs` (answer parsing). Spec: `docs/superpowers/specs/2026-05-08-suspend-qa-response-format-design.md`.
- **Add a node init hook** → implement `InitializableNode` alongside `ExecutableNode`. Example: `sql.rs`.

---

### `llm/` — LLM provider abstraction

All LLM communication — synchronous calls, streaming, conversation history, attachments, the ReAct loop — lives in this module.

**Layer breakdown:**

- `llm/domain/`
  - `llm_repository.rs` — **`LlmRepository` trait**: three methods (`call`, `stream`, `health_check`) + `provider_name()`. Every provider adapter implements this. Also defines `LlmStream` type alias.
  - `tools.rs` — `ToolDefinition`, `ToolParameters`, `ParameterProperty`, `ToolCall`, `ToolResult`. The provider-neutral tool schema types that go into `LlmRequest`.
  - `tool_executor.rs` — `ToolExecutor` trait: `execute_tool(name, args) → ToolResult`. `DagToolExecutor` implements this and is injected into `AgentService`.
  - `llm_request.rs` — `LlmRequest` struct: model, messages, tools, config options.
  - `llm_response.rs` — `LlmResponse` struct: text content, `tool_calls`, `usage`.
  - `llm_message.rs` — `LlmMessage`, `MessageRole`, `LlmStreamPart`, `LlmStreamChunk`.
  - `memory.rs` — `ConversationKey` (session_id + agent_session_id + node_id), `ConversationRepository` trait, `Conversation` struct. The three-part key is what enables multi-turn memory across agent sessions.
  - `llm_config.rs` — `LlmConfig` (model, temperature, max_tokens, etc.), `LlmProvider`.
  - `llm_provider.rs` — `ProviderKind` enum (OpenAi, Google, Anthropic, Mock, Scripted).
  - `llm_error.rs` — `LlmError` enum (thiserror).
  - `tts.rs` / `tts_repository.rs` — TTS request/response types and port.
  - `attachments/` — attachment domain types:
    - `attachment_registry.rs` — **`AttachmentRegistry` port**: upsert/find/mark-used/list catalog/find-stale. The central contract for the attachment subsystem.
    - `conversation_attachment.rs` — `ConversationAttachment` value object (document_id, mime, label, description, storage_key, etc.).
    - `stream_resolver.rs` — `AttachmentStreamResolver` port: resolve a `storage_key` to a byte stream.
    - `summary_generator.rs` — `AttachmentSummaryGenerator` port: generate a one-line description for an attachment.
    - `auto_id.rs` — derives a stable `document_id` from attachment fields.
  - `value_objects/` — `LlmRequestId`, `LlmResponseId` newtypes.
  - `file_provider_repository.rs` / `file_cache_repository.rs` / `file_provider_factory_port.rs` — ports for Files API (large file upload cache, per-provider file ID).
  - `signed_url_fetcher.rs` — port for resolving signed GCS/HTTP URLs.

- `llm/application/`
  - `agent_service.rs` — **`AgentService`**: the ReAct loop. Takes `AgentServiceParams`, runs turn-by-turn: `LlmRepository::call` or `stream` → check `tool_calls` → `ToolExecutor::execute_tool` → append result → repeat until no tool calls or max turns. Also exposes `ToolsProvider` type alias and `LoadAttachmentResolver` trait.
  - `llm_call_use_case.rs` — single-shot `LlmCallUseCase::call(request)`.
  - `llm_stream_use_case.rs` — streaming `LlmStreamUseCase::stream(request)`.
  - `llm_health_check_use_case.rs` — `LlmHealthCheckUseCase::check()`.
  - `attachment_catalog.rs` — builds the attachment catalog system message block injected into every `llm_call` that has attachments enabled.

- `llm/infrastructure/`
  - `openai_adapter.rs` — `OpenAiAdapter`: translates `LlmRequest → OpenAI API JSON → LlmResponse`. Handles streaming SSE parsing.
  - `anthropic_adapter.rs` — `AnthropicAdapter`: Anthropic Messages API adapter.
  - `gemini_adapter.rs` — `GeminiAdapter`: Google Gemini API adapter.
  - `mock_adapter.rs` — `MockAdapter`: returns canned responses for unit tests. `mockall::automock` is declared on `LlmRepository`.
  - `scripted_adapter.rs` — `ScriptedAdapter`: replays a scripted sequence of responses. Used in deterministic integration tests.
  - `llm_provider_factory.rs` — `LlmProviderFactory::create(kind)`: maps `ProviderKind → Arc<dyn LlmRepository>`. Includes a process-global test override via `OVERRIDE` static.
  - `openai_tts_adapter.rs` / `elevenlabs_tts_adapter.rs` / `google_tts_adapter.rs` — TTS provider adapters.
  - `tts_provider_factory.rs` — factory for TTS adapters.
  - `files/` — Files API adapters: `openai_files_api.rs`, `anthropic_files_api.rs`, `gemini_files_api.rs`, `postgres_file_cache.rs`, `signed_url_downloader.rs`, `file_provider_factory.rs`.
  - `attachments/stream_resolver_impl.rs` — concrete `AttachmentStreamResolver`: checks local `LocalCacheStorageAdapter` first, then calls `HttpCallbackStorageAdapter::sign_get` for cross-process reads.
  - `attachment_summary/` — `LlmSummaryGenerator` (calls cheap-tier LLM), `TextExtractor`, `ByteAcquisition`, `CheapTier` selector.
  - `persistence/` — conversation history adapters:
    - `in_memory_conversation_repository.rs` — for tests / stateless runs.
    - `sqlite_conversation_repository.rs` — SQLite-backed (local dev).
    - `postgres_conversation_repository.rs` — Postgres-backed (production).
    - `postgres_attachment_registry.rs` / `sqlite_attachment_registry.rs` — attachment registry adapters.
    - `repository_factory.rs` — `ConversationRepositoryFactory`: selects SQLite vs Postgres based on env.

**Key files to know:**

- `llm/domain/llm_repository.rs` — the abstract LLM interface. Three methods. Every provider implements this.
- `llm/application/agent_service.rs` — the ReAct loop. If you need to understand how LLM calls and tool calls interleave, start here.
- `llm/infrastructure/openai_adapter.rs` (and siblings) — provider-specific request/response translation.
- `llm/domain/attachments/attachment_registry.rs` — the central attachment port. If attachment behavior is wrong, check the contract here first.
- `llm/infrastructure/llm_provider_factory.rs` — process-global factory. Includes the test override mechanism.

**Common contribution patterns:**

- **Add a new LLM provider** → implement `LlmRepository` in a new `<name>_adapter.rs` → add to `LlmProviderFactory::create` match → add `ProviderKind` variant. Full guide: [`04_adding_providers.md`](./developer_guide/04_adding_providers.md).
- **Modify ReAct loop behavior** (max turns, tool call handling, streaming) → `agent_service.rs`.
- **Add a new attachment source** → `llm/domain/attachments/` (add `AttachmentSource` variant if needed) → `attachments/stream_resolver_impl.rs`.

---

### `skills/` — Skills feature

Markdown knowledge packages compiled into the binary or loaded from the filesystem at runtime.

**Layer breakdown:**

- `skills/domain/`
  - `skill.rs` — `Skill` struct: `name`, `description`, `body` (markdown without frontmatter), `references`, `source`, optional `node_type` (marks it as a layer-1 node-type guide).
  - `skill_repository.rs` — **`SkillRepository` trait**: `list_available()`, `find_by_node_type()`, `load_by_name()`, `load_reference()`. The composite wraps two implementations of this.
  - `skill_config.rs` — `SkillsConfig`: the graph-level skills config (paths, allowed dirs, eager list, etc.).
  - `skill_error.rs` — `SkillError` enum.

- `skills/infrastructure/`
  - `builtin_skill_repository.rs` — `BuiltinSkillRepository`: uses `include_dir!("$CARGO_MANIFEST_DIR/skills")` to compile all `SKILL.md` files into the binary. Scans for single-file and folder-of-skills layouts.
  - `filesystem_skill_repository.rs` — `FilesystemSkillRepository`: loads skills from operator-declared paths at runtime. Applies allowed-dirs whitelist for security.
  - `composite_skill_repository.rs` — `CompositeSkillRepository`: merges builtin + filesystem sources. Deduplicates by name (builtin wins).
  - `frontmatter_parser.rs` — parses the YAML frontmatter block (`---`) from a `SKILL.md` to extract `name`, `description`, `node_type`, etc.

**Key files to know:**

- `skills/domain/skill_repository.rs` — the port. If skill loading is broken, check the trait contract first.
- `skills/infrastructure/builtin_skill_repository.rs` — where `include_dir!` compiles skills into the binary. Built-in skill files live at `src/libs/colmena/skills/`.
- `skills/infrastructure/frontmatter_parser.rs` — parses `SKILL.md` frontmatter. If a skill isn't loading, check frontmatter format here.

**Common contribution patterns:**

- **Add a built-in skill** → create `src/libs/colmena/skills/<name>/SKILL.md` with valid frontmatter → no code change needed (picked up at compile time). Full guide: [`24_skills.md`](./developer_guide/24_skills.md).
- **Add a node-type guide** → set `node_type: <your_node_type>` in the skill frontmatter → the engine auto-folds it into the tool description for that node type.

---

### `storage/` — Output storage abstraction

Artifact storage for generated media (images, audio). Designed so the engine has no GCS credentials.

**Layer breakdown:**

- `storage/domain/`
  - `output_storage_repository.rs` — **`OutputStorageRepository` trait**: `store(StoreRequest) → StoredOutput`, `read(storage_key) → Vec<u8>`, `read_stream(storage_key) → Stream`. `StoreRequest` carries bytes, mime type, filename, and session context. `StoredOutput` carries `storage_key` (stable handle) and `read_url` (short-lived URL for the LLM).
  - `storage_error.rs` — `StorageError` enum.

- `storage/infrastructure/`
  - `local_cache_adapter.rs` — `LocalCacheStorageAdapter`: in-process `DashMap` keyed by `storage_key`. No filesystem, no network. Used for CLI runs and unit tests. `read_url = storage_key` (intentionally short to avoid TPM saturation).
  - `local_http_adapter.rs` — `LocalHttpStorageAdapter`: spins up an Axum server on `127.0.0.1` to serve blobs via HTTP. Used in dev (`COLMENA_LOCAL=true`) for URL symmetry with production.
  - `http_callback_adapter.rs` — `HttpCallbackStorageAdapter`: requests a signed PUT URL from the host app, uploads bytes, returns the resulting `read_url`. Used in production (the ADP worker). Never holds GCS credentials itself.

**Key files to know:**

- `storage/domain/output_storage_repository.rs` — the port. Three methods, two types. Read this before touching any storage adapter.
- `storage/infrastructure/http_callback_adapter.rs` — production behavior. Also handles the `delete` (Plan C GC) and `sign-get` (cross-process read) endpoints.

**Common contribution patterns:**

- **Add a new storage backend** → implement `OutputStorageRepository` in a new `<name>_adapter.rs` → wire it in `dag_engine/engine.rs` based on env config.
- Full reference: [`32_multimedia_generation.md`](./developer_guide/32_multimedia_generation.md).

---

### `documents/` — Office document library

Word/Excel document generation and versioned editing. The LLM interacts with documents via 7 synthetic tools injected into the `llm_call` node.

**Layer breakdown:**

- `documents/domain/`
  - `ir/` — **Intermediate Representation**: `word.rs` (`WordIR`, `Block`, `Run`, `TableRow`...) and `excel.rs` (`ExcelIR`, `Workbook`, `Sheet`, `Cell`, `NamedTable`...). JSON-serializable IR is the source of truth — not the rendered file.
  - `patch.rs` — `Patch` enum: the atomic operations the LLM applies to the IR (`InsertRow`, `SetCell`, `AppendBlock`, etc.).
  - `ports.rs` — `ArtifactStore` trait (CRUD for versioned artifacts), `IRRenderer` trait (IR → binary), `IRValidator` trait, `IdGenerator` trait.
  - `artifact.rs` — `ArtifactMeta`, `VersionData`, `ArtifactSummary`, `PatchApplied`.
  - `ids.rs` — `ArtifactId`, `VersionId`, `SessionId`, `ArtifactKind` (Word/Excel).
  - `error.rs` — `DocumentError`, `IndexError`, `RenderError`, `StorageError`.

- `documents/application/`
  - `runtime.rs` — **`DocumentRuntime`**: the facade injected into the `llm_call` node. Holds the store + renderer + validator.
  - `apply_patch.rs` — applies a `Patch` to the current IR version, validates, renders, writes new version.
  - `create_document.rs` — creates a blank artifact with metadata.
  - `read_document.rs` — reads the current rendered binary.
  - `get_head.rs` / `list_versions.rs` / `rollback.rs` — version management use cases.
  - `apply_excel_ops.rs` / `apply_word_ops.rs` — domain-level op appliers for each format.

- `documents/infrastructure/`
  - `render/excel_renderer.rs` — renders `ExcelIR → xlsx` via `rust_xlsxwriter`.
  - `render/word_renderer.rs` — renders `WordIR → docx` via `docx-rs`.
  - `storage/local_fs_store.rs` — stores versioned IR JSON + rendered binary on the local filesystem, scoped by session.
  - `storage/gcs_store.rs` — GCS-backed store.
  - `validation/excel_validator.rs` / `word_validator.rs` — structural correctness checks on the IR before rendering.
  - `ids.rs` — ID generation (UUIDs).

**Key files to know:**

- `documents/domain/ir/` — the IR types. If you want to support a new document element, start here.
- `documents/domain/patch.rs` — the patch operations the LLM can apply.
- `documents/application/runtime.rs` — the `DocumentRuntime` facade injected into `llm_call`.

**Common contribution patterns:**

- **Add a new patch operation** → add variant to `Patch` in `domain/patch.rs` → implement in `apply_excel_ops.rs` or `apply_word_ops.rs` → update IR if new data needed. Full guide: [`27_documents_library.md`](./developer_guide/27_documents_library.md).

---

### `web/` — Web/HTTP toolkits

The domain and application logic behind the `api_explorer` and `tavily_client` toolkit nodes. Note: the node *implementations* live in `dag_engine/infrastructure/nodes/api_explorer.rs` and `tavily_client.rs`; this module provides the domain ports and use cases they depend on.

**Layer breakdown:**

- `web/domain/`
  - `api_spec_port.rs` — **`ApiSpecPort` trait**: `fetch_and_parse(url, etag, last_modified)`. Returns `SpecFetchResult` (modified spec or `NotModified`). Hides `oas3`/`serde_yaml` from domain.
  - `search_port.rs` — **`SearchPort` trait**: `search(SearchRequest) → Vec<SearchResult>`. Provider-neutral interface used by `tavily_client`.
  - `session.rs` — `SessionKey`, `SessionRegistry` trait (per-conversation spec cache).
  - `errors.rs` — `WebDomainError` enum.

- `web/application/`
  - `api_spec_use_case.rs` — `ApiSpecUseCase`: orchestrates fetch / ETag cache / fuzzy search / `build_http_request` for `api_explorer`. Holds an LRU cache of parsed specs per conversation.
  - `search_use_case.rs` — `SearchUseCase`: wraps `SearchPort`, applies result limits.
  - `swagger2_to_oas3.rs` — in-memory Swagger 2.0 → OpenAPI 3.0 converter (no network call).
  - `url_normalizer.rs` — normalizes spec URLs for cache keying.

- `web/infrastructure/`
  - `openapi_adapter.rs` — `OpenAPIAdapter`: implements `ApiSpecPort`. Fetches and parses YAML/JSON specs, does conditional GET.
  - `tavily_adapter.rs` — `TavilyAdapter`: implements `SearchPort` by calling the Tavily API.

**Key files to know:**

- `web/domain/api_spec_port.rs` — the contract for `api_explorer`. If the spec loading is wrong, start here.
- `web/domain/search_port.rs` — the contract for `tavily_client` (and future search providers).

**Common contribution patterns:**

- **Add a new search provider** (e.g. Exa, SearxNG) → implement `SearchPort` → wire in `api_explorer.rs` or a new toolkit node. Guide: [`25_web_nodes.md`](./developer_guide/25_web_nodes.md).

---

### `shared/` — Cross-cutting helpers

No domain/application layers. Only `infrastructure/`.

- `shared/infrastructure/config_resolver.rs` — `ConfigResolver`: resolves API keys from explicit config or environment variables (maps `ProviderKind → env var name`).
- `shared/infrastructure/service_container.rs` — `ServiceContainer`: wires `LlmCallUseCase`, `LlmStreamUseCase`, `LlmHealthCheckUseCase` for direct use from bindings. Not used by `ColmenaEngine` (which builds its own wiring).

---

### `python_bindings/` — PyO3 bindings

Feature-gated (`#[cfg(feature = "python")]`). Exposes Colmena to Python as the `colmena` package (compiled with `maturin develop`).

- `python_bindings/mod.rs` — the entire Python surface:
  - `PyLlmStream` (`#[pyclass]`) — wraps `LlmStream` as an async Python iterator.
  - `ColmenaLlm` (`#[pyclass]`) — `call()`, `stream()`, `health_check()`.
  - `run_dag()`, `serve_dag()`, `validate_graph()` — free functions.
  - `LlmException` — custom Python exception bridging `LlmError`.

The binding uses `pyo3_asyncio_0_21::tokio::future_into_py` to bridge Rust futures → Python `asyncio`.

Guide: [`docs/examples/python_usage.md`](./examples/python_usage.md).

---

### `node_bindings/` — napi-rs Node.js bindings

Feature-gated (`#[cfg(feature = "node")]`). Exposes Colmena to Node.js via `napi-rs` (compiled with `npm run build --features node`).

- `node_bindings/mod.rs` — the entire Node.js surface:
  - `NodeLlmConfigOptions` (`#[napi(object)]`) — config struct.
  - `NodeLlmMessage` (`#[napi(object)]`) — message type.
  - `ColmenaLlm` (`#[napi]`) — `call()`, `healthCheck()`.
  - `runDag()`, `serveDag()` — async graph runners.

Fewer features are exposed than through the Python bindings (no streaming iterator, no `validate_graph`).

---

### `attachment_gc/` — Standalone GC binary

Not a module in `lib.rs` — a separate Cargo binary (`attachment_gc/main.rs`).

- `attachment_gc/main.rs` — TTL cleanup for `conversation_attachments` rows and their backing blobs. Reads `COLMENA_ATTACHMENT_TTL_DAYS` (default 7), `COLMENA_ATTACHMENT_GC_BATCH_SIZE` (default 100). Pipeline: `find_stale_attachments` in batches → `storage.delete(storage_key)` first (if fails, preserve row for retry) → `registry.delete(row)`. Designed for Cloud Scheduler → Cloud Run Job.

Guide: [`36_attachment_gc.md`](./developer_guide/36_attachment_gc.md).

---

## Common navigation patterns

### "I need to add a new node type"

1. `dag_engine/domain/node.rs` — read `ExecutableNode` trait signature.
2. `dag_engine/infrastructure/nodes/math.rs` — copy this as the minimal template.
3. `dag_engine/infrastructure/nodes/mod.rs` — add `pub mod <your_node>;`.
4. `dag_engine/infrastructure/registry.rs` — register your node in `HashMapNodeRegistry::new()`.
5. `docs/developer_guide/12_dag_engine_guide.md` — full walkthrough with examples.
6. `docs/node_configurations.json` — add canonical config schema for the new node type.

### "I need to debug a tool call lifecycle"

1. `dag_engine/infrastructure/dag_tool_executor.rs::execute_inner` — the entry point when the LLM fires a tool call.
2. `dag_engine/infrastructure/dag_tool_executor.rs::generate_tool_definition` — where the schema shown to the LLM is built.
3. `dag_engine/domain/tool_configuration.rs::parse_node_schema` — schema parsing from the graph JSON.
4. `docs/developer_guide/22_tool_execution_flow.md` — end-to-end flow with a detailed diagram.

### "I need to understand how a graph JSON becomes execution"

1. `dag_engine/api.rs::run_dag` — where graph JSON is loaded and the engine is created.
2. `dag_engine/domain/graph.rs` — `Graph` struct (deserialized from JSON) + `Graph::validate()`.
3. `dag_engine/engine.rs` — `ColmenaEngine::execute_stream()` — hands off to the use case.
4. `dag_engine/application/run_use_case.rs::execute_stream` — the topological execution loop.
5. `dag_engine/infrastructure/nodes/<node>.rs` — the per-node `execute()` implementation.

### "I need to modify the LLM ReAct loop"

1. `llm/application/agent_service.rs` — the loop. Look for the `run` method.
2. `llm/domain/llm_repository.rs` — the trait the loop calls to talk to the provider.
3. `llm/infrastructure/<provider>_adapter.rs` — actual HTTP call to the provider.

### "I need to add an LLM provider"

1. `llm/domain/llm_provider.rs` — add a variant to `ProviderKind`.
2. `llm/infrastructure/<name>_adapter.rs` — implement `LlmRepository`.
3. `llm/infrastructure/llm_provider_factory.rs` — add a match arm.
4. `shared/infrastructure/config_resolver.rs` — add the env var name.
5. `docs/developer_guide/04_adding_providers.md` — full step-by-step guide.

---

## What is NOT in `src/libs/colmena/src/`

- `python/` — Python test scripts and examples (not binding source).
- `tests/` — Rust integration tests and JSON test graphs.
- `docs/` — all guides, specs, plans, node config schemas.
- `src/.claude/` — AI workflow settings.
- `node-app/` — Node.js consumer demo app.
- `src/libs/colmena/skills/` — built-in SKILL.md files compiled into the binary (not `.rs`).

---

## See also

- [`developer_guide/00_architecture_overview.md`](./developer_guide/00_architecture_overview.md) — 30,000-foot view, master diagram, execution lifecycle
- [`developer_guide/01_architecture.md`](./developer_guide/01_architecture.md) — hexagonal pattern deep-dive
- [`developer_guide/12_dag_engine_guide.md`](./developer_guide/12_dag_engine_guide.md) — DAG engine: adding nodes, topological order, config reference
- [`developer_guide/14_llm_deep_dive.md`](./developer_guide/14_llm_deep_dive.md) — LLM node parameters, streaming, memory
- [`developer_guide/22_tool_execution_flow.md`](./developer_guide/22_tool_execution_flow.md) — tool call lifecycle end-to-end
- [`ONBOARDING.md`](./ONBOARDING.md) — reading paths by contributor role
- [`DEVELOPER_GUIDE.md`](./DEVELOPER_GUIDE.md) — index of all 37 developer guides
