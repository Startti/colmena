# src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs

**Layer:** infrastructure  
**Purpose:** Text-to-speech synthesis node implementing ExecutableNode trait for OpenAI, ElevenLabs, and Google Gemini TTS providers; stores audio bytes via OutputStorageRepository and registers artifacts in conversation_attachments for downstream tool reference.

## Symbols

- `TtsNode` (struct, pub) — wrapper holding storage, secure values service, attachment registry, and test-only mock repository
- `TtsNode::new()` (fn, pub) — constructs node with storage dependency
- `TtsNode::with_secure_values()` (fn, pub) — builder injects SecureValueService for secret placeholder resolution
- `TtsNode::with_attachment_registry()` (fn, pub) — builder injects AttachmentRegistry for audio artifact registration
- `TtsNode::with_test_repository()` (fn, pub conditional) — test-only builder overrides TTS repository with mock
- `TtsNode::resolve_env_var()` (fn, private) — static helper expands ${VAR} syntax in config strings
- `ExecutableNode::execute()` (fn async, impl) — main entry point: resolves config, calls TTS provider, stores audio bytes, registers artifact, returns document_id handle
- `ExecutableNode::schema()` (fn, impl) — returns JSON schema of config fields (provider, model, api_key, text, voice, format, speed)
- `ExecutableNode::description()` (fn, impl) — returns text describing node purpose and tool-use pattern with $attachment reference
- `ExecutableNode::default_input()` (fn, impl) — returns "text" as primary input key
- `ExecutableNode::default_output()` (fn, impl) — returns "output" as primary output key
- `tests` (mod, private) — integration test suite covering happy path, error cases, config/input merging, attachment registry
- `stored_ok()` (fn, test) — factory returns mock StoredOutput with mp3 mime type
- `audio_resp()` (fn, test) — factory returns mock TtsResponse with 4-byte audio and 500ms duration
- `base_config()` (fn, test) — factory returns minimal valid config (openai tts-1, sk-test key, Spanish text)
- `happy_path_dispatches_to_repo_and_stores_audio()` (test) — verifies synthesize call, storage call, Plan B document_id emission (no attachment_id/url)
- `missing_provider_errors()` (test) — verifies error when provider omitted
- `provider_can_arrive_via_inputs_for_tool_use()` (test) — verifies config={} path merges all infra fields from inputs for LLM tool execution
- `missing_text_errors()` (test) — verifies error when text omitted
- `missing_voice_errors()` (test) — verifies error when voice omitted
- `inputs_text_overrides_config()` (test) — verifies inputs.text takes precedence over config.text for LLM controllability
- `invalid_format_errors()` (test) — verifies error on unrecognized audio format (flac)
- `unknown_provider_errors_via_factory()` (test) — verifies factory rejects unknown provider (nuance)
- `session_ids_forwarded_to_storage()` (test) — verifies engine-injected __colmena_session_id and __colmena_agent_session_id are forwarded to storage adapter
- `tts_auto_registers_artifact_in_registry()` (test) — verifies Plan A: synthesized audio auto-registered in conversation_attachments with generated_by:tts origin, storage_key linkage, and document_id lookup
- `no_registry_means_no_registration_but_still_emits_document_id()` (test) — verifies Plan B: document_id emitted even without registry (no crash on missing registry)
- `env_var_api_key_resolved()` (test) — verifies ${__COLMENA_TEST_TTS_KEY__} syntax expansion in api_key config field

## File-level notes

- **Well-structured builder pattern:** TtsNode uses fluent builders (with_secure_values, with_attachment_registry) following Colmena patterns; test-only mock repository isolated behind `#[cfg(test)]` guard.
- **Config/inputs duality:** execute() implements Colmena's inputs-over-config resolution: infrastructure fields (provider, model, api_key) read from inputs first (for tool-execution mode), then config (for graph-node mode); LLM-controllable fields (text, voice, format, speed) same pattern.
- **Plan A (attachment registry):** synthesized audio auto-registered in conversation_attachments with generated_by:tts origin and storage_key linkage; fail-soft if registry missing (log warning, continue).
- **Plan B (D8):** output removed legacy attachment_id alias and url field; only document_id, mime_type, size_bytes, duration_ms, provider, model emitted. Storage key and read_url still recorded internally on registry row.
- **Secure value injection:** code calls secure_values.inject_secrets() before config resolution, enabling ${SECURE_VALUE:<name>} placeholders alongside ${ENV_VAR}.
- **Test coverage:** 12 tests covering happy path, all error paths, inputs/config precedence, tool-execution path, env var resolution, session ID forwarding, attachment registry integration. Uses mockall mocks for TtsRepository and OutputStorageRepository.
- **No flags detected:** all symbols documented, no dead code, error handling intentional (fail-soft on registry upsert), trait method parameters marked with underscore by design, conditional compilation correct.
