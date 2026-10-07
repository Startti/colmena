# Module Dependency Map (auto-generated)

> **DO NOT EDIT BY HAND.** Regenerate with `python3 scripts/gen_module_map.py`.
> Derived from `use crate::...` statements — the intra-crate import graph.

**How to use (for the exploration/spec phase):** before opening files to assess a change, look up the target file below. **Used by** is its blast radius — the files that break if you change its public surface. **Depends on** is what it needs. Start by reading only those, not the whole repo.

- Files indexed: **474**
- Modules with at least one importer: **235**

## Blast-radius ranking (change these with the most care)

| Importers | Module | File |
|---:|---|---|
| 98 | `llm::domain` | `src/libs/colmena/src/llm/domain/mod.rs` |
| 40 | `dag_engine::domain::node` | `src/libs/colmena/src/dag_engine/domain/node.rs` |
| 33 | `dag_engine::domain::lint` | `src/libs/colmena/src/dag_engine/domain/lint/mod.rs` |
| 33 | `dag_engine::domain::observer` | `src/libs/colmena/src/dag_engine/domain/observer.rs` |
| 32 | `storage::domain` | `src/libs/colmena/src/storage/domain/mod.rs` |
| 25 | `llm::domain::tools` | `src/libs/colmena/src/llm/domain/tools.rs` |
| 23 | `dag_engine::application::ports` | `src/libs/colmena/src/dag_engine/application/ports.rs` |
| 23 | `documents::domain` | `src/libs/colmena/src/documents/domain/mod.rs` |
| 22 | `llm::domain::attachments` | `src/libs/colmena/src/llm/domain/attachments/mod.rs` |
| 21 | `gdocs::domain` | `src/libs/colmena/src/gdocs/domain/mod.rs` |
| 20 | `documents::domain::ids` | `src/libs/colmena/src/documents/domain/ids.rs` |
| 18 | `llm::infrastructure` | `src/libs/colmena/src/llm/infrastructure/mod.rs` |
| 17 | `crdt_documents` | `src/libs/colmena/src/crdt_documents/mod.rs` |
| 16 | `dag_engine::domain::python_executor` | `src/libs/colmena/src/dag_engine/domain/python_executor.rs` |
| 15 | `tabular_prepare::manifest` | `src/libs/colmena/src/tabular_prepare/manifest.rs` |
| 14 | `dag_engine::domain::error` | `src/libs/colmena/src/dag_engine/domain/error.rs` |
| 14 | `dag_engine::domain::tool_configuration` | `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs` |
| 14 | `dag_engine::infrastructure::pool_registry` | `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/mod.rs` |
| 14 | `llm::infrastructure::persistence` | `src/libs/colmena/src/llm/infrastructure/persistence/mod.rs` |
| 13 | `google_oauth::infrastructure` | `src/libs/colmena/src/google_oauth/infrastructure/mod.rs` |
| 12 | `dag_engine::application::secure_value_service` | `src/libs/colmena/src/dag_engine/application/secure_value_service.rs` |
| 11 | `documents::domain::ports` | `src/libs/colmena/src/documents/domain/ports.rs` |
| 11 | `google_oauth::domain` | `src/libs/colmena/src/google_oauth/domain/mod.rs` |
| 11 | `text` | `src/libs/colmena/src/text/mod.rs` |
| 10 | `dag_engine::domain::events` | `src/libs/colmena/src/dag_engine/domain/events.rs` |
| 10 | `documents::domain::artifact` | `src/libs/colmena/src/documents/domain/artifact.rs` |
| 10 | `documents::domain::patch` | `src/libs/colmena/src/documents/domain/patch.rs` |
| 10 | `gdocs::application::_test_helpers` | `src/libs/colmena/src/gdocs/application/_test_helpers.rs` |
| 9 | `crdt_documents::tool_executor` | `src/libs/colmena/src/crdt_documents/tool_executor.rs` |
| 9 | `documents::domain::ir` | `src/libs/colmena/src/documents/domain/ir/mod.rs` |

## Per-file dependencies

### attachment_gc

#### `src/libs/colmena/src/attachment_gc/main.rs`
- Module: `(crate root)`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

### bin

#### `src/libs/colmena/src/bin/colmena_oauth_setup.rs`
- Module: `bin::colmena_oauth_setup`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/bin/python_executor.rs`
- Module: `bin::python_executor`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

### crdt_documents

#### `src/libs/colmena/src/crdt_documents/artifact_id.rs`
- Module: `crdt_documents::artifact_id`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/crdt_documents/change_tracker.rs`
- Module: `crdt_documents::change_tracker`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `crdt_documents`

#### `src/libs/colmena/src/crdt_documents/change_tracker_store.rs`
- Module: `crdt_documents::change_tracker_store`
- **Used by (2)**: `src/libs/colmena/src/crdt_documents/crdt_backend.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_summary.rs`
- Depends on (1): `crdt_documents`

#### `src/libs/colmena/src/crdt_documents/crdt_backend.rs`
- Module: `crdt_documents::crdt_backend`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `crdt_documents`, `crdt_documents::change_tracker_store`

#### `src/libs/colmena/src/crdt_documents/df_records.rs`
- Module: `crdt_documents::df_records`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `crdt_documents::projection`, `crdt_documents::tool_executor`

#### `src/libs/colmena/src/crdt_documents/df_writer.rs`
- Module: `crdt_documents::df_writer`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (4): `crdt_documents::formula_engine`, `crdt_documents::formula_engine_yrs_resolver`, `crdt_documents::projection`, `crdt_documents::tool_executor`

#### `src/libs/colmena/src/crdt_documents/doc_registry.rs`
- Module: `crdt_documents::doc_registry`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `crdt_documents`

#### `src/libs/colmena/src/crdt_documents/formula_engine.rs`
- Module: `crdt_documents::formula_engine`
- **Used by (4)**: `src/libs/colmena/src/crdt_documents/df_writer.rs`, `src/libs/colmena/src/crdt_documents/formula_engine_yrs_resolver.rs`, `src/libs/colmena/src/crdt_documents/recalc_observer.rs`, `src/libs/colmena/src/crdt_documents/tool_executor.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/crdt_documents/formula_engine_yrs_resolver.rs`
- Module: `crdt_documents::formula_engine_yrs_resolver`
- **Used by (3)**: `src/libs/colmena/src/crdt_documents/df_writer.rs`, `src/libs/colmena/src/crdt_documents/recalc_observer.rs`, `src/libs/colmena/src/crdt_documents/tool_executor.rs`
- Depends on (2): `crdt_documents::formula_engine`, `crdt_documents::tool_executor`

#### `src/libs/colmena/src/crdt_documents/mod.rs`
- Module: `crdt_documents`
- **Used by (17)**: `src/libs/colmena/src/crdt_documents/change_tracker.rs`, `src/libs/colmena/src/crdt_documents/change_tracker_store.rs`, `src/libs/colmena/src/crdt_documents/crdt_backend.rs`, `src/libs/colmena/src/crdt_documents/doc_registry.rs`, `src/libs/colmena/src/crdt_documents/process_runtime.rs`, `src/libs/colmena/src/crdt_documents/runtime.rs`, `src/libs/colmena/src/crdt_documents/server.rs`, `src/libs/colmena/src/crdt_documents/snapshot_writer.rs`, `src/libs/colmena/src/crdt_documents/storage/mod.rs`, `src/libs/colmena/src/crdt_documents/ws_peer.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_context.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs`, `src/libs/colmena/src/node_bindings/documents.rs`, `src/libs/colmena/src/python_bindings/crdt_documents.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/crdt_documents/narration.rs`
- Module: `crdt_documents::narration`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `crdt_documents::projection`, `crdt_documents::tool_executor`

#### `src/libs/colmena/src/crdt_documents/process_runtime.rs`
- Module: `crdt_documents::process_runtime`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `crdt_documents`

#### `src/libs/colmena/src/crdt_documents/projection.rs`
- Module: `crdt_documents::projection`
- **Used by (6)**: `src/libs/colmena/src/crdt_documents/df_records.rs`, `src/libs/colmena/src/crdt_documents/df_writer.rs`, `src/libs/colmena/src/crdt_documents/narration.rs`, `src/libs/colmena/src/crdt_documents/tool_executor.rs`, `src/libs/colmena/src/crdt_documents/xlsx_export.rs`, `src/libs/colmena/src/crdt_documents/xlsx_import.rs`
- Depends on (1): `crdt_documents::tool_executor`

#### `src/libs/colmena/src/crdt_documents/recalc_observer.rs`
- Module: `crdt_documents::recalc_observer`
- **Used by (1)**: `src/libs/colmena/src/crdt_documents/tool_executor.rs`
- Depends on (3): `crdt_documents::formula_engine`, `crdt_documents::formula_engine_yrs_resolver`, `crdt_documents::tool_executor`

#### `src/libs/colmena/src/crdt_documents/runtime.rs`
- Module: `crdt_documents::runtime`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `crdt_documents`

#### `src/libs/colmena/src/crdt_documents/server.rs`
- Module: `crdt_documents::server`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `crdt_documents`

#### `src/libs/colmena/src/crdt_documents/snapshot_writer.rs`
- Module: `crdt_documents::snapshot_writer`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `crdt_documents`

#### `src/libs/colmena/src/crdt_documents/storage/gcs.rs`
- Module: `crdt_documents::storage::gcs`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/crdt_documents/storage/localfs.rs`
- Module: `crdt_documents::storage::localfs`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/crdt_documents/storage/mod.rs`
- Module: `crdt_documents::storage`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `crdt_documents`

#### `src/libs/colmena/src/crdt_documents/tool_executor.rs`
- Module: `crdt_documents::tool_executor`
- **Used by (9)**: `src/libs/colmena/src/crdt_documents/df_records.rs`, `src/libs/colmena/src/crdt_documents/df_writer.rs`, `src/libs/colmena/src/crdt_documents/formula_engine_yrs_resolver.rs`, `src/libs/colmena/src/crdt_documents/narration.rs`, `src/libs/colmena/src/crdt_documents/projection.rs`, `src/libs/colmena/src/crdt_documents/recalc_observer.rs`, `src/libs/colmena/src/crdt_documents/xlsx_export.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs`
- Depends on (5): `crdt_documents::formula_engine`, `crdt_documents::formula_engine_yrs_resolver`, `crdt_documents::projection`, `crdt_documents::recalc_observer`, `crdt_documents::yjs_protocol`

#### `src/libs/colmena/src/crdt_documents/ws_peer.rs`
- Module: `crdt_documents::ws_peer`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `crdt_documents`, `crdt_documents::yjs_protocol`

#### `src/libs/colmena/src/crdt_documents/xlsx_export.rs`
- Module: `crdt_documents::xlsx_export`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `crdt_documents::projection`, `crdt_documents::tool_executor`

#### `src/libs/colmena/src/crdt_documents/xlsx_import.rs`
- Module: `crdt_documents::xlsx_import`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `crdt_documents::projection`

#### `src/libs/colmena/src/crdt_documents/yjs_protocol.rs`
- Module: `crdt_documents::yjs_protocol`
- **Used by (2)**: `src/libs/colmena/src/crdt_documents/tool_executor.rs`, `src/libs/colmena/src/crdt_documents/ws_peer.rs`
- Depends on (0): — (no intra-crate imports)

### dag_engine

#### `src/libs/colmena/src/dag_engine/api.rs`
- Module: `dag_engine::api`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `dag_engine::domain::events`, `dag_engine::domain::graph`, `dag_engine::sse_mapper`

#### `src/libs/colmena/src/dag_engine/application/list_tool_executor.rs`
- Module: `dag_engine::application::list_tool_executor`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/application/liveness.rs`
- Module: `dag_engine::application::liveness`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/application/mod.rs`
- Module: `dag_engine::application`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/application/ports.rs`
- Module: `dag_engine::application::ports`
- **Used by (23)**: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor/large_object_guard.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/node_harness.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python/large.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/bind.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/wire.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/node.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`, `src/libs/colmena/src/gdocs/infrastructure/http_client.rs`, `src/libs/colmena/src/google_oauth/infrastructure/host_refresh_provider.rs`, `src/libs/colmena/src/google_oauth/infrastructure/workspace_auth.rs`, `src/libs/colmena/src/gsheets/infrastructure/http_client.rs`, `src/libs/colmena/src/llm/infrastructure/mcp_client/rmcp_http_client.rs`, `src/libs/colmena/src/node_bindings/registry.rs`, `src/libs/colmena/src/python_bindings/mod.rs`
- Depends on (2): `dag_engine::domain::error`, `dag_engine::domain::node`

#### `src/libs/colmena/src/dag_engine/application/preflight.rs`
- Module: `dag_engine::application::preflight`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`
- Depends on (7): `dag_engine::application::preflight_cache`, `dag_engine::domain::child_graph_source`, `dag_engine::domain::error`, `dag_engine::domain::graph`, `llm::domain`, `llm::infrastructure`, `llm::infrastructure::transient`

#### `src/libs/colmena/src/dag_engine/application/preflight_cache.rs`
- Module: `dag_engine::application::preflight_cache`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/application/preflight.rs`
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/dag_engine/application/run_control.rs`
- Module: `dag_engine::application::run_control`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/engine.rs`
- Depends on (2): `llm::domain::call_cancels`, `llm::domain::steering`

#### `src/libs/colmena/src/dag_engine/application/run_use_case.rs`
- Module: `dag_engine::application::run_use_case`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (17): `dag_engine::application::liveness`, `dag_engine::application::ports`, `dag_engine::application::preflight`, `dag_engine::application::secure_value_service`, `dag_engine::domain::error`, `dag_engine::domain::events`, `dag_engine::domain::graph`, `dag_engine::domain::graph_skeleton`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::domain::secure_value_repository`, `dag_engine::domain::state`, `dag_engine::infrastructure::dag_tool_executor`, `dag_engine::infrastructure::nodes::http`, `dag_engine::infrastructure::nodes::trigger`, `llm::domain::call_cancels`, `llm::domain::steering`

#### `src/libs/colmena/src/dag_engine/application/secure_value_service.rs`
- Module: `dag_engine::application::secure_value_service`
- **Used by (12)**: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/bind.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/wire.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (1): `dag_engine::domain`

#### `src/libs/colmena/src/dag_engine/application/sql_execution_service.rs`
- Module: `dag_engine::application::sql_execution_service`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`
- Depends on (4): `dag_engine::domain::sql_errors`, `dag_engine::domain::sql_permissions`, `dag_engine::domain::sql_ports`, `dag_engine::infrastructure::sql_ast`

#### `src/libs/colmena/src/dag_engine/domain/child_graph_source.rs`
- Module: `dag_engine::domain::child_graph_source`
- **Used by (5)**: `src/libs/colmena/src/dag_engine/application/preflight.rs`, `src/libs/colmena/src/dag_engine/infrastructure/node_schema_merge.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/config.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/domain/error.rs`
- Module: `dag_engine::domain::error`
- **Used by (14)**: `src/libs/colmena/src/dag_engine/application/ports.rs`, `src/libs/colmena/src/dag_engine/application/preflight.rs`, `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, `src/libs/colmena/src/dag_engine/domain/graph.rs`, `src/libs/colmena/src/dag_engine/domain/secure_value_repository.rs`, `src/libs/colmena/src/dag_engine/domain/state.rs`, `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/bind.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/task_mutations.rs`, `src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_dag_state_repository.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/domain/events.rs`
- Module: `dag_engine::domain::events`
- **Used by (10)**: `src/libs/colmena/src/dag_engine/api.rs`, `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, `src/libs/colmena/src/dag_engine/domain/observer.rs`, `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`, `src/libs/colmena/src/dag_engine/sse_mapper.rs`
- Depends on (1): `dag_engine::domain::observer`

#### `src/libs/colmena/src/dag_engine/domain/graph.rs`
- Module: `dag_engine::domain::graph`
- **Used by (7)**: `src/libs/colmena/src/dag_engine/api.rs`, `src/libs/colmena/src/dag_engine/application/preflight.rs`, `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, `src/libs/colmena/src/dag_engine/domain/graph_skeleton.rs`, `src/libs/colmena/src/dag_engine/domain/lint/linter.rs`, `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (2): `dag_engine::domain::error`, `dag_engine::domain::tool_configuration`

#### `src/libs/colmena/src/dag_engine/domain/graph_skeleton.rs`
- Module: `dag_engine::domain::graph_skeleton`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs`
- Depends on (1): `dag_engine::domain::graph`

#### `src/libs/colmena/src/dag_engine/domain/initializable_node.rs`
- Module: `dag_engine::domain::initializable_node`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/domain/node.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/domain/lint/catalog.rs`
- Module: `dag_engine::domain::lint::catalog`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/domain/lint/diagnostic.rs`
- Module: `dag_engine::domain::lint::diagnostic`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/domain/lint/linter.rs`
- Module: `dag_engine::domain::lint::linter`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `dag_engine::domain::graph`, `dag_engine::domain::tool_configuration`

