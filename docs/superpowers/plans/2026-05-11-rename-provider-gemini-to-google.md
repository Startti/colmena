# Rename Provider `gemini` → `google` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename the LLM provider identifier from `"gemini"` to `"google"` across the codebase (string + enum variant), reflecting that Google — not Gemini — is the actual provider; Gemini is the model family.

**Architecture:** Option **(c) clean cut, no alias**. The user-facing string and the `ProviderKind` Rust variant are renamed. Internal symbols that name the SDK/product (file `gemini_adapter.rs`, struct `GeminiAdapter`, struct `GeminiFilesApiAdapter`, env var `GEMINI_API_KEY`, model identifiers like `gemini-2.5-flash`) **stay as-is** — they reference the product Google ships, not the provider. The Postgres `provider_file_cache` table stores provider strings via `Display`; existing `gemini` rows must be migrated or truncated (file IDs expire in 48 h so the cache repopulates fast).

**Tech Stack:** Rust 1.95.0, `colmena_dag_engine` crate, serde, sqlx (Postgres), DAG engine CLI.

---

## File Structure

**Rust source (single responsibility per file):**

- `src/libs/colmena/src/llm/domain/llm_provider.rs` — enum `ProviderKind`, `Display`, `FromStr`, `default_model`, `env_var_name`. Owns the rename.
- `src/libs/colmena/src/llm/infrastructure/llm_provider_factory.rs` — dispatch `ProviderKind::Google` → `GeminiAdapter`.
- `src/libs/colmena/src/llm/infrastructure/files/file_provider_factory.rs` — dispatch `ProviderKind::Google` → `GeminiFilesApiAdapter`.
- `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs` — `provider_name()` returns `"google"`; internal tests use `ProviderKind::Google`.
- `src/libs/colmena/src/llm/infrastructure/files/gemini_files_api.rs` — error-message string `"gemini"` → `"google"`; tests use `ProviderKind::Google`.
- `src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs` — test array uses `"google"`.
- `src/libs/colmena/src/llm/domain/{llm_config,llm_request,llm_response,file_cache_repository}.rs` — test references to `ProviderKind::Gemini`.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/{llm,critic,extraction,reactor,orchestrator,planner}.rs` — `match` arms parsing the provider string.
- `src/libs/colmena/src/shared/infrastructure/service_container.rs` — references.

**Test/data files:**

- `src/libs/colmena/tests/orchestrator_agent_suspend.rs`, `tests/outbound_masking_integration.rs`, `tests/trip_planner_v2.json` — Rust integration tests.
- `tests/graphs/**/*.json` — ~45 graph fixtures.

**Docs:**

- `docs/node_configurations.json`, `docs/node_as_tools_reference.json` — canonical schemas.
- `docs/developer_guide/{04_adding_providers,14_llm_deep_dive,18_troubleshooting}.md` and other guides.
- `CLAUDE.md` — top-level project doc.

**Memory:**

- `/home/daniel-garcia4/.claude/projects/-home-daniel-garcia4-startti-colmena/memory/` — add a new entry recording the rename.

---

## Task 1: Baseline — capture current green state

**Files:** none (read-only checks).

- [ ] **Step 1: Confirm clean working tree**

Run: `git status`
Expected: `nothing to commit, working tree clean` on branch `develop`.

- [ ] **Step 2: Run the full unit test suite as a baseline**

Run: `cargo test --lib`
Expected: all tests pass. Record the count for comparison after the rename. If anything fails before we touch code, STOP and fix that first.

- [ ] **Step 3: Note the rust-toolchain pin**

Run: `cat rust-toolchain.toml`
Expected: `channel = "1.95.0"` (per `CLAUDE.md`). Make sure your local toolchain matches.

---

## Task 2: Rename `ProviderKind::Gemini` → `Google` in the domain

**Files:**
- Modify: `src/libs/colmena/src/llm/domain/llm_provider.rs:7-58, 119-171`

The compiler will drive the rest of the work in later tasks. This task only edits the domain file and its inline tests.

- [ ] **Step 1: Rewrite the unit tests to assert the new variant/string (failing tests first)**

Edit `src/libs/colmena/src/llm/domain/llm_provider.rs` — replace the entire `#[cfg(test)] mod tests` block (lines 101-172) with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation_success() {
        let provider = LlmProvider::new(
            ProviderKind::OpenAi,
            "test_key".to_string(),
            Some("gpt-4".to_string()),
        )
        .unwrap();

        assert_eq!(*provider.kind(), ProviderKind::OpenAi);
        assert_eq!(provider.api_key(), "test_key");
        assert_eq!(provider.model(), "gpt-4");
    }

    #[test]
    fn test_provider_creation_uses_default_model() {
        let provider =
            LlmProvider::new(ProviderKind::Google, "test_key".to_string(), None).unwrap();

        assert_eq!(*provider.kind(), ProviderKind::Google);
        assert_eq!(provider.model(), ProviderKind::Google.default_model());
    }

    #[test]
    fn test_provider_creation_trims_api_key() {
        let provider =
            LlmProvider::new(ProviderKind::Anthropic, "  spaced_key  ".to_string(), None).unwrap();
        assert_eq!(provider.api_key(), "spaced_key");
    }

    #[test]
    fn test_provider_creation_fails_on_empty_api_key() {
        let result = LlmProvider::new(ProviderKind::OpenAi, "".to_string(), None);
        assert!(matches!(result, Err(LlmError::InvalidApiKey)));

        let result_whitespace = LlmProvider::new(ProviderKind::OpenAi, "   ".to_string(), None);
        assert!(matches!(result_whitespace, Err(LlmError::InvalidApiKey)));
    }

    #[test]
    fn test_provider_kind_from_str() {
        assert_eq!(
            ProviderKind::from_str("openai").unwrap(),
            ProviderKind::OpenAi
        );
        assert_eq!(
            ProviderKind::from_str("Google").unwrap(),
            ProviderKind::Google
        );
        assert_eq!(
            ProviderKind::from_str("ANTHROPIC").unwrap(),
            ProviderKind::Anthropic
        );

        // "gemini" is no longer accepted — clean cut, no alias.
        let result = ProviderKind::from_str("gemini");
        assert!(matches!(result, Err(LlmError::UnsupportedProvider { .. })));

        let result = ProviderKind::from_str("unknown_provider");
        assert!(result.is_err());
        if let Err(LlmError::UnsupportedProvider { provider }) = result {
            assert_eq!(provider, "unknown_provider");
        } else {
            panic!(
                "Expected an UnsupportedProvider error, but got {:?}",
                result
            );
        }
    }

    #[test]
    fn test_provider_kind_display() {
        assert_eq!(ProviderKind::OpenAi.to_string(), "openai");
        assert_eq!(ProviderKind::Google.to_string(), "google");
        assert_eq!(ProviderKind::Anthropic.to_string(), "anthropic");
        assert_eq!(ProviderKind::Mock.to_string(), "mock");
    }

    #[test]
    fn test_provider_kind_env_var_name_preserves_gemini() {
        // The env var name intentionally stays as GEMINI_API_KEY — that is
        // the official name Google uses in its Gemini SDK / docs. Renaming
        // it would confuse users who already have it set.
        assert_eq!(ProviderKind::Google.env_var_name(), "GEMINI_API_KEY");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail to compile**

Run: `cargo test --lib -p colmena_dag_engine llm_provider`
Expected: compile errors — `no variant ProviderKind::Google`, etc.

- [ ] **Step 3: Apply the production rename**

Edit `src/libs/colmena/src/llm/domain/llm_provider.rs` lines 6-58. Replace:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    OpenAi,
    Gemini,
    Anthropic,
    Mock,
}

