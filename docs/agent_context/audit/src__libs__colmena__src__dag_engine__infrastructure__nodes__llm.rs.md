# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs

**Layer:** infrastructure  
**Purpose:** Implements the LLM node type for Colmena DAG execution engine, handling multi-provider LLM calls (OpenAI, Google, Anthropic, Mock), tool calling orchestration, file/attachment resolution with cross-provider upload, conversation history persistence, skill loading, and structured output parsing.

## Symbols

### Constants
- `LLM_DEFAULT_SYSTEM` (const) — Default system message included from `llm_default_system.md` to ground model behavior
- `FILE_DATA_LIMIT_BYTES` (const) — 30 MB size threshold for inline file data vs. rejection
- `GDOCS_SURGICAL_EDIT_TOOL_NAMES` (const array) — List of gdocs write tools that auto-trigger skill enrollment

### Public Functions
- `filter_enabled_tools()` — Filters tool catalog by `enabled_tools` config, supporting wildcard, toolkit aliases, and exclusions
- `dedup_tools_by_name()` — Removes duplicate tool definitions by name, keeping first occurrence (config-wins over built-in)
- `resolve_synthetic_enabled_tools()` — Resolves enabled tools for synthetic tool blocks (gsheets, gdocs) with same filter semantics
- `parse_file_entries()` — Parses JSON file array entries into FileData, handling base64 data, signed URLs, size validation