#### `src/libs/colmena/src/dag_engine/domain/lint/mod.rs`
- Module: `dag_engine::domain::lint`
- **Used by (33)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/critic.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/current_time.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/debug.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/extraction.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/input.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/loop_controller.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/math.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/output.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/output_parser.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/planner.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/reactor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/node.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/socketio.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/task_memory_writer.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/trigger.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`, `src/libs/colmena/src/node_bindings/dag.rs`, `src/libs/colmena/src/python_bindings/mod.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/domain/mod.rs`
- Module: `dag_engine::domain`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/application/secure_value_service.rs`, `src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/domain/node.rs`
- Module: `dag_engine::domain::node`
- **Used by (40)**: `src/libs/colmena/src/dag_engine/application/ports.rs`, `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, `src/libs/colmena/src/dag_engine/domain/toolkit_node.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor/large_object_guard.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/env_provenance.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/critic.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/current_time.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/debug.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/echo_toolkit.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/extraction.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/http_oauth.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/input.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python/large.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/loop_controller.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/math.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/output.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/output_parser.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/planner.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/reactor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/node.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/socketio.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/task_memory_writer.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/trigger.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (2): `dag_engine::domain::initializable_node`, `dag_engine::domain::observer`

#### `src/libs/colmena/src/dag_engine/domain/observer.rs`
- Module: `dag_engine::domain::observer`
- **Used by (33)**: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, `src/libs/colmena/src/dag_engine/domain/events.rs`, `src/libs/colmena/src/dag_engine/domain/node.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/critic.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/current_time.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/echo_toolkit.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/loop_controller.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/output.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/output_parser.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/planner.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/reactor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/decision_model.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/extract_and_route.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/llm_direct.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/node.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/billed_llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/extract_with_schema.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs`
- Depends on (1): `dag_engine::domain::events`

#### `src/libs/colmena/src/dag_engine/domain/python_executor.rs`
- Module: `dag_engine::domain::python_executor`
- **Used by (16)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/data_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs`, `src/libs/colmena/src/dag_engine/infrastructure/python_exec/config.rs`, `src/libs/colmena/src/dag_engine/infrastructure/python_exec/inprocess.rs`, `src/libs/colmena/src/dag_engine/infrastructure/python_exec/mod.rs`, `src/libs/colmena/src/dag_engine/infrastructure/python_exec/protocol.rs`, `src/libs/colmena/src/dag_engine/infrastructure/python_exec/remote.rs`, `src/libs/colmena/src/dag_engine/infrastructure/python_exec/subprocess.rs`, `src/libs/colmena/src/tabular_run/local.rs`, `src/libs/colmena/src/tabular_run/mounted.rs`, `src/libs/colmena/src/tabular_run/remote_seam.rs`, `src/libs/colmena/src/tabular_run/runtime.rs`, `src/libs/colmena/src/tabular_run/testkit.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/domain/router_rules.rs`
- Module: `dag_engine::domain::router_rules`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/config.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/decision_model.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/domain/secure_value_repository.rs`
- Module: `dag_engine::domain::secure_value_repository`
- **Used by (4)**: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/bind.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (1): `dag_engine::domain::error`

#### `src/libs/colmena/src/dag_engine/domain/sql_errors.rs`
- Module: `dag_engine::domain::sql_errors`
- **Used by (7)**: `src/libs/colmena/src/dag_engine/application/sql_execution_service.rs`, `src/libs/colmena/src/dag_engine/domain/sql_ports.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_function_registry.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_port_factory.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/domain/sql_permissions.rs`
- Module: `dag_engine::domain::sql_permissions`
- **Used by (8)**: `src/libs/colmena/src/dag_engine/application/sql_execution_service.rs`, `src/libs/colmena/src/dag_engine/domain/sql_ports.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/data_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/table_writer.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tabular_bindings.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_ast.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/domain/sql_ports.rs`
- Module: `dag_engine::domain::sql_ports`
- **Used by (8)**: `src/libs/colmena/src/dag_engine/application/sql_execution_service.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/data_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tabular_bindings.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_function_registry.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs`
- Depends on (2): `dag_engine::domain::sql_errors`, `dag_engine::domain::sql_permissions`

#### `src/libs/colmena/src/dag_engine/domain/state.rs`
- Module: `dag_engine::domain::state`
- **Used by (8)**: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/node_harness.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/task_mutations.rs`, `src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_dag_state_repository.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (1): `dag_engine::domain::error`

#### `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs`
- Module: `dag_engine::domain::tool_configuration`
- **Used by (14)**: `src/libs/colmena/src/dag_engine/domain/graph.rs`, `src/libs/colmena/src/dag_engine/domain/lint/linter.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/node_schema_merge.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/lazy_tools_catalog.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/list_threads.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/bind.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/expose.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/wire.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (2): `llm::domain`, `llm::domain::mcp`

#### `src/libs/colmena/src/dag_engine/domain/toolkit_node.rs`
- Module: `dag_engine::domain::toolkit_node`
- **Used by (5)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/echo_toolkit.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (2): `dag_engine::domain::node`, `llm::domain::tools`

#### `src/libs/colmena/src/dag_engine/engine.rs`
- Module: `dag_engine::engine`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`, `src/libs/colmena/src/dag_engine/log_policy.rs`, `src/libs/colmena/src/tabular_prepare/ports.rs`
- Depends on (20): `dag_engine::application::ports`, `dag_engine::application::run_control`, `dag_engine::application::run_use_case`, `dag_engine::application::secure_value_service`, `dag_engine::domain::error`, `dag_engine::domain::events`, `dag_engine::domain::graph`, `dag_engine::domain::state`, `dag_engine::infrastructure::persistence`, `dag_engine::infrastructure::persistence::postgres_dag_state_repository`, `dag_engine::infrastructure::pool_registry`, `dag_engine::infrastructure::registry`, `dag_engine::infrastructure::sql_port_factory`, `dag_engine::sse_mapper`, `llm::domain`, `llm::infrastructure::persistence`, `llm::infrastructure::persistence::repository_factory`, `storage::domain`, `storage::infrastructure`, `tabular_prepare::ports`

#### `src/libs/colmena/src/dag_engine/frame_redaction.rs`
- Module: `dag_engine::frame_redaction`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/sse_mapper.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor/large_object_guard.rs`
- Module: `dag_engine::infrastructure::dag_tool_executor::large_object_guard`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (12): `dag_engine::application::ports`, `dag_engine::domain::node`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_run_python`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::gdocs_tools`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::gsheets_tools`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::sql_bulk_tools`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::tabular_bindings`, `llm::domain`, `llm::domain::attachments`, `llm::domain::attachments::attachment_registry`, `llm::domain::large_tabular`, `storage::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor/large_route.rs`
- Module: `dag_engine::infrastructure::dag_tool_executor::large_route`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `tabular_run::runtime`

#### `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`
- Module: `dag_engine::infrastructure::dag_tool_executor`
- **Used by (8)**: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, `src/libs/colmena/src/dag_engine/infrastructure/node_schema_merge.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python/large.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (33): `dag_engine::application::ports`, `dag_engine::application::secure_value_service`, `dag_engine::domain::error`, `dag_engine::domain::events`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::domain::state`, `dag_engine::domain::tool_configuration`, `dag_engine::domain::toolkit_node`, `dag_engine::infrastructure::env_provenance`, `dag_engine::infrastructure::node_schema_merge`, `dag_engine::infrastructure::nodes::api_explorer`, `dag_engine::infrastructure::nodes::echo_toolkit`, `dag_engine::infrastructure::nodes::http`, `dag_engine::infrastructure::nodes::llm_synthetic_tools`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_run_python`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::data_run_python`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::gdocs_tools`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::gsheets_inspect_guard`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::gsheets_tools`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::mcp`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::sql_bulk_tools`, `dag_engine::infrastructure::nodes::subgraph`, `google_oauth::infrastructure`, `llm::domain`, `llm::domain::attachments`, `llm::domain::attachments::attachment_registry`, `llm::domain::large_tabular`, `llm::domain::tools`, `llm::infrastructure::persistence`, `llm::infrastructure::persistence::in_memory_conversation_repository`, `skills::domain`, `storage::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/env_provenance.rs`
- Module: `dag_engine::infrastructure::env_provenance`
- **Used by (8)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/http_oauth.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/socketio.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`
- Depends on (1): `dag_engine::domain::node`