impl Display for ProviderKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderKind::OpenAi => write!(f, "openai"),
            ProviderKind::Gemini => write!(f, "gemini"),
            ProviderKind::Anthropic => write!(f, "anthropic"),
            ProviderKind::Mock => write!(f, "mock"),
        }
    }
}

impl FromStr for ProviderKind {
    type Err = LlmError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(ProviderKind::OpenAi),
            "gemini" => Ok(ProviderKind::Gemini),
            "anthropic" => Ok(ProviderKind::Anthropic),
            "mock" => Ok(ProviderKind::Mock),
            _ => Err(LlmError::UnsupportedProvider {
                provider: s.to_string(),
            }),
        }
    }
}

impl ProviderKind {
    pub fn default_model(&self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "gpt-4o",
            ProviderKind::Gemini => "gemini-pro",
            ProviderKind::Anthropic => "claude-3-sonnet",
            ProviderKind::Mock => "mock-model",
        }
    }

    pub fn env_var_name(&self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "OPENAI_API_KEY",
            ProviderKind::Gemini => "GEMINI_API_KEY",
            ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
            ProviderKind::Mock => "MOCK_API_KEY",
        }
    }
}
```

with:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    OpenAi,
    Google,
    Anthropic,
    Mock,
}

impl Display for ProviderKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderKind::OpenAi => write!(f, "openai"),
            ProviderKind::Google => write!(f, "google"),
            ProviderKind::Anthropic => write!(f, "anthropic"),
            ProviderKind::Mock => write!(f, "mock"),
        }
    }
}

impl FromStr for ProviderKind {
    type Err = LlmError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(ProviderKind::OpenAi),
            "google" => Ok(ProviderKind::Google),
            "anthropic" => Ok(ProviderKind::Anthropic),
            "mock" => Ok(ProviderKind::Mock),
            _ => Err(LlmError::UnsupportedProvider {
                provider: s.to_string(),
            }),
        }
    }
}

impl ProviderKind {
    pub fn default_model(&self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "gpt-4o",
            // Model identifier stays as "gemini-*" — Gemini is Google's product name.
            ProviderKind::Google => "gemini-pro",
            ProviderKind::Anthropic => "claude-3-sonnet",
            ProviderKind::Mock => "mock-model",
        }
    }

    pub fn env_var_name(&self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "OPENAI_API_KEY",
            // Env var stays as GEMINI_API_KEY — that is Google's official name for the key.
            ProviderKind::Google => "GEMINI_API_KEY",
            ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
            ProviderKind::Mock => "MOCK_API_KEY",
        }
    }
}
```