### Private Functions
- `find_pending_tool_call()` — Finds first unresolved tool call in message history for resume path
- `generate_one_summary()` — Generates LLM or structured summary for attachment (tabular auto-summary, text extraction, image bytes)
- `list_skills_in_path()` — Async directory scan for skill directories containing SKILL.md
- `list_skill_dirs_sync()` — Synchronous directory scan for skill subdirectories under a root path
- `ws_url_to_http_base()` — Converts WebSocket URL to HTTP base for skill/context endpoints
- `sqlite_url_for_node()` — Extracts SQLite connection URL from node config
- `format_temporal_context_block()` — Formats temporal context (ISO 8601 timestamp, location, locale, timezone) for LLM system message
- `should_register_attachment_row()` — Gate: whether to register text-only attachment row (requires storage_key fallback when no provider_file_id)
- `persist_attachment_bytes()` — Persists attachment bytes (inline or fetched from signed URL) to OutputStorageRepository, returns storage key
- `build_initial_user_message()` — Creates first user message (Plan B: no file content inline; catalog block tells model what's available)

### Structs
- `SkillLoadedLogEntry` (private) — Diagnostic struct: skill name, reference (path), source string
- `SummaryTarget` (private) — Helper struct: document_id, attachment source, mime type, filename, optional inline bytes
- `AttachmentResolverImpl` (private) — Implements LoadAttachmentResolver for attachment lookup, cross-provider lazy upload, stale refresh, text-from-storage fallback
- `LlmNode` (public) — Main node struct: repository factory, registry weak ref, task memory repo, secure value service, output storage adapter

### Struct Implementations
- `impl LlmNode` — Constructor and builder methods:
  - `resolve_prompt_or_task()` — Resolves prompt with blank-aware fallback to task (handles null and empty object)
  - `resolve_stream_enabled()` — Resolves streaming flag (default true, inputs > config precedence)
  - `new()` — Constructor
  - `with_secure_values()` — Builder: attach SecureValueService
  - `with_storage()` — Builder: attach OutputStorageRepository
  - `resolve_env_var()` — Resolves `${VAR}` format env placeholders
  - `parse_allowed_dirs_env()` — Parses COLMENA_SKILLS_ALLOWED_DIRS into PathBuf list
  - `resolve_skill_names()` — Union of explicit `skills` array + directory scan results from `skills_path(s)`
  - `build_skill_repository_from_config()` — Assembles BuiltinSkillRepository + FilesystemSkillRepository with auto-enrollment of gdocs-surgical-edits, gsheets-presentable-output, gsheets-editing skills
  - `resolve_context_vars()` — Replaces `${var}` placeholders with values from inputs
  - `resolve_context_in_node_schema()` — Recursively resolves `${context.var}` in NodeSchema fixed values
  - `resolve_template_vars()` — Replaces `{{var}}` Handlebars-style placeholders with input values
  - `agent_has_gdocs_edit_tools()` — Checks if LLM will expose surgical gdocs write tools (for skill auto-enrollment)
  - `agent_has_gsheets_format_tool()` — Checks if LLM has gsheets_format_range tool (for gsheets-presentable-output skill)
  - `agent_has_gsheets_write_tools()` — Checks if LLM can write to sheets (gsheets_run_python, set_cell, set_range, or alias)
  - `agent_has_gsheets_read_tools()` — Checks if LLM has read-only gsheets tools

- `impl ExecutableNode for LlmNode` — Main async execute method (≈2500 lines, core LLM orchestration):
  - Resolves provider/model/api_key/system_message from inputs > config
  - Loads conversation history from persistence (if connection_url configured)
  - Resolves file entries (base64, signed URLs) with provider upload and caching
  - Auto-registers uploads in AttachmentRegistry with cross-provider lazy upload fallback
  - Assembles temporal context block (timestamp, location, timezone, locale)
  - Generates attachment summaries (tabular CSV/XLSX → structured, PDF/text → LLM/extraction)
  - Builds skill repository and system message with attachment catalog prelude
  - Invokes AgentService for multi-turn LLM loop with tool calling
  - Handles resume path (pending tool call + answer injection)
  - Persists final conversation state
  - Returns structured output or Value::Null on skip
  - `description()` — Returns tool description (supports `{{var}}` template resolution, layer-2 skills injection)
  - `default_input()` — Returns default input key ("prompt")
  - `default_output()` — Returns default output key ("output")
  - `schema()` — Returns JSON schema for LLM node configuration

- `impl LoadAttachmentResolver for AttachmentResolverImpl` — Async resolve method:
  - Looks up attachment by (agent_session_id, document_id) for current provider
  - Falls back to Generated provider row → cross-provider lazy upload if target provider row missing
  - Handles stale provider_file_id recovery (re-uploads from recoverable sources after 24h)
  - Serves text attachments from OutputStorageRepository (no provider file_id needed)
  - Touches last_used_at for GC staleness tracking (best-effort)

### Test Modules (all under `#[cfg(test)]`)
- `prompt_or_task_fallback_tests` (6 tests) — Regression tests for null/empty prompt → task fallback behavior
- `stream_default_tests` (3 tests) — Verifies streaming defaults to true, explicit false disables, inputs override config
- `build_initial_user_message_tests` (2 tests) — Verifies Plan B: initial user message never carries file content inline
- `persist_attachment_bytes_tests` (5 async tests) — Tests byte persistence from inline/signed-URL sources, storage errors, precedence rules
- `files_parser_tests` (7 tests) — Tests file JSON parsing: base64 data, size limits, signed URLs, data/URL precedence, legacy compat
- `find_pending_tool_call_tests` (5 tests) — Tests tool call resume detection: unmatched calls, resolved calls, multiple messages, empty history, multiple calls per message
- `resolver_tests` (5 async tests) — Tests AttachmentResolverImpl: re-upload on expiry, unknown documents, missing storage on Generated rows, text-from-storage fallback, Step-3 text persistence

## File-level notes

- **Size & Complexity**: File is 6407 lines; the `ExecutableNode::execute()` method is ~2500 lines (single-responsibility violation candidate — orchestration of file resolution, skill loading, temporal context, attachment catalog, agent service invocation could be factored into smaller private methods for maintainability, though current structure is not broken).

- **TODO Comments** (lines 1842, 4047, 4089): All reference "TODO(plan-a-opt): share bytes with provider upload to avoid re-fetch" — optimization note to avoid re-fetching signed-URL bytes when also uploading to provider Files API; acceptable as future work, non-blocking.

- **Unreachable Pattern** (line 1183): `AttachmentSource::Inline => unreachable!()` in `resolve()` is justified — Inline source never reaches that code path because text-like inline attachments are handled separately earlier.

- **Comprehensive Test Coverage**: 33 total tests covering prompt fallback, streaming, file parsing, persistence, resume detection, and attachment resolution; tests use mocking (mockall) and async runtime (tokio).

- **Blast Radius**: Only imported by `dag_engine::infrastructure::registry` (1 importer); changes to public API (LlmNode constructor, ExecutableNode trait) affect only registry's node registration. Large dependency footprint (28 modules) reflects complexity of LLM orchestration, not coupling.

- **Skill Auto-Enrollment**: Implements deterministic skill discovery and auto-enrollment logic for gdocs-surgical-edits, gsheets-presentable-output, and gsheets-editing based on tool catalog inspection — pairs with tool description nudges.

- **Plan A/Plan B Attachment Design**: File implements the complete attachment lifecycle: Plan A (uniform registration to OutputStorageRepository), cross-provider lazy upload (Generated → target provider on-demand), Plan B (LLM catalog-driven, not inline content), load_attachment resolver with fallback chains (provider file_id → signed URL re-upload → storage bytes for text). Complexity is necessary but well-documented.

- **Error Handling**: Uses `Result<Value, Box<dyn Error>>` for ExecutableNode (generic errors), structured domain errors (LlmError) for LLM-specific failures, string returns for config validation — error messages are pedagogical (e.g., tool_configurations parse failure includes field hints).