#### `src/libs/colmena/src/dag_engine/infrastructure/mcp_registry/key.rs`
- Module: `dag_engine::infrastructure::mcp_registry::key`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/mcp_registry/registry.rs`
- Depends on (1): `llm::domain::mcp`

#### `src/libs/colmena/src/dag_engine/infrastructure/mcp_registry/mod.rs`
- Module: `dag_engine::infrastructure::mcp_registry`
- **Used by (4)**: `src/libs/colmena/src/dag_engine/infrastructure/mcp_registry/registry.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/bind.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/dispatch.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/wire.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/mcp_registry/registry.rs`
- Module: `dag_engine::infrastructure::mcp_registry::registry`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `dag_engine::infrastructure::mcp_registry`, `dag_engine::infrastructure::mcp_registry::key`, `llm::domain::mcp`

#### `src/libs/colmena/src/dag_engine/infrastructure/mod.rs`
- Module: `dag_engine::infrastructure`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/node_schema_merge.rs`
- Module: `dag_engine::infrastructure::node_schema_merge`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`
- Depends on (3): `dag_engine::domain::child_graph_source`, `dag_engine::domain::tool_configuration`, `dag_engine::infrastructure::dag_tool_executor`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`
- Module: `dag_engine::infrastructure::nodes::api_explorer`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (10): `dag_engine::application::secure_value_service`, `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::domain::toolkit_node`, `llm::domain`, `web::application::api_spec_use_case`, `web::domain`, `web::domain::api_spec_port`, `web::infrastructure::openapi_adapter`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/critic.rs`
- Module: `dag_engine::infrastructure::nodes::critic`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (7): `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `llm::application`, `llm::domain`, `llm::infrastructure`, `llm::infrastructure::persistence::in_memory_conversation_repository`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/current_time.rs`
- Module: `dag_engine::infrastructure::nodes::current_time`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/debug.rs`
- Module: `dag_engine::infrastructure::nodes::debug`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `dag_engine::domain::lint`, `dag_engine::domain::node`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs`
- Module: `dag_engine::infrastructure::nodes::document_nodes`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (10): `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `documents::application`, `documents::application::apply_patch`, `documents::application::create_document`, `documents::application::read_document`, `documents::domain`, `documents::domain::ids`, `documents::domain::patch`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/echo_toolkit.rs`
- Module: `dag_engine::infrastructure::nodes::echo_toolkit`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`
- Depends on (4): `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::domain::toolkit_node`, `llm::domain::tools`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/extraction.rs`
- Module: `dag_engine::infrastructure::nodes::extraction`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (4): `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::infrastructure::nodes::util::extract_with_schema`, `llm::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`
- Module: `dag_engine::infrastructure::nodes::for_each`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (16): `dag_engine::application::list_tool_executor`, `dag_engine::application::ports`, `dag_engine::domain::events`, `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::domain::tool_configuration`, `dag_engine::infrastructure::dag_tool_executor`, `dag_engine::infrastructure::env_provenance`, `dag_engine::infrastructure::node_schema_merge`, `dag_engine::infrastructure::nodes::http`, `dag_engine::infrastructure::nodes::llm_synthetic_tools`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::gsheets_tools`, `dag_engine::infrastructure::nodes::math`, `google_oauth::infrastructure`, `llm::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`
- Module: `dag_engine::infrastructure::nodes::http`
- **Used by (4)**: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/socketio.rs`
- Depends on (20): `dag_engine::application::ports`, `dag_engine::application::run_use_case`, `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::infrastructure::dag_tool_executor`, `dag_engine::infrastructure::env_provenance`, `dag_engine::infrastructure::nodes::http_oauth`, `dag_engine::infrastructure::nodes::trigger`, `dag_engine::infrastructure::nodes::util::attachment_id`, `dag_engine::infrastructure::nodes::util::response_file`, `dag_engine::infrastructure::nodes::util::session_attachment`, `google_oauth`, `google_oauth::infrastructure`, `llm::domain`, `llm::domain::attachments`, `llm::infrastructure::attachments`, `llm::infrastructure::files`, `llm::infrastructure::files::signed_url_downloader`, `llm::infrastructure::persistence`, `storage::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/http_oauth.rs`
- Module: `dag_engine::infrastructure::nodes::http_oauth`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`
- Depends on (4): `dag_engine::domain::node`, `dag_engine::infrastructure::env_provenance`, `google_oauth::domain`, `google_oauth::infrastructure`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`
- Module: `dag_engine::infrastructure::nodes::image_edit`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (12): `dag_engine::application::secure_value_service`, `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::infrastructure::nodes::util::attachment_id`, `dag_engine::infrastructure::nodes::util::session_attachment`, `llm::domain`, `llm::domain::attachments`, `llm::infrastructure::attachments`, `llm::infrastructure::files`, `llm::infrastructure::persistence`, `storage::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs`
- Module: `dag_engine::infrastructure::nodes::image_generation`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (9): `dag_engine::application::secure_value_service`, `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::infrastructure::nodes::util::attachment_id`, `llm::domain`, `llm::domain::attachments`, `llm::infrastructure::persistence`, `storage::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/input.rs`
- Module: `dag_engine::infrastructure::nodes::input`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::infrastructure::nodes::util::template`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/attachment_notices.rs`
- Module: `dag_engine::infrastructure::nodes::llm::attachment_notices`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/characterisation.rs`
- Module: `dag_engine::infrastructure::nodes::llm::characterisation`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (5): `llm::application`, `llm::domain`, `llm::domain::attachments`, `llm::infrastructure::persistence`, `storage::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/large_files.rs`
- Module: `dag_engine::infrastructure::nodes::llm::large_files`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `llm::application`, `llm::domain`, `llm::domain::attachments`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/large_run_turn.rs`
- Module: `dag_engine::infrastructure::nodes::llm::large_run_turn`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `llm::infrastructure`, `tabular_run::testkit`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/load_attachment_redirect.rs`
- Module: `dag_engine::infrastructure::nodes::llm::load_attachment_redirect`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (7): `llm::application`, `llm::domain`, `llm::domain::attachments`, `llm::domain::attachments::attachment_registry`, `llm::domain::large_tabular`, `llm::infrastructure::persistence`, `storage::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/node_harness.rs`
- Module: `dag_engine::infrastructure::nodes::llm::node_harness`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (10): `dag_engine::application::ports`, `dag_engine::domain::state`, `dag_engine::infrastructure::persistence`, `dag_engine::infrastructure::pool_registry`, `dag_engine::infrastructure::registry`, `dag_engine::infrastructure::sql_port_factory`, `llm::domain`, `llm::infrastructure`, `storage::domain`, `storage::infrastructure`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/storage_key_entries.rs`
- Module: `dag_engine::infrastructure::nodes::llm::storage_key_entries`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/storage_ref_turn.rs`
- Module: `dag_engine::infrastructure::nodes::llm::storage_ref_turn`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (5): `llm::domain`, `llm::domain::attachments`, `llm::domain::large_tabular`, `llm::infrastructure`, `llm::infrastructure::persistence`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/whole_read_guard.rs`
- Module: `dag_engine::infrastructure::nodes::llm::whole_read_guard`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `llm::domain::large_tabular`, `llm::infrastructure`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
- Module: `dag_engine::infrastructure::nodes::llm`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (38): `crdt_documents`, `dag_engine::application::ports`, `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::domain::tool_configuration`, `dag_engine::infrastructure::dag_tool_executor`, `dag_engine::infrastructure::env_provenance`, `dag_engine::infrastructure::nodes::llm_synthetic_tools`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_run_python`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::data_run_python`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::mcp`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::sql_bulk_tools`, `dag_engine::infrastructure::nodes::util::billed_llm`, `dag_engine::infrastructure::pool_registry`, `dag_engine::infrastructure::registry`, `documents::application`, `documents::domain::ids`, `llm::application`, `llm::application::agent_service`, `llm::application::attachment_catalog`, `llm::domain`, `llm::domain::attachments`, `llm::domain::call_cancels`, `llm::domain::large_tabular`, `llm::domain::steering`, `llm::domain::tools`, `llm::infrastructure`, `llm::infrastructure::attachment_summary`, `llm::infrastructure::attachments`, `llm::infrastructure::files`, `llm::infrastructure::files::signed_url_downloader`, `llm::infrastructure::persistence`, `llm::infrastructure::persistence::in_memory_conversation_repository`, `skills::domain`, `skills::infrastructure`, `storage::domain`, `storage::infrastructure`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python/large.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_run_python::large`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (10): `dag_engine::application::ports`, `dag_engine::domain::node`, `dag_engine::infrastructure::dag_tool_executor`, `llm::domain`, `llm::domain::attachments`, `storage::domain`, `tabular_run::mounted`, `tabular_run::refusal`, `tabular_run::runtime`, `tabular_run::testkit`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_run_python`
- **Used by (4)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor/large_object_guard.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/tabular_run/prelude.rs`
- Depends on (2): `dag_engine::domain::python_executor`, `llm::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_writer.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_writer`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_context.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::crdt_doc_context`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `crdt_documents`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::crdt_doc_import_sheet`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `crdt_documents`, `llm::domain::tools`, `text`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::crdt_doc_run_python`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (6): `crdt_documents`, `crdt_documents::tool_executor`, `dag_engine::domain::python_executor`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::crdt_doc_tools`, `llm::domain::tools`, `text`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::crdt_doc_tools`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs`
- Depends on (4): `crdt_documents`, `crdt_documents::tool_executor`, `llm::domain::tools`, `text`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_summary.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::crdt_summary`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `crdt_documents::change_tracker_store`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/data_run_python.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::data_run_python`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
- Depends on (8): `dag_engine::domain::python_executor`, `dag_engine::domain::sql_permissions`, `dag_engine::domain::sql_ports`, `gsheets::domain`, `gsheets::infrastructure::config`, `gsheets::infrastructure::http_client`, `llm::domain::tools`, `text`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::describe_tool`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `dag_engine::domain::tool_configuration`, `llm::domain`, `llm::domain::tools`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/diff_writer.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::diff_writer`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::document_tools`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (11): `documents::application::apply_patch`, `documents::application::create_document`, `documents::application::get_head`, `documents::application::list_versions`, `documents::application::read_document`, `documents::application::rollback`, `documents::domain`, `documents::domain::ids`, `documents::domain::patch`, `llm::domain::tools`, `text`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::gdocs_tools`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor/large_object_guard.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`
- Depends on (10): `gdocs::application`, `gdocs::application::table_format`, `gdocs::domain`, `gdocs::infrastructure::config`, `gdocs::infrastructure::http_client`, `gdocs::infrastructure::outline_cache`, `gdocs::infrastructure::revision_store`, `google_oauth::infrastructure`, `llm::domain::tools`, `text`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/google_workspace_prelude.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::google_workspace_prelude`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_inspect_guard.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::gsheets_inspect_guard`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::gsheets_run_python`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (5): `dag_engine::domain::python_executor`, `gsheets::domain`, `gsheets::infrastructure::http_client`, `llm::domain::tools`, `text`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::gsheets_tools`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor/large_object_guard.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`
- Depends on (7): `google_oauth::infrastructure`, `gsheets::application::format`, `gsheets::domain`, `gsheets::infrastructure::config`, `gsheets::infrastructure::http_client`, `llm::domain::tools`, `text`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/lazy_tools_catalog.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::lazy_tools_catalog`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `dag_engine::domain::tool_configuration`, `llm::domain`, `llm::domain::tools`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/list_threads.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::list_threads`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (4): `dag_engine::domain::tool_configuration`, `llm::domain`, `llm::domain::tools`, `text`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::load_attachment_tool`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (5): `llm::domain`, `llm::domain::attachments`, `llm::domain::tools`, `storage::domain`, `storage::infrastructure`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::load_skill_tool`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `llm::domain`, `llm::domain::tools`, `skills::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/markdown_to_docs_ops.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::markdown_to_docs_ops`
- **Used by (3)**: `src/libs/colmena/src/gdocs/application/apply_edits.rs`, `src/libs/colmena/src/gdocs/application/insert.rs`, `src/libs/colmena/src/gdocs/application/replace_section.rs`
- Depends on (1): `gdocs::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/allowlist.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::mcp::allowlist`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/socketio.rs`, `src/libs/colmena/src/llm/infrastructure/files/signed_url_downloader.rs`, `src/libs/colmena/src/llm/infrastructure/mcp_client/rmcp_http_client.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/bind.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::mcp::bind`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (11): `dag_engine::application::ports`, `dag_engine::application::secure_value_service`, `dag_engine::domain::error`, `dag_engine::domain::secure_value_repository`, `dag_engine::domain::tool_configuration`, `dag_engine::infrastructure::mcp_registry`, `google_oauth::domain`, `google_oauth::infrastructure`, `llm::domain::mcp`, `llm::infrastructure::mcp_client`, `llm::infrastructure::mcp_client::rmcp_http_client`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/contain.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::mcp::contain`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `llm::domain::mcp`, `llm::domain::text_bounds`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/dispatch.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::mcp::dispatch`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `dag_engine::infrastructure::mcp_registry`, `llm::domain::mcp`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/expose.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::mcp::expose`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (4): `dag_engine::domain::tool_configuration`, `llm::domain::mcp`, `llm::domain::text_bounds`, `llm::domain::tools`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/mod.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::mcp`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/wire.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::mcp::wire`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (6): `dag_engine::application::ports`, `dag_engine::application::secure_value_service`, `dag_engine::domain::tool_configuration`, `dag_engine::infrastructure::mcp_registry`, `llm::domain::mcp`, `llm::domain::tools`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools`
- **Used by (4)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (3): `llm::domain::tools`, `skills::domain`, `text`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/recall_history.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::recall_history`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `llm::domain`, `llm::domain::tools`, `text`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/sheet_collision.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::sheet_collision`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/sheet_writer.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::sheet_writer`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `gsheets::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/sql_bulk_tools.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::sql_bulk_tools`
- **Used by (4)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor/large_object_guard.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/table_writer.rs`
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/table_writer.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::table_writer`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `dag_engine::domain::sql_permissions`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::sql_bulk_tools`, `gsheets::infrastructure::http_client`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tabular_bindings.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::tabular_bindings`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor/large_object_guard.rs`
- Depends on (5): `dag_engine::domain::sql_permissions`, `dag_engine::domain::sql_ports`, `dag_engine::infrastructure::sql_ast`, `gsheets::domain`, `gsheets::infrastructure::http_client`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs`
- Module: `dag_engine::infrastructure::nodes::llm_synthetic_tools::toolkit_packages`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/loop_controller.rs`
- Module: `dag_engine::infrastructure::nodes::loop_controller`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/math.rs`
- Module: `dag_engine::infrastructure::nodes::math`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`
- Depends on (2): `dag_engine::domain::lint`, `dag_engine::domain::node`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`
- Module: `dag_engine::infrastructure::nodes`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs`
- Module: `dag_engine::infrastructure::nodes::orchestrator`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (7): `dag_engine::application::ports`, `dag_engine::domain::child_graph_source`, `dag_engine::domain::events`, `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::domain::state`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/output.rs`
- Module: `dag_engine::infrastructure::nodes::output`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/output_parser.rs`
- Module: `dag_engine::infrastructure::nodes::output_parser`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (4): `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `llm::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/planner.rs`
- Module: `dag_engine::infrastructure::nodes::planner`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (7): `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `llm::application`, `llm::domain`, `llm::infrastructure`, `llm::infrastructure::persistence::in_memory_conversation_repository`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs`
- Module: `dag_engine::infrastructure::nodes::python_node`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/infrastructure/python_exec/child.rs`, `src/libs/colmena/src/dag_engine/infrastructure/python_exec/inprocess.rs`, `src/libs/colmena/src/tabular_run/prelude.rs`
- Depends on (6): `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::python_executor`, `dag_engine::infrastructure::env_provenance`, `dag_engine::infrastructure::python_exec`, `dag_engine::log_policy`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/qa_response_parser.rs`
- Module: `dag_engine::infrastructure::nodes::qa_response_parser`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/reactor.rs`
- Module: `dag_engine::infrastructure::nodes::reactor`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (7): `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `llm::application`, `llm::domain`, `llm::infrastructure`, `llm::infrastructure::persistence::in_memory_conversation_repository`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/config.rs`
- Module: `dag_engine::infrastructure::nodes::router::config`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/decision_model.rs`
- Depends on (2): `dag_engine::domain::child_graph_source`, `dag_engine::domain::router_rules`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/decision_model.rs`
- Module: `dag_engine::infrastructure::nodes::router::decision_model`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (4): `dag_engine::domain::observer`, `dag_engine::domain::router_rules`, `dag_engine::infrastructure::nodes::router::config`, `llm::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/extract_and_route.rs`
- Module: `dag_engine::infrastructure::nodes::router::extract_and_route`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (4): `dag_engine::domain::observer`, `dag_engine::infrastructure::nodes::util::extract_with_schema`, `dag_engine::infrastructure::nodes::util::inline_schema`, `llm::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/llm_direct.rs`
- Module: `dag_engine::infrastructure::nodes::router::llm_direct`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `dag_engine::domain::observer`, `dag_engine::infrastructure::nodes::util::extract_with_schema`, `llm::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/mod.rs`
- Module: `dag_engine::infrastructure::nodes::router`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/node.rs`
- Module: `dag_engine::infrastructure::nodes::router::node`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (6): `dag_engine::application::ports`, `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::infrastructure::nodes::subgraph`, `llm::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/when_dsl.rs`
- Module: `dag_engine::infrastructure::nodes::router::when_dsl`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`
- Module: `dag_engine::infrastructure::nodes::secure_suspend`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (8): `dag_engine::application::secure_value_service`, `dag_engine::domain::error`, `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::domain::secure_value_repository`, `dag_engine::domain::tool_configuration`, `dag_engine::infrastructure::nodes::qa_response_parser`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/socketio.rs`
- Module: `dag_engine::infrastructure::nodes::socketio`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (6): `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::infrastructure::env_provenance`, `dag_engine::infrastructure::nodes::http`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::mcp::allowlist`, `llm::infrastructure::files::signed_url_downloader`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`
- Module: `dag_engine::infrastructure::nodes::sql`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (18): `dag_engine::application::ports`, `dag_engine::application::sql_execution_service`, `dag_engine::domain::initializable_node`, `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::domain::sql_errors`, `dag_engine::domain::sql_permissions`, `dag_engine::domain::sql_ports`, `dag_engine::infrastructure::dag_tool_executor`, `dag_engine::infrastructure::env_provenance`, `dag_engine::infrastructure::pool_registry`, `dag_engine::infrastructure::sql_function_registry`, `dag_engine::infrastructure::sql_llm_critic`, `dag_engine::infrastructure::sql_pool_adapter`, `dag_engine::infrastructure::sql_port_factory`, `dag_engine::infrastructure::sql_static_validator`, `llm::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs`
- Module: `dag_engine::infrastructure::nodes::subgraph`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/node.rs`
- Depends on (9): `dag_engine::application::ports`, `dag_engine::domain::child_graph_source`, `dag_engine::domain::error`, `dag_engine::domain::events`, `dag_engine::domain::graph_skeleton`, `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `llm::domain::call_cancels`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs`
- Module: `dag_engine::infrastructure::nodes::suspend`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (4): `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::infrastructure::nodes::qa_response_parser`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/task_memory_writer.rs`
- Module: `dag_engine::infrastructure::nodes::task_memory_writer`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `dag_engine::domain::lint`, `dag_engine::domain::node`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/task_mutations.rs`
- Module: `dag_engine::infrastructure::nodes::task_mutations`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `dag_engine::domain::error`, `dag_engine::domain::state`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`
- Module: `dag_engine::infrastructure::nodes::tavily_client`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (10): `dag_engine::application::secure_value_service`, `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::domain::toolkit_node`, `llm::domain`, `web::application::search_use_case`, `web::domain::errors`, `web::domain::search_port`, `web::infrastructure::tavily_adapter`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/trigger.rs`
- Module: `dag_engine::infrastructure::nodes::trigger`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`
- Depends on (2): `dag_engine::domain::lint`, `dag_engine::domain::node`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`
- Module: `dag_engine::infrastructure::nodes::tts`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (12): `dag_engine::application::secure_value_service`, `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::infrastructure::nodes::util::attachment_id`, `llm::domain`, `llm::domain::attachments`, `llm::domain::tts`, `llm::domain::tts_repository`, `llm::infrastructure`, `llm::infrastructure::persistence`, `storage::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/attachment_id.rs`
- Module: `dag_engine::infrastructure::nodes::util::attachment_id`
- **Used by (4)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/billed_llm.rs`
- Module: `dag_engine::infrastructure::nodes::util::billed_llm`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs`
- Depends on (2): `dag_engine::domain::observer`, `llm::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/extract_with_schema.rs`
- Module: `dag_engine::infrastructure::nodes::util::extract_with_schema`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/extraction.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/extract_and_route.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/llm_direct.rs`
- Depends on (5): `dag_engine::domain::observer`, `llm::application`, `llm::domain`, `llm::infrastructure`, `llm::infrastructure::persistence::in_memory_conversation_repository`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/inline_schema.rs`
- Module: `dag_engine::infrastructure::nodes::util::inline_schema`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/extract_and_route.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/mod.rs`
- Module: `dag_engine::infrastructure::nodes::util`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/response_file.rs`
- Module: `dag_engine::infrastructure::nodes::util::response_file`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/session_attachment.rs`
- Module: `dag_engine::infrastructure::nodes::util::session_attachment`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`
- Depends on (5): `llm::domain`, `llm::domain::attachments`, `llm::infrastructure::attachments`, `llm::infrastructure::persistence`, `storage::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/template.rs`
- Module: `dag_engine::infrastructure::nodes::util::template`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/input.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/persistence/mod.rs`
- Module: `dag_engine::infrastructure::persistence`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/node_harness.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_dag_state_repository.rs`
- Module: `dag_engine::infrastructure::persistence::postgres_dag_state_repository`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/engine.rs`
- Depends on (2): `dag_engine::domain::error`, `dag_engine::domain::state`

#### `src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs`
- Module: `dag_engine::infrastructure::persistence::postgres_secure_value_repository`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `dag_engine::domain`

#### `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/config.rs`
- Module: `dag_engine::infrastructure::pool_registry::config`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/error.rs`
- Module: `dag_engine::infrastructure::pool_registry::error`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/metrics.rs`
- Module: `dag_engine::infrastructure::pool_registry::metrics`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/mod.rs`
- Module: `dag_engine::infrastructure::pool_registry`
- **Used by (14)**: `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/node_harness.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_port_factory.rs`, `src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs`, `src/libs/colmena/src/llm/infrastructure/persistence/ownership_contract.rs`, `src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs`, `src/libs/colmena/src/llm/infrastructure/persistence/repository_factory.rs`, `src/libs/colmena/src/node_bindings/registry.rs`, `src/libs/colmena/src/python_bindings/mod.rs`, `src/libs/colmena/src/tabular_prepare/postgres_registry.rs`, `src/libs/colmena/src/tabular_prepare/registry.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/registry.rs`
- Module: `dag_engine::infrastructure::pool_registry::registry`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/url_key.rs`
- Module: `dag_engine::infrastructure::pool_registry::url_key`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/python_exec/child.rs`
- Module: `dag_engine::infrastructure::python_exec::child`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `dag_engine::infrastructure::nodes::python_node`

#### `src/libs/colmena/src/dag_engine/infrastructure/python_exec/config.rs`
- Module: `dag_engine::infrastructure::python_exec::config`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `dag_engine::domain::python_executor`

#### `src/libs/colmena/src/dag_engine/infrastructure/python_exec/frame.rs`
- Module: `dag_engine::infrastructure::python_exec::frame`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/python_exec/id_token.rs`
- Module: `dag_engine::infrastructure::python_exec::id_token`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/python_exec/inprocess.rs`
- Module: `dag_engine::infrastructure::python_exec::inprocess`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `dag_engine::domain::python_executor`, `dag_engine::infrastructure::nodes::python_node`

#### `src/libs/colmena/src/dag_engine/infrastructure/python_exec/jail.rs`
- Module: `dag_engine::infrastructure::python_exec::jail`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `dag_engine::infrastructure::python_exec::staging`

#### `src/libs/colmena/src/dag_engine/infrastructure/python_exec/mod.rs`
- Module: `dag_engine::infrastructure::python_exec`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs`
- Depends on (3): `dag_engine::domain::python_executor`, `dag_engine::log_policy`, `tabular_run::mounted`

#### `src/libs/colmena/src/dag_engine/infrastructure/python_exec/protocol.rs`
- Module: `dag_engine::infrastructure::python_exec::protocol`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/python_exec/remote.rs`, `src/libs/colmena/src/tabular_run/runtime.rs`
- Depends on (1): `dag_engine::domain::python_executor`

#### `src/libs/colmena/src/dag_engine/infrastructure/python_exec/remote.rs`
- Module: `dag_engine::infrastructure::python_exec::remote`
- **Used by (1)**: `src/libs/colmena/src/tabular_run/remote_seam.rs`
- Depends on (3): `dag_engine::domain::python_executor`, `dag_engine::infrastructure::python_exec::protocol`, `dag_engine::log_policy`

#### `src/libs/colmena/src/dag_engine/infrastructure/python_exec/seccomp.rs`
- Module: `dag_engine::infrastructure::python_exec::seccomp`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/python_exec/selftest.rs`
- Module: `dag_engine::infrastructure::python_exec::selftest`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/python_exec/server.rs`
- Module: `dag_engine::infrastructure::python_exec::server`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `dag_engine::log_policy`

#### `src/libs/colmena/src/dag_engine/infrastructure/python_exec/staging.rs`
- Module: `dag_engine::infrastructure::python_exec::staging`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/python_exec/jail.rs`, `src/libs/colmena/src/tabular_run/local.rs`
- Depends on (1): `dag_engine::log_policy`

#### `src/libs/colmena/src/dag_engine/infrastructure/python_exec/subprocess.rs`
- Module: `dag_engine::infrastructure::python_exec::subprocess`
- **Used by (1)**: `src/libs/colmena/src/tabular_run/local.rs`
- Depends on (2): `dag_engine::domain::python_executor`, `dag_engine::log_policy`