- [ ] **Step 4: Verify the domain compiles and its own tests pass**

Run: `cargo check -p colmena_dag_engine --lib`
Expected: many errors in OTHER files (every `ProviderKind::Gemini` reference) — that is the work for Tasks 3–6.

Then run just the file's tests, isolated:

Run: `cargo test --lib -p colmena_dag_engine llm_provider`
Expected: still won't compile (whole crate must build first). Move on — we'll get green at the end of Task 6.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/llm/domain/llm_provider.rs
git commit -m "$(cat <<'EOF'
refactor(llm): rename ProviderKind::Gemini to ::Google

Google is the provider; Gemini is the model family. Env var
GEMINI_API_KEY and model names stay (those are Google's official
identifiers).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Update both factories

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/llm_provider_factory.rs:23-40`
- Modify: `src/libs/colmena/src/llm/infrastructure/files/file_provider_factory.rs:30-90`

- [ ] **Step 1: Edit `llm_provider_factory.rs`**

Replace lines 23-40 from:

```rust
        match kind {
            ProviderKind::OpenAi => Arc::new(OpenAiAdapter::new()),
            ProviderKind::Gemini => Arc::new(GeminiAdapter::new()),
            ProviderKind::Anthropic => Arc::new(AnthropicAdapter::new()),
            ProviderKind::Mock => Arc::new(MockAdapter::new()),
        }
    }

    pub fn create_all() -> Vec<(ProviderKind, Arc<dyn LlmRepository>)> {
        vec![
            (ProviderKind::OpenAi, Self::create(ProviderKind::OpenAi)),
            (ProviderKind::Gemini, Self::create(ProviderKind::Gemini)),
            (
                ProviderKind::Anthropic,
                Self::create(ProviderKind::Anthropic),
            ),
        ]
    }
```

to:

```rust
        match kind {
            ProviderKind::OpenAi => Arc::new(OpenAiAdapter::new()),
            ProviderKind::Google => Arc::new(GeminiAdapter::new()),
            ProviderKind::Anthropic => Arc::new(AnthropicAdapter::new()),
            ProviderKind::Mock => Arc::new(MockAdapter::new()),
        }
    }

    pub fn create_all() -> Vec<(ProviderKind, Arc<dyn LlmRepository>)> {
        vec![
            (ProviderKind::OpenAi, Self::create(ProviderKind::OpenAi)),
            (ProviderKind::Google, Self::create(ProviderKind::Google)),
            (
                ProviderKind::Anthropic,
                Self::create(ProviderKind::Anthropic),
            ),
        ]
    }
```

- [ ] **Step 2: Edit `file_provider_factory.rs`**

Open `src/libs/colmena/src/llm/infrastructure/files/file_provider_factory.rs`. Replace every `ProviderKind::Gemini` with `ProviderKind::Google`. There are three occurrences (lines 35, 78, 79 — one production match arm + two test references). Verify with:

Run: `grep -n "ProviderKind::Gemini\|ProviderKind::Google" src/libs/colmena/src/llm/infrastructure/files/file_provider_factory.rs`
Expected: zero `::Gemini`, three `::Google`.

- [ ] **Step 3: Verify both factory crates compile in isolation**

Run: `cargo check -p colmena_dag_engine --lib 2>&1 | grep -E "ProviderKind|llm_provider_factory|file_provider_factory" | head -20`
Expected: remaining `Gemini` errors only come from files outside these two factories (we'll fix them in Tasks 4–6).

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/llm_provider_factory.rs \
        src/libs/colmena/src/llm/infrastructure/files/file_provider_factory.rs
git commit -m "$(cat <<'EOF'
refactor(llm): dispatch ProviderKind::Google in both factories

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Update DAG node provider-string parsers (6 files)

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs:318-326`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/critic.rs:93`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/extraction.rs:60`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/reactor.rs:118`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs:587`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/planner.rs:112`

Each file contains an inline `match provider_str.to_lowercase().as_str()` block. The change is the same in all six: replace `"gemini" => ProviderKind::Gemini` with `"google" => ProviderKind::Google`, and update the error-message string that lists supported providers.

- [ ] **Step 1: Edit `llm.rs` (line ~318-326)**

Replace:

```rust
        let provider_kind = match provider_str.to_lowercase().as_str() {
            "openai" => ProviderKind::OpenAi,
            "gemini" => ProviderKind::Gemini,
            "anthropic" => ProviderKind::Anthropic,
            "mock" => ProviderKind::Mock,
            _ => {
                return Err(format!(
                    "Invalid provider '{}'. Supported: openai, gemini, anthropic, mock",
                    provider_str
                )
                .into());
            }
        };
