# src/libs/colmena/src/llm/infrastructure/llm_provider_factory.rs

**Layer:** infrastructure  
**Purpose:** Factory for instantiating concrete LLM provider adapters (OpenAI, Gemini, Anthropic, Mock) with environment-based base_url configuration and process-global test override via RAII guard pattern.

## Symbols

- `LlmProviderFactory` (struct, pub) — Unit struct serving as namespace for factory methods
- `OVERRIDE` (static, private) — Process-global lazy RwLock<Option<Arc<dyn LlmRepository>>> for test adapter override
- `base_url_override(kind: ProviderKind)` (fn, private) — Resolves provider-specific (OPENAI_BASE_URL, GEMINI_BASE_URL, ANTHROPIC_BASE_URL) or catch-all (COLMENA_LLM_BASE_URL) base_url from environment; Mock and Generated never override
- `LlmProviderFactory::create(kind: ProviderKind)` (pub fn) — Factory method returning Arc<dyn LlmRepository>, checking test override first then dispatching by ProviderKind with optional base_url override
- `LlmProviderFactory::create_all()` (pub fn) — Convenience method returning vec of (OpenAi, Google, Anthropic) provider pairs; excludes Mock and Generated
- `LlmProviderFactory::set_test_override(adapter: Option<Arc<dyn LlmRepository>>)` (pub fn) — Test-only: installs or clears process-global adapter override (must serialize via override_lock)
- `LlmProviderFactory::override_lock()` (pub fn) — Test-only: returns static Mutex<()> for serializing access to set_test_override
- `OverrideGuard<'a>` (struct, pub) — RAII guard holding serialization lock; installs adapter on construction, clears on drop
- `OverrideGuard::install(adapter: Arc<dyn LlmRepository>)` (pub fn) — Constructor acquiring lock, tolerating poisoning via into_inner(), and installing adapter
- `OverrideGuard::drop()` (impl, pub) — Drop impl clearing override and releasing lock
- `base_url_override_tests` (mod, cfg(test)) — Test module for base_url_override behavior
- `base_url_override_tests::ENV_LOCK` (static) — Mutex<()> serializing env-var mutation; coordinated via #[serial(base_url_env)] with FileProviderFactory tests
- `base_url_override_tests::with_clean_env(f: FnOnce())` (fn) — Helper acquiring ENV_LOCK, removing all BASE_URL env vars before and after closure
- `base_url_override_tests::none_when_no_env_set()` (test) — Verifies base_url_override returns None when no env vars set
- `base_url_override_tests::per_provider_var_wins()` (test) — Verifies per-provider var takes precedence over catch-all
- `base_url_override_tests::catchall_used_when_no_per_provider_var()` (test) — Verifies catch-all var used as fallback
- `base_url_override_tests::mock_and_generated_never_overridden()` (test) — Verifies Mock and Generated ProviderKinds are never overridden by env vars

## File-level notes

- **Design: Generated is sentinel** — ProviderKind::Generated (line 64–67) is never intended to be a live LLM provider; it exists for AttachmentRegistry rows and safely returns MockAdapter as a placeholder. Logic error if code path actually calls it.
- **Test serialization discipline** — Two independent locks manage concurrency: `OVERRIDE` (RwLock) guards set_test_override calls; `override_lock()` (Mutex) serializes test setup/teardown. ENV_LOCK (in test module) separately serializes env-var mutations, coordinated with FileProviderFactory tests via #[serial(base_url_env)].
- **Poisoning tolerance** — OverrideGuard::install silently recovers from poisoned Mutex via into_inner() (line 117) with comment justifying it; safe because OVERRIDE RwLock is independently managed and always reset on Drop.
- **No error handling** — All adapter constructors (OpenAiAdapter::new(), GeminiAdapter::with_base_url(), etc.) are assumed infallible; panics would propagate uncaught. This is standard for factory methods but limits graceful degradation.