#### `src/libs/colmena/src/dag_engine/infrastructure/python_exec/zygote.rs`
- Module: `dag_engine::infrastructure::python_exec::zygote`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Module: `dag_engine::infrastructure::registry`
- **Used by (5)**: `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/node_harness.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/node_bindings/registry.rs`, `src/libs/colmena/src/python_bindings/mod.rs`
- Depends on (31): `dag_engine::application::ports`, `dag_engine::application::run_use_case`, `dag_engine::application::secure_value_service`, `dag_engine::domain::error`, `dag_engine::domain::events`, `dag_engine::domain::graph`, `dag_engine::domain::lint`, `dag_engine::domain::node`, `dag_engine::domain::observer`, `dag_engine::domain::secure_value_repository`, `dag_engine::domain::state`, `dag_engine::domain::tool_configuration`, `dag_engine::domain::toolkit_node`, `dag_engine::engine`, `dag_engine::infrastructure::dag_tool_executor`, `dag_engine::infrastructure::nodes`, `dag_engine::infrastructure::nodes::api_explorer`, `dag_engine::infrastructure::nodes::image_edit`, `dag_engine::infrastructure::nodes::image_generation`, `dag_engine::infrastructure::nodes::llm`, `dag_engine::infrastructure::nodes::llm_synthetic_tools`, `dag_engine::infrastructure::nodes::tavily_client`, `dag_engine::infrastructure::nodes::tts`, `dag_engine::infrastructure::pool_registry`, `dag_engine::infrastructure::sql_port_factory`, `llm::domain`, `llm::domain::tool_executor`, `llm::infrastructure`, `llm::infrastructure::files`, `storage::domain`, `storage::infrastructure`

#### `src/libs/colmena/src/dag_engine/infrastructure/sql_ast.rs`
- Module: `dag_engine::infrastructure::sql_ast`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/application/sql_execution_service.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tabular_bindings.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs`
- Depends on (1): `dag_engine::domain::sql_permissions`

#### `src/libs/colmena/src/dag_engine/infrastructure/sql_function_registry.rs`
- Module: `dag_engine::infrastructure::sql_function_registry`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`
- Depends on (2): `dag_engine::domain::sql_errors`, `dag_engine::domain::sql_ports`

#### `src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs`
- Module: `dag_engine::infrastructure::sql_llm_critic`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`
- Depends on (6): `dag_engine::domain::observer`, `dag_engine::domain::sql_errors`, `dag_engine::domain::sql_ports`, `dag_engine::infrastructure::nodes::util::billed_llm`, `llm::domain`, `llm::infrastructure`

#### `src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs`
- Module: `dag_engine::infrastructure::sql_pool_adapter`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_port_factory.rs`
- Depends on (2): `dag_engine::domain::sql_errors`, `dag_engine::domain::sql_ports`

#### `src/libs/colmena/src/dag_engine/infrastructure/sql_port_factory.rs`
- Module: `dag_engine::infrastructure::sql_port_factory`
- **Used by (6)**: `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/node_harness.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`, `src/libs/colmena/src/node_bindings/registry.rs`, `src/libs/colmena/src/python_bindings/mod.rs`
- Depends on (3): `dag_engine::domain::sql_errors`, `dag_engine::infrastructure::pool_registry`, `dag_engine::infrastructure::sql_pool_adapter`

#### `src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs`
- Module: `dag_engine::infrastructure::sql_static_validator`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`
- Depends on (3): `dag_engine::domain::sql_permissions`, `dag_engine::domain::sql_ports`, `dag_engine::infrastructure::sql_ast`

#### `src/libs/colmena/src/dag_engine/log_policy.rs`
- Module: `dag_engine::log_policy`
- **Used by (7)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs`, `src/libs/colmena/src/dag_engine/infrastructure/python_exec/mod.rs`, `src/libs/colmena/src/dag_engine/infrastructure/python_exec/remote.rs`, `src/libs/colmena/src/dag_engine/infrastructure/python_exec/server.rs`, `src/libs/colmena/src/dag_engine/infrastructure/python_exec/staging.rs`, `src/libs/colmena/src/dag_engine/infrastructure/python_exec/subprocess.rs`, `src/libs/colmena/src/tabular_run/local.rs`
- Depends on (1): `dag_engine::engine`

#### `src/libs/colmena/src/dag_engine/main.rs`
- Module: `(crate root)`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/mod.rs`
- Module: `dag_engine`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/dag_engine/sse_mapper.rs`
- Module: `dag_engine::sse_mapper`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/api.rs`, `src/libs/colmena/src/dag_engine/engine.rs`
- Depends on (2): `dag_engine::domain::events`, `dag_engine::frame_redaction`

#### `src/libs/colmena/src/dag_engine/verbose.rs`
- Module: `dag_engine::verbose`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

### documents

#### `src/libs/colmena/src/documents/application/apply_excel_ops.rs`
- Module: `documents::application::apply_excel_ops`
- **Used by (1)**: `src/libs/colmena/src/documents/application/artifact_ops.rs`
- Depends on (5): `documents::domain`, `documents::domain::artifact`, `documents::domain::ir`, `documents::domain::patch`, `documents::infrastructure::ids`

#### `src/libs/colmena/src/documents/application/apply_html_ops.rs`
- Module: `documents::application::apply_html_ops`
- **Used by (1)**: `src/libs/colmena/src/documents/application/artifact_ops.rs`
- Depends on (5): `documents::domain`, `documents::domain::artifact`, `documents::domain::ir::html`, `documents::domain::patch`, `documents::infrastructure::ids`

#### `src/libs/colmena/src/documents/application/apply_patch.rs`
- Module: `documents::application::apply_patch`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`, `src/libs/colmena/src/documents/application/runtime.rs`
- Depends on (12): `documents::application::artifact_ops`, `documents::application::create_document`, `documents::domain`, `documents::domain::artifact`, `documents::domain::ids`, `documents::domain::ir::html`, `documents::domain::patch`, `documents::domain::ports`, `documents::infrastructure::ids`, `documents::infrastructure::render`, `documents::infrastructure::storage`, `documents::infrastructure::validation`

#### `src/libs/colmena/src/documents/application/apply_word_ops.rs`
- Module: `documents::application::apply_word_ops`
- **Used by (1)**: `src/libs/colmena/src/documents/application/artifact_ops.rs`
- Depends on (5): `documents::domain`, `documents::domain::artifact`, `documents::domain::ir`, `documents::domain::patch`, `documents::infrastructure::ids`

#### `src/libs/colmena/src/documents/application/artifact_ops.rs`
- Module: `documents::application::artifact_ops`
- **Used by (2)**: `src/libs/colmena/src/documents/application/apply_patch.rs`, `src/libs/colmena/src/documents/application/create_document.rs`
- Depends on (9): `documents::application::apply_excel_ops`, `documents::application::apply_html_ops`, `documents::application::apply_word_ops`, `documents::domain`, `documents::domain::artifact`, `documents::domain::ids`, `documents::domain::ir`, `documents::domain::ir::html`, `documents::domain::patch`

#### `src/libs/colmena/src/documents/application/create_document.rs`
- Module: `documents::application::create_document`
- **Used by (4)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`, `src/libs/colmena/src/documents/application/apply_patch.rs`, `src/libs/colmena/src/documents/application/runtime.rs`
- Depends on (10): `documents::application::artifact_ops`, `documents::domain`, `documents::domain::artifact`, `documents::domain::ids`, `documents::domain::patch`, `documents::domain::ports`, `documents::infrastructure::ids`, `documents::infrastructure::render`, `documents::infrastructure::storage`, `documents::infrastructure::validation`

#### `src/libs/colmena/src/documents/application/delete_asset.rs`
- Module: `documents::application::delete_asset`
- **Used by (1)**: `src/libs/colmena/src/documents/application/runtime.rs`
- Depends on (5): `documents::domain::artifact`, `documents::domain::error`, `documents::domain::ids`, `documents::domain::ports`, `documents::infrastructure::storage`

#### `src/libs/colmena/src/documents/application/get_head.rs`
- Module: `documents::application::get_head`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`, `src/libs/colmena/src/documents/application/runtime.rs`
- Depends on (2): `documents::domain`, `documents::domain::ids`

#### `src/libs/colmena/src/documents/application/list_assets.rs`
- Module: `documents::application::list_assets`
- **Used by (1)**: `src/libs/colmena/src/documents/application/runtime.rs`
- Depends on (4): `documents::domain::error`, `documents::domain::ids`, `documents::domain::ports`, `documents::infrastructure::storage`

#### `src/libs/colmena/src/documents/application/list_versions.rs`
- Module: `documents::application::list_versions`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`, `src/libs/colmena/src/documents/application/runtime.rs`
- Depends on (2): `documents::domain`, `documents::domain::ids`

#### `src/libs/colmena/src/documents/application/mod.rs`
- Module: `documents::application`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/documents/application/read_document.rs`
- Module: `documents::application::read_document`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`, `src/libs/colmena/src/documents/application/runtime.rs`
- Depends on (2): `documents::domain`, `documents::domain::ids`

#### `src/libs/colmena/src/documents/application/rollback.rs`
- Module: `documents::application::rollback`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`, `src/libs/colmena/src/documents/application/runtime.rs`
- Depends on (4): `documents::domain`, `documents::domain::artifact`, `documents::domain::ids`, `documents::domain::patch`

#### `src/libs/colmena/src/documents/application/runtime.rs`
- Module: `documents::application::runtime`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (17): `documents::application::apply_patch`, `documents::application::create_document`, `documents::application::delete_asset`, `documents::application::get_head`, `documents::application::list_assets`, `documents::application::list_versions`, `documents::application::read_document`, `documents::application::rollback`, `documents::application::upload_asset`, `documents::domain`, `documents::domain::ids`, `documents::domain::patch`, `documents::domain::ports`, `documents::infrastructure::ids`, `documents::infrastructure::render`, `documents::infrastructure::storage`, `documents::infrastructure::validation`

#### `src/libs/colmena/src/documents/application/upload_asset.rs`
- Module: `documents::application::upload_asset`
- **Used by (1)**: `src/libs/colmena/src/documents/application/runtime.rs`
- Depends on (6): `documents::domain`, `documents::domain::error`, `documents::domain::ids`, `documents::domain::ports`, `documents::infrastructure::ids`, `documents::infrastructure::storage`

#### `src/libs/colmena/src/documents/domain/artifact.rs`
- Module: `documents::domain::artifact`
- **Used by (10)**: `src/libs/colmena/src/documents/application/apply_excel_ops.rs`, `src/libs/colmena/src/documents/application/apply_html_ops.rs`, `src/libs/colmena/src/documents/application/apply_patch.rs`, `src/libs/colmena/src/documents/application/apply_word_ops.rs`, `src/libs/colmena/src/documents/application/artifact_ops.rs`, `src/libs/colmena/src/documents/application/create_document.rs`, `src/libs/colmena/src/documents/application/delete_asset.rs`, `src/libs/colmena/src/documents/application/rollback.rs`, `src/libs/colmena/src/documents/infrastructure/storage/gcs_store.rs`, `src/libs/colmena/src/documents/infrastructure/storage/local_fs_store.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/documents/domain/error.rs`
- Module: `documents::domain::error`
- **Used by (5)**: `src/libs/colmena/src/documents/application/delete_asset.rs`, `src/libs/colmena/src/documents/application/list_assets.rs`, `src/libs/colmena/src/documents/application/upload_asset.rs`, `src/libs/colmena/src/documents/infrastructure/storage/gcs_asset_store.rs`, `src/libs/colmena/src/documents/infrastructure/storage/local_fs_asset_store.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/documents/domain/ids.rs`
- Module: `documents::domain::ids`
- **Used by (20)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`, `src/libs/colmena/src/documents/application/apply_patch.rs`, `src/libs/colmena/src/documents/application/artifact_ops.rs`, `src/libs/colmena/src/documents/application/create_document.rs`, `src/libs/colmena/src/documents/application/delete_asset.rs`, `src/libs/colmena/src/documents/application/get_head.rs`, `src/libs/colmena/src/documents/application/list_assets.rs`, `src/libs/colmena/src/documents/application/list_versions.rs`, `src/libs/colmena/src/documents/application/read_document.rs`, `src/libs/colmena/src/documents/application/rollback.rs`, `src/libs/colmena/src/documents/application/runtime.rs`, `src/libs/colmena/src/documents/application/upload_asset.rs`, `src/libs/colmena/src/documents/domain/ports.rs`, `src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs`, `src/libs/colmena/src/documents/infrastructure/storage/gcs_asset_store.rs`, `src/libs/colmena/src/documents/infrastructure/storage/gcs_store.rs`, `src/libs/colmena/src/documents/infrastructure/storage/local_fs_asset_store.rs`, `src/libs/colmena/src/documents/infrastructure/storage/local_fs_store.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/documents/domain/ir/common.rs`
- Module: `documents::domain::ir::common`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/documents/domain/ir/excel.rs`
- Module: `documents::domain::ir::excel`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/documents/domain/ir/html.rs`
- Module: `documents::domain::ir::html`
- **Used by (5)**: `src/libs/colmena/src/documents/application/apply_html_ops.rs`, `src/libs/colmena/src/documents/application/apply_patch.rs`, `src/libs/colmena/src/documents/application/artifact_ops.rs`, `src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs`, `src/libs/colmena/src/documents/infrastructure/validation/html_validator.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/documents/domain/ir/mod.rs`
- Module: `documents::domain::ir`
- **Used by (9)**: `src/libs/colmena/src/documents/application/apply_excel_ops.rs`, `src/libs/colmena/src/documents/application/apply_word_ops.rs`, `src/libs/colmena/src/documents/application/artifact_ops.rs`, `src/libs/colmena/src/documents/domain/patch.rs`, `src/libs/colmena/src/documents/infrastructure/render/excel_renderer.rs`, `src/libs/colmena/src/documents/infrastructure/render/word_renderer.rs`, `src/libs/colmena/src/documents/infrastructure/validation/excel_validator.rs`, `src/libs/colmena/src/documents/infrastructure/validation/html_validator.rs`, `src/libs/colmena/src/documents/infrastructure/validation/word_validator.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/documents/domain/ir/word.rs`
- Module: `documents::domain::ir::word`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/documents/domain/mod.rs`
- Module: `documents::domain`
- **Used by (23)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`, `src/libs/colmena/src/documents/application/apply_excel_ops.rs`, `src/libs/colmena/src/documents/application/apply_html_ops.rs`, `src/libs/colmena/src/documents/application/apply_patch.rs`, `src/libs/colmena/src/documents/application/apply_word_ops.rs`, `src/libs/colmena/src/documents/application/artifact_ops.rs`, `src/libs/colmena/src/documents/application/create_document.rs`, `src/libs/colmena/src/documents/application/get_head.rs`, `src/libs/colmena/src/documents/application/list_versions.rs`, `src/libs/colmena/src/documents/application/read_document.rs`, `src/libs/colmena/src/documents/application/rollback.rs`, `src/libs/colmena/src/documents/application/runtime.rs`, `src/libs/colmena/src/documents/application/upload_asset.rs`, `src/libs/colmena/src/documents/infrastructure/ids.rs`, `src/libs/colmena/src/documents/infrastructure/render/excel_renderer.rs`, `src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs`, `src/libs/colmena/src/documents/infrastructure/render/word_renderer.rs`, `src/libs/colmena/src/documents/infrastructure/storage/gcs_store.rs`, `src/libs/colmena/src/documents/infrastructure/storage/local_fs_store.rs`, `src/libs/colmena/src/documents/infrastructure/validation/excel_validator.rs`, `src/libs/colmena/src/documents/infrastructure/validation/html_validator.rs`, `src/libs/colmena/src/documents/infrastructure/validation/word_validator.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/documents/domain/patch.rs`
- Module: `documents::domain::patch`
- **Used by (10)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`, `src/libs/colmena/src/documents/application/apply_excel_ops.rs`, `src/libs/colmena/src/documents/application/apply_html_ops.rs`, `src/libs/colmena/src/documents/application/apply_patch.rs`, `src/libs/colmena/src/documents/application/apply_word_ops.rs`, `src/libs/colmena/src/documents/application/artifact_ops.rs`, `src/libs/colmena/src/documents/application/create_document.rs`, `src/libs/colmena/src/documents/application/rollback.rs`, `src/libs/colmena/src/documents/application/runtime.rs`
- Depends on (1): `documents::domain::ir`

#### `src/libs/colmena/src/documents/domain/ports.rs`
- Module: `documents::domain::ports`
- **Used by (11)**: `src/libs/colmena/src/documents/application/apply_patch.rs`, `src/libs/colmena/src/documents/application/create_document.rs`, `src/libs/colmena/src/documents/application/delete_asset.rs`, `src/libs/colmena/src/documents/application/list_assets.rs`, `src/libs/colmena/src/documents/application/runtime.rs`, `src/libs/colmena/src/documents/application/upload_asset.rs`, `src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs`, `src/libs/colmena/src/documents/infrastructure/storage/gcs_asset_store.rs`, `src/libs/colmena/src/documents/infrastructure/storage/gcs_store.rs`, `src/libs/colmena/src/documents/infrastructure/storage/local_fs_asset_store.rs`, `src/libs/colmena/src/documents/infrastructure/storage/local_fs_store.rs`
- Depends on (1): `documents::domain::ids`