```

with:

```rust
        let provider_kind = match provider_str.to_lowercase().as_str() {
            "openai" => ProviderKind::OpenAi,
            "google" => ProviderKind::Google,
            "anthropic" => ProviderKind::Anthropic,
            "mock" => ProviderKind::Mock,
            _ => {
                return Err(format!(
                    "Invalid provider '{}'. Supported: openai, google, anthropic, mock",
                    provider_str
                )
                .into());
            }
        };
```

Also update the schema doc block lower in the file. Run:

`grep -n "openai, gemini, anthropic\|openai, google, anthropic" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

If you see any remaining literal listing `gemini`, change it to `google`. Around line 1493 / 1508 there are documentation strings of the form `"provider": "string (openai, gemini, anthropic)"` — update them too.

- [ ] **Step 2: Edit the other five node files**

For each of `critic.rs`, `extraction.rs`, `reactor.rs`, `orchestrator.rs`, `planner.rs`, locate the single match arm:

```rust
"gemini" => ProviderKind::Gemini,
```

and change it to:

```rust
"google" => ProviderKind::Google,
```

Use `grep` to confirm:

Run: `grep -n "\"gemini\"\|::Gemini" src/libs/colmena/src/dag_engine/infrastructure/nodes/{critic,extraction,reactor,orchestrator,planner}.rs`
Expected: no hits remaining.

Also scan each file for any error-message string listing supported providers and update it to `"google"`.

- [ ] **Step 3: Compile and run unit tests for these nodes**

Run: `cargo check -p colmena_dag_engine --lib`
Expected: no errors from these six files.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/critic.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/extraction.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/reactor.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/planner.rs
git commit -m "$(cat <<'EOF'
refactor(dag_engine): parse "google" instead of "gemini" in node providers

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Update remaining Rust references (unit tests + infra strings)

**Files:**
- Modify: `src/libs/colmena/src/llm/domain/llm_config.rs:165, 177`
- Modify: `src/libs/colmena/src/llm/domain/llm_request.rs:134, 155`
- Modify: `src/libs/colmena/src/llm/domain/llm_response.rs:337`
- Modify: `src/libs/colmena/src/llm/domain/file_cache_repository.rs:68, 157, 162, 166, 170`
- Modify: `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs:685, 863, 873, 906, 930`
- Modify: `src/libs/colmena/src/llm/infrastructure/files/gemini_files_api.rs:80, 88, 98, 125, 133, 143, 188, 223, 231, 244, 295`
- Modify: `src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs:348`
- Modify: `src/libs/colmena/src/shared/infrastructure/service_container.rs:48-49`

This is a mechanical sweep. The compiler already enumerated the locations after Task 2.

- [ ] **Step 1: Replace every remaining `ProviderKind::Gemini` with `ProviderKind::Google`**

For files where the only change is the variant name, replace_all is safe. Run (one at a time, NOT a global sed — review each diff):

```
grep -rln "ProviderKind::Gemini" src/libs/colmena/src
```

For each file listed, open it and do find-and-replace `ProviderKind::Gemini` → `ProviderKind::Google`. Files in scope:

- `src/libs/colmena/src/llm/domain/llm_config.rs`
- `src/libs/colmena/src/llm/domain/llm_request.rs`
- `src/libs/colmena/src/llm/domain/llm_response.rs`
- `src/libs/colmena/src/llm/domain/file_cache_repository.rs`
- `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs`
- `src/libs/colmena/src/llm/infrastructure/files/gemini_files_api.rs`
- `src/libs/colmena/src/shared/infrastructure/service_container.rs`

After each edit, verify:

Run: `grep -n "ProviderKind::Gemini" <file>`
Expected: no output.

- [ ] **Step 2: Update string literals `"gemini"` that travel through code paths**

These three places intentionally use the lowercase string (not the enum) and must change to `"google"`:

(a) `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs` line ~685 — the `provider_name()` impl on `GeminiAdapter`:

```rust
        "gemini"
```

→

```rust
        "google"
```

(b) `src/libs/colmena/src/llm/infrastructure/files/gemini_files_api.rs` — 8 occurrences of `provider: "gemini".into()` inside `LlmError::FileApiUploadFailed { provider: ..., message: ... }` (lines 80, 88, 98, 125, 133, 143, 188, 223). Replace all with `provider: "google".into()`. After:

Run: `grep -n "\"gemini\"" src/libs/colmena/src/llm/infrastructure/files/gemini_files_api.rs`
Expected: no output.

(c) `src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs` line 348 — the test array of valid provider strings. Replace:

```rust
        for s in ["anthropic", "openai", "gemini", "mock"] {
```

with:

```rust
        for s in ["anthropic", "openai", "google", "mock"] {
```

- [ ] **Step 3: Run the full unit test suite**

Run: `cargo test --lib -p colmena_dag_engine`
Expected: PASS — every test that exercised `ProviderKind::Gemini` now exercises `::Google`, and the `Display`/`FromStr` round-trip now uses the string `"google"`.

If any test fails, read the error message and fix the spot it points at — do not invent new logic.

- [ ] **Step 4: Run clippy to catch lint drift**

Run: `cargo clippy --lib -p colmena_dag_engine -- -D warnings`
Expected: zero warnings (the crate has `warnings = "deny"`).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/llm/domain/llm_config.rs \
        src/libs/colmena/src/llm/domain/llm_request.rs \
        src/libs/colmena/src/llm/domain/llm_response.rs \
        src/libs/colmena/src/llm/domain/file_cache_repository.rs \
        src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs \
        src/libs/colmena/src/llm/infrastructure/files/gemini_files_api.rs \
        src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs \
        src/libs/colmena/src/shared/infrastructure/service_container.rs
git commit -m "$(cat <<'EOF'
refactor(llm): propagate Google rename through tests and adapter strings

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Update Rust integration tests

**Files:**
- Modify: `src/libs/colmena/tests/orchestrator_agent_suspend.rs` — lines 47, 77, 83 (`"provider": "gemini"` → `"provider": "google"`).
- Modify: `src/libs/colmena/tests/outbound_masking_integration.rs` — line 79.
- Modify: `src/libs/colmena/tests/trip_planner_v2.json` — 8 occurrences of `"provider": "gemini"`.

Model strings (`"gemini-2.5-flash"`) stay.

- [ ] **Step 1: Update `orchestrator_agent_suspend.rs`**

Open the file and replace each `"provider": "gemini"` with `"provider": "google"`. Verify:

Run: `grep -n "\"provider\": \"gemini\"" src/libs/colmena/tests/orchestrator_agent_suspend.rs`
Expected: no output.

- [ ] **Step 2: Update `outbound_masking_integration.rs`**

Same — single occurrence on line 79. Verify with grep as above.

- [ ] **Step 3: Update `trip_planner_v2.json`**

The JSON has 8 `"provider": "gemini"` lines. Replace all with `"provider": "google"`. Verify:

Run: `grep -c "\"provider\": \"gemini\"" src/libs/colmena/tests/trip_planner_v2.json`
Expected: `0`.

Run: `grep -c "\"provider\": \"google\"" src/libs/colmena/tests/trip_planner_v2.json`
Expected: `8`.

