# Load Attachment — Design

**Status:** Approved for planning
**Date:** 2026-05-13
**Author:** Daniel Garcia (brainstormed with Claude)
**Target version:** 0.4.0
**Related:**
- [docs/developer_guide/28_large_files_api.md](../../developer_guide/28_large_files_api.md) — file upload pipeline (re-used)
- [docs/developer_guide/24_skills.md](../../developer_guide/24_skills.md) — `load_skill` pattern (mirrored)
- [docs/developer_guide/15_memory_guide.md](../../developer_guide/15_memory_guide.md) — conversation history backend

---

## Summary

Add a synthetic tool `load_attachment` that lets an `llm_call` node fetch a previously uploaded document on-demand mid-conversation, instead of re-sending the document in every turn. Attachments are scoped per `agent_session_id` (shared by every `llm_call` node in the session, including those inside subgraphs). Each `llm_call` node controls whether it exposes the tool via a new boolean config field `attachments_enabled` (default `true`). The implementation mirrors the `load_skill` synthetic-tool pattern and reuses the existing `provider_file_cache` upload pipeline.

## Motivation

`LlmMessage` carries an optional `files: Vec<FileData>` payload that the providers upload to their Files API during the first turn. **That `files` field is never persisted in `llm_node_history`** (only `role`, `content`, `tool_call_id`, `tool_calls` are stored — confirmed in `sqlite_conversation_repository.rs`). When the conversation resumes, the rebuilt history loses the document context. Two options exist today:

1. Re-attach the document on every turn — costly in tokens and provider quota.
2. Stuff the document into `content` as text — loses fidelity for non-text formats (images, PDFs).

Neither is acceptable for long, multi-turn workflows where the user expects "the assistant still has the document I uploaded earlier."

This design lets the LLM **decide when it needs the document** and pull it from a session-wide registry, paying the file-injection cost only when relevant.

## Goals

- Persist a per-session **catalog** of available attachments (metadata only, no bytes).
- Provide a synthetic `load_attachment` tool that the LLM can invoke with a `document_id` to bring a document into context.
- Inject the loaded document as a synthetic `user` message with `files[]`, persist that message in history so future turns retain the file reference.
- Reuse `provider_file_cache` for cross-turn `provider_file_id` reuse; recover transparently when the cached ID has expired.
- Subgraphs inherit the catalog automatically via the existing `agent_session_id` propagation.
- Zero overhead for `llm_call` nodes that opt out (`attachments_enabled: false`).
- Zero new code path when the session has no attachments registered.

## Non-goals

- Storing the raw bytes of attachments in Colmena's database. The registry holds metadata + the original source descriptor only.
- Sharing attachments across **different** `agent_session_id`s. A new session starts with an empty catalog.
- Per-node allowlists of specific `document_id`s (over-engineering — surfaced and rejected during brainstorming).
- Server-side deletion or "forget" semantics. Attachments live for the session's lifetime in the registry; provider-side files follow the provider's own retention rules (with on-demand re-upload as the recovery mechanism).
- A Python/TypeScript first-class API surface — the feature is configured through the standard `llm_call` config dict and accessible through existing bindings via `serde_json`.
- Mitigating prompt injection embedded in attachment content. Same trust model as today's `files[]`.
- Variable interpolation inside attachment metadata. The `label` / `description` strings are taken as-is.

## Architecture

### Data flow — first upload (registration)

```
Graph JSON → llm_call.config.files[]
   ↓
parse_file_entries → FileSource::{InlineBytes | SignedUrl | Uploaded}
   ↓
resolve_files (existing) → uploads via provider Files API, returns Uploaded(ProviderFileRef)
   ↓
[NEW] AttachmentRegistry::upsert(
    agent_session_id,
    document_id,                 // explicit from config.files[i].id, or auto-generated
    provider,
    provider_file_id,
    mime_type, filename, size_bytes,
    label, description,          // caller-supplied, optional
    original_source              // for re-upload recovery — see below
)
   ↓
LLM call proceeds normally with the resolved files attached to the first user message.
```

### Data flow — subsequent turn (on-demand load)