#### `src/libs/colmena/src/documents/infrastructure/ids.rs`
- Module: `documents::infrastructure::ids`
- **Used by (7)**: `src/libs/colmena/src/documents/application/apply_excel_ops.rs`, `src/libs/colmena/src/documents/application/apply_html_ops.rs`, `src/libs/colmena/src/documents/application/apply_patch.rs`, `src/libs/colmena/src/documents/application/apply_word_ops.rs`, `src/libs/colmena/src/documents/application/create_document.rs`, `src/libs/colmena/src/documents/application/runtime.rs`, `src/libs/colmena/src/documents/application/upload_asset.rs`
- Depends on (1): `documents::domain`

#### `src/libs/colmena/src/documents/infrastructure/mod.rs`
- Module: `documents::infrastructure`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/documents/infrastructure/render/excel_renderer.rs`
- Module: `documents::infrastructure::render::excel_renderer`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `documents::domain`, `documents::domain::ir`

#### `src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs`
- Module: `documents::infrastructure::render::html_renderer`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (5): `documents::domain`, `documents::domain::ids`, `documents::domain::ir::html`, `documents::domain::ports`, `documents::infrastructure::storage`

#### `src/libs/colmena/src/documents/infrastructure/render/mod.rs`
- Module: `documents::infrastructure::render`
- **Used by (3)**: `src/libs/colmena/src/documents/application/apply_patch.rs`, `src/libs/colmena/src/documents/application/create_document.rs`, `src/libs/colmena/src/documents/application/runtime.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/documents/infrastructure/render/word_renderer.rs`
- Module: `documents::infrastructure::render::word_renderer`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `documents::domain`, `documents::domain::ir`

#### `src/libs/colmena/src/documents/infrastructure/storage/gcs_asset_store.rs`
- Module: `documents::infrastructure::storage::gcs_asset_store`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `documents::domain::error`, `documents::domain::ids`, `documents::domain::ports`

#### `src/libs/colmena/src/documents/infrastructure/storage/gcs_store.rs`
- Module: `documents::infrastructure::storage::gcs_store`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (4): `documents::domain`, `documents::domain::artifact`, `documents::domain::ids`, `documents::domain::ports`

#### `src/libs/colmena/src/documents/infrastructure/storage/local_fs_asset_store.rs`
- Module: `documents::infrastructure::storage::local_fs_asset_store`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `documents::domain::error`, `documents::domain::ids`, `documents::domain::ports`

#### `src/libs/colmena/src/documents/infrastructure/storage/local_fs_store.rs`
- Module: `documents::infrastructure::storage::local_fs_store`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (4): `documents::domain`, `documents::domain::artifact`, `documents::domain::ids`, `documents::domain::ports`

#### `src/libs/colmena/src/documents/infrastructure/storage/mod.rs`
- Module: `documents::infrastructure::storage`
- **Used by (7)**: `src/libs/colmena/src/documents/application/apply_patch.rs`, `src/libs/colmena/src/documents/application/create_document.rs`, `src/libs/colmena/src/documents/application/delete_asset.rs`, `src/libs/colmena/src/documents/application/list_assets.rs`, `src/libs/colmena/src/documents/application/runtime.rs`, `src/libs/colmena/src/documents/application/upload_asset.rs`, `src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/documents/infrastructure/validation/excel_validator.rs`
- Module: `documents::infrastructure::validation::excel_validator`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `documents::domain`, `documents::domain::ir`

#### `src/libs/colmena/src/documents/infrastructure/validation/html_validator.rs`
- Module: `documents::infrastructure::validation::html_validator`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `documents::domain`, `documents::domain::ir`, `documents::domain::ir::html`

#### `src/libs/colmena/src/documents/infrastructure/validation/mod.rs`
- Module: `documents::infrastructure::validation`
- **Used by (3)**: `src/libs/colmena/src/documents/application/apply_patch.rs`, `src/libs/colmena/src/documents/application/create_document.rs`, `src/libs/colmena/src/documents/application/runtime.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/documents/infrastructure/validation/word_validator.rs`
- Module: `documents::infrastructure::validation::word_validator`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `documents::domain`, `documents::domain::ir`

#### `src/libs/colmena/src/documents/mod.rs`
- Module: `documents`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

### gdocs

#### `src/libs/colmena/src/gdocs/application/_test_helpers.rs`
- Module: `gdocs::application::_test_helpers`
- **Used by (10)**: `src/libs/colmena/src/gdocs/application/apply_edits.rs`, `src/libs/colmena/src/gdocs/application/co_edit_guard.rs`, `src/libs/colmena/src/gdocs/application/delete_text.rs`, `src/libs/colmena/src/gdocs/application/insert.rs`, `src/libs/colmena/src/gdocs/application/named_range.rs`, `src/libs/colmena/src/gdocs/application/replace_section.rs`, `src/libs/colmena/src/gdocs/application/replace_text.rs`, `src/libs/colmena/src/gdocs/application/style.rs`, `src/libs/colmena/src/gdocs/application/table.rs`, `src/libs/colmena/src/gdocs/application/table_format.rs`
- Depends on (4): `gdocs::domain`, `gdocs::domain::traits`, `gdocs::infrastructure::outline_cache`, `gdocs::infrastructure::revision_store`

#### `src/libs/colmena/src/gdocs/application/apply_edits.rs`
- Module: `gdocs::application::apply_edits`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (5): `dag_engine::infrastructure::nodes::llm_synthetic_tools::markdown_to_docs_ops`, `gdocs::application::_test_helpers`, `gdocs::application::co_edit_guard`, `gdocs::application::insert`, `gdocs::domain`

#### `src/libs/colmena/src/gdocs/application/co_edit_guard.rs`
- Module: `gdocs::application::co_edit_guard`
- **Used by (9)**: `src/libs/colmena/src/gdocs/application/apply_edits.rs`, `src/libs/colmena/src/gdocs/application/delete_text.rs`, `src/libs/colmena/src/gdocs/application/insert.rs`, `src/libs/colmena/src/gdocs/application/named_range.rs`, `src/libs/colmena/src/gdocs/application/replace_section.rs`, `src/libs/colmena/src/gdocs/application/replace_text.rs`, `src/libs/colmena/src/gdocs/application/style.rs`, `src/libs/colmena/src/gdocs/application/table.rs`, `src/libs/colmena/src/gdocs/application/table_format.rs`
- Depends on (6): `gdocs::application::_test_helpers`, `gdocs::application::diff`, `gdocs::application::scope_resolver`, `gdocs::domain`, `gdocs::infrastructure::outline_cache`, `gdocs::infrastructure::revision_store`

#### `src/libs/colmena/src/gdocs/application/delete_text.rs`
- Module: `gdocs::application::delete_text`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `gdocs::application::_test_helpers`, `gdocs::application::co_edit_guard`, `gdocs::domain`

#### `src/libs/colmena/src/gdocs/application/diff.rs`
- Module: `gdocs::application::diff`
- **Used by (1)**: `src/libs/colmena/src/gdocs/application/co_edit_guard.rs`
- Depends on (1): `gdocs::domain`

#### `src/libs/colmena/src/gdocs/application/insert.rs`
- Module: `gdocs::application::insert`
- **Used by (4)**: `src/libs/colmena/src/gdocs/application/apply_edits.rs`, `src/libs/colmena/src/gdocs/application/replace_section.rs`, `src/libs/colmena/src/gdocs/application/table.rs`, `src/libs/colmena/src/gdocs/application/table_format.rs`
- Depends on (4): `dag_engine::infrastructure::nodes::llm_synthetic_tools::markdown_to_docs_ops`, `gdocs::application::_test_helpers`, `gdocs::application::co_edit_guard`, `gdocs::domain`

#### `src/libs/colmena/src/gdocs/application/mod.rs`
- Module: `gdocs::application`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/gdocs/application/named_range.rs`
- Module: `gdocs::application::named_range`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `gdocs::application::_test_helpers`, `gdocs::application::co_edit_guard`, `gdocs::domain`

#### `src/libs/colmena/src/gdocs/application/replace_section.rs`
- Module: `gdocs::application::replace_section`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (5): `dag_engine::infrastructure::nodes::llm_synthetic_tools::markdown_to_docs_ops`, `gdocs::application::_test_helpers`, `gdocs::application::co_edit_guard`, `gdocs::application::insert`, `gdocs::domain`

#### `src/libs/colmena/src/gdocs/application/replace_text.rs`
- Module: `gdocs::application::replace_text`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `gdocs::application::_test_helpers`, `gdocs::application::co_edit_guard`, `gdocs::domain`

#### `src/libs/colmena/src/gdocs/application/scope_resolver.rs`
- Module: `gdocs::application::scope_resolver`
- **Used by (1)**: `src/libs/colmena/src/gdocs/application/co_edit_guard.rs`
- Depends on (1): `gdocs::domain`

#### `src/libs/colmena/src/gdocs/application/style.rs`
- Module: `gdocs::application::style`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `gdocs::application::_test_helpers`, `gdocs::application::co_edit_guard`, `gdocs::domain`

#### `src/libs/colmena/src/gdocs/application/table.rs`
- Module: `gdocs::application::table`
- **Used by (1)**: `src/libs/colmena/src/gdocs/application/table_format.rs`
- Depends on (6): `gdocs::application::_test_helpers`, `gdocs::application::co_edit_guard`, `gdocs::application::insert`, `gdocs::application::util`, `gdocs::domain`, `gdocs::domain::traits`

#### `src/libs/colmena/src/gdocs/application/table_format.rs`
- Module: `gdocs::application::table_format`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`
- Depends on (6): `gdocs::application::_test_helpers`, `gdocs::application::co_edit_guard`, `gdocs::application::insert`, `gdocs::application::table`, `gdocs::application::util`, `gdocs::domain`

#### `src/libs/colmena/src/gdocs/application/util.rs`
- Module: `gdocs::application::util`
- **Used by (2)**: `src/libs/colmena/src/gdocs/application/table.rs`, `src/libs/colmena/src/gdocs/application/table_format.rs`
- Depends on (1): `gdocs::domain`

#### `src/libs/colmena/src/gdocs/domain/errors.rs`
- Module: `gdocs::domain::errors`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `gdocs::domain::types`

#### `src/libs/colmena/src/gdocs/domain/mod.rs`
- Module: `gdocs::domain`
- **Used by (21)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/markdown_to_docs_ops.rs`, `src/libs/colmena/src/gdocs/application/_test_helpers.rs`, `src/libs/colmena/src/gdocs/application/apply_edits.rs`, `src/libs/colmena/src/gdocs/application/co_edit_guard.rs`, `src/libs/colmena/src/gdocs/application/delete_text.rs`, `src/libs/colmena/src/gdocs/application/diff.rs`, `src/libs/colmena/src/gdocs/application/insert.rs`, `src/libs/colmena/src/gdocs/application/named_range.rs`, `src/libs/colmena/src/gdocs/application/replace_section.rs`, `src/libs/colmena/src/gdocs/application/replace_text.rs`, `src/libs/colmena/src/gdocs/application/scope_resolver.rs`, `src/libs/colmena/src/gdocs/application/style.rs`, `src/libs/colmena/src/gdocs/application/table.rs`, `src/libs/colmena/src/gdocs/application/table_format.rs`, `src/libs/colmena/src/gdocs/application/util.rs`, `src/libs/colmena/src/gdocs/domain/traits.rs`, `src/libs/colmena/src/gdocs/infrastructure/auth.rs`, `src/libs/colmena/src/gdocs/infrastructure/http_client.rs`, `src/libs/colmena/src/gdocs/infrastructure/outline_cache.rs`, `src/libs/colmena/src/gdocs/infrastructure/revision_store.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/gdocs/domain/traits.rs`
- Module: `gdocs::domain::traits`
- **Used by (2)**: `src/libs/colmena/src/gdocs/application/_test_helpers.rs`, `src/libs/colmena/src/gdocs/application/table.rs`
- Depends on (1): `gdocs::domain`

#### `src/libs/colmena/src/gdocs/domain/types.rs`
- Module: `gdocs::domain::types`
- **Used by (2)**: `src/libs/colmena/src/gdocs/domain/errors.rs`, `src/libs/colmena/src/gdocs/infrastructure/http_client.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/gdocs/infrastructure/auth.rs`
- Module: `gdocs::infrastructure::auth`
- **Used by (1)**: `src/libs/colmena/src/gdocs/infrastructure/http_client.rs`
- Depends on (3): `gdocs::domain`, `google_oauth::domain`, `google_oauth::infrastructure`

#### `src/libs/colmena/src/gdocs/infrastructure/config.rs`
- Module: `gdocs::infrastructure::config`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`, `src/libs/colmena/src/gdocs/infrastructure/http_client.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/gdocs/infrastructure/http_client.rs`
- Module: `gdocs::infrastructure::http_client`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`
- Depends on (6): `dag_engine::application::ports`, `gdocs::domain`, `gdocs::domain::types`, `gdocs::infrastructure::auth`, `gdocs::infrastructure::config`, `google_oauth::infrastructure`

#### `src/libs/colmena/src/gdocs/infrastructure/mod.rs`
- Module: `gdocs::infrastructure`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/gdocs/infrastructure/outline_cache.rs`
- Module: `gdocs::infrastructure::outline_cache`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`, `src/libs/colmena/src/gdocs/application/_test_helpers.rs`, `src/libs/colmena/src/gdocs/application/co_edit_guard.rs`
- Depends on (1): `gdocs::domain`

#### `src/libs/colmena/src/gdocs/infrastructure/revision_store.rs`
- Module: `gdocs::infrastructure::revision_store`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`, `src/libs/colmena/src/gdocs/application/_test_helpers.rs`, `src/libs/colmena/src/gdocs/application/co_edit_guard.rs`
- Depends on (1): `gdocs::domain`

#### `src/libs/colmena/src/gdocs/mod.rs`
- Module: `gdocs`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

### google_oauth

#### `src/libs/colmena/src/google_oauth/domain/errors.rs`
- Module: `google_oauth::domain::errors`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/google_oauth/domain/mod.rs`
- Module: `google_oauth::domain`
- **Used by (11)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http_oauth.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/bind.rs`, `src/libs/colmena/src/gdocs/infrastructure/auth.rs`, `src/libs/colmena/src/google_oauth/domain/traits.rs`, `src/libs/colmena/src/google_oauth/infrastructure/config.rs`, `src/libs/colmena/src/google_oauth/infrastructure/host_refresh_provider.rs`, `src/libs/colmena/src/google_oauth/infrastructure/refresh_client.rs`, `src/libs/colmena/src/google_oauth/infrastructure/token_provider.rs`, `src/libs/colmena/src/google_oauth/infrastructure/workspace_auth.rs`, `src/libs/colmena/src/gsheets/infrastructure/auth.rs`, `src/libs/colmena/src/llm/infrastructure/mcp_client/rmcp_http_client.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/google_oauth/domain/traits.rs`
- Module: `google_oauth::domain::traits`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `google_oauth::domain`

#### `src/libs/colmena/src/google_oauth/domain/types.rs`
- Module: `google_oauth::domain::types`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/google_oauth/infrastructure/config.rs`
- Module: `google_oauth::infrastructure::config`
- **Used by (2)**: `src/libs/colmena/src/google_oauth/infrastructure/refresh_client.rs`, `src/libs/colmena/src/google_oauth/infrastructure/token_provider.rs`
- Depends on (1): `google_oauth::domain`

#### `src/libs/colmena/src/google_oauth/infrastructure/host_refresh_provider.rs`
- Module: `google_oauth::infrastructure::host_refresh_provider`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `dag_engine::application::ports`, `google_oauth::domain`

#### `src/libs/colmena/src/google_oauth/infrastructure/mod.rs`
- Module: `google_oauth::infrastructure`
- **Used by (13)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/http_oauth.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/bind.rs`, `src/libs/colmena/src/gdocs/infrastructure/auth.rs`, `src/libs/colmena/src/gdocs/infrastructure/http_client.rs`, `src/libs/colmena/src/google_oauth/infrastructure/provider_cache.rs`, `src/libs/colmena/src/gsheets/infrastructure/auth.rs`, `src/libs/colmena/src/gsheets/infrastructure/http_client.rs`, `src/libs/colmena/src/llm/infrastructure/mcp_client/rmcp_http_client.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/google_oauth/infrastructure/provider_cache.rs`
- Module: `google_oauth::infrastructure::provider_cache`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `google_oauth::infrastructure`

#### `src/libs/colmena/src/google_oauth/infrastructure/refresh_client.rs`
- Module: `google_oauth::infrastructure::refresh_client`
- **Used by (1)**: `src/libs/colmena/src/google_oauth/infrastructure/token_provider.rs`
- Depends on (2): `google_oauth::domain`, `google_oauth::infrastructure::config`

#### `src/libs/colmena/src/google_oauth/infrastructure/token_provider.rs`
- Module: `google_oauth::infrastructure::token_provider`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `google_oauth::domain`, `google_oauth::infrastructure::config`, `google_oauth::infrastructure::refresh_client`

#### `src/libs/colmena/src/google_oauth/infrastructure/workspace_auth.rs`
- Module: `google_oauth::infrastructure::workspace_auth`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `dag_engine::application::ports`, `google_oauth::domain`

#### `src/libs/colmena/src/google_oauth/mod.rs`
- Module: `google_oauth`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`
- Depends on (0): — (no intra-crate imports)

### gsheets