- [ ] **Step 4: Run the integration tests (those that don't require live API keys)**

Run: `cargo test --verbose -p colmena_dag_engine`
Expected: PASS. The orchestrator/masking integration tests that DO hit live APIs are likely `#[ignore]`d — see `CLAUDE.md` "`#[ignore]` convention". Run them explicitly only if `.env` is loaded:

Run: `set -a && source .env && set +a && cargo test --verbose -p colmena_dag_engine -- --ignored orchestrator_agent_suspend`
Expected: PASS (live Gemini call succeeds).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/tests/orchestrator_agent_suspend.rs \
        src/libs/colmena/tests/outbound_masking_integration.rs \
        src/libs/colmena/tests/trip_planner_v2.json
git commit -m "$(cat <<'EOF'
test(llm): use "google" provider string in integration tests

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Bulk-update JSON graph fixtures under `tests/graphs/`

**Files:** every JSON under `/home/daniel-garcia4/startti/colmena/tests/graphs/` that currently contains `"provider": "gemini"`. There are ~45 such files. Run the grep at the start of the task to get the live list.

Model identifiers (`"gemini-2.5-flash"`, `"gemini-pro"`, etc.) are NOT touched.

- [ ] **Step 1: Confirm the working set**

Run:

```bash
grep -rln '"provider"[[:space:]]*:[[:space:]]*"gemini"' tests/graphs/ | sort
```

Expected: a list of ~45 files. Capture this exact set — Step 4 verifies the same files now contain `"google"` and no more `"gemini"`.

- [ ] **Step 2: Apply the bulk replace (safe — pattern only matches the provider field)**

Run:

```bash
grep -rl '"provider"[[:space:]]*:[[:space:]]*"gemini"' tests/graphs/ \
  | xargs sed -i 's/"provider"\([[:space:]]*\):\([[:space:]]*\)"gemini"/"provider"\1:\2"google"/g'
```

This regex deliberately matches only `"provider"` followed by `:` and `"gemini"` — it will NOT touch `"model": "gemini-2.5-flash"`, filenames like `pdf_gemini.json`, or comment strings.

- [ ] **Step 3: Verify**

Run:

```bash
grep -rln '"provider"[[:space:]]*:[[:space:]]*"gemini"' tests/graphs/
```

Expected: no output.

Run:

```bash
grep -rln '"provider"[[:space:]]*:[[:space:]]*"google"' tests/graphs/ | wc -l
```

Expected: matches the count from Step 1.

- [ ] **Step 4: Spot-check one graph parses and validates**

Run:

```bash
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json
```

Expected: completes without "Invalid provider" error. (This graph does not use the LLM node, so any error means the engine failed to load — investigate before proceeding.)

- [ ] **Step 5: Commit**

```bash
git add tests/graphs/
git commit -m "$(cat <<'EOF'
test(graphs): rename provider "gemini" to "google" across fixtures

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Smoke-test a representative LLM graph end-to-end

**Files:** none — read-only verification with a real API call.

Per memory note `feedback_demo_defaults.md`, the canonical demo uses Gemini 2.5 Flash. Pick a small graph that hits the real provider.

- [ ] **Step 1: Source API keys**

Run:

```bash
set -a && source .env && set +a
```

Confirm `$GEMINI_API_KEY` is set:

Run: `[ -n "$GEMINI_API_KEY" ] && echo OK || echo MISSING`
Expected: `OK`.

- [ ] **Step 2: Run a single-turn LLM graph**

Run:

```bash
cargo run --bin dag_engine -- run tests/graphs/agents/llm_call.json --agent-session-id cmox2c4ba000n01s66tygjo3d
```

(The `agent_session_id` value comes from memory `feedback_valid_agent_session_id.md` — an FK-valid existing id.)

Expected: a real Gemini response in stdout, no errors. If you see `Invalid provider 'gemini'`, a JSON fixture was missed in Task 7 — `grep -rn '"provider": "gemini"'` and fix.

- [ ] **Step 3: Migrate the `provider_file_cache` table (if DATABASE_URL is set)**

If you have a Postgres `DATABASE_URL` configured (memory page mentions it), update existing rows so the cache stays addressable. Otherwise skip.

Run:

```bash
psql "$DATABASE_URL" -c "UPDATE provider_file_cache SET provider='google' WHERE provider='gemini';"
```

Expected: `UPDATE <n>` (zero or more rows). After: rows that previously keyed on `gemini` now key on `google` and `ProviderKind::from_str` resolves them.

Alternative (acceptable for alpha): `TRUNCATE provider_file_cache` — the 48 h TTL repopulates fast.

- [ ] **Step 4: No commit** (this is a verification task — nothing changed in the working tree).

---

## Task 9: Update canonical docs (`node_configurations.json`, `node_as_tools_reference.json`)

**Files:**
- Modify: `docs/node_configurations.json` — lines 84, 87, 104, 105, 1022, 1130, 1383, 1472, 1473, 1488, 1551, 1627, 1628, 1643, 1726, 1727, 1774, 1775, 1794, 1795, 1813, 1814, and any nearby occurrences.
- Modify: `docs/node_as_tools_reference.json` — line 808 and any neighbours.

These files are referenced by CLAUDE.md as the **canonical schema**, so they MUST be correct.

- [ ] **Step 1: Update `valid_values` arrays in `node_configurations.json`**

Across the file, every `"valid_values": ["openai", "gemini", "anthropic", "mock"]` (and the no-mock variant) becomes `"valid_values": ["openai", "google", "anthropic", "mock"]` (and `["openai", "google", "anthropic"]`).

Run this to verify after editing:

```bash
grep -n '"valid_values"' docs/node_configurations.json | grep -i "gemini"
```

Expected: no output.

- [ ] **Step 2: Update example/default snippets**

Every `"provider": "gemini"` example becomes `"provider": "google"`. Every `"example": "gemini"` becomes `"example": "google"`. Model name examples (`"gemini-2.5-flash"`, `"gemini-pro"`) stay.

Specifically, in `node_configurations.json`:
- The `"default":` string around line 104 (`"Provider-dependent (openai: 'gpt-4o', gemini: 'gemini-pro', ...)"`) becomes `"Provider-dependent (openai: 'gpt-4o', google: 'gemini-pro', ...)"`. Note: the **model** stays `gemini-pro`; only the **provider key** in the string changes.
- The provider-table entry around line 87 (`"gemini": { "default_model": "gemini-pro", "env_var": "GEMINI_API_KEY" }`) becomes `"google": { "default_model": "gemini-pro", "env_var": "GEMINI_API_KEY" }`.

In `node_as_tools_reference.json` line 808-809, `"provider": "gemini"` → `"provider": "google"`. The model `"gemini-2.0-flash"` stays. The filename reference `tests/graphs/web/tavily_llm_gemini.json` (line 612) stays — file names are not in scope.

- [ ] **Step 3: Verify**

Run:

```bash
grep -n '"provider"[[:space:]]*:[[:space:]]*"gemini"' docs/node_configurations.json docs/node_as_tools_reference.json
```

Expected: no output.

```bash
grep -n '"example"[[:space:]]*:[[:space:]]*"gemini"' docs/node_configurations.json
```

Expected: no output (provider-example strings only — model examples like `"gemini-2.5-flash"` are fine).

- [ ] **Step 4: Commit**

```bash
git add docs/node_configurations.json docs/node_as_tools_reference.json
git commit -m "$(cat <<'EOF'
docs(nodes): canonical schema now lists "google" as provider

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Update developer guides + `CLAUDE.md`

**Files:**
- Modify: `docs/developer_guide/04_adding_providers.md`
- Modify: `docs/developer_guide/14_llm_deep_dive.md`
- Modify: `docs/developer_guide/18_troubleshooting.md`
- Modify: `docs/developer_guide/12_dag_engine_guide.md`
- Modify: `docs/developer_guide/13_security_strategy.md`
- Modify: `docs/developer_guide/21_socketio_node.md`
- Modify: `docs/developer_guide/23_sql_node.md`
- Modify: `docs/developer_guide/03_coding_conventions.md`
- Modify: `docs/agent_context/node_ports_reference.md`
- Modify: `docs/dds/MODULO_LLM_DISEÑO.md`
- Modify: `docs/examples/python_usage.md`, `docs/examples/USAGE_EXAMPLES.md`, `docs/examples/amadeus_test.md`
- Modify: `docs/testing/critic_feedback_test_plan.md`
- Modify: `CLAUDE.md` (if any reference exists — verify)

**Out of scope (historical):** anything under `docs/superpowers/plans/`, `docs/superpowers/specs/`, `docs/history/`. Those documents capture state at the time they were written and changing them would falsify the record.

- [ ] **Step 1: List all in-scope docs that mention the gemini provider string**

Run:

```bash
grep -rln '"provider": "gemini"\|provider: gemini\|provider=gemini\|\"gemini\"' docs/developer_guide docs/agent_context docs/dds docs/examples docs/testing CLAUDE.md 2>/dev/null
```

Capture the list.

- [ ] **Step 2: For each file, distinguish the provider field from model/file-name references**

Replace:
- `"provider": "gemini"` → `"provider": "google"`
- `provider: gemini` (YAML / prose) → `provider: google`
- Prose like "the gemini provider" → "the google provider"
- Sentences naming the env var (`GEMINI_API_KEY`) — **keep as-is**.
- Model identifiers (`gemini-2.5-flash`, `gemini-pro`) — **keep as-is**.
- File names (`tavily_llm_gemini.json`, etc.) — **keep as-is**.

In `docs/developer_guide/04_adding_providers.md` specifically, add a brief note clarifying that the provider id is `google` while the env var remains `GEMINI_API_KEY` and the SDK adapter is still `GeminiAdapter`. One short paragraph, no marketing.

- [ ] **Step 3: Verify**

Run:

```bash
grep -rn '"provider"[[:space:]]*:[[:space:]]*"gemini"' docs/developer_guide docs/agent_context docs/dds docs/examples docs/testing 2>/dev/null
```

Expected: no output.

- [ ] **Step 4: Update `CLAUDE.md` if it mentions the provider string**

Run: `grep -n "gemini" CLAUDE.md`

If any line names the provider string (not the model, not the env var), update it. The memory note `feedback_demo_defaults.md` references "Gemini 2.5 Flash" — that's the model, not the provider, and stays.

- [ ] **Step 5: Commit**

```bash
git add docs/ CLAUDE.md
git commit -m "$(cat <<'EOF'
docs: rename provider "gemini" to "google" in developer guides

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Record the rename in auto-memory

**Files:**
- Create: `/home/daniel-garcia4/.claude/projects/-home-daniel-garcia4-startti-colmena/memory/feedback_provider_string_is_google.md`
- Modify: `/home/daniel-garcia4/.claude/projects/-home-daniel-garcia4-startti-colmena/memory/MEMORY.md`

- [ ] **Step 1: Write the memory file**

Create the file with this content:

```markdown
---
name: Provider string is "google", not "gemini"
description: After the 2026-05-11 rename, the LLM provider identifier is "google" — "gemini" is rejected at FromStr.
type: feedback
---

The provider string in graph JSON, the `ProviderKind` Rust variant, and every `match` arm parsing the `provider` field is `google`. `"gemini"` is no longer a valid input — `ProviderKind::from_str` returns `UnsupportedProvider`.

**Why:** Google is the actual provider; Gemini is the model family. The user requested a clean rename without backward-compat aliases.

**How to apply:**
- In any new graph JSON, use `"provider": "google"`.
- Model identifiers (`"gemini-2.5-flash"`, `"gemini-pro"`) stay — those are Google's product names.
- Env var stays `GEMINI_API_KEY` — that is what the Google Gemini SDK officially uses.
- Adapter struct names (`GeminiAdapter`, `GeminiFilesApiAdapter`) and file names (`gemini_adapter.rs`, `gemini_files_api.rs`) stay — they reference the SDK/product, not the provider identity.
- If you see an "Invalid provider 'gemini'" error in CI, a fixture was missed during the rename — `grep -rn '"provider": "gemini"'` and fix.
```

- [ ] **Step 2: Append a one-line pointer to `MEMORY.md`**

Add this line to `MEMORY.md` in the top bullet list (semantic position: near the demo defaults / cargo package name entries):

```markdown
- [Provider string is "google"](feedback_provider_string_is_google.md) — Use `"google"` in graph JSON; `"gemini"` is no longer accepted (env var still `GEMINI_API_KEY`)
```

- [ ] **Step 3: No commit** (memory files live under `~/.claude`, outside the repo).

---

## Task 12: Final verification

**Files:** none — read-only verification.

- [ ] **Step 1: Full test suite (matches CI)**

Run: `cargo test --verbose -p colmena_dag_engine`
Expected: PASS. This includes unit + integration + doctests, which is what CI runs.

- [ ] **Step 2: Clippy with deny-warnings (matches `Cargo.toml` lint config)**

Run: `cargo clippy --lib --tests -p colmena_dag_engine -- -D warnings`
Expected: zero warnings, zero errors.

- [ ] **Step 3: Format check**

Run: `cargo fmt --check`
Expected: no diff. If there is one: `cargo fmt && git add -u && git commit --amend --no-edit`.

- [ ] **Step 4: Sanity-check there are no stray `"gemini"` provider strings left in the repo**

Run:

```bash
grep -rn '"provider"[[:space:]]*:[[:space:]]*"gemini"' src tests docs CLAUDE.md 2>/dev/null
```

Expected: no output.

Run:

```bash
grep -rn "ProviderKind::Gemini" src/libs/colmena/src 2>/dev/null
```

Expected: no output.

- [ ] **Step 5: Smoke-test the canonical LLM demo graph one more time**

Run:

```bash
set -a && source .env && set +a
cargo run --bin dag_engine -- run tests/graphs/agents/llm_call.json --agent-session-id cmox2c4ba000n01s66tygjo3d
```

Expected: real Gemini response, no errors.

- [ ] **Step 6: Optional ignored tests against live APIs**

Run:

```bash
cargo test --verbose -p colmena_dag_engine -- --ignored
```

Expected: tests that hit live Anthropic / OpenAI / Google APIs pass. Skip if you don't want to spend API credit.

---

## Risks & rollback

- **Postgres cache rows with `provider='gemini'`** become unreachable after the rename. Mitigation: Task 8 Step 3 migration. If forgotten, `parse_provider_from_row` will fail-fast with `LlmError::RequestFailed` (per the existing test at `postgres_file_cache.rs:354`) — operator sees the row and migrates.
- **External consumers of the crate** (any service depending on `colmena_dag_engine` from `apps/service/ia/platform/` per the memory note) will break at compile time. This is intentional — the user asked for a clean cut. Coordinate ADP rollout separately.
- **Serde wire format**: `ProviderKind` derives `Serialize, Deserialize`. The default serde variant name (PascalCase) is therefore `"Google"` instead of `"Gemini"` in any serialized form. If anything reads serde-serialized `ProviderKind` from disk/network (currently no known consumers — verified by `grep -rn "serde_json.*ProviderKind"` returning nothing), it will fail. Add a `#[serde(rename_all = ...)]` only if such a consumer surfaces — don't preemptively.

**Rollback:** every task ends with a single self-contained commit, so `git revert <sha>` for the relevant commits undoes the change.