```
LLM emits tool_call: load_attachment({ document_id: "doc-abc" })
   ↓
DagToolExecutor intercepts (mirror of load_skill interception)
   ↓
Synthetic dispatcher returns:
   { "__colmena_status": "LOAD_ATTACHMENT", "document_id": "doc-abc" }
   ↓
AgentService.run() sentinel handler (new block, alongside SUSPENDED handler):
   1. Look up (agent_session_id, document_id, provider) in conversation_attachments
   2. Resolve the FileData:
        a. If provider_file_id is still live → FileSource::Uploaded(ref)
        b. If provider_file_id has expired (or absent for this provider):
             - Re-upload from `original_source` via the existing file pipeline
             - Update conversation_attachments with the new provider_file_id
             - If `original_source` is irrecoverable (e.g., raw bytes not retained,
               or a SignedUrl that has itself expired) → emit a tool result with
               a clear error JSON so the LLM can react.
   3. Append a synthetic LlmMessage::user_with_files(
          "[Attachment requested by the model: <label or filename>]",
          vec![file_data]
      )
   4. Persist that message in conversation history.
   5. Continue the agent loop — next iteration sees the file in the context window.
```

### Discovery — catalog inside the tool description

Following `load_skill`, the `load_attachment` tool description is built dynamically at the start of each `llm_call.execute`:

- The registry is queried for the current session's attachments at the start of `llm_call.execute`.
- **If `attachments_enabled = false`:** no query, no tool, no description bloat. The node sees no `load_attachment` in its tool list.
- **If `attachments_enabled = true` AND the catalog is empty:** the tool is **not** exposed for this `execute` call. This mirrors `load_skill`, which never registers without at least one skill. Rationale: a tool whose description is "no attachments available" wastes a tool slot and dilutes attention. A future `llm_call` node in the same session — running after a registration — will see the catalog and expose the tool naturally.
- **If `attachments_enabled = true` AND the catalog has ≥1 entry:** the description lists each entry as
  `"<document_id>" — <label or filename> (<mime_type>, <human-readable size>)<. description if present>`.

The single input parameter of the tool is `document_id: string`.

### Subgraph inheritance

Subgraphs already inherit `__colmena_agent_session_id` (see `subgraph.rs:51-55`, propagated when spawning and resuming child runs). With the global scope, **no code change is needed on the subgraph node**: a child `llm_call` automatically queries the same registry rows. This is the intended behaviour, documented for users.

### Layers (hexagonal)

**Domain** (`src/libs/colmena/src/llm/domain/attachments/`):
- `ConversationAttachment` — value object (session id, document id, provider, file id, metadata, original source descriptor, timestamps).
- `AttachmentSource` — enum mirroring the recoverable shapes (`SignedUrl(String)`, `Path(String)`, `Inline { retained: false }`). Used to decide expiry-recovery strategy.
- `AttachmentRegistry` — trait (port) with `upsert`, `lookup`, `list_for_session`.
- `AttachmentError` — typed errors via `thiserror` (NotFound, Expired, RecoveryFailed, RepositoryFailed).

**Infrastructure** (`src/libs/colmena/src/llm/infrastructure/persistence/`):
- `sqlite_attachment_registry.rs`
- `postgres_attachment_registry.rs`
- Both register through the existing service container, keyed by the memory backend type already chosen for the session.

**Integration with `llm_call`** (`src/libs/colmena/src/dag_engine/infrastructure/nodes/`):
- `llm_synthetic_tools/load_attachment_tool.rs` — builds the `ToolDefinition`, dispatches the sentinel.
- `llm_call_node.rs` (existing) — after `resolve_files`, call `AttachmentRegistry::upsert` for each registered file; read `attachments_enabled` from config; pass the registry handle to the `AgentService`.

**Integration with `AgentService`** (`src/libs/colmena/src/llm/application/agent_service.rs`):
- New block immediately after the SUSPENDED sentinel handler (`agent_service.rs:281`).
- The service receives the registry through its existing constructor injection pattern (additional optional dependency).

## Config schema

### `llm_call.config`

One new optional field:

```jsonc
{
  "type": "llm_call",
  "config": {
    "attachments_enabled": true,   // absent | true | false — default true
    "files": [
      {
        "id": "doc-abc-123",                 // optional — auto-generated if missing
        "label": "Q3 Financial Report",      // optional — shown in catalog
        "description": "Revenue & expense breakdown, Q3 2026",  // optional
        "mime_type": "application/pdf",
        "filename": "Q3_Financial.pdf",
        "url": "https://storage.googleapis.com/...?X-Goog-Signature=..."
        // OR "bytes_base64": "...", OR "path": "..."
      }
    ]
  }
}
```

`files[]` already exists; the new optional keys are `id`, `label`, `description`. The other keys remain backwards-compatible.

### Behavioural rules

- `attachments_enabled` absent → default `true`.
- `attachments_enabled: false` → the node never exposes `load_attachment` and (still) registers any incoming `files[]` so other nodes can see them. (Rationale: a node may upload but not consume.)
- `files[i].id` absent → auto-generate a stable id `att_<hex16>` where the hash inputs depend on the source kind:
  - `signed_url` → SHA-256 of `filename | mime_type | size | url`.
  - `path` → SHA-256 of `filename | mime_type | size | absolute_path`.
  - `inline` → SHA-256 of `filename | mime_type | size | raw_bytes`.
  This keeps re-runs of the same JSON idempotent (same id → upsert, not duplicate) while preventing collisions between distinct files with the same filename.
- Duplicate `(agent_session_id, document_id, provider)` on upsert → overwrite metadata and refresh the `provider_file_id` if changed. No error.

## Data model

```sql
CREATE TABLE conversation_attachments (
    agent_session_id  TEXT NOT NULL,
    document_id       TEXT NOT NULL,
    provider          TEXT NOT NULL,             -- 'openai' | 'anthropic' | 'gemini'
    provider_file_id  TEXT NOT NULL,
    mime_type         TEXT NOT NULL,
    filename          TEXT NOT NULL,
    size_bytes        BIGINT,
    label             TEXT,                      -- caller-supplied, fallback = filename
    description       TEXT,                      -- caller-supplied, optional
    source_kind       TEXT NOT NULL,             -- 'signed_url' | 'path' | 'inline'
    source_value      TEXT,                      -- the URL / path; NULL when source_kind='inline'
    registered_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    refreshed_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_session_id, document_id, provider)
);

CREATE INDEX idx_conversation_attachments_session
    ON conversation_attachments(agent_session_id);
```

Notes:
- `source_kind = 'inline'` with `source_value = NULL` is the irrecoverable case: we knowingly accept that an expired `provider_file_id` cannot be re-uploaded for inline binaries. The `load_attachment` flow surfaces this as a clear error to the LLM.
- The SQLite migration uses `TEXT` for the timestamp columns with `CURRENT_TIMESTAMP` defaults, matching the project's existing SQLite conventions.

## `load_attachment` tool contract

**Tool definition (single, fixed parameter):**

```json
{
  "name": "load_attachment",
  "description": "Load a document that has been attached to this conversation. Use this when you need to inspect the contents of a previously uploaded file.\n\nAvailable attachments:\n- \"doc-abc-123\" — Q3 Financial Report (application/pdf, 12 MB). Revenue & expense breakdown, Q3 2026\n- \"doc-xyz-456\" — photo.jpg (image/jpeg, 2 MB)\n",
  "input_schema": {
    "type": "object",
    "properties": {
      "document_id": {
        "type": "string",
        "description": "Exact id from the available-attachments list above."
      }
    },
    "required": ["document_id"]
  }
}
```

**Tool result on the wire (only the sentinel — the file is injected as a follow-up `user` message):**

```json
{ "__colmena_status": "LOAD_ATTACHMENT", "document_id": "doc-abc-123" }
```

**Error variants** (returned as a normal tool result, no sentinel — so the LLM can recover):

```json
{ "error": "unknown_document_id", "document_id": "...", "hint": "Check the available-attachments list in the tool description." }
{ "error": "attachment_expired_unrecoverable", "document_id": "...", "reason": "Original upload was inline bytes that were not retained; re-upload required." }
```

## Activation condition