#### `src/libs/colmena/src/gsheets/application/format.rs`
- Module: `gsheets::application::format`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/gsheets/application/mod.rs`
- Module: `gsheets::application`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/gsheets/domain/errors.rs`
- Module: `gsheets::domain::errors`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/gsheets/domain/mod.rs`
- Module: `gsheets::domain`
- **Used by (8)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/data_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/sheet_writer.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tabular_bindings.rs`, `src/libs/colmena/src/gsheets/domain/traits.rs`, `src/libs/colmena/src/gsheets/infrastructure/auth.rs`, `src/libs/colmena/src/gsheets/infrastructure/http_client.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/gsheets/domain/traits.rs`
- Module: `gsheets::domain::traits`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `gsheets::domain`

#### `src/libs/colmena/src/gsheets/domain/types.rs`
- Module: `gsheets::domain::types`
- **Used by (1)**: `src/libs/colmena/src/gsheets/infrastructure/http_client.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/gsheets/infrastructure/auth.rs`
- Module: `gsheets::infrastructure::auth`
- **Used by (1)**: `src/libs/colmena/src/gsheets/infrastructure/http_client.rs`
- Depends on (3): `google_oauth::domain`, `google_oauth::infrastructure`, `gsheets::domain`

#### `src/libs/colmena/src/gsheets/infrastructure/config.rs`
- Module: `gsheets::infrastructure::config`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/data_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs`, `src/libs/colmena/src/gsheets/infrastructure/http_client.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/gsheets/infrastructure/http_client.rs`
- Module: `gsheets::infrastructure::http_client`
- **Used by (5)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/data_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/table_writer.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tabular_bindings.rs`
- Depends on (7): `dag_engine::application::ports`, `google_oauth::infrastructure`, `gsheets::domain`, `gsheets::domain::types`, `gsheets::infrastructure::auth`, `gsheets::infrastructure::config`, `gsheets::infrastructure::merge_fill`

#### `src/libs/colmena/src/gsheets/infrastructure/merge_fill.rs`
- Module: `gsheets::infrastructure::merge_fill`
- **Used by (1)**: `src/libs/colmena/src/gsheets/infrastructure/http_client.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/gsheets/infrastructure/mod.rs`
- Module: `gsheets::infrastructure`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/gsheets/mod.rs`
- Module: `gsheets`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

### lib.rs

#### `src/libs/colmena/src/lib.rs`
- Module: `(crate root)`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

### llm

#### `src/libs/colmena/src/llm/application/agent_service.rs`
- Module: `llm::application::agent_service`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
- Depends on (4): `llm::application::tool_batches`, `llm::domain`, `llm::domain::call_cancels`, `llm::domain::steering`

#### `src/libs/colmena/src/llm/application/attachment_catalog.rs`
- Module: `llm::application::attachment_catalog`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
- Depends on (2): `llm::domain`, `llm::domain::attachments`

#### `src/libs/colmena/src/llm/application/history_compaction.rs`
- Module: `llm::application::history_compaction`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (4): `llm::application::tool_digest`, `llm::domain`, `llm::domain::tools`, `llm::infrastructure::persistence`

#### `src/libs/colmena/src/llm/application/llm_call_use_case.rs`
- Module: `llm::application::llm_call_use_case`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `llm::domain`, `llm::infrastructure::files`

#### `src/libs/colmena/src/llm/application/llm_health_check_use_case.rs`
- Module: `llm::application::llm_health_check_use_case`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/application/llm_stream_use_case.rs`
- Module: `llm::application::llm_stream_use_case`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/application/load_failure.rs`
- Module: `llm::application::load_failure`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain::large_tabular`

#### `src/libs/colmena/src/llm/application/mod.rs`
- Module: `llm::application`
- **Used by (8)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/critic.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/characterisation.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/large_files.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/load_attachment_redirect.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/planner.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/reactor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/extract_with_schema.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/application/tool_batches.rs`
- Module: `llm::application::tool_batches`
- **Used by (1)**: `src/libs/colmena/src/llm/application/agent_service.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/application/tool_digest.rs`
- Module: `llm::application::tool_digest`
- **Used by (1)**: `src/libs/colmena/src/llm/application/history_compaction.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/domain/attachments/attachment_error.rs`
- Module: `llm::domain::attachments::attachment_error`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs`
- Module: `llm::domain::attachments::attachment_registry`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor/large_object_guard.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/load_attachment_redirect.rs`
- Depends on (2): `llm::domain`, `llm::domain::attachments`

#### `src/libs/colmena/src/llm/domain/attachments/auto_id.rs`
- Module: `llm::domain::attachments::auto_id`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain::attachments`

#### `src/libs/colmena/src/llm/domain/attachments/conversation_attachment.rs`
- Module: `llm::domain::attachments::conversation_attachment`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/domain/attachments/mod.rs`
- Module: `llm::domain::attachments`
- **Used by (22)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor/large_object_guard.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/characterisation.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/large_files.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/load_attachment_redirect.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/storage_ref_turn.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python/large.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/session_attachment.rs`, `src/libs/colmena/src/llm/application/attachment_catalog.rs`, `src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs`, `src/libs/colmena/src/llm/domain/attachments/auto_id.rs`, `src/libs/colmena/src/llm/domain/attachments/stream_resolver.rs`, `src/libs/colmena/src/llm/infrastructure/attachment_summary/byte_acquisition.rs`, `src/libs/colmena/src/llm/infrastructure/attachment_summary/llm_summary_generator.rs`, `src/libs/colmena/src/llm/infrastructure/attachments/stream_resolver_impl.rs`, `src/libs/colmena/src/llm/infrastructure/persistence/ownership_contract.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/domain/attachments/stream_resolver.rs`
- Module: `llm::domain::attachments::stream_resolver`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `llm::domain::attachments`, `storage::domain`, `storage::domain::storage_error`

#### `src/libs/colmena/src/llm/domain/attachments/summary_generator.rs`
- Module: `llm::domain::attachments::summary_generator`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/domain/call_cancels.rs`
- Module: `llm::domain::call_cancels`
- **Used by (5)**: `src/libs/colmena/src/dag_engine/application/run_control.rs`, `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs`, `src/libs/colmena/src/llm/application/agent_service.rs`
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/domain/decision_model.rs`
- Module: `llm::domain::decision_model`
- **Used by (2)**: `src/libs/colmena/src/llm/domain/decision_model_repository.rs`, `src/libs/colmena/src/llm/infrastructure/typesafe_jev_adapter.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/domain/decision_model_repository.rs`
- Module: `llm::domain::decision_model_repository`
- **Used by (2)**: `src/libs/colmena/src/llm/infrastructure/decision_model_provider_factory.rs`, `src/libs/colmena/src/llm/infrastructure/typesafe_jev_adapter.rs`
- Depends on (1): `llm::domain::decision_model`

#### `src/libs/colmena/src/llm/domain/file_cache_repository.rs`
- Module: `llm::domain::file_cache_repository`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/domain/file_provider_factory_port.rs`
- Module: `llm::domain::file_provider_factory_port`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/domain/file_provider_repository.rs`
- Module: `llm::domain::file_provider_repository`
- **Used by (1)**: `src/libs/colmena/src/llm/infrastructure/attachment_summary/byte_acquisition.rs`
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/domain/large_tabular.rs`
- Module: `llm::domain::large_tabular`
- **Used by (9)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor/large_object_guard.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/load_attachment_redirect.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/storage_ref_turn.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/whole_read_guard.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/llm/application/load_failure.rs`, `src/libs/colmena/src/llm/infrastructure/attachments/stream_resolver_impl.rs`, `src/libs/colmena/src/tabular_run/refusal.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/domain/llm_config.rs`
- Module: `llm::domain::llm_config`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/domain/llm_error.rs`
- Module: `llm::domain::llm_error`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/domain/llm_message.rs`
- Module: `llm::domain::llm_message`
- **Used by (1)**: `src/libs/colmena/src/llm/domain/llm_request.rs`
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/domain/llm_provider.rs`
- Module: `llm::domain::llm_provider`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/domain/llm_repository.rs`
- Module: `llm::domain::llm_repository`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/domain/llm_request.rs`
- Module: `llm::domain::llm_request`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `llm::domain`, `llm::domain::llm_message`, `llm::domain::tools`

#### `src/libs/colmena/src/llm/domain/llm_response.rs`
- Module: `llm::domain::llm_response`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/domain/mcp.rs`
- Module: `llm::domain::mcp`
- **Used by (9)**: `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs`, `src/libs/colmena/src/dag_engine/infrastructure/mcp_registry/key.rs`, `src/libs/colmena/src/dag_engine/infrastructure/mcp_registry/registry.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/bind.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/contain.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/dispatch.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/expose.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/wire.rs`, `src/libs/colmena/src/llm/infrastructure/mcp_client/rmcp_http_client.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/domain/memory.rs`
- Module: `llm::domain::memory`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/domain/message_summarizer.rs`
- Module: `llm::domain::message_summarizer`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/domain/mod.rs`
- Module: `llm::domain`
- **Used by (98)**: `src/libs/colmena/src/dag_engine/application/preflight.rs`, `src/libs/colmena/src/dag_engine/application/preflight_cache.rs`, `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs`, `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor/large_object_guard.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/critic.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/extraction.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/characterisation.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/large_files.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/load_attachment_redirect.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/node_harness.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/storage_key_entries.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/storage_ref_turn.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python/large.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/lazy_tools_catalog.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/list_threads.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/recall_history.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/sql_bulk_tools.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/output_parser.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/planner.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/reactor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/decision_model.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/extract_and_route.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/llm_direct.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/node.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/billed_llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/extract_with_schema.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/session_attachment.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs`, `src/libs/colmena/src/llm/application/agent_service.rs`, `src/libs/colmena/src/llm/application/attachment_catalog.rs`, `src/libs/colmena/src/llm/application/history_compaction.rs`, `src/libs/colmena/src/llm/application/llm_call_use_case.rs`, `src/libs/colmena/src/llm/application/llm_health_check_use_case.rs`, `src/libs/colmena/src/llm/application/llm_stream_use_case.rs`, `src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs`, `src/libs/colmena/src/llm/domain/attachments/conversation_attachment.rs`, `src/libs/colmena/src/llm/domain/attachments/summary_generator.rs`, `src/libs/colmena/src/llm/domain/call_cancels.rs`, `src/libs/colmena/src/llm/domain/file_cache_repository.rs`, `src/libs/colmena/src/llm/domain/file_provider_factory_port.rs`, `src/libs/colmena/src/llm/domain/file_provider_repository.rs`, `src/libs/colmena/src/llm/domain/llm_config.rs`, `src/libs/colmena/src/llm/domain/llm_message.rs`, `src/libs/colmena/src/llm/domain/llm_provider.rs`, `src/libs/colmena/src/llm/domain/llm_repository.rs`, `src/libs/colmena/src/llm/domain/llm_request.rs`, `src/libs/colmena/src/llm/domain/llm_response.rs`, `src/libs/colmena/src/llm/domain/memory.rs`, `src/libs/colmena/src/llm/domain/message_summarizer.rs`, `src/libs/colmena/src/llm/domain/signed_url_fetcher.rs`, `src/libs/colmena/src/llm/domain/tool_executor.rs`, `src/libs/colmena/src/llm/infrastructure/anthropic_adapter.rs`, `src/libs/colmena/src/llm/infrastructure/attachment_summary/byte_acquisition.rs`, `src/libs/colmena/src/llm/infrastructure/attachment_summary/cheap_tier.rs`, `src/libs/colmena/src/llm/infrastructure/attachment_summary/llm_summary_generator.rs`, `src/libs/colmena/src/llm/infrastructure/attachments/stream_resolver_impl.rs`, `src/libs/colmena/src/llm/infrastructure/cheap_models.rs`, `src/libs/colmena/src/llm/infrastructure/files/anthropic_files_api.rs`, `src/libs/colmena/src/llm/infrastructure/files/file_provider_factory.rs`, `src/libs/colmena/src/llm/infrastructure/files/gemini_files_api.rs`, `src/libs/colmena/src/llm/infrastructure/files/openai_files_api.rs`, `src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs`, `src/libs/colmena/src/llm/infrastructure/files/signed_url_downloader.rs`, `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs`, `src/libs/colmena/src/llm/infrastructure/llm_provider_factory.rs`, `src/libs/colmena/src/llm/infrastructure/message_summarizer/llm_message_summarizer.rs`, `src/libs/colmena/src/llm/infrastructure/mock_adapter.rs`, `src/libs/colmena/src/llm/infrastructure/openai_adapter.rs`, `src/libs/colmena/src/llm/infrastructure/persistence/hydration.rs`, `src/libs/colmena/src/llm/infrastructure/persistence/in_memory_conversation_repository.rs`, `src/libs/colmena/src/llm/infrastructure/persistence/ownership_contract.rs`, `src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs`, `src/libs/colmena/src/llm/infrastructure/persistence/postgres_conversation_repository.rs`, `src/libs/colmena/src/llm/infrastructure/persistence/repository_factory.rs`, `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs`, `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_conversation_repository.rs`, `src/libs/colmena/src/llm/infrastructure/scripted_adapter.rs`, `src/libs/colmena/src/llm/infrastructure/transient.rs`, `src/libs/colmena/src/node_bindings/llm.rs`, `src/libs/colmena/src/python_bindings/mod.rs`, `src/libs/colmena/src/shared/infrastructure/config_resolver.rs`, `src/libs/colmena/src/web/infrastructure/openapi_adapter.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/domain/signed_url_fetcher.rs`
- Module: `llm::domain::signed_url_fetcher`
- **Used by (1)**: `src/libs/colmena/src/llm/infrastructure/attachment_summary/byte_acquisition.rs`
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/domain/steering.rs`
- Module: `llm::domain::steering`
- **Used by (4)**: `src/libs/colmena/src/dag_engine/application/run_control.rs`, `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/llm/application/agent_service.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/domain/text_bounds.rs`
- Module: `llm::domain::text_bounds`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/contain.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/expose.rs`, `src/libs/colmena/src/llm/infrastructure/mcp_client/rmcp_http_client.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/domain/tool_executor.rs`
- Module: `llm::domain::tool_executor`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/domain/tools.rs`
- Module: `llm::domain::tools`
- **Used by (25)**: `src/libs/colmena/src/dag_engine/domain/toolkit_node.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/echo_toolkit.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/data_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/lazy_tools_catalog.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/list_threads.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/expose.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/wire.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/recall_history.rs`, `src/libs/colmena/src/llm/application/history_compaction.rs`, `src/libs/colmena/src/llm/domain/llm_request.rs`, `src/libs/colmena/src/llm/infrastructure/anthropic_adapter.rs`, `src/libs/colmena/src/llm/infrastructure/scripted_adapter.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/domain/tts.rs`
- Module: `llm::domain::tts`
- **Used by (5)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`, `src/libs/colmena/src/llm/domain/tts_repository.rs`, `src/libs/colmena/src/llm/infrastructure/elevenlabs_tts_adapter.rs`, `src/libs/colmena/src/llm/infrastructure/google_tts_adapter.rs`, `src/libs/colmena/src/llm/infrastructure/openai_tts_adapter.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/domain/tts_repository.rs`
- Module: `llm::domain::tts_repository`
- **Used by (5)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`, `src/libs/colmena/src/llm/infrastructure/elevenlabs_tts_adapter.rs`, `src/libs/colmena/src/llm/infrastructure/google_tts_adapter.rs`, `src/libs/colmena/src/llm/infrastructure/openai_tts_adapter.rs`, `src/libs/colmena/src/llm/infrastructure/tts_provider_factory.rs`
- Depends on (1): `llm::domain::tts`

#### `src/libs/colmena/src/llm/domain/value_objects/llm_request_id.rs`
- Module: `llm::domain::value_objects::llm_request_id`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/domain/value_objects/llm_response_id.rs`
- Module: `llm::domain::value_objects::llm_response_id`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/domain/value_objects/mod.rs`
- Module: `llm::domain::value_objects`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/infrastructure/anthropic_adapter.rs`
- Module: `llm::infrastructure::anthropic_adapter`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `llm::domain`, `llm::domain::tools`, `llm::infrastructure::transient`

#### `src/libs/colmena/src/llm/infrastructure/attachment_summary/byte_acquisition.rs`
- Module: `llm::infrastructure::attachment_summary::byte_acquisition`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (4): `llm::domain`, `llm::domain::attachments`, `llm::domain::file_provider_repository`, `llm::domain::signed_url_fetcher`

#### `src/libs/colmena/src/llm/infrastructure/attachment_summary/cheap_tier.rs`
- Module: `llm::infrastructure::attachment_summary::cheap_tier`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/attachment_summary/llm_summary_generator.rs`
- Module: `llm::infrastructure::attachment_summary::llm_summary_generator`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `llm::domain`, `llm::domain::attachments`

#### `src/libs/colmena/src/llm/infrastructure/attachment_summary/mod.rs`
- Module: `llm::infrastructure::attachment_summary`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/infrastructure/attachment_summary/text_extractor.rs`
- Module: `llm::infrastructure::attachment_summary::text_extractor`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/infrastructure/attachments/mod.rs`
- Module: `llm::infrastructure::attachments`
- **Used by (4)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/session_attachment.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/infrastructure/attachments/stream_resolver_impl.rs`
- Module: `llm::infrastructure::attachments::stream_resolver_impl`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (5): `llm::domain`, `llm::domain::attachments`, `llm::domain::large_tabular`, `llm::infrastructure::persistence::sqlite_attachment_registry`, `storage::domain`

