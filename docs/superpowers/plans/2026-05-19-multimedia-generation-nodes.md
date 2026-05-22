# Implementation Plan: Multimedia generation nodes (image-gen, image-edit, TTS)

> **Status (2026-05-20)**: All in-colmena scope is **SHIPPED** (Phases 1-6 of the
> original plan + 4 extra capabilities + dev/prod URL symmetry). The only
> remaining piece is **Phase 7 (host application wiring)** which lives in
> the downstream host application repository (private, consuming this library).
>
> See [§ Implementation status](#implementation-status-2026-05-20) below for
> the shipped-vs-planned delta. The rest of this document is preserved as the
> original design reference.

---

## Implementation status (2026-05-20)

### Phases 1-6 (in-colmena) — ✅ DONE

| Phase | Planned | Shipped | Notes |
|---|---|---|---|
| 1 | Storage port + 2 adapters (LocalCache, HttpCallback) | ✅ + `read(storage_key)` method + 3rd adapter `LocalHttpStorageAdapter` | `read()` added for cross-provider lazy upload and `$attachment:` placeholder. `LocalHttpStorageAdapter` (embedded `axum` server on `127.0.0.1`) added for dev/prod URL symmetry. |
| 2 | `TtsRepository` trait + 3 adapters | ✅ | OpenAI, ElevenLabs, Google Gemini TTS. `gpt-4o-mini-tts`, `eleven_multilingual_v2`, `gemini-2.5-flash-preview-tts`. |
| 3 | `image_generation` node (A2) | ✅ | OpenAI gpt-image-1 + Google Vertex Imagen 4 (with `yup-oauth2` for service-account JWT exchange). |
| 4 | `image_edit` node | ✅ | OpenAI gpt-image-1 multipart `/v1/images/edits`. Source accepted as `data:`, `http(s)://`, or `local://<key>` (resolved via storage). |
| 5 | `tts` node (A1) | ✅ | Thin node + factory dispatch. Format defaults to MP3 for predictable output shape. |
| 6 | Sample agent graph | ✅ | `tests/graphs/agents/multimedia_agent.json` + `multimedia_agent_with_load.json` (full chain: gen → load_attachment → http_post via `$attachment:` placeholder). |

### Extras shipped beyond original plan

1. **Artifacts unification** — outputs auto-register in `AttachmentRegistry`
   (`provider: Generated`), so `load_attachment` lets the agent "see" its own
   generations. Cross-provider lazy upload resolves bytes on first load from a
   different provider. See `dag_engine/infrastructure/nodes/llm.rs:447+`.
2. **`$attachment:<storage_key>` placeholder in `http_request`** — the engine
   resolves the placeholder to a `data:` URI by reading bytes via the storage
   adapter, so the LLM can ship artifacts to external endpoints without ever
   seeing the bytes. See `dag_engine/infrastructure/nodes/http.rs:30+`.
3. **Universal binary scrubber in `DagToolExecutor`** — every tool result is
   walked before returning to the LLM; `data:*;base64,*` strings are replaced
   with `[binary elided: mime=X, encoded_size=N bytes]`, and any string above
   `max_tool_result_bytes` (default 50 KB, override via llm_call config) is
   truncated. Prevents echo-bodies (e.g. `httpbin.org/post`) from saturating
   the LLM context. See `dag_engine/infrastructure/dag_tool_executor.rs:1048+`.
4. **`COLMENA_LOCAL=true|false` env guard rail** — explicit selector for
   storage adapter mode. `true` → `LocalHttpStorageAdapter` (with sane
   defaults for dir/port). `false` → `HttpCallbackStorageAdapter`, hard-fail
   if callback vars are missing. Unset → implicit fallback (back-compat).
   Each path logs `storage_mode_selected` at startup. See
   `dag_engine/engine.rs:73+`.

### URL strategy (dev/prod symmetry)

| Mode | `read_url` shape | Bytes location | LLM tool-result size |
|---|---|---|---|
| `COLMENA_LOCAL=true` (dev) | `http://127.0.0.1:<port>/files/<uuid>.png` | `/tmp/colmena-out/<uuid>.png` (disk) | small (URL only) |
| `COLMENA_LOCAL=false` (prod) | `https://storage.googleapis.com/...?X-Goog-Signature=...` (signed GCS) | GCS bucket via callback to host application | small (URL only) |
| unset (CI/tests) | `local://<uuid>` opaque handle | in-process `DashMap` | small (handle only) |

**Architectural invariant locked in**: the LLM context never holds raw binary
bytes. Outbound (tool → LLM) uses short URLs/handles + the scrubber.
Inbound (LLM → tool) uses `$attachment:<key>` placeholders that the engine
resolves before the request leaves. Echo path (external endpoint → tool →
LLM) is scrubbed at the executor boundary.

### Quality gates at ship

- `cargo test --lib`: **852 pass / 0 fail / 19 ignored**
- `cargo clippy --lib`: clean
- `cargo fmt`: applied
- Smoke E2E with OpenAI gpt-image-1 + LocalHttp adapter: ✅ end-to-end (gen → POST httpbin → finish)
- Smoke E2E of standalone `image_generation` and `tts`: ✅

### What's still PENDING — Phase 7 (private host application repository (consumed downstream))

Lives in `&lt;downstream host repo&gt;`. From this document's perspective,
the colmena side already implements the client contract (`HttpCallbackStorageAdapter`
calls `POST <callback>/sign-put` with `{session_id, agent_session_id, mime_type,
filename, purpose}` and expects `{put_url, read_url, storage_key}` back).

To activate Phase 7 in the host application:

1. **`POST /internal/gcs/sign-put`** endpoint in `apps/api/src/gcs/gcs.controller.ts`
2. **`InternalServiceGuard`** validating `x-internal-token` shared secret
3. **Worker env vars** (`apps/service/ia/platform/worker`):
   ```
   COLMENA_LOCAL=false
   COLMENA_STORAGE_CALLBACK_URL=https://your-host-api/internal/gcs/sign-put
   COLMENA_STORAGE_CALLBACK_SECRET=<shared with InternalServiceGuard>
   ```
4. **`persistColmenaResult`** in `chat.service.ts`: detect tool outputs with
   shape `{attachment_id, url, mime_type, size_bytes}` and INSERT
   `AgentAttachment` rows with the right `messageId` and `source`.
5. **`schema.prisma`**: add `AttachmentSource` enum + nullable migration →
   backfill → NOT NULL (Phase 6 deferred pattern).

When this lands, the colmena worker switches storage from "in-memory" /
"local-http" to the real GCS-backed path with zero code changes — only env
vars flip.

### Files shipped (in this branch, not yet committed)

```
NEW:
  src/libs/colmena/src/storage/                         (entire module)
  src/libs/colmena/src/llm/domain/tts.rs
  src/libs/colmena/src/llm/domain/tts_repository.rs
  src/libs/colmena/src/llm/infrastructure/openai_tts_adapter.rs
  src/libs/colmena/src/llm/infrastructure/elevenlabs_tts_adapter.rs
  src/libs/colmena/src/llm/infrastructure/google_tts_adapter.rs
  src/libs/colmena/src/llm/infrastructure/tts_provider_factory.rs
  src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs
  src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs
  src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs
  tests/graphs/media/image_generation_basic.json
  tests/graphs/media/tts_basic.json
  tests/graphs/media/image_edit_basic.json
  tests/graphs/media/image_gen_then_edit.json
  tests/graphs/agents/multimedia_agent.json
  tests/graphs/agents/multimedia_agent_with_load.json

MODIFIED:
  src/libs/colmena/src/lib.rs                            (pub mod storage)
  src/libs/colmena/src/dag_engine/engine.rs              (EngineConfig storage + attachment_registry + COLMENA_LOCAL guard rail)
  src/libs/colmena/src/dag_engine/infrastructure/registry.rs  (3 media nodes + storage + attachment_registry wiring)
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs (AttachmentResolver lazy upload + storage threading + tool_configurations hard-fail)
  src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs ($attachment:<id> placeholder resolution)
  src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs (binary scrubber + max_tool_result_bytes)
  src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs  (NodeSchemaField.field_type Option migration)
  src/libs/colmena/src/llm/domain/tool_configuration.rs  (type optional when fixed; parse errors propagate)
  src/libs/colmena/src/llm/domain/llm_provider.rs        (ProviderKind::Generated)
  src/libs/colmena/src/llm/domain/mod.rs                 (re-exports for tts + tts_repository)
  src/libs/colmena/src/llm/infrastructure/mod.rs         (re-exports for tts adapters + factory)
  src/libs/colmena/src/llm/infrastructure/files/file_provider_factory.rs  (ProviderKind::Generated branch)
  src/libs/colmena/src/llm/infrastructure/llm_provider_factory.rs         (ProviderKind::Generated branch)
  src/libs/colmena/src/llm/infrastructure/attachment_summary/cheap_tier.rs (ProviderKind::Generated branch)
  src/libs/colmena/Cargo.toml                            (+yup-oauth2)
  docs/node_configurations.json                          (+image_generation, +tts, +image_edit, +media category, +http_request $attachment note)
  docs/agent_context/node_ports_reference.md             (+3 media nodes)
  .env                                                   (+COLMENA_LOCAL block; user-side change)
```

---

## Original design (preserved below for reference)

## Summary
Add three new DAG node types (`image_generation`, `image_edit`, `tts`) usable
both as standalone DAG nodes and as agent tools. Outputs are persisted via a
new `OutputStorageRepository` port; the worker (consuming host application) wires a callback adapter
that asks the host application to issue a signed PUT URL and uploads to GCS — keeping
colmena storage-blind.

## Motivation
Today the LLM module is text-in / text-out (with multimodal *input* but no
*output*). Agents cannot generate images or speech as part of a conversation.
The user-facing flow exists in the host application for inbound files (GCS + signed URLs +
hardening Phase 1-5) but there is no symmetric path for outbound media. This
plan adds that capability while:
- Respecting the invariant that colmena has zero GCS credentials.
- Reusing the host application's existing path layout, ownership validation, MIME allowlist,
  cascade-delete, and orphan-sweep.
- Exposing the new nodes through the existing "nodes-as-tools" pattern so
  agents can call `generate_image()`, `edit_image()`, `synthesize_speech()`
  via `tool_configurations`.

## Architectural Decisions (recorded)
1. **Hybrid node strategy**: `image_generation` and `image_edit` are
   self-contained (A2 pattern, `match provider`); `tts` uses a trait + adapters
   (A1 pattern) because there are three serious providers from day 1.
2. **Outbound storage via callback** (Pattern C): colmena calls the host application
   to get a one-shot signed PUT URL, uploads the bytes, returns the read URL.
   Colmena never sees GCS credentials.
3. **Granular attachment source enum**: `AgentAttachment.source` becomes a
   typed column (`user | image_gen | tts | image_edit`) for downstream
   filtering / billing / retention.
4. **Path layout for outputs**:
   `chat-attachments/<userId>/<agentSessionDbId>/generated/<cuid>-<file>`.
5. **MVP scope**: OpenAI (`gpt-image-1`, `tts-1`), Google Imagen 4 via Vertex,
   ElevenLabs TTS. No STT (Whisper). No async polling. No streaming TTS chunks.
   No cost tracking beyond opaque `usage` blob.

## Architectural Impact
- **Layers affected**: domain, application (light), infrastructure (heavy)
- **New traits/ports**:
  - `colmena::storage::domain::OutputStorageRepository`
  - `colmena::llm::domain::TtsRepository`
- **New adapters**:
  - `LocalCacheStorageAdapter` (CLI/tests)
  - `HttpCallbackStorageAdapter` (worker / downstream host)
  - `OpenAiTtsAdapter`, `ElevenLabsTtsAdapter`, `GoogleTtsAdapter`
- **New nodes**: `image_generation`, `image_edit`, `tts`
- **Modified files**: see "Detailed Steps" below
- **Binding impact**:
  - Python: no surface change initially (nodes are used via DAG JSON)
  - TypeScript: no surface change initially
  - Both bindings get the new node types automatically via registry

## Detailed Steps

### Phase 1 — Storage port (foundation, no functional change yet)

1. Create `src/libs/colmena/src/storage/` module
   - `storage/domain/output_storage_repository.rs` — trait + `StoreRequest`,
     `StoredOutput`, `StorageError`
   - `storage/domain/mod.rs`
   - `storage/infrastructure/local_cache_adapter.rs` — writes to the existing
     `attachment_registry` with `InlineBytes`, returns a synthetic
     `storage_key` (`local://<uuid>`) and a `data:` URL
   - `storage/infrastructure/http_callback_adapter.rs` — `reqwest::Client`;
     POSTs to a callback URL (configured at construction), then PUTs the bytes
     to the returned `put_url`, returns `StoredOutput { storage_key, read_url, ...}`
   - `storage/infrastructure/mod.rs`
   - `storage/mod.rs`
2. Wire `OutputStorageRepository` into the service container
   - `shared/service_container.rs` — add `storage: Arc<dyn OutputStorageRepository>` field
   - Constructor takes the adapter; default for tests is `LocalCacheStorageAdapter`
3. Engine config plumbing
   - `dag_engine/engine.rs` — `EngineConfig::from_env()` checks
     `COLMENA_STORAGE_CALLBACK_URL` and `COLMENA_STORAGE_CALLBACK_SECRET`;
     if both present → `HttpCallbackStorageAdapter`, else → `LocalCacheStorageAdapter`
4. Unit tests
   - `LocalCacheStorageAdapter` round-trip: store → read by `storage_key` returns same bytes
   - `HttpCallbackStorageAdapter` with `httpmock`: validates POST body shape, PUT happens, response shape

### Phase 2 — TTS port and adapters

5. Create `src/libs/colmena/src/llm/domain/tts.rs`
   - `TtsRequest { text, voice, format: AudioFormat, speed: Option<f32>, model }`
   - `TtsResponse { audio_bytes, mime_type, duration_estimate_ms: Option<u64>, usage: Option<Value> }`
   - `AudioFormat { Mp3, Wav, Opus, Pcm }` → `mime_type()` helper
   - `TtsError` (thiserror)
6. Create `src/libs/colmena/src/llm/domain/tts_repository.rs`
   - `trait TtsRepository: Send + Sync { synthesize(req) -> Result<TtsResponse, TtsError>; fn provider_name() -> &'static str; }`
   - `#[cfg_attr(test, mockall::automock)]` for testing
7. Adapters in `src/libs/colmena/src/llm/infrastructure/`
   - `openai_tts_adapter.rs` — `POST /v1/audio/speech`, model `tts-1` | `gpt-4o-mini-tts`,
     voices `alloy|echo|fable|onyx|nova|shimmer`
   - `elevenlabs_tts_adapter.rs` — `POST /v1/text-to-speech/{voice_id}`,
     authorization via `xi-api-key` header
   - `google_tts_adapter.rs` — Gemini TTS via the `generateContent` endpoint
     returning audio inline_data
8. `tts_provider_factory.rs` — `fn build_tts_repository(provider: &str, api_key: &str, model: &str) -> Arc<dyn TtsRepository>`
9. Unit tests per adapter (mock HTTP with `httpmock`):
   - Request body shape matches provider spec
   - Response audio bytes are extracted correctly
   - Errors map to `TtsError` variants

### Phase 3 — Image generation node (A2 pattern)

10. Create `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs`
    - `struct ImageGenerationNode { config_template: Value, http: reqwest::Client, storage: Arc<dyn OutputStorageRepository> }`
    - `ImageGenerationConfig { provider, model, api_key, prompt, size?, quality?, n? }` — `Deserialize`
    - In `execute()`:
      - Resolve `$ref` and secure values
      - `match provider { "openai" => openai_images(...).await?, "google" => vertex_imagen(...).await?, _ => Err }`
      - For each returned image: `storage.store(...)` → push `{attachment_id, url, ...}` to outputs array
    - Helper modules in same file: `mod openai_images { ... }`, `mod google_imagen { ... }`
11. `dag_engine/infrastructure/registry.rs` — register `"image_generation"` → factory
12. `dag_engine/infrastructure/nodes/mod.rs` — `pub mod image_generation;`
13. Tests:
    - Unit: with `MockOutputStorageRepository` + `httpmock` for OpenAI API
    - Integration: JSON graph in `tests/graphs/media/image_generation_basic.json`
      that calls `image_generation` standalone (no agent loop). Marked
      `#[ignore]` (requires real API key).

### Phase 4 — Image edit node (A2, single provider)

14. Create `nodes/image_edit.rs` mirroring `image_generation` structure
    - Config: `provider: "openai"`, `model: "gpt-image-1"`, `api_key`,
      `prompt`, `source_attachment_id` (resolves from attachment_registry),
      `mask_attachment_id` (optional), `size`, `quality`
    - In `execute()`: pull source bytes from registry, build `multipart/form-data`,
      POST `/v1/images/edits`, store output, return shape identical to image_generation
15. Registry + mod.rs wiring
16. Tests analogous to Phase 3

### Phase 5 — TTS node (A1, injected repository)

17. Create `nodes/tts.rs`
    - Config: `provider, model, api_key, text, voice, format?, speed?`
    - Builds `TtsRepository` via factory (provider+api_key+model)
    - Calls `synthesize`, stores audio, returns `{attachment_id, url, duration_ms, mime_type, ...}`
18. Registry + mod.rs wiring
19. Tests:
    - Unit with `MockTtsRepository` + `MockOutputStorageRepository`
    - Integration JSON graph `tests/graphs/media/tts_basic.json` (ignored)

### Phase 6 — Tool exposure samples (no code change in colmena, only docs + sample graphs)

20. Sample agent graph: `tests/graphs/agents/multimedia_agent.json`
    - LLM node with `tool_configurations` exposing `generate_image`, `edit_image`,
      `synthesize_speech` with `node_schema+fixed` for provider/api_key
    - System prompt that encourages the model to call them appropriately
21. Update `docs/node_configurations.json` with the 3 new node types' schemas
22. Update `docs/node_as_tools_reference.json` with worked examples per node
23. New doc: `docs/developer_guide/30_multimedia_generation.md`
    - Overview of the 3 nodes, output shape, storage adapter selection, agent
      usage patterns, error handling, sample graph walkthrough

### Phase 7 — host application side (separate PR in the downstream host repo)

24. `apps/api/src/gcs/gcs.controller.ts` — `POST /internal/gcs/sign-put`
    - Guard: `InternalServiceGuard` (new) — validates `x-internal-token` against
      `process.env.COLMENA_INTERNAL_TOKEN`
    - Body: `{ colmena_session_id, mime_type, filename, purpose: "generated_output" }`
    - Steps:
      - Validate `mime_type` against existing allowlist
      - Look up `AgentSession` by `colmenaSessionId` → derive `userId` and DB `id`
      - Compute key: `chat-attachments/<userId>/<agentSessionDbId>/generated/<cuid>-<sanitized>`
      - Generate signed PUT URL (5min) and signed GET URL (1h)
      - Return `{ put_url, read_url, storage_key }`
25. `apps/service/ia/platform/worker/src/main.rs`
    - Read env: `COLMENA_STORAGE_CALLBACK_URL`, `COLMENA_STORAGE_CALLBACK_SECRET`
    - Construct `HttpCallbackStorageAdapter` and pass to `EngineConfig`
26. `apps/api/src/chat/application/chat.service.ts` — in `persistColmenaResult`
    - Parse `tool_call_finish` events from the run
    - When `result.output` JSON matches `{ attachment_id, url, mime_type, size_bytes }`,
      insert a row into `AgentAttachment` with:
      - `messageId` = current assistant message id
      - `storageKey` = `result.output.attachment_id`
      - `url` = `result.output.url`
      - `mimeType` = `result.output.mime_type`
      - `sizeInBytes` = `result.output.size_bytes`
      - `source` = derived from `tool_name`: `generate_image → image_gen`,
        `edit_image → image_edit`, `synthesize_speech → tts`
27. `packages/database/prisma/schema.prisma`
    - Add enum `AttachmentSource { user image_gen tts image_edit }`
    - Add `AgentAttachment.source AttachmentSource @default(user)`
    - Migration: nullable column first, backfill existing rows to `user`, set NOT NULL
28. Frontend rendering (optional, can be later)
    - `Message.tsx` differentiates `generated` attachments visually
      (small badge "Generated by AI") and shows a play button for audio
      `mime_type` starting with `audio/`

## Testing Strategy

### Colmena
- Unit tests: per adapter (TTS providers, storage adapters, each node)
  using `mockall`, `httpmock`. All marked normal (no API keys needed).
- Integration tests: 3 JSON graphs in `tests/graphs/media/` exercising real
  APIs. All marked `#[ignore = "requires X_API_KEY"]`. Run locally with
  `source .env && cargo test -- --ignored`.
- Tool-loop integration: `tests/graphs/agents/multimedia_agent.json`
  exercises an agent calling `generate_image` as tool, validates `attachment_id`
  shape in output. Ignored (requires LLM + image-gen API keys).

### Host application (out of scope for this repo)
- Unit: `InternalServiceGuard` rejects without token, accepts with valid token
- Unit: `/internal/gcs/sign-put` derives correct path, returns expected shape
- Integration (e2e): start worker against host API, run a DAG with `image_generation`,
  verify a row appears in the host's attachment table with the right `source` value

### Manual verification
- Run a chat in the host application frontend, ask the agent to "generate an image of X"
- Verify image appears inline in the message
- Refresh page → image still renders (URL refresh via existing `getSessionMessages`)
- Wait >1h, refresh → URL is re-signed correctly
- Delete the session → blob is cascade-deleted (existing Phase 4 hardening)

## Documentation Updates
- `docs/node_configurations.json` — schemas for `image_generation`, `image_edit`, `tts`
- `docs/node_as_tools_reference.json` — tool config examples per node
- `docs/agent_context/node_ports_reference.md` — ports & outputs for the 3 nodes
- `docs/developer_guide/30_multimedia_generation.md` — new comprehensive guide
- `docs/DEVELOPER_GUIDE.md` — add section 30 link
- `docs/dds/MODULO_LLM_DISEÑO.md` — appendix on TTS extension
- `docs/dds/MEDIA_GENERATION_DISEÑO.md` — new DDS for the whole subsystem
- Host application repo (out of scope here): document the new "Output flow (generated)" alongside the existing image-upload-flow.md equivalent, and add the callback contract spec to the integration docs.

## Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Image-gen latency (5-20s) blocks agent loop | Bad UX, possible timeout in tool-loop | Set tool-loop timeout per-tool; emit progress events; document expected latency in tool description |
| Callback request fails (API down, network) | Tool returns error, agent retries | Add 1 retry with exponential backoff in `HttpCallbackStorageAdapter`; map to `NodeError` clearly |
| Signed PUT URL expires before colmena uses it (5 min) | First call fails | Generate with 5min TTL; storage adapter uploads immediately after fetching URL (no batching) |
| Mistaken use of `agent_session_id` vs `session_id` for path scope | Outputs land under wrong conversation | Pass `colmena_session_id` only; API does the lookup to `AgentSession` and derives path from DB id. Test that wrong session_id returns 404. |
| Cost overrun (agent generates many images in a loop) | $$$ | Document a max-call limit in the tool description; consider a per-session generation quota at the API layer (out of scope for this plan, flag as follow-up) |
| Generated output gets garbage-collected before user sees it | Broken images in UI | Reuse existing cascade-delete + orphan-sweep; outputs have same retention as user uploads |
| Vertex AI auth (Google Imagen) is harder than API key | Adapter complexity | Document Workload Identity vs service-account JSON as alternatives; start with service-account JSON path; punt to follow-up if too complex for MVP |
| Migration of `AgentAttachment.source` to NOT NULL on a hot table | Lock contention | Phased migration: add column nullable + default → backfill in batches → enforce NOT NULL. Mirrors existing `storageKey` Phase 6 deferred pattern |

## Open Questions

- **`agent_session_id` end-to-end**: host application does not forward `agent_session_id`
  to colmena today (only `session_id`). This works for the storage callback
  because we derive the conversation from `colmenaSessionId` lookup. But if
  Colmena ever wants to do its own conversation-scoped caching of generated
  outputs, this gap matters. **Non-blocking for this plan**, flag for separate
  hardening.
- **Inline retention vs delete-after-LLM-saw-it**: when an agent generates an
  image and uses it in a follow-up `edit_image` call, do we need to re-fetch
  it from GCS or keep an in-memory cache in the run context? **Recommendation**:
  the existing `attachment_registry` already retains InlineBytes within a run;
  outputs go into the registry on creation, eviction happens at run end.
- **Pricing transparency to the user**: should the tool return cost info so the
  agent can warn the user? Day 1: no, just emit `usage` opaquely; can be a
  follow-up after we have a `MediaUsage` value object.
- **Image edit via attachment_id from user upload (not generated)**: a user
  uploads an image and says "edit it". The agent calls `edit_image(source_attachment_id=...)`.
  Does the LLM know the attachment_id for user uploads? It should, via the
  existing `load_attachment_tool` flow. **Verify** this works end-to-end as
  part of the agent integration test.

## Execution
Use `/rust_dev` for Colmena work (Phases 1-6), and the downstream host repo's standard
workflow for Phase 7. Phases 1-2 are foundation and can land first as a
no-functional-change PR; Phases 3-5 each add one node and ship independently;
Phase 7 ships when at least one node is merged.