The `AttachmentRegistry` adapter is constructed lazily per session, gated by the same backend selection used by `llm_node_history` (SQLite default; Postgres when `connection_url` is set on the node).

The synthetic tool's interception in `DagToolExecutor::execute` is **always present**, just like `load_skill` and `describe_tool`. The decision of whether to expose the tool to the LLM is made per call inside `llm_call.execute`, based on `attachments_enabled`. No tool registration happens for a node that opts out.

## Error handling

| Situation | Response |
|---|---|
| Unknown `document_id` (not in registry for this session) | Tool result with `error: "unknown_document_id"` — LLM can apologise/try a different id. |
| Cached `provider_file_id` expired, `source_kind = 'signed_url' \| 'path'` | Silent re-upload; update row; proceed. |
| Cached `provider_file_id` expired, `source_kind = 'inline'` (bytes not retained) | Tool result with `error: "attachment_expired_unrecoverable"`. |
| Cached `provider_file_id` expired, `source_kind = 'signed_url'` and the SignedUrl itself has expired | Same as the inline case — `attachment_expired_unrecoverable`. |
| Registry write failure on upsert | Surface as a node-level error; the file is still attached to the current first turn, but future loads will fail. |
| Sentinel detected but `agent_session_id` is missing on the run | Domain error `AttachmentError::SessionMissing` — fail loudly, this is a graph-config bug. |

## Observability

- `attachment.registered` SSE event when a new file is upserted into the registry.
- `attachment.loaded` SSE event when the sentinel handler resolves a file into the conversation.
- `attachment.recovery_attempted` SSE event when an expired `provider_file_id` triggers a re-upload, with success/failure outcome.
- A summary at the end of each run lists every attachment loaded during the run (`document_id`, label).

These events feed the same observer plumbing already used by `load_skill` and `describe_tool` — no new transport.

## Testing

**Domain unit tests** (`#[cfg(test)]` in `attachments/`):
- Auto-id generation is deterministic.
- Upsert overwrites metadata and refreshes `provider_file_id`.
- Source-kind classification (`SignedUrl` vs `Path` vs `Inline`).

**Infrastructure unit tests:**
- SQLite registry round-trip (insert, upsert, lookup).
- Postgres registry round-trip — marked `#[ignore = "requires DATABASE_URL — run with \`cargo test -- --ignored\`"]`.

**Integration tests** (`tests/`):
- Mocked `LlmRepository` returning a tool call to `load_attachment`, asserting that:
  1. The sentinel is intercepted in `AgentService`.
  2. The synthetic `user` message is appended with the correct `FileData`.
  3. The synthetic message is persisted in `llm_node_history`.
  4. The next loop iteration sees the file in the messages slice.
- Subgraph inheritance test: parent `llm_call` registers; child `llm_call` (inside `subgraph` node, same `agent_session_id`) loads successfully.
- Expiry recovery test: simulate expired `provider_file_id` for a `signed_url` source; assert re-upload happens and the new id is persisted.
- Opt-out test: with `attachments_enabled: false`, the LLM does not see the tool in the catalog.

**Test graphs** (`tests/graphs/agents/`):
- `load_attachment_basic.json` — single `llm_call` with a `files[]` entry, conversation across two runs (suspend → resume), the model invokes the tool on the second run.
- `load_attachment_subgraph.json` — orchestrator registers a doc, child subgraph loads it.
- `load_attachment_opt_out.json` — node with `attachments_enabled: false` cannot load.

## Migration / backwards compatibility

- New tables only. No existing column changes.
- `attachments_enabled` defaults to `true`, so old graphs that previously passed `files[]` keep working with no JSON change. The only **new** observable behaviour for them is that the `load_attachment` tool appears in the tool list — this is the intended improvement.
- For graphs that should preserve the exact prior tool-list contents (e.g., golden-fixture tests), authors must set `attachments_enabled: false`.

## Open questions deferred to implementation

- Whether the registry should garbage-collect rows when the conversation history is cleared. The default is "no" — the orphan rows are harmless and the next conversation in a different `agent_session_id` will not see them.
- Whether to expose `attachments_enabled` through Python/TypeScript bindings as a first-class parameter or rely on the existing dict-based path. Defer to the binding layer's own conventions.