#### `src/libs/colmena/src/llm/infrastructure/cheap_models.rs`
- Module: `llm::infrastructure::cheap_models`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/decision_model_provider_factory.rs`
- Module: `llm::infrastructure::decision_model_provider_factory`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `llm::domain::decision_model_repository`, `llm::infrastructure`

#### `src/libs/colmena/src/llm/infrastructure/elevenlabs_tts_adapter.rs`
- Module: `llm::infrastructure::elevenlabs_tts_adapter`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `llm::domain::tts`, `llm::domain::tts_repository`

#### `src/libs/colmena/src/llm/infrastructure/files/anthropic_files_api.rs`
- Module: `llm::infrastructure::files::anthropic_files_api`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/files/file_provider_factory.rs`
- Module: `llm::infrastructure::files::file_provider_factory`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `llm::domain`, `llm::infrastructure::files`

#### `src/libs/colmena/src/llm/infrastructure/files/gemini_files_api.rs`
- Module: `llm::infrastructure::files::gemini_files_api`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/files/mod.rs`
- Module: `llm::infrastructure::files`
- **Used by (7)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`, `src/libs/colmena/src/llm/application/llm_call_use_case.rs`, `src/libs/colmena/src/llm/infrastructure/files/file_provider_factory.rs`, `src/libs/colmena/src/web/infrastructure/openapi_adapter.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/infrastructure/files/openai_files_api.rs`
- Module: `llm::infrastructure::files::openai_files_api`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs`
- Module: `llm::infrastructure::files::postgres_file_cache`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `dag_engine::infrastructure::pool_registry`, `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/files/signed_url_downloader.rs`
- Module: `llm::infrastructure::files::signed_url_downloader`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/socketio.rs`
- Depends on (2): `dag_engine::infrastructure::nodes::llm_synthetic_tools::mcp::allowlist`, `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs`
- Module: `llm::infrastructure::gemini_adapter`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `llm::domain`, `llm::infrastructure::gemini_schema`, `llm::infrastructure::transient`

#### `src/libs/colmena/src/llm/infrastructure/gemini_schema.rs`
- Module: `llm::infrastructure::gemini_schema`
- **Used by (1)**: `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/infrastructure/google_tts_adapter.rs`
- Module: `llm::infrastructure::google_tts_adapter`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `llm::domain::tts`, `llm::domain::tts_repository`

#### `src/libs/colmena/src/llm/infrastructure/llm_provider_factory.rs`
- Module: `llm::infrastructure::llm_provider_factory`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `llm::domain`, `llm::infrastructure`

#### `src/libs/colmena/src/llm/infrastructure/mcp_client/mod.rs`
- Module: `llm::infrastructure::mcp_client`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/bind.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/infrastructure/mcp_client/rmcp_http_client.rs`
- Module: `llm::infrastructure::mcp_client::rmcp_http_client`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/bind.rs`
- Depends on (6): `dag_engine::application::ports`, `dag_engine::infrastructure::nodes::llm_synthetic_tools::mcp::allowlist`, `google_oauth::domain`, `google_oauth::infrastructure`, `llm::domain::mcp`, `llm::domain::text_bounds`

#### `src/libs/colmena/src/llm/infrastructure/message_summarizer/llm_message_summarizer.rs`
- Module: `llm::infrastructure::message_summarizer::llm_message_summarizer`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/message_summarizer/mod.rs`
- Module: `llm::infrastructure::message_summarizer`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/infrastructure/mock_adapter.rs`
- Module: `llm::infrastructure::mock_adapter`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/mod.rs`
- Module: `llm::infrastructure`
- **Used by (18)**: `src/libs/colmena/src/dag_engine/application/preflight.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/critic.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/large_run_turn.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/node_harness.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/storage_ref_turn.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/whole_read_guard.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/planner.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/reactor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/extract_with_schema.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`, `src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs`, `src/libs/colmena/src/llm/infrastructure/decision_model_provider_factory.rs`, `src/libs/colmena/src/llm/infrastructure/llm_provider_factory.rs`, `src/libs/colmena/src/llm/infrastructure/tts_provider_factory.rs`, `src/libs/colmena/src/node_bindings/registry.rs`, `src/libs/colmena/src/python_bindings/mod.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/infrastructure/openai_adapter.rs`
- Module: `llm::infrastructure::openai_adapter`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `llm::domain`, `llm::infrastructure::transient`

#### `src/libs/colmena/src/llm/infrastructure/openai_tts_adapter.rs`
- Module: `llm::infrastructure::openai_tts_adapter`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `llm::domain::tts`, `llm::domain::tts_repository`

#### `src/libs/colmena/src/llm/infrastructure/persistence/hydration.rs`
- Module: `llm::infrastructure::persistence::hydration`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/persistence/in_memory_conversation_repository.rs`
- Module: `llm::infrastructure::persistence::in_memory_conversation_repository`
- **Used by (6)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/critic.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/planner.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/reactor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/extract_with_schema.rs`
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/persistence/mod.rs`
- Module: `llm::infrastructure::persistence`
- **Used by (14)**: `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/characterisation.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/load_attachment_redirect.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/storage_ref_turn.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/session_attachment.rs`, `src/libs/colmena/src/llm/application/history_compaction.rs`, `src/libs/colmena/src/llm/infrastructure/persistence/ownership_contract.rs`, `src/libs/colmena/src/llm/infrastructure/persistence/repository_factory.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/infrastructure/persistence/ownership_contract.rs`
- Module: `llm::infrastructure::persistence::ownership_contract`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (4): `dag_engine::infrastructure::pool_registry`, `llm::domain`, `llm::domain::attachments`, `llm::infrastructure::persistence`

#### `src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs`
- Module: `llm::infrastructure::persistence::postgres_attachment_registry`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `dag_engine::infrastructure::pool_registry`, `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/persistence/postgres_conversation_repository.rs`
- Module: `llm::infrastructure::persistence::postgres_conversation_repository`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/persistence/repository_factory.rs`
- Module: `llm::infrastructure::persistence::repository_factory`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/engine.rs`
- Depends on (3): `dag_engine::infrastructure::pool_registry`, `llm::domain`, `llm::infrastructure::persistence`

#### `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs`
- Module: `llm::infrastructure::persistence::sqlite_attachment_registry`
- **Used by (1)**: `src/libs/colmena/src/llm/infrastructure/attachments/stream_resolver_impl.rs`
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_conversation_repository.rs`
- Module: `llm::infrastructure::persistence::sqlite_conversation_repository`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/scripted_adapter.rs`
- Module: `llm::infrastructure::scripted_adapter`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `llm::domain`, `llm::domain::tools`

#### `src/libs/colmena/src/llm/infrastructure/tool_args.rs`
- Module: `llm::infrastructure::tool_args`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/llm/infrastructure/transient.rs`
- Module: `llm::infrastructure::transient`
- **Used by (4)**: `src/libs/colmena/src/dag_engine/application/preflight.rs`, `src/libs/colmena/src/llm/infrastructure/anthropic_adapter.rs`, `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs`, `src/libs/colmena/src/llm/infrastructure/openai_adapter.rs`
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/llm/infrastructure/tts_provider_factory.rs`
- Module: `llm::infrastructure::tts_provider_factory`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `llm::domain::tts_repository`, `llm::infrastructure`

#### `src/libs/colmena/src/llm/infrastructure/typesafe_jev_adapter.rs`
- Module: `llm::infrastructure::typesafe_jev_adapter`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `llm::domain::decision_model`, `llm::domain::decision_model_repository`

#### `src/libs/colmena/src/llm/mod.rs`
- Module: `llm`
- **Used by (1)**: `src/libs/colmena/src/shared/infrastructure/service_container.rs`
- Depends on (0): — (no intra-crate imports)

### main.rs

#### `src/libs/colmena/src/main.rs`
- Module: `(crate root)`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

### node_bindings

#### `src/libs/colmena/src/node_bindings/dag.rs`
- Module: `node_bindings::dag`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `dag_engine::domain::lint`, `node_bindings::stream`

#### `src/libs/colmena/src/node_bindings/documents.rs`
- Module: `node_bindings::documents`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `crdt_documents`

#### `src/libs/colmena/src/node_bindings/llm.rs`
- Module: `node_bindings::llm`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `llm::domain`, `node_bindings::stream`, `shared::infrastructure`

#### `src/libs/colmena/src/node_bindings/mod.rs`
- Module: `node_bindings`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/node_bindings/registry.rs`
- Module: `node_bindings::registry`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (5): `dag_engine::application::ports`, `dag_engine::infrastructure::pool_registry`, `dag_engine::infrastructure::registry`, `dag_engine::infrastructure::sql_port_factory`, `llm::infrastructure`

#### `src/libs/colmena/src/node_bindings/stream.rs`
- Module: `node_bindings::stream`
- **Used by (2)**: `src/libs/colmena/src/node_bindings/dag.rs`, `src/libs/colmena/src/node_bindings/llm.rs`
- Depends on (0): — (no intra-crate imports)

### python_bindings

#### `src/libs/colmena/src/python_bindings/crdt_documents.rs`
- Module: `python_bindings::crdt_documents`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `crdt_documents`

#### `src/libs/colmena/src/python_bindings/mod.rs`
- Module: `python_bindings`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (8): `dag_engine::application::ports`, `dag_engine::domain::lint`, `dag_engine::infrastructure::pool_registry`, `dag_engine::infrastructure::registry`, `dag_engine::infrastructure::sql_port_factory`, `llm::domain`, `llm::infrastructure`, `shared::infrastructure`

### shared

#### `src/libs/colmena/src/shared/infrastructure/config_resolver.rs`
- Module: `shared::infrastructure::config_resolver`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm::domain`

#### `src/libs/colmena/src/shared/infrastructure/http_client.rs`
- Module: `shared::infrastructure::http_client`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/shared/infrastructure/mod.rs`
- Module: `shared::infrastructure`
- **Used by (2)**: `src/libs/colmena/src/node_bindings/llm.rs`, `src/libs/colmena/src/python_bindings/mod.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/shared/infrastructure/service_container.rs`
- Module: `shared::infrastructure::service_container`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `llm`

#### `src/libs/colmena/src/shared/mod.rs`
- Module: `shared`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

### skills

#### `src/libs/colmena/src/skills/domain/mod.rs`
- Module: `skills::domain`
- **Used by (9)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`, `src/libs/colmena/src/skills/domain/skill_repository.rs`, `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs`, `src/libs/colmena/src/skills/infrastructure/composite_skill_repository.rs`, `src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs`, `src/libs/colmena/src/skills/infrastructure/frontmatter_parser.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/skills/domain/skill.rs`
- Module: `skills::domain::skill`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/skills/domain/skill_config.rs`
- Module: `skills::domain::skill_config`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/skills/domain/skill_error.rs`
- Module: `skills::domain::skill_error`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/skills/domain/skill_repository.rs`
- Module: `skills::domain::skill_repository`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `skills::domain`

#### `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs`
- Module: `skills::infrastructure::builtin_skill_repository`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `skills::domain`, `skills::infrastructure::frontmatter_parser`

#### `src/libs/colmena/src/skills/infrastructure/composite_skill_repository.rs`
- Module: `skills::infrastructure::composite_skill_repository`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `skills::domain`

#### `src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs`
- Module: `skills::infrastructure::filesystem_skill_repository`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `skills::domain`, `skills::infrastructure::frontmatter_parser`

#### `src/libs/colmena/src/skills/infrastructure/frontmatter_parser.rs`
- Module: `skills::infrastructure::frontmatter_parser`
- **Used by (2)**: `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs`, `src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs`
- Depends on (1): `skills::domain`

#### `src/libs/colmena/src/skills/infrastructure/mod.rs`
- Module: `skills::infrastructure`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/skills/mod.rs`
- Module: `skills`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

### storage

#### `src/libs/colmena/src/storage/domain/mod.rs`
- Module: `storage::domain`
- **Used by (32)**: `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor/large_object_guard.rs`, `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/characterisation.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/load_attachment_redirect.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/node_harness.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python/large.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/session_attachment.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`, `src/libs/colmena/src/llm/domain/attachments/stream_resolver.rs`, `src/libs/colmena/src/llm/infrastructure/attachments/stream_resolver_impl.rs`, `src/libs/colmena/src/storage/infrastructure/http_callback_adapter.rs`, `src/libs/colmena/src/storage/infrastructure/local_cache_adapter.rs`, `src/libs/colmena/src/storage/infrastructure/local_http_adapter.rs`, `src/libs/colmena/src/tabular_prepare/convert.rs`, `src/libs/colmena/src/tabular_prepare/driver.rs`, `src/libs/colmena/src/tabular_prepare/driver_xlsx_tests.rs`, `src/libs/colmena/src/tabular_prepare/gc.rs`, `src/libs/colmena/src/tabular_prepare/prepare.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_package.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_spool.rs`, `src/libs/colmena/src/tabular_run/mounted.rs`, `src/libs/colmena/src/tabular_run/runtime.rs`, `src/libs/colmena/src/tabular_run/stage.rs`, `src/libs/colmena/src/tabular_run/testkit.rs`, `src/libs/colmena/src/tabular_run/verify.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/storage/domain/output_storage_repository.rs`
- Module: `storage::domain::output_storage_repository`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `storage::domain::storage_error`

#### `src/libs/colmena/src/storage/domain/storage_error.rs`
- Module: `storage::domain::storage_error`
- **Used by (2)**: `src/libs/colmena/src/llm/domain/attachments/stream_resolver.rs`, `src/libs/colmena/src/storage/domain/output_storage_repository.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/storage/infrastructure/http_callback_adapter.rs`
- Module: `storage::infrastructure::http_callback_adapter`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `storage::domain`

#### `src/libs/colmena/src/storage/infrastructure/local_cache_adapter.rs`
- Module: `storage::infrastructure::local_cache_adapter`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `storage::domain`

#### `src/libs/colmena/src/storage/infrastructure/local_http_adapter.rs`
- Module: `storage::infrastructure::local_http_adapter`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `storage::domain`

#### `src/libs/colmena/src/storage/infrastructure/mod.rs`
- Module: `storage::infrastructure`
- **Used by (6)**: `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/node_harness.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs`, `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`, `src/libs/colmena/src/tabular_prepare/prepare.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/storage/mod.rs`
- Module: `storage`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

### tabular_prepare

#### `src/libs/colmena/src/tabular_prepare/convert.rs`
- Module: `tabular_prepare::convert`
- **Used by (5)**: `src/libs/colmena/src/tabular_prepare/driver.rs`, `src/libs/colmena/src/tabular_prepare/prepare.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_convert.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_run.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_spool.rs`
- Depends on (7): `storage::domain`, `tabular_prepare::csv`, `tabular_prepare::infer`, `tabular_prepare::manifest`, `tabular_prepare::part_sink`, `tabular_prepare::scan`, `tabular_prepare::writer`

#### `src/libs/colmena/src/tabular_prepare/csv.rs`
- Module: `tabular_prepare::csv`
- **Used by (6)**: `src/libs/colmena/src/tabular_prepare/convert.rs`, `src/libs/colmena/src/tabular_prepare/driver.rs`, `src/libs/colmena/src/tabular_prepare/scan.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_columns.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_convert.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_run.rs`
- Depends on (3): `tabular_prepare::infer`, `tabular_prepare::manifest`, `tabular_prepare::scan`

#### `src/libs/colmena/src/tabular_prepare/driver.rs`
- Module: `tabular_prepare::driver`
- **Used by (2)**: `src/libs/colmena/src/tabular_prepare/driver_xlsx_tests.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_convert.rs`
- Depends on (15): `storage::domain`, `tabular_prepare::convert`, `tabular_prepare::csv`, `tabular_prepare::manifest`, `tabular_prepare::part_sink`, `tabular_prepare::ports`, `tabular_prepare::precheck`, `tabular_prepare::prepare`, `tabular_prepare::registry`, `tabular_prepare::registry_faults`, `tabular_prepare::sqlite_registry`, `tabular_prepare::writer`, `tabular_prepare::xlsx_convert`, `tabular_prepare::xlsx_sheet`, `tabular_prepare::xlsx_spool`

#### `src/libs/colmena/src/tabular_prepare/driver_xlsx_tests.rs`
- Module: `tabular_prepare::driver_xlsx_tests`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (10): `storage::domain`, `tabular_prepare::driver`, `tabular_prepare::manifest`, `tabular_prepare::ports`, `tabular_prepare::prepare`, `tabular_prepare::registry`, `tabular_prepare::sqlite_registry`, `tabular_prepare::writer`, `tabular_prepare::xlsxfix`, `tabular_prepare::zipfix`

#### `src/libs/colmena/src/tabular_prepare/gc.rs`
- Module: `tabular_prepare::gc`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `storage::domain`, `tabular_prepare::registry`, `tabular_prepare::sqlite_registry`

#### `src/libs/colmena/src/tabular_prepare/infer.rs`
- Module: `tabular_prepare::infer`
- **Used by (4)**: `src/libs/colmena/src/tabular_prepare/convert.rs`, `src/libs/colmena/src/tabular_prepare/csv.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_columns.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_run.rs`
- Depends on (1): `tabular_prepare::manifest`

#### `src/libs/colmena/src/tabular_prepare/manifest.rs`
- Module: `tabular_prepare::manifest`
- **Used by (15)**: `src/libs/colmena/src/tabular_prepare/convert.rs`, `src/libs/colmena/src/tabular_prepare/csv.rs`, `src/libs/colmena/src/tabular_prepare/driver.rs`, `src/libs/colmena/src/tabular_prepare/driver_xlsx_tests.rs`, `src/libs/colmena/src/tabular_prepare/infer.rs`, `src/libs/colmena/src/tabular_prepare/part_sink.rs`, `src/libs/colmena/src/tabular_prepare/prepare.rs`, `src/libs/colmena/src/tabular_prepare/writer.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_columns.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_convert.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_run.rs`, `src/libs/colmena/src/tabular_run/prelude.rs`, `src/libs/colmena/src/tabular_run/stage.rs`, `src/libs/colmena/src/tabular_run/testkit.rs`, `src/libs/colmena/src/tabular_run/verify.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/tabular_prepare/mod.rs`
- Module: `tabular_prepare`
- **Used by (3)**: `src/libs/colmena/src/tabular_prepare/registry_contract.rs`, `src/libs/colmena/src/tabular_run/runtime.rs`, `src/libs/colmena/src/tabular_run/testkit.rs`
- Depends on (2): `tabular_prepare::ports`, `tabular_prepare::registry`

#### `src/libs/colmena/src/tabular_prepare/part_sink.rs`
- Module: `tabular_prepare::part_sink`
- **Used by (6)**: `src/libs/colmena/src/tabular_prepare/convert.rs`, `src/libs/colmena/src/tabular_prepare/driver.rs`, `src/libs/colmena/src/tabular_prepare/prepare.rs`, `src/libs/colmena/src/tabular_prepare/writer.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_convert.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_run.rs`
- Depends on (1): `tabular_prepare::manifest`

#### `src/libs/colmena/src/tabular_prepare/ports.rs`
- Module: `tabular_prepare::ports`
- **Used by (7)**: `src/libs/colmena/src/dag_engine/engine.rs`, `src/libs/colmena/src/tabular_prepare/driver.rs`, `src/libs/colmena/src/tabular_prepare/driver_xlsx_tests.rs`, `src/libs/colmena/src/tabular_prepare/mod.rs`, `src/libs/colmena/src/tabular_prepare/registry_contract.rs`, `src/libs/colmena/src/tabular_run/runtime.rs`, `src/libs/colmena/src/tabular_run/testkit.rs`
- Depends on (1): `dag_engine::engine`

#### `src/libs/colmena/src/tabular_prepare/postgres_registry.rs`
- Module: `tabular_prepare::postgres_registry`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `dag_engine::infrastructure::pool_registry`, `tabular_prepare::registry_contract`

#### `src/libs/colmena/src/tabular_prepare/precheck.rs`
- Module: `tabular_prepare::precheck`
- **Used by (3)**: `src/libs/colmena/src/tabular_prepare/driver.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_package.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_spool.rs`
- Depends on (1): `tabular_prepare::zipfix`

#### `src/libs/colmena/src/tabular_prepare/prepare.rs`
- Module: `tabular_prepare::prepare`
- **Used by (2)**: `src/libs/colmena/src/tabular_prepare/driver.rs`, `src/libs/colmena/src/tabular_prepare/driver_xlsx_tests.rs`
- Depends on (7): `storage::domain`, `storage::infrastructure`, `tabular_prepare::convert`, `tabular_prepare::manifest`, `tabular_prepare::part_sink`, `tabular_prepare::xlsx_convert`, `tabular_prepare::xlsx_spool`

#### `src/libs/colmena/src/tabular_prepare/registry.rs`
- Module: `tabular_prepare::registry`
- **Used by (7)**: `src/libs/colmena/src/tabular_prepare/driver.rs`, `src/libs/colmena/src/tabular_prepare/driver_xlsx_tests.rs`, `src/libs/colmena/src/tabular_prepare/gc.rs`, `src/libs/colmena/src/tabular_prepare/mod.rs`, `src/libs/colmena/src/tabular_run/runtime.rs`, `src/libs/colmena/src/tabular_run/testkit.rs`, `src/libs/colmena/src/tabular_run/verify.rs`
- Depends on (1): `dag_engine::infrastructure::pool_registry`

#### `src/libs/colmena/src/tabular_prepare/registry_contract.rs`
- Module: `tabular_prepare::registry_contract`
- **Used by (1)**: `src/libs/colmena/src/tabular_prepare/postgres_registry.rs`
- Depends on (3): `tabular_prepare`, `tabular_prepare::ports`, `tabular_prepare::sqlite_registry`

#### `src/libs/colmena/src/tabular_prepare/registry_faults.rs`
- Module: `tabular_prepare::registry_faults`
- **Used by (1)**: `src/libs/colmena/src/tabular_prepare/driver.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/tabular_prepare/scan.rs`
- Module: `tabular_prepare::scan`
- **Used by (2)**: `src/libs/colmena/src/tabular_prepare/convert.rs`, `src/libs/colmena/src/tabular_prepare/csv.rs`
- Depends on (1): `tabular_prepare::csv`

#### `src/libs/colmena/src/tabular_prepare/sqlite_registry.rs`
- Module: `tabular_prepare::sqlite_registry`
- **Used by (5)**: `src/libs/colmena/src/tabular_prepare/driver.rs`, `src/libs/colmena/src/tabular_prepare/driver_xlsx_tests.rs`, `src/libs/colmena/src/tabular_prepare/gc.rs`, `src/libs/colmena/src/tabular_prepare/registry_contract.rs`, `src/libs/colmena/src/tabular_run/testkit.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/tabular_prepare/writer.rs`
- Module: `tabular_prepare::writer`
- **Used by (6)**: `src/libs/colmena/src/tabular_prepare/convert.rs`, `src/libs/colmena/src/tabular_prepare/driver.rs`, `src/libs/colmena/src/tabular_prepare/driver_xlsx_tests.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_convert.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_run.rs`, `src/libs/colmena/src/tabular_run/stage.rs`
- Depends on (2): `tabular_prepare::manifest`, `tabular_prepare::part_sink`

#### `src/libs/colmena/src/tabular_prepare/xlsx_columns.rs`
- Module: `tabular_prepare::xlsx_columns`
- **Used by (1)**: `src/libs/colmena/src/tabular_prepare/xlsx_run.rs`
- Depends on (5): `tabular_prepare::csv`, `tabular_prepare::infer`, `tabular_prepare::manifest`, `tabular_prepare::xlsx_sheet`, `tabular_prepare::xlsx_styles`

#### `src/libs/colmena/src/tabular_prepare/xlsx_convert.rs`
- Module: `tabular_prepare::xlsx_convert`
- **Used by (2)**: `src/libs/colmena/src/tabular_prepare/driver.rs`, `src/libs/colmena/src/tabular_prepare/prepare.rs`
- Depends on (11): `tabular_prepare::convert`, `tabular_prepare::csv`, `tabular_prepare::driver`, `tabular_prepare::manifest`, `tabular_prepare::part_sink`, `tabular_prepare::writer`, `tabular_prepare::xlsx_package`, `tabular_prepare::xlsx_run`, `tabular_prepare::xlsx_sheet`, `tabular_prepare::xlsx_spool`, `tabular_prepare::xlsxfix`

#### `src/libs/colmena/src/tabular_prepare/xlsx_package.rs`
- Module: `tabular_prepare::xlsx_package`
- **Used by (6)**: `src/libs/colmena/src/tabular_prepare/xlsx_convert.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_run.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_sheet.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_strings.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_styles.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_workbook.rs`
- Depends on (4): `storage::domain`, `tabular_prepare::precheck`, `tabular_prepare::xlsx_spool`, `tabular_prepare::zipfix`

#### `src/libs/colmena/src/tabular_prepare/xlsx_run.rs`
- Module: `tabular_prepare::xlsx_run`
- **Used by (1)**: `src/libs/colmena/src/tabular_prepare/xlsx_convert.rs`
- Depends on (14): `tabular_prepare::convert`, `tabular_prepare::csv`, `tabular_prepare::infer`, `tabular_prepare::manifest`, `tabular_prepare::part_sink`, `tabular_prepare::writer`, `tabular_prepare::xlsx_columns`, `tabular_prepare::xlsx_package`, `tabular_prepare::xlsx_sheet`, `tabular_prepare::xlsx_spool`, `tabular_prepare::xlsx_strings`, `tabular_prepare::xlsx_styles`, `tabular_prepare::xlsx_workbook`, `tabular_prepare::xlsxfix`

#### `src/libs/colmena/src/tabular_prepare/xlsx_sheet.rs`
- Module: `tabular_prepare::xlsx_sheet`
- **Used by (4)**: `src/libs/colmena/src/tabular_prepare/driver.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_columns.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_convert.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_run.rs`
- Depends on (6): `tabular_prepare::xlsx_package`, `tabular_prepare::xlsx_spool`, `tabular_prepare::xlsx_strings`, `tabular_prepare::xlsx_styles`, `tabular_prepare::xlsx_workbook`, `tabular_prepare::xlsxfix`

#### `src/libs/colmena/src/tabular_prepare/xlsx_spool.rs`
- Module: `tabular_prepare::xlsx_spool`
- **Used by (9)**: `src/libs/colmena/src/tabular_prepare/driver.rs`, `src/libs/colmena/src/tabular_prepare/prepare.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_convert.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_package.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_run.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_sheet.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_strings.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_styles.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_workbook.rs`
- Depends on (3): `storage::domain`, `tabular_prepare::convert`, `tabular_prepare::precheck`

#### `src/libs/colmena/src/tabular_prepare/xlsx_strings.rs`
- Module: `tabular_prepare::xlsx_strings`
- **Used by (2)**: `src/libs/colmena/src/tabular_prepare/xlsx_run.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_sheet.rs`
- Depends on (3): `tabular_prepare::xlsx_package`, `tabular_prepare::xlsx_spool`, `tabular_prepare::zipfix`

#### `src/libs/colmena/src/tabular_prepare/xlsx_styles.rs`
- Module: `tabular_prepare::xlsx_styles`
- **Used by (3)**: `src/libs/colmena/src/tabular_prepare/xlsx_columns.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_run.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_sheet.rs`
- Depends on (4): `tabular_prepare::xlsx_package`, `tabular_prepare::xlsx_spool`, `tabular_prepare::xlsxfix`, `tabular_prepare::zipfix`

#### `src/libs/colmena/src/tabular_prepare/xlsx_workbook.rs`
- Module: `tabular_prepare::xlsx_workbook`
- **Used by (2)**: `src/libs/colmena/src/tabular_prepare/xlsx_run.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_sheet.rs`
- Depends on (4): `tabular_prepare::xlsx_package`, `tabular_prepare::xlsx_spool`, `tabular_prepare::xlsxfix`, `tabular_prepare::zipfix`

#### `src/libs/colmena/src/tabular_prepare/xlsxfix.rs`
- Module: `tabular_prepare::xlsxfix`
- **Used by (6)**: `src/libs/colmena/src/tabular_prepare/driver_xlsx_tests.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_convert.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_run.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_sheet.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_styles.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_workbook.rs`
- Depends on (1): `tabular_prepare::zipfix`

#### `src/libs/colmena/src/tabular_prepare/zipfix.rs`
- Module: `tabular_prepare::zipfix`
- **Used by (7)**: `src/libs/colmena/src/tabular_prepare/driver_xlsx_tests.rs`, `src/libs/colmena/src/tabular_prepare/precheck.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_package.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_strings.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_styles.rs`, `src/libs/colmena/src/tabular_prepare/xlsx_workbook.rs`, `src/libs/colmena/src/tabular_prepare/xlsxfix.rs`
- Depends on (0): — (no intra-crate imports)

### tabular_run

#### `src/libs/colmena/src/tabular_run/local.rs`
- Module: `tabular_run::local`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (4): `dag_engine::domain::python_executor`, `dag_engine::infrastructure::python_exec::staging`, `dag_engine::infrastructure::python_exec::subprocess`, `dag_engine::log_policy`

#### `src/libs/colmena/src/tabular_run/mod.rs`
- Module: `tabular_run`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/tabular_run/mounted.rs`
- Module: `tabular_run::mounted`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python/large.rs`, `src/libs/colmena/src/dag_engine/infrastructure/python_exec/mod.rs`
- Depends on (2): `dag_engine::domain::python_executor`, `storage::domain`

#### `src/libs/colmena/src/tabular_run/prelude.rs`
- Module: `tabular_run::prelude`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_run_python`, `dag_engine::infrastructure::nodes::python_node`, `tabular_prepare::manifest`

#### `src/libs/colmena/src/tabular_run/refusal.rs`
- Module: `tabular_run::refusal`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python/large.rs`
- Depends on (1): `llm::domain::large_tabular`

#### `src/libs/colmena/src/tabular_run/remote_seam.rs`
- Module: `tabular_run::remote_seam`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (2): `dag_engine::domain::python_executor`, `dag_engine::infrastructure::python_exec::remote`

#### `src/libs/colmena/src/tabular_run/runtime.rs`
- Module: `tabular_run::runtime`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor/large_route.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python/large.rs`
- Depends on (6): `dag_engine::domain::python_executor`, `dag_engine::infrastructure::python_exec::protocol`, `storage::domain`, `tabular_prepare`, `tabular_prepare::ports`, `tabular_prepare::registry`

#### `src/libs/colmena/src/tabular_run/stage.rs`
- Module: `tabular_run::stage`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `storage::domain`, `tabular_prepare::manifest`, `tabular_prepare::writer`

#### `src/libs/colmena/src/tabular_run/testkit.rs`
- Module: `tabular_run::testkit`
- **Used by (2)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm/large_run_turn.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python/large.rs`
- Depends on (7): `dag_engine::domain::python_executor`, `storage::domain`, `tabular_prepare`, `tabular_prepare::manifest`, `tabular_prepare::ports`, `tabular_prepare::registry`, `tabular_prepare::sqlite_registry`

#### `src/libs/colmena/src/tabular_run/verify.rs`
- Module: `tabular_run::verify`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (3): `storage::domain`, `tabular_prepare::manifest`, `tabular_prepare::registry`

### text

#### `src/libs/colmena/src/text/mod.rs`
- Module: `text`
- **Used by (11)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/data_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/list_threads.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`, `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/recall_history.rs`
- Depends on (0): — (no intra-crate imports)

### web

#### `src/libs/colmena/src/web/application/api_spec_use_case.rs`
- Module: `web::application::api_spec_use_case`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`
- Depends on (1): `web::domain`

#### `src/libs/colmena/src/web/application/mod.rs`
- Module: `web::application`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/web/application/search_use_case.rs`
- Module: `web::application::search_use_case`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`
- Depends on (2): `web::domain::errors`, `web::domain::search_port`

#### `src/libs/colmena/src/web/application/swagger2_to_oas3.rs`
- Module: `web::application::swagger2_to_oas3`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (1): `web::domain`

#### `src/libs/colmena/src/web/application/url_normalizer.rs`
- Module: `web::application::url_normalizer`
- **Used by (1)**: `src/libs/colmena/src/web/infrastructure/openapi_adapter.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/web/domain/api_spec_port.rs`
- Module: `web::domain::api_spec_port`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`
- Depends on (1): `web::domain::errors`

#### `src/libs/colmena/src/web/domain/errors.rs`
- Module: `web::domain::errors`
- **Used by (5)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`, `src/libs/colmena/src/web/application/search_use_case.rs`, `src/libs/colmena/src/web/domain/api_spec_port.rs`, `src/libs/colmena/src/web/domain/search_port.rs`, `src/libs/colmena/src/web/infrastructure/tavily_adapter.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/web/domain/mod.rs`
- Module: `web::domain`
- **Used by (4)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`, `src/libs/colmena/src/web/application/api_spec_use_case.rs`, `src/libs/colmena/src/web/application/swagger2_to_oas3.rs`, `src/libs/colmena/src/web/infrastructure/openapi_adapter.rs`
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/web/domain/search_port.rs`
- Module: `web::domain::search_port`
- **Used by (3)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`, `src/libs/colmena/src/web/application/search_use_case.rs`, `src/libs/colmena/src/web/infrastructure/tavily_adapter.rs`
- Depends on (1): `web::domain::errors`

#### `src/libs/colmena/src/web/domain/session.rs`
- Module: `web::domain::session`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/web/infrastructure/mod.rs`
- Module: `web::infrastructure`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)

#### `src/libs/colmena/src/web/infrastructure/openapi_adapter.rs`
- Module: `web::infrastructure::openapi_adapter`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`
- Depends on (4): `llm::domain`, `llm::infrastructure::files`, `web::application::url_normalizer`, `web::domain`

#### `src/libs/colmena/src/web/infrastructure/tavily_adapter.rs`
- Module: `web::infrastructure::tavily_adapter`
- **Used by (1)**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`
- Depends on (2): `web::domain::errors`, `web::domain::search_port`

#### `src/libs/colmena/src/web/mod.rs`
- Module: `web`
- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)
- Depends on (0): — (no intra-crate imports)
