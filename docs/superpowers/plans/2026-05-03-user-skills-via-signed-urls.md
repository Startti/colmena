# User-Provided Skills via Signed URLs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the ADP worker materialize user-declared skills (signed URL or inline) into a per-agent cache before invoking Colmena, plus a small Colmena-side change so a `paths[]` entry can be either a single skill directory or a root containing many.

**Architecture:** ADP worker preprocesses the graph JSON: validates `skills.declared`, downloads or writes inline content under `/tmp/colmena-skills-cache/<agent_id>/<version>/<skill>/`, then rewrites the JSON to standard `skills.paths`. Colmena's `FilesystemSkillRepository::from_paths` is extended to treat path entries as either single skills (existing behavior) or roots whose immediate children with `SKILL.md` are loaded. The worker serializes concurrent materializations of the same skill with an in-process `Mutex` keyed by cache path, and uses temp-dir + atomic rename for crash-safe writes.

**Tech Stack:** Rust 2021, `tokio`, `axum`, `reqwest` (downloads), `dashmap` (per-path mutex map), `serde_yaml` (frontmatter peek), `wiremock` (HTTP mocks in tests), `tempfile` (test cache roots), `colmena_dag_engine` (consumed via git dep). Two repos: `colmena` (this repo) and `adp` (`/home/daniel-garcia4/startti/adp`).

**Cross-repo merge order:** Colmena tasks (1–2) ship first; the new error variant + root-scan must be on `develop` before the ADP worker pins to it. The remaining tasks (3+) are in the ADP repo.

---

## File Structure

### Colmena repo (`/home/daniel-garcia4/startti/colmena/`)

| File | Responsibility | Action |
|---|---|---|
| `src/libs/colmena/src/skills/domain/skill_error.rs` | Domain error type for skills | Modify: add `EmptyRoot` variant |
| `src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs` | Filesystem-based skill loader | Modify: extract `load_skill_dir` helper; auto-detect single vs root in `from_paths` |

### ADP repo (`/home/daniel-garcia4/startti/adp/apps/service/ia/platform/`)

| File | Responsibility | Action |
|---|---|---|
| `shared/src/lib.rs` | `JobRequest` struct shared between API and worker | Modify: add `agent_id: Option<String>` |
| `api/src/handlers.rs` | API entry that enqueues jobs to Redis | Modify: accept and forward `agent_id` |
| `worker/Cargo.toml` | Worker crate dependencies | Modify: add `reqwest`, `dashmap`, `serde_yaml`; dev-deps `wiremock`, `tempfile` |
| `worker/src/skills/mod.rs` | Module entry, `preprocess` orchestrator, error enum | Create |
| `worker/src/skills/types.rs` | `DeclaredSkill`, `DeclaredReference`, `SkillSource` types | Create |
| `worker/src/skills/runtime.rs` | `SkillsRuntime`, `SkillsConfig::from_env`, host whitelist matcher | Create |
| `worker/src/skills/parser.rs` | Walk JSON, extract per-`llm_call` `declared` lists, rewrite `paths` | Create |
| `worker/src/skills/validator.rs` | Shape and limit validations | Create |
| `worker/src/skills/downloader.rs` | `reqwest`-based fetch with whitelist + size cap + timeout | Create |
| `worker/src/skills/cache.rs` | Cache path layout + locking + `ensure_materialized` | Create |
| `worker/src/skills/materializer.rs` | `sanitize_reference_name`, `normalize_skill_md`, `write_atomic` | Create |
| `worker/src/main.rs` | Worker process entry; integrates preprocessor | Modify: build runtime, call `preprocess` from `process_job` |
| `worker/tests/preprocess_e2e.rs` | E2E integration tests with `wiremock` | Create |

---

## Phase 1 — Colmena: extend `FilesystemSkillRepository`

This phase ships first to `develop` of the Colmena repo so the worker can pin to it.

### Task 1: Add `EmptyRoot` error variant

**Files:**
- Modify: `/home/daniel-garcia4/startti/colmena/src/libs/colmena/src/skills/domain/skill_error.rs`

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)] mod tests` block at the bottom of `skill_error.rs`:

```rust
    #[test]
    fn empty_root_display_contains_path() {
        let err = SkillError::EmptyRoot("/tmp/skills-cache".to_string());
        let s = err.to_string();
        assert!(s.contains("/tmp/skills-cache"));
        assert!(s.contains("no SKILL.md"));
        assert!(s.contains("subdirectories"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib -p colmena_dag_engine skills::domain::skill_error::tests::empty_root_display_contains_path`
Expected: FAIL with `error[E0599]: no variant or associated item named 'EmptyRoot' found for enum 'SkillError'`

- [ ] **Step 3: Add the variant**

Insert a new variant in the `SkillError` enum, immediately after `NotADirectory`:

```rust
    #[error("path '{0}' has no SKILL.md and contains no skill subdirectories")]
    EmptyRoot(String),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib -p colmena_dag_engine skills::domain::skill_error`
Expected: PASS (4 tests, including the new one)

- [ ] **Step 5: Commit**

```bash
cd /home/daniel-garcia4/startti/colmena
git add src/libs/colmena/src/skills/domain/skill_error.rs
git commit -m "feat(skills): add EmptyRoot error variant"
```

### Task 2: Extract `load_skill_dir` and add root-scan to `from_paths`

**Files:**
- Modify: `/home/daniel-garcia4/startti/colmena/src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs`

This task does three things in one commit because they are inseparable: extracting the helper enables the second branch in `from_paths` to share validation logic, and the new tests cover both branches together.

- [ ] **Step 1: Write the failing test for root-scan happy path**

Append to the `#[cfg(test)] mod tests` block at the bottom of `filesystem_skill_repository.rs`:

```rust
    #[test]
    fn from_paths_scans_root_directory_with_multiple_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let skill_a = root.join("alpha");
        std::fs::create_dir_all(skill_a.join("references")).unwrap();
        std::fs::write(
            skill_a.join("SKILL.md"),
            "---\nname: alpha\ndescription: a desc\n---\nbody A\n",
        )
        .unwrap();
        let skill_b = root.join("beta");
        std::fs::create_dir_all(&skill_b).unwrap();
        std::fs::write(
            skill_b.join("SKILL.md"),
            "---\nname: beta\ndescription: b desc\n---\nbody B\n",
        )
        .unwrap();
        // A non-skill child must be ignored, not error.
        std::fs::write(root.join("notes.txt"), "ignored").unwrap();

        let repo = FilesystemSkillRepository::from_paths(
            &[root.to_string_lossy().into_owned()],
            tmp.path(),
            &[],
        )
        .expect("root with two skill children must load");

        let names: std::collections::HashSet<String> =
            repo.list_available().into_iter().map(|e| e.name).collect();
        assert!(names.contains("alpha"));
        assert!(names.contains("beta"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn from_paths_empty_root_returns_empty_root_error() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("empty");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("notes.txt"), "no skills here").unwrap();

        let err = FilesystemSkillRepository::from_paths(
            &[root.to_string_lossy().into_owned()],
            tmp.path(),
            &[],
        )
        .expect_err("empty root should error");

        match err {
            SkillError::EmptyRoot(p) => {
                assert!(p.contains("empty"));
            }
            other => panic!("expected EmptyRoot, got {other:?}"),
        }
    }

    #[test]
    fn from_paths_single_skill_still_works() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("solo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: solo\ndescription: d\n---\nbody\n",
        )
        .unwrap();

        let repo = FilesystemSkillRepository::from_paths(
            &[dir.to_string_lossy().into_owned()],
            tmp.path(),
            &[],
        )
        .expect("single skill directory must still load");
        let names: Vec<String> = repo.list_available().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["solo".to_string()]);
    }

    #[test]
    fn from_paths_root_and_single_in_same_array() {
        let tmp = tempfile::tempdir().unwrap();
        // root with one skill
        let root = tmp.path().join("baseline");
        let baseline_skill = root.join("ground");
        std::fs::create_dir_all(&baseline_skill).unwrap();
        std::fs::write(
            baseline_skill.join("SKILL.md"),
            "---\nname: ground\ndescription: g\n---\nx\n",
        )
        .unwrap();
        // separate single skill directory
        let single = tmp.path().join("user-skill");
        std::fs::create_dir_all(&single).unwrap();
        std::fs::write(
            single.join("SKILL.md"),
            "---\nname: user-skill\ndescription: u\n---\nx\n",
        )
        .unwrap();

        let repo = FilesystemSkillRepository::from_paths(
            &[
                root.to_string_lossy().into_owned(),
                single.to_string_lossy().into_owned(),
            ],
            tmp.path(),
            &[],
        )
        .expect("mixed array must load");

        let names: std::collections::HashSet<String> =
            repo.list_available().into_iter().map(|e| e.name).collect();
        assert!(names.contains("ground"));
        assert!(names.contains("user-skill"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn from_paths_collision_across_root_and_single_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("baseline");
        let baseline = root.join("dup");
        std::fs::create_dir_all(&baseline).unwrap();
        std::fs::write(
            baseline.join("SKILL.md"),
            "---\nname: dup\ndescription: g\n---\nx\n",
        )
        .unwrap();
        let single = tmp.path().join("dup");
        std::fs::create_dir_all(&single).unwrap();
        std::fs::write(
            single.join("SKILL.md"),
            "---\nname: dup\ndescription: u\n---\nx\n",
        )
        .unwrap();

        let err = FilesystemSkillRepository::from_paths(
            &[
                root.to_string_lossy().into_owned(),
                single.to_string_lossy().into_owned(),
            ],
            tmp.path(),
            &[],
        )
        .expect_err("collision across root expansion + single must error");
        match err {
            SkillError::SkillNameCollision { name } => assert_eq!(name, "dup"),
            other => panic!("expected SkillNameCollision, got {other:?}"),
        }
    }
```

If `tempfile` is not yet a dev-dependency of the colmena crate, add it. Run:

```bash
grep -A0 'tempfile' /home/daniel-garcia4/startti/colmena/src/libs/colmena/Cargo.toml || true
```

If absent, append to the `[dev-dependencies]` block in `src/libs/colmena/Cargo.toml`:

```toml
tempfile = "3"
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p colmena_dag_engine skills::infrastructure::filesystem_skill_repository -- --nocapture`
Expected: at least three of the new tests FAIL with assertion errors or panic from `SkillMdMissing` (because root scanning is not yet implemented). The single-skill test may pass already.

- [ ] **Step 3: Refactor `from_paths` — extract `load_skill_dir`**

Replace the body of `from_paths` so that the per-skill block (lines 77-152 in the current file: SKILL.md existence, size, parse, name match, references presence/size, dedup, insert) is moved into a new private function `load_skill_dir`, and `from_paths` calls it once for the simple case. Then add the root branch.

Replace the entire `impl FilesystemSkillRepository` block (everything from `pub fn from_paths(...)` to its closing `}` near line 156) with:

```rust
impl FilesystemSkillRepository {
    /// Build from a list of user-supplied paths plus the directory of the graph JSON.
    ///
    /// Each path is auto-detected as one of:
    /// - **single skill directory** if it contains `SKILL.md`,
    /// - **root of skills** if it does not — every immediate child that contains
    ///   `SKILL.md` is loaded; non-skill children are silently ignored. A root
    ///   with zero skill children returns `SkillError::EmptyRoot`.
    pub fn from_paths(
        paths: &[String],
        graph_dir: &Path,
        extra_allowed_dirs: &[PathBuf],
    ) -> Result<Self, SkillError> {
        let mut allowed: Vec<PathBuf> = Vec::new();
        let graph_canonical = graph_dir.canonicalize().map_err(|e| SkillError::Io {
            path: graph_dir.display().to_string(),
            source: e,
        })?;
        allowed.push(graph_canonical.clone());
        for p in extra_allowed_dirs {
            if let Ok(canon) = p.canonicalize() {
                allowed.push(canon);
            }
        }

        let mut skills: HashMap<String, SkillEntry> = HashMap::new();

        for raw_path in paths {
            let path_buf = if Path::new(raw_path).is_absolute() {
                PathBuf::from(raw_path)
            } else {
                graph_dir.join(raw_path)
            };

            let canonical = path_buf.canonicalize().map_err(|e| SkillError::Io {
                path: path_buf.display().to_string(),
                source: e,
            })?;

            let is_allowed = allowed.iter().any(|root| canonical.starts_with(root));
            if !is_allowed {
                return Err(SkillError::PathNotAllowed(canonical.display().to_string()));
            }

            if !canonical.is_dir() {
                return Err(SkillError::NotADirectory(canonical.display().to_string()));
            }

            if canonical.join("SKILL.md").exists() {
                Self::load_skill_dir(&canonical, &mut skills)?;
            } else {
                let mut found = 0usize;
                let read = std::fs::read_dir(&canonical).map_err(|e| SkillError::Io {
                    path: canonical.display().to_string(),
                    source: e,
                })?;
                for entry in read {
                    let entry = entry.map_err(|e| SkillError::Io {
                        path: canonical.display().to_string(),
                        source: e,
                    })?;
                    let sub = entry.path();
                    if sub.is_dir() && sub.join("SKILL.md").exists() {
                        Self::load_skill_dir(&sub, &mut skills)?;
                        found += 1;
                    }
                }
                if found == 0 {
                    return Err(SkillError::EmptyRoot(canonical.display().to_string()));
                }
            }
        }

        Ok(Self { skills })
    }

    /// Load a single skill from a canonical directory containing `SKILL.md`.
    /// Caller must have already verified that the path is allowed and is a directory.
    fn load_skill_dir(
        canonical: &Path,
        skills: &mut HashMap<String, SkillEntry>,
    ) -> Result<(), SkillError> {
        let skill_md_path = canonical.join("SKILL.md");
        let md_size = std::fs::metadata(&skill_md_path)
            .map_err(|e| SkillError::Io {
                path: skill_md_path.display().to_string(),
                source: e,
            })?
            .len();
        if md_size > MAX_FILE_SIZE_BYTES {
            return Err(SkillError::FileTooLarge {
                path: skill_md_path.display().to_string(),
                size: md_size,
                limit: MAX_FILE_SIZE_BYTES,
            });
        }

        let md_content = std::fs::read_to_string(&skill_md_path).map_err(|e| SkillError::Io {
            path: skill_md_path.display().to_string(),
            source: e,
        })?;
        let parsed = parse_skill_md(&md_content, &skill_md_path.display().to_string())?;

        let dir_name = canonical
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<unknown>")
            .to_string();
        if parsed.name != dir_name {
            return Err(SkillError::NameMismatch {
                name: parsed.name,
                dir: dir_name,
                path: canonical.display().to_string(),
            });
        }

        for r in &parsed.references {
            let ref_path = canonical.join("references").join(format!("{}.md", r.name));
            if !ref_path.exists() {
                return Err(SkillError::ReferenceFileMissing {
                    skill: parsed.name.clone(),
                    path: ref_path.display().to_string(),
                });
            }
            let ref_size = std::fs::metadata(&ref_path)
                .map_err(|e| SkillError::Io {
                    path: ref_path.display().to_string(),
                    source: e,
                })?
                .len();
            if ref_size > MAX_FILE_SIZE_BYTES {
                return Err(SkillError::FileTooLarge {
                    path: ref_path.display().to_string(),
                    size: ref_size,
                    limit: MAX_FILE_SIZE_BYTES,
                });
            }
        }

        if skills.contains_key(&parsed.name) {
            return Err(SkillError::SkillNameCollision { name: parsed.name });
        }

        skills.insert(
            parsed.name.clone(),
            SkillEntry {
                canonical_dir: canonical.to_path_buf(),
                description: parsed.description,
                references: parsed.references,
            },
        );
        Ok(())
    }
}
```

- [ ] **Step 4: Run all skill tests to verify they pass**

Run: `cargo test --lib -p colmena_dag_engine skills:: -- --nocapture`
Expected: PASS for all skill tests, including the five new ones added in Step 1 and the pre-existing repository tests.

- [ ] **Step 5: Commit**

```bash
cd /home/daniel-garcia4/startti/colmena
git add src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs src/libs/colmena/Cargo.toml
git commit -m "feat(skills): auto-detect single-skill vs root in from_paths"
```

### Task 3: Push Colmena changes to develop

Per `worker/Cargo.toml:18`, the ADP worker pulls Colmena via `git = "https://github.com/Startti/colmena", branch = "develop"`. Tasks 1–2 must be on `origin/develop` before the worker can pin to them.

- [ ] **Step 1: Verify both tasks compile and the full test suite still passes**

Run: `cargo test --lib -p colmena_dag_engine`
Expected: PASS, no regressions.

- [ ] **Step 2: Push to origin/develop**

```bash
cd /home/daniel-garcia4/startti/colmena
git push origin develop
```

Expected: `develop` updated on GitHub. Note the latest commit SHA — the ADP worker will reference this branch (no pin to a specific SHA is required; cargo will resolve to the tip of `develop` on next `cargo update`).

---

## Phase 2 — ADP cross-crate `agent_id` field

### Task 4: Add `agent_id` to `JobRequest`

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/shared/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Replace the contents of `shared/src/lib.rs` so it includes a test module:

```rust
pub mod config;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JobRequest {
    pub job_id: String,
    pub session_id: Option<String>,
    pub resume_answer: Option<String>,
    pub dag_json: Value,
    pub inputs: Value,
    pub created_at: i64,
    #[serde(default)]
    pub agent_session_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn job_request_roundtrips_with_agent_id() {
        let job = JobRequest {
            job_id: "j1".into(),
            session_id: None,
            resume_answer: None,
            dag_json: json!({}),
            inputs: json!({}),
            created_at: 0,
            agent_session_id: Some("s1".into()),
            agent_id: Some("a1".into()),
        };
        let s = serde_json::to_string(&job).unwrap();
        let back: JobRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.agent_id.as_deref(), Some("a1"));
    }

    #[test]
    fn job_request_deserializes_without_agent_id_field() {
        let s = r#"{"job_id":"j1","dag_json":{},"inputs":{},"created_at":0}"#;
        let job: JobRequest = serde_json::from_str(s).unwrap();
        assert!(job.agent_id.is_none());
    }
}
```

- [ ] **Step 2: Run tests to verify failure (then success)**

Run: `cargo test -p platform_shared`
Expected: PASS for both new tests if `agent_id` was correctly added; otherwise the first test fails to compile (`unknown field`).

- [ ] **Step 3: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/shared/src/lib.rs
git commit -m "feat(shared): add agent_id field to JobRequest"
```

### Task 5: Accept and forward `agent_id` in the API

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/api/src/handlers.rs`

- [ ] **Step 1: Add the field to `CreateExecutionRequest`**

Locate the `pub struct CreateExecutionRequest` block (around lines 23-35) and replace it with:

```rust
#[derive(Deserialize)]
pub struct CreateExecutionRequest {
    pub dag_json: Value,
    pub inputs: Value,
    pub session_id: Option<String>,
    pub resume_answer: Option<String>,
    pub agent_session_id: Option<String>,
    /// Persistent agent identifier. Used by the worker to scope the user-skills
    /// cache (`/tmp/colmena-skills-cache/<agent_id>/...`) so that warm
    /// instances reuse downloads across all conversations of the same agent.
    pub agent_id: Option<String>,
    /// Captures any extra top-level fields in the body (e.g. `user_id`,
    /// `owner_id`) used to populate `tenant_user_id` in SQL nodes that
    /// declare `auto_rls`.
    #[serde(flatten)]
    pub tenant_context: HashMap<String, Value>,
}
```

- [ ] **Step 2: Forward the field into `JobRequest`**

In the same file, locate the `JobRequest { ... }` constructor inside `create_execution` (around lines 61-69) and replace it with:

```rust
    let job = JobRequest {
        job_id: job_id.clone(),
        session_id: payload.session_id,
        resume_answer: payload.resume_answer,
        dag_json: payload.dag_json,
        inputs: payload.inputs,
        created_at: Utc::now().timestamp_millis(),
        agent_session_id: payload.agent_session_id,
        agent_id: payload.agent_id,
    };
```

- [ ] **Step 3: Build to verify**

Run: `cd /home/daniel-garcia4/startti/adp && cargo build -p api`
Expected: clean build, no warnings about unused fields.

- [ ] **Step 4: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/api/src/handlers.rs
git commit -m "feat(api): accept and forward agent_id in CreateExecutionRequest"
```

---

## Phase 3 — ADP worker preprocessor

### Task 6: Add worker dependencies and pin to new Colmena

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/Cargo.toml`

- [ ] **Step 1: Add runtime + dev dependencies**

Open `worker/Cargo.toml` and replace the `[dependencies]` block by extending it with the new entries (keep the existing entries unchanged):

```toml
[dependencies]
tokio = { version = "1.0", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
serde_yaml = "0.9"
redis = { version = "0.24", features = ["tokio-comp"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
futures = "0.3"
dotenv = "0.15"
platform_shared = { path = "../shared" }
axum = "0.7"
tower-http = { version = "0.5", features = ["trace", "cors"] }
colmena = { package = "colmena_dag_engine", git = "https://github.com/Startti/colmena", branch = "develop" }
sqlx = { version = "0.8", features = ["runtime-tokio-rustls", "postgres", "json", "uuid", "chrono"] }
uuid = { version = "1.0", features = ["v4"] }
pyo3 = "0.21"
reqwest = { version = "0.11", default-features = false, features = ["rustls-tls", "json", "stream"] }
dashmap = "5"
async-trait = "0.1"
url = "2"
sha2 = "0.10"

[dev-dependencies]
wiremock = "0.6"
tempfile = "3"
```

- [ ] **Step 2: Force resolver to pick up the new Colmena tip**

Run:

```bash
cd /home/daniel-garcia4/startti/adp
cargo update -p colmena
```

Expected: `Updating git repository ... colmena ... -> <new SHA>`.

- [ ] **Step 3: Build to verify**

Run: `cargo build -p worker`
Expected: clean build (the new deps unused → no warnings; the Colmena update brings in the EmptyRoot variant transitively).

- [ ] **Step 4: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/Cargo.toml Cargo.lock
git commit -m "build(worker): add reqwest/dashmap/serde_yaml deps and bump colmena"
```

### Task 7: Create skills module skeleton with error type

**Files:**
- Create: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/src/skills/mod.rs`

- [ ] **Step 1: Create the module file with the public error and a placeholder `preprocess`**

```rust
//! User-skills preprocessor for the ADP worker.
//!
//! Walks the graph JSON, materializes every entry under
//! `nodes[*].config.skills.declared` to a per-agent cache, and rewrites the
//! JSON to standard `skills.paths` so Colmena's `FilesystemSkillRepository`
//! can read them. The preprocessor runs before the engine is invoked; if
//! anything fails, the job fails before any node is executed.

pub mod cache;
pub mod downloader;
pub mod materializer;
pub mod parser;
pub mod runtime;
pub mod types;
pub mod validator;

use serde_json::Value;
use thiserror::Error;

pub use runtime::{SkillsConfig, SkillsRuntime};

#[derive(Debug, Error)]
pub enum PreprocessError {
    #[error("BadRequest: agent_id required when an llm_call uses skills")]
    AgentIdRequired,

    #[error("BadRequest: skill name '{0}' duplicated within the same llm_call")]
    DuplicateName(String),

    #[error("BadRequest: skill '{0}' source must be exactly one of url or content")]
    SourceAmbiguous(String),

    #[error("BadRequest: invalid url for skill '{0}': {1}")]
    InvalidUrl(String, String),

    #[error("BadRequest: skill '{0}' version invalid")]
    InvalidVersion(String),

    #[error("BadRequest: skill '{skill}' frontmatter name mismatch (frontmatter='{found}', expected='{expected}')")]
    FrontmatterNameMismatch {
        skill: String,
        found: String,
        expected: String,
    },

    #[error("BadRequest: skill '{0}' inline content too large")]
    InlineTooLarge(String),

    #[error("BadRequest: invalid skill name or reference name '{0}': must match [a-zA-Z0-9_-]+")]
    InvalidIdentifier(String),

    #[error("Forbidden: host '{0}' not allowed")]
    HostNotAllowed(String),

    #[error("PayloadTooLarge: skill '{skill}' file '{file}' exceeded {limit} bytes")]
    PayloadTooLarge {
        skill: String,
        file: String,
        limit: usize,
    },

    #[error("PayloadTooLarge: skill '{skill}' total exceeded {limit} bytes")]
    TotalTooLarge { skill: String, limit: usize },

    #[error("BadGateway: failed to fetch skill '{skill}' file '{file}' from {host}: {reason}")]
    Fetch {
        skill: String,
        file: String,
        host: String,
        reason: String,
    },

    #[error("Io: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serde: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Walk the graph JSON, materialize every declared skill, and rewrite
/// `skills.declared` into `skills.paths` on each `llm_call` node.
///
/// Mutates `graph_json` in place. On error, `graph_json` may be left in a
/// partially-rewritten state; callers must discard it (which is the natural
/// path because the entire job is aborted).
pub async fn preprocess(
    graph_json: &mut Value,
    agent_id: Option<&str>,
    runtime: &SkillsRuntime,
) -> Result<(), PreprocessError> {
    parser::preprocess_in_place(graph_json, agent_id, runtime).await
}
```

- [ ] **Step 2: Wire the module from `main.rs` so it compiles**

Open `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/src/main.rs` and add the module declaration at the very top, just below the existing `use` statements:

```rust
mod skills;
```

- [ ] **Step 3: Stub all the sibling files so the `pub mod` declarations resolve**

Create each of the following files with the indicated minimal content. They will be filled in subsequent tasks; this step is just to get the crate compiling.

`worker/src/skills/types.rs`:
```rust
//! Filled in Task 8.
```

`worker/src/skills/runtime.rs`:
```rust
//! Filled in Task 9.

#[derive(Debug, Default, Clone)]
pub struct SkillsConfig;

#[derive(Debug, Default)]
pub struct SkillsRuntime;
```

`worker/src/skills/parser.rs`:
```rust
//! Filled in Task 14.

use serde_json::Value;

pub async fn preprocess_in_place(
    _graph_json: &mut Value,
    _agent_id: Option<&str>,
    _runtime: &super::SkillsRuntime,
) -> Result<(), super::PreprocessError> {
    Ok(())
}
```

`worker/src/skills/validator.rs`:
```rust
//! Filled in Task 10.
```

`worker/src/skills/downloader.rs`:
```rust
//! Filled in Task 11.
```

`worker/src/skills/cache.rs`:
```rust
//! Filled in Task 12.
```

`worker/src/skills/materializer.rs`:
```rust
//! Filled in Task 13.
```

- [ ] **Step 4: Build to verify**

Run: `cd /home/daniel-garcia4/startti/adp && cargo build -p worker`
Expected: clean build. The placeholder `PreprocessError::Io` and `Serde` `#[from]` impls require `thiserror`. If `thiserror` is not yet a dep of worker, add `thiserror = "1"` to `worker/Cargo.toml` `[dependencies]`.

- [ ] **Step 5: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/src/skills/ apps/service/ia/platform/worker/src/main.rs apps/service/ia/platform/worker/Cargo.toml
git commit -m "feat(worker/skills): module skeleton with PreprocessError"
```

### Task 8: `types` — `DeclaredSkill`, `DeclaredReference`, `SkillSource`

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/src/skills/types.rs`

- [ ] **Step 1: Write the failing test**

Replace `types.rs` with:

```rust
//! Strongly-typed view of `skills.declared[*]` from the graph JSON.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct DeclaredSkill {
    pub name: String,
    pub description: String,
    pub version: String,
    pub skill_md: SkillSource,
    #[serde(default)]
    pub references: Vec<DeclaredReference>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeclaredReference {
    pub name: String,
    pub description: String,
    #[serde(flatten)]
    pub source: SkillSource,
}

/// Either a URL to download or inline content. Exactly one variant must be
/// present in the JSON; the validator enforces this.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillSource {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
}

impl SkillSource {
    pub fn is_well_formed(&self) -> bool {
        self.url.is_some() ^ self.content.is_some()
    }
}

/// Parse the `declared` array from a node config value. Returns an empty
/// vec if the array is absent or empty.
pub fn parse_declared(value: &Value) -> Result<Vec<DeclaredSkill>, serde_json::Error> {
    let arr = match value.get("declared") {
        Some(v) => v,
        None => return Ok(Vec::new()),
    };
    serde_json::from_value(arr.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn skill_source_well_formed_xor_check() {
        let url = SkillSource {
            url: Some("https://x".into()),
            content: None,
        };
        let inline = SkillSource {
            url: None,
            content: Some("body".into()),
        };
        let both = SkillSource {
            url: Some("https://x".into()),
            content: Some("body".into()),
        };
        let neither = SkillSource {
            url: None,
            content: None,
        };
        assert!(url.is_well_formed());
        assert!(inline.is_well_formed());
        assert!(!both.is_well_formed());
        assert!(!neither.is_well_formed());
    }

    #[test]
    fn parses_full_declared_block() {
        let cfg = json!({
            "declared": [{
                "name": "amadeus",
                "description": "fares",
                "version": "v1",
                "skill_md": { "url": "https://example.com/skill.md" },
                "references": [
                    { "name": "fare_codes", "description": "iata", "content": "Y=eco" },
                    { "name": "airports", "description": "codes", "url": "https://example.com/a.md" }
                ]
            }]
        });
        let parsed = parse_declared(&cfg).unwrap();
        assert_eq!(parsed.len(), 1);
        let s = &parsed[0];
        assert_eq!(s.name, "amadeus");
        assert_eq!(s.version, "v1");
        assert_eq!(s.skill_md.url.as_deref(), Some("https://example.com/skill.md"));
        assert_eq!(s.references.len(), 2);
        assert_eq!(s.references[0].source.content.as_deref(), Some("Y=eco"));
        assert_eq!(s.references[1].source.url.as_deref(), Some("https://example.com/a.md"));
    }

    #[test]
    fn parses_missing_declared_as_empty() {
        let cfg = json!({});
        let parsed = parse_declared(&cfg).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn parses_empty_declared_as_empty() {
        let cfg = json!({"declared": []});
        let parsed = parse_declared(&cfg).unwrap();
        assert!(parsed.is_empty());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /home/daniel-garcia4/startti/adp && cargo test -p worker skills::types`
Expected: PASS (4 tests).

- [ ] **Step 3: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/src/skills/types.rs
git commit -m "feat(worker/skills): typed view of declared skills"
```

### Task 9: `runtime` — `SkillsConfig::from_env`, `SkillsRuntime`, host whitelist matcher

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/src/skills/runtime.rs`

- [ ] **Step 1: Write the failing test (host matcher)**

Replace `runtime.rs` with:

```rust
//! Process-wide runtime for the skills preprocessor: HTTP client, lock map,
//! and parsed configuration from environment variables.

use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct SkillsConfig {
    pub cache_root: PathBuf,
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
    pub http_timeout: Duration,
    pub allowed_hosts: Vec<HostMatcher>,
}

#[derive(Debug, Clone)]
pub enum HostMatcher {
    /// Exact-match hostname.
    Exact(String),
    /// Wildcard matching one left-most subdomain label only, e.g.
    /// `*.r2.cloudflarestorage.com` matches `acme.r2.cloudflarestorage.com`
    /// but not `acme.staging.r2.cloudflarestorage.com`.
    Wildcard { tail: String },
}

impl HostMatcher {
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        if let Some(rest) = raw.strip_prefix("*.") {
            if rest.is_empty() {
                return None;
            }
            Some(HostMatcher::Wildcard {
                tail: rest.to_string(),
            })
        } else {
            Some(HostMatcher::Exact(raw.to_string()))
        }
    }

    pub fn matches(&self, host: &str) -> bool {
        match self {
            HostMatcher::Exact(s) => s.eq_ignore_ascii_case(host),
            HostMatcher::Wildcard { tail } => {
                // host must end with ".<tail>" and the substring before that dot
                // must contain no further dots.
                let needle = format!(".{}", tail);
                if !host.to_ascii_lowercase().ends_with(&needle.to_ascii_lowercase()) {
                    return false;
                }
                let prefix_len = host.len().saturating_sub(needle.len());
                let prefix = &host[..prefix_len];
                !prefix.is_empty() && !prefix.contains('.')
            }
        }
    }
}

impl SkillsConfig {
    pub fn from_env() -> Result<Self, String> {
        let cache_root = std::env::var("COLMENA_SKILLS_CACHE_ROOT")
            .unwrap_or_else(|_| "/tmp/colmena-skills-cache".to_string());
        let max_file_bytes = parse_usize_env("COLMENA_SKILLS_MAX_FILE_BYTES", 65_536)?;
        let max_total_bytes = parse_usize_env("COLMENA_SKILLS_MAX_TOTAL_BYTES", 524_288)?;
        let timeout_ms = parse_usize_env("COLMENA_SKILLS_HTTP_TIMEOUT_MS", 15_000)?;
        let allowed_hosts: Vec<HostMatcher> = std::env::var("COLMENA_SKILLS_ALLOWED_HOSTS")
            .unwrap_or_default()
            .split(',')
            .filter_map(HostMatcher::parse)
            .collect();
        Ok(SkillsConfig {
            cache_root: PathBuf::from(cache_root),
            max_file_bytes,
            max_total_bytes,
            http_timeout: Duration::from_millis(timeout_ms as u64),
            allowed_hosts,
        })
    }

    pub fn host_allowed(&self, host: &str) -> bool {
        self.allowed_hosts.iter().any(|m| m.matches(host))
    }
}

fn parse_usize_env(name: &str, default: usize) -> Result<usize, String> {
    match std::env::var(name) {
        Ok(v) => v
            .parse::<usize>()
            .map_err(|e| format!("{name}: invalid usize: {e}")),
        Err(_) => Ok(default),
    }
}

pub struct SkillsRuntime {
    pub http: reqwest::Client,
    pub locks: Arc<DashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    pub config: SkillsConfig,
}

impl SkillsRuntime {
    pub fn from_env() -> Result<Self, String> {
        let config = SkillsConfig::from_env()?;
        let http = reqwest::Client::builder()
            .timeout(config.http_timeout)
            .build()
            .map_err(|e| format!("reqwest client build: {e}"))?;
        Ok(Self {
            http,
            locks: Arc::new(DashMap::new()),
            config,
        })
    }
}

impl std::fmt::Debug for SkillsRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillsRuntime")
            .field("config", &self.config)
            .field("locks_size", &self.locks.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_matcher_exact() {
        let m = HostMatcher::parse("storage.googleapis.com").unwrap();
        assert!(m.matches("storage.googleapis.com"));
        assert!(!m.matches("evil.com"));
        assert!(!m.matches("sub.storage.googleapis.com"));
    }

    #[test]
    fn host_matcher_wildcard_one_label() {
        let m = HostMatcher::parse("*.r2.cloudflarestorage.com").unwrap();
        assert!(m.matches("acme.r2.cloudflarestorage.com"));
        // Two-label prefix is rejected:
        assert!(!m.matches("acme.staging.r2.cloudflarestorage.com"));
        // Empty prefix rejected:
        assert!(!m.matches(".r2.cloudflarestorage.com"));
        assert!(!m.matches("r2.cloudflarestorage.com"));
    }

    #[test]
    fn host_matcher_case_insensitive() {
        let m = HostMatcher::parse("Storage.GoogleApis.com").unwrap();
        assert!(m.matches("storage.googleapis.com"));
        assert!(m.matches("STORAGE.GOOGLEAPIS.COM"));
    }

    #[test]
    fn host_matcher_invalid_inputs() {
        assert!(HostMatcher::parse("").is_none());
        assert!(HostMatcher::parse("   ").is_none());
        assert!(HostMatcher::parse("*.").is_none());
    }

    #[test]
    fn skills_config_defaults_when_env_absent() {
        // Snapshot and clear env that could affect the test.
        for k in [
            "COLMENA_SKILLS_CACHE_ROOT",
            "COLMENA_SKILLS_MAX_FILE_BYTES",
            "COLMENA_SKILLS_MAX_TOTAL_BYTES",
            "COLMENA_SKILLS_HTTP_TIMEOUT_MS",
            "COLMENA_SKILLS_ALLOWED_HOSTS",
        ] {
            std::env::remove_var(k);
        }
        let cfg = SkillsConfig::from_env().unwrap();
        assert_eq!(cfg.cache_root, PathBuf::from("/tmp/colmena-skills-cache"));
        assert_eq!(cfg.max_file_bytes, 65_536);
        assert_eq!(cfg.max_total_bytes, 524_288);
        assert_eq!(cfg.http_timeout, Duration::from_millis(15_000));
        assert!(cfg.allowed_hosts.is_empty());
        assert!(!cfg.host_allowed("storage.googleapis.com"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /home/daniel-garcia4/startti/adp && cargo test -p worker skills::runtime`
Expected: PASS (5 tests).

- [ ] **Step 3: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/src/skills/runtime.rs
git commit -m "feat(worker/skills): runtime with host whitelist matcher"
```

### Task 10: `validator` — shape and limit checks

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/src/skills/validator.rs`

- [ ] **Step 1: Write the failing tests + implementation together**

Replace `validator.rs` with:

```rust
//! Pre-flight shape and limit validations for declared skills.

use super::types::{DeclaredReference, DeclaredSkill, SkillSource};
use super::{PreprocessError, SkillsConfig};

const MAX_VERSION_LEN: usize = 64;
const MAX_NAME_LEN: usize = 64;

/// Validate a single declared skill in isolation. Cross-skill duplication
/// checks happen one level up in the parser.
pub fn check(entry: &DeclaredSkill, cfg: &SkillsConfig) -> Result<(), PreprocessError> {
    check_identifier(&entry.name)?;
    check_version(&entry.name, &entry.version)?;
    check_source(&entry.name, "skill_md", &entry.skill_md, cfg)?;
    for r in &entry.references {
        check_reference(&entry.name, r, cfg)?;
    }
    Ok(())
}

fn check_identifier(s: &str) -> Result<(), PreprocessError> {
    if s.is_empty() || s.len() > MAX_NAME_LEN {
        return Err(PreprocessError::InvalidIdentifier(s.to_string()));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(PreprocessError::InvalidIdentifier(s.to_string()));
    }
    Ok(())
}

fn check_version(skill: &str, v: &str) -> Result<(), PreprocessError> {
    if v.is_empty() || v.len() > MAX_VERSION_LEN {
        return Err(PreprocessError::InvalidVersion(skill.to_string()));
    }
    if !v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(PreprocessError::InvalidVersion(skill.to_string()));
    }
    Ok(())
}

fn check_source(
    skill: &str,
    file_label: &str,
    src: &SkillSource,
    cfg: &SkillsConfig,
) -> Result<(), PreprocessError> {
    if !src.is_well_formed() {
        return Err(PreprocessError::SourceAmbiguous(skill.to_string()));
    }
    if let Some(content) = &src.content {
        if content.len() > cfg.max_file_bytes {
            return Err(PreprocessError::PayloadTooLarge {
                skill: skill.to_string(),
                file: file_label.to_string(),
                limit: cfg.max_file_bytes,
            });
        }
    }
    if let Some(url) = &src.url {
        let parsed = url::Url::parse(url)
            .map_err(|e| PreprocessError::InvalidUrl(skill.to_string(), e.to_string()))?;
        if parsed.scheme() != "https" {
            return Err(PreprocessError::InvalidUrl(
                skill.to_string(),
                format!("scheme '{}' not allowed; must be https", parsed.scheme()),
            ));
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| PreprocessError::InvalidUrl(skill.to_string(), "missing host".into()))?;
        if !cfg.host_allowed(host) {
            return Err(PreprocessError::HostNotAllowed(host.to_string()));
        }
    }
    Ok(())
}

fn check_reference(
    skill: &str,
    r: &DeclaredReference,
    cfg: &SkillsConfig,
) -> Result<(), PreprocessError> {
    check_identifier(&r.name)?;
    check_source(skill, &r.name, &r.source, cfg)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::runtime::HostMatcher;

    fn cfg_with(allowed_hosts: Vec<&str>, max_file: usize) -> SkillsConfig {
        SkillsConfig {
            cache_root: std::path::PathBuf::from("/tmp"),
            max_file_bytes: max_file,
            max_total_bytes: max_file * 4,
            http_timeout: std::time::Duration::from_secs(1),
            allowed_hosts: allowed_hosts
                .into_iter()
                .filter_map(HostMatcher::parse)
                .collect(),
        }
    }

    fn skill(name: &str, src: SkillSource) -> DeclaredSkill {
        DeclaredSkill {
            name: name.to_string(),
            description: "d".into(),
            version: "v1".into(),
            skill_md: src,
            references: Vec::new(),
        }
    }

    #[test]
    fn happy_path_url_passes() {
        let cfg = cfg_with(vec!["example.com"], 1024);
        let s = skill(
            "ok",
            SkillSource {
                url: Some("https://example.com/skill.md".into()),
                content: None,
            },
        );
        assert!(check(&s, &cfg).is_ok());
    }

    #[test]
    fn happy_path_inline_passes() {
        let cfg = cfg_with(vec![], 1024);
        let s = skill(
            "ok",
            SkillSource {
                url: None,
                content: Some("body".into()),
            },
        );
        assert!(check(&s, &cfg).is_ok());
    }

    #[test]
    fn rejects_ambiguous_source() {
        let cfg = cfg_with(vec!["example.com"], 1024);
        let s = skill(
            "ok",
            SkillSource {
                url: Some("https://example.com/x".into()),
                content: Some("body".into()),
            },
        );
        assert!(matches!(check(&s, &cfg), Err(PreprocessError::SourceAmbiguous(_))));
    }

    #[test]
    fn rejects_missing_source() {
        let cfg = cfg_with(vec![], 1024);
        let s = skill(
            "ok",
            SkillSource {
                url: None,
                content: None,
            },
        );
        assert!(matches!(check(&s, &cfg), Err(PreprocessError::SourceAmbiguous(_))));
    }

    #[test]
    fn rejects_non_https_scheme() {
        let cfg = cfg_with(vec!["example.com"], 1024);
        let s = skill(
            "ok",
            SkillSource {
                url: Some("http://example.com/x".into()),
                content: None,
            },
        );
        assert!(matches!(check(&s, &cfg), Err(PreprocessError::InvalidUrl(_, _))));
    }

    #[test]
    fn rejects_host_not_in_whitelist() {
        let cfg = cfg_with(vec!["example.com"], 1024);
        let s = skill(
            "ok",
            SkillSource {
                url: Some("https://evil.com/x".into()),
                content: None,
            },
        );
        assert!(matches!(check(&s, &cfg), Err(PreprocessError::HostNotAllowed(_))));
    }

    #[test]
    fn rejects_inline_too_large() {
        let cfg = cfg_with(vec![], 8);
        let s = skill(
            "ok",
            SkillSource {
                url: None,
                content: Some("a".repeat(16)),
            },
        );
        assert!(matches!(check(&s, &cfg), Err(PreprocessError::PayloadTooLarge { .. })));
    }

    #[test]
    fn rejects_invalid_identifier() {
        let cfg = cfg_with(vec![], 1024);
        let s = skill(
            "bad name!",
            SkillSource {
                url: None,
                content: Some("x".into()),
            },
        );
        assert!(matches!(check(&s, &cfg), Err(PreprocessError::InvalidIdentifier(_))));
    }

    #[test]
    fn rejects_empty_version() {
        let cfg = cfg_with(vec![], 1024);
        let mut s = skill(
            "ok",
            SkillSource {
                url: None,
                content: Some("x".into()),
            },
        );
        s.version = "".into();
        assert!(matches!(check(&s, &cfg), Err(PreprocessError::InvalidVersion(_))));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /home/daniel-garcia4/startti/adp && cargo test -p worker skills::validator`
Expected: PASS (9 tests).

- [ ] **Step 3: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/src/skills/validator.rs
git commit -m "feat(worker/skills): shape and limit validator"
```

### Task 11: `downloader` — `reqwest` fetch with whitelist + size cap + timeout

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/src/skills/downloader.rs`

- [ ] **Step 1: Write the implementation with tests**

Replace `downloader.rs` with:

```rust
//! HTTP fetcher used to materialize URL-backed skill files.
//!
//! Enforces the worker-side limits (host whitelist already pre-checked by the
//! validator, total-size cap to bound runaway streaming, per-file timeout
//! configured on the reqwest::Client).

use super::{PreprocessError, SkillsRuntime};
use futures::StreamExt;

/// Download `url` into a `Vec<u8>`. Aborts and returns `PayloadTooLarge` if
/// the streamed body exceeds `runtime.config.max_file_bytes`. The HTTP
/// timeout is set globally on the client at runtime build time.
///
/// `skill` and `file_label` are propagated into the error so the operator can
/// pinpoint which entry failed without having to parse the URL back out.
pub async fn fetch(
    url: &str,
    skill: &str,
    file_label: &str,
    runtime: &SkillsRuntime,
) -> Result<Vec<u8>, PreprocessError> {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_else(|| "<unparsable>".to_string());

    let resp = runtime
        .http
        .get(url)
        .send()
        .await
        .map_err(|e| PreprocessError::Fetch {
            skill: skill.to_string(),
            file: file_label.to_string(),
            host: host.clone(),
            reason: e.to_string(),
        })?;

    if !resp.status().is_success() {
        return Err(PreprocessError::Fetch {
            skill: skill.to_string(),
            file: file_label.to_string(),
            host,
            reason: format!("HTTP {}", resp.status()),
        });
    }

    let cap = runtime.config.max_file_bytes;
    let mut buf = Vec::with_capacity(1024.min(cap));
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| PreprocessError::Fetch {
            skill: skill.to_string(),
            file: file_label.to_string(),
            host: host.clone(),
            reason: e.to_string(),
        })?;
        if buf.len() + chunk.len() > cap {
            return Err(PreprocessError::PayloadTooLarge {
                skill: skill.to_string(),
                file: file_label.to_string(),
                limit: cap,
            });
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::runtime::{HostMatcher, SkillsConfig};
    use dashmap::DashMap;
    use std::sync::Arc;
    use std::time::Duration;
    use wiremock::{matchers::any, Mock, MockServer, ResponseTemplate};

    fn runtime_for(server: &MockServer, cap: usize) -> SkillsRuntime {
        let host = server.uri().replace("http://", "").replace("https://", "");
        let host = host.split(':').next().unwrap().to_string();
        let cfg = SkillsConfig {
            cache_root: std::path::PathBuf::from("/tmp"),
            max_file_bytes: cap,
            max_total_bytes: cap * 4,
            http_timeout: Duration::from_secs(2),
            allowed_hosts: vec![HostMatcher::Exact(host)],
        };
        SkillsRuntime {
            http: reqwest::Client::builder()
                .timeout(cfg.http_timeout)
                .build()
                .unwrap(),
            locks: Arc::new(DashMap::new()),
            config: cfg,
        }
    }

    #[tokio::test]
    async fn fetch_ok_returns_body_bytes() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello".as_slice()))
            .mount(&server)
            .await;
        let rt = runtime_for(&server, 1024);
        let url = format!("{}/x.md", server.uri());
        let bytes = fetch(&url, "skill", "skill_md", &rt).await.unwrap();
        assert_eq!(&bytes[..], b"hello");
    }

    #[tokio::test]
    async fn fetch_non_2xx_maps_to_fetch_error() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let rt = runtime_for(&server, 1024);
        let url = format!("{}/x.md", server.uri());
        let err = fetch(&url, "skill", "f", &rt).await.unwrap_err();
        match err {
            PreprocessError::Fetch { reason, .. } => {
                assert!(reason.contains("503"), "got reason: {reason}");
            }
            other => panic!("expected Fetch, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_oversized_body_maps_to_payload_too_large() {
        let server = MockServer::start().await;
        let big = vec![0u8; 64];
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_bytes(big))
            .mount(&server)
            .await;
        let rt = runtime_for(&server, 16);
        let url = format!("{}/x.md", server.uri());
        let err = fetch(&url, "skill", "f", &rt).await.unwrap_err();
        assert!(matches!(err, PreprocessError::PayloadTooLarge { .. }));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /home/daniel-garcia4/startti/adp && cargo test -p worker skills::downloader`
Expected: PASS (3 tests). Note: tests start a real HTTP server on a random port via `wiremock`. No network egress.

- [ ] **Step 3: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/src/skills/downloader.rs
git commit -m "feat(worker/skills): downloader with size-cap streaming"
```

### Task 12: `cache` — path layout, `ensure_materialized`, locking

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/src/skills/cache.rs`

- [ ] **Step 1: Write the implementation with tests**

Replace `cache.rs` with:

```rust
//! Cache layout and atomic materialization for declared skills.

use super::types::DeclaredSkill;
use super::{materializer, PreprocessError, SkillsRuntime};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Cache directory for one (agent, skill, version) tuple.
pub fn path_for(cache_root: &Path, agent_id: &str, skill: &str, version: &str) -> PathBuf {
    cache_root.join(agent_id).join(version).join(skill)
}

/// Returns true if the cache directory already contains a populated skill.
pub fn is_cached(cache_dir: &Path) -> bool {
    cache_dir.is_dir() && cache_dir.join("SKILL.md").exists()
}

/// Ensure the cache directory is populated for the given skill. Idempotent;
/// concurrent callers are serialized per cache-path with an in-process Mutex.
///
/// On cache hit (directory + SKILL.md exist) returns immediately.
/// On cache miss writes via `materializer::write_atomic`, which is crash-safe.
pub async fn ensure_materialized(
    cache_dir: &Path,
    entry: &DeclaredSkill,
    runtime: &SkillsRuntime,
) -> Result<(), PreprocessError> {
    if is_cached(cache_dir) {
        return Ok(());
    }
    let lock = runtime
        .locks
        .entry(cache_dir.to_path_buf())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;
    if is_cached(cache_dir) {
        return Ok(());
    }
    materializer::write_atomic(cache_dir, entry, runtime).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_for_layout() {
        let root = Path::new("/tmp/colmena-skills-cache");
        let p = path_for(root, "agent-1", "amadeus", "v1");
        assert_eq!(
            p.to_string_lossy(),
            "/tmp/colmena-skills-cache/agent-1/v1/amadeus"
        );
    }

    #[test]
    fn is_cached_false_for_missing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("not-there");
        assert!(!is_cached(&p));
    }

    #[test]
    fn is_cached_false_for_dir_without_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("dir");
        std::fs::create_dir_all(&p).unwrap();
        assert!(!is_cached(&p));
    }

    #[test]
    fn is_cached_true_when_skill_md_present() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("dir");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("SKILL.md"), "x").unwrap();
        assert!(is_cached(&p));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /home/daniel-garcia4/startti/adp && cargo test -p worker skills::cache`
Expected: PASS (4 tests). The `ensure_materialized` integration cases happen in the E2E suite later — they require the materializer wired in.

- [ ] **Step 3: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/src/skills/cache.rs
git commit -m "feat(worker/skills): cache path layout and per-path locking"
```

### Task 13: `materializer` — `sanitize_reference_name`, `normalize_skill_md`, `write_atomic`

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/src/skills/materializer.rs`

- [ ] **Step 1: Write `sanitize_reference_name` with tests**

Start the file with:

```rust
//! Atomically materializes one declared skill into the on-disk cache.
//!
//! Layout produced (relative to the cache directory passed in):
//!   SKILL.md
//!   references/<ref_name>.md   (one per declared reference)

use super::types::{DeclaredReference, DeclaredSkill, SkillSource};
use super::{downloader, PreprocessError, SkillsRuntime};
use std::path::{Path, PathBuf};

const MAX_NAME_LEN: usize = 64;

/// Reject filename components that could escape the `references/` directory.
pub fn sanitize_reference_name(name: &str) -> Result<&str, PreprocessError> {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(PreprocessError::InvalidIdentifier(name.to_string()));
    }
    if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return Err(PreprocessError::InvalidIdentifier(name.to_string()));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(PreprocessError::InvalidIdentifier(name.to_string()));
    }
    Ok(name)
}
```

Then add tests at the bottom of the file (we will append more code in subsequent steps before this `#[cfg(test)]` block, but for now place it where it can stay):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_accepts_simple_name() {
        assert_eq!(sanitize_reference_name("fare_codes").unwrap(), "fare_codes");
        assert_eq!(sanitize_reference_name("ABC-123").unwrap(), "ABC-123");
    }

    #[test]
    fn sanitize_rejects_traversal() {
        assert!(sanitize_reference_name("..").is_err());
        assert!(sanitize_reference_name(".").is_err());
        assert!(sanitize_reference_name("foo/bar").is_err());
        assert!(sanitize_reference_name("foo\\bar").is_err());
    }

    #[test]
    fn sanitize_rejects_empty_or_too_long() {
        assert!(sanitize_reference_name("").is_err());
        let long = "a".repeat(65);
        assert!(sanitize_reference_name(&long).is_err());
    }

    #[test]
    fn sanitize_rejects_special_chars() {
        assert!(sanitize_reference_name("foo bar").is_err());
        assert!(sanitize_reference_name("foo!").is_err());
        assert!(sanitize_reference_name("foo@bar").is_err());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /home/daniel-garcia4/startti/adp && cargo test -p worker skills::materializer::tests::sanitize`
Expected: PASS (4 tests).

- [ ] **Step 3: Add `normalize_skill_md` with tests**

In `materializer.rs`, append the function before the `#[cfg(test)]` block:

```rust
/// Ensure the SKILL.md content has a frontmatter block whose `name` matches
/// `entry.name`. If the input has no frontmatter, prepend one generated from
/// the JSON metadata. If it does, validate the frontmatter `name` only —
/// other fields are not inspected (Colmena's parser will validate them
/// authoritatively at engine load time).
pub fn normalize_skill_md(
    raw: &[u8],
    entry: &DeclaredSkill,
) -> Result<Vec<u8>, PreprocessError> {
    let text = std::str::from_utf8(raw).map_err(|_| {
        PreprocessError::FrontmatterNameMismatch {
            skill: entry.name.clone(),
            found: "<non-utf8 input>".into(),
            expected: entry.name.clone(),
        }
    })?;

    if let Some(rest) = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n")) {
        // Find the closing `---` line.
        let (front, _body) = match find_frontmatter_end(rest) {
            Some(idx) => (&rest[..idx], &rest[idx..]),
            None => {
                return Err(PreprocessError::FrontmatterNameMismatch {
                    skill: entry.name.clone(),
                    found: "<unterminated frontmatter>".into(),
                    expected: entry.name.clone(),
                });
            }
        };
        let yaml: serde_yaml::Value = serde_yaml::from_str(front).map_err(|e| {
            PreprocessError::FrontmatterNameMismatch {
                skill: entry.name.clone(),
                found: format!("<yaml parse error: {e}>"),
                expected: entry.name.clone(),
            }
        })?;
        let found_name = yaml
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if found_name != entry.name {
            return Err(PreprocessError::FrontmatterNameMismatch {
                skill: entry.name.clone(),
                found: found_name,
                expected: entry.name.clone(),
            });
        }
        return Ok(raw.to_vec());
    }

    // No frontmatter: synthesize one from the JSON metadata.
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("name: {}\n", entry.name));
    out.push_str(&format!(
        "description: {}\n",
        yaml_escape_scalar(&entry.description)
    ));
    if !entry.references.is_empty() {
        out.push_str("references:\n");
        for r in &entry.references {
            out.push_str(&format!("  - name: {}\n", r.name));
            out.push_str(&format!(
                "    description: {}\n",
                yaml_escape_scalar(&r.description)
            ));
        }
    }
    out.push_str("---\n");
    out.push_str(text);
    Ok(out.into_bytes())
}

fn find_frontmatter_end(rest: &str) -> Option<usize> {
    // Look for a line that is exactly "---" (followed by either newline or EOF).
    let mut idx = 0usize;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" {
            return Some(idx + line.len());
        }
        idx += line.len();
    }
    None
}

fn yaml_escape_scalar(s: &str) -> String {
    // Conservative quoting: if the value contains any character that could
    // break plain YAML, double-quote and escape backslashes/quotes.
    let needs_quote = s.is_empty()
        || s.contains(':')
        || s.contains('#')
        || s.contains('\n')
        || s.contains('"')
        || s.starts_with(' ')
        || s.ends_with(' ');
    if !needs_quote {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}
```

Append to the test module in the same file:

```rust
    fn skill_with_refs(name: &str, refs: &[(&str, &str)]) -> DeclaredSkill {
        DeclaredSkill {
            name: name.to_string(),
            description: "desc".into(),
            version: "v1".into(),
            skill_md: SkillSource {
                url: None,
                content: Some(String::new()),
            },
            references: refs
                .iter()
                .map(|(n, d)| DeclaredReference {
                    name: n.to_string(),
                    description: d.to_string(),
                    source: SkillSource {
                        url: None,
                        content: Some(String::new()),
                    },
                })
                .collect(),
        }
    }

    #[test]
    fn normalize_prepends_frontmatter_when_absent() {
        let entry = skill_with_refs("amadeus", &[("fare_codes", "iata"), ("airports", "codes")]);
        let raw = b"# Body without frontmatter\n";
        let out = normalize_skill_md(raw, &entry).unwrap();
        let s = std::str::from_utf8(&out).unwrap();
        assert!(s.starts_with("---\n"));
        assert!(s.contains("name: amadeus"));
        assert!(s.contains("description: desc"));
        assert!(s.contains("- name: fare_codes"));
        assert!(s.contains("- name: airports"));
        assert!(s.ends_with("# Body without frontmatter\n"));
    }

    #[test]
    fn normalize_preserves_when_frontmatter_name_matches() {
        let entry = skill_with_refs("amadeus", &[]);
        let raw = b"---\nname: amadeus\ndescription: x\n---\nbody\n";
        let out = normalize_skill_md(raw, &entry).unwrap();
        assert_eq!(out, raw);
    }

    #[test]
    fn normalize_rejects_when_frontmatter_name_mismatches() {
        let entry = skill_with_refs("amadeus", &[]);
        let raw = b"---\nname: other\ndescription: x\n---\nbody\n";
        let err = normalize_skill_md(raw, &entry).unwrap_err();
        match err {
            PreprocessError::FrontmatterNameMismatch { found, expected, .. } => {
                assert_eq!(found, "other");
                assert_eq!(expected, "amadeus");
            }
            other => panic!("expected FrontmatterNameMismatch, got {other:?}"),
        }
    }

    #[test]
    fn normalize_rejects_unterminated_frontmatter() {
        let entry = skill_with_refs("amadeus", &[]);
        let raw = b"---\nname: amadeus\nno terminator here";
        let err = normalize_skill_md(raw, &entry).unwrap_err();
        assert!(matches!(err, PreprocessError::FrontmatterNameMismatch { .. }));
    }

    #[test]
    fn yaml_escape_quotes_when_needed() {
        assert_eq!(yaml_escape_scalar("plain"), "plain");
        assert_eq!(yaml_escape_scalar("has:colon"), "\"has:colon\"");
        assert_eq!(yaml_escape_scalar("with \"quote\""), "\"with \\\"quote\\\"\"");
    }
```

- [ ] **Step 4: Run tests**

Run: `cd /home/daniel-garcia4/startti/adp && cargo test -p worker skills::materializer`
Expected: PASS (9 tests so far — 4 sanitize + 5 normalize-related).

- [ ] **Step 5: Add `write_atomic` and supporting helpers**

Append to `materializer.rs`, immediately before the `#[cfg(test)]` block:

```rust
/// Atomically materialize a declared skill into `cache_dir`.
///
/// Writes everything to a sibling temp directory named
/// `<cache_dir>.tmp.<random>/`, then renames it onto `cache_dir`. If another
/// caller has already published the same target, the temp dir is removed and
/// `Ok(())` is returned (the existing cache is good).
pub async fn write_atomic(
    cache_dir: &Path,
    entry: &DeclaredSkill,
    runtime: &SkillsRuntime,
) -> Result<(), PreprocessError> {
    let tmp_dir = tmp_sibling_path(cache_dir);
    if let Some(parent) = tmp_dir.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::create_dir_all(&tmp_dir).await?;
    tokio::fs::create_dir_all(tmp_dir.join("references")).await?;

    let mut total: usize = 0;

    // SKILL.md
    let raw = source_bytes(&entry.name, "skill_md", &entry.skill_md, runtime).await?;
    let normalized = normalize_skill_md(&raw, entry)?;
    if normalized.len() > runtime.config.max_file_bytes {
        cleanup(&tmp_dir).await;
        return Err(PreprocessError::PayloadTooLarge {
            skill: entry.name.clone(),
            file: "skill_md".to_string(),
            limit: runtime.config.max_file_bytes,
        });
    }
    tokio::fs::write(tmp_dir.join("SKILL.md"), &normalized).await?;
    total += normalized.len();

    // References
    for r in &entry.references {
        let safe = sanitize_reference_name(&r.name)?;
        let body = source_bytes(&entry.name, &r.name, &r.source, runtime).await?;
        if body.len() > runtime.config.max_file_bytes {
            cleanup(&tmp_dir).await;
            return Err(PreprocessError::PayloadTooLarge {
                skill: entry.name.clone(),
                file: r.name.clone(),
                limit: runtime.config.max_file_bytes,
            });
        }
        tokio::fs::write(tmp_dir.join("references").join(format!("{safe}.md")), &body).await?;
        total += body.len();
        if total > runtime.config.max_total_bytes {
            cleanup(&tmp_dir).await;
            return Err(PreprocessError::TotalTooLarge {
                skill: entry.name.clone(),
                limit: runtime.config.max_total_bytes,
            });
        }
    }

    match tokio::fs::rename(&tmp_dir, cache_dir).await {
        Ok(_) => Ok(()),
        Err(_) if cache_dir.exists() => {
            // Another job won the race. Discard our temp dir; their result is fine.
            cleanup(&tmp_dir).await;
            Ok(())
        }
        Err(e) => {
            cleanup(&tmp_dir).await;
            Err(PreprocessError::Io(e))
        }
    }
}

async fn source_bytes(
    skill: &str,
    file_label: &str,
    src: &SkillSource,
    runtime: &SkillsRuntime,
) -> Result<Vec<u8>, PreprocessError> {
    match (&src.url, &src.content) {
        (Some(u), None) => downloader::fetch(u, skill, file_label, runtime).await,
        (None, Some(c)) => Ok(c.as_bytes().to_vec()),
        _ => Err(PreprocessError::SourceAmbiguous(skill.to_string())),
    }
}

fn tmp_sibling_path(cache_dir: &Path) -> PathBuf {
    let parent = cache_dir.parent().map(Path::to_path_buf).unwrap_or_default();
    let leaf = cache_dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "skill".to_string());
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    parent.join(format!("{leaf}.tmp.{suffix}"))
}

async fn cleanup(dir: &Path) {
    let _ = tokio::fs::remove_dir_all(dir).await;
}
```

Append additional tests for `write_atomic` (still inside the existing test module):

```rust
    use std::path::PathBuf;

    fn runtime_inline_only(cap: usize) -> SkillsRuntime {
        use crate::skills::runtime::SkillsConfig;
        SkillsRuntime {
            http: reqwest::Client::new(),
            locks: std::sync::Arc::new(dashmap::DashMap::new()),
            config: SkillsConfig {
                cache_root: PathBuf::from("/tmp"),
                max_file_bytes: cap,
                max_total_bytes: cap * 4,
                http_timeout: std::time::Duration::from_secs(1),
                allowed_hosts: vec![],
            },
        }
    }

    #[tokio::test]
    async fn write_atomic_inline_skill_with_references() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache").join("v1").join("amadeus");
        let entry = DeclaredSkill {
            name: "amadeus".to_string(),
            description: "desc".to_string(),
            version: "v1".to_string(),
            skill_md: SkillSource {
                url: None,
                content: Some("---\nname: amadeus\ndescription: d\nreferences:\n  - name: codes\n    description: cd\n---\nbody\n".to_string()),
            },
            references: vec![DeclaredReference {
                name: "codes".to_string(),
                description: "cd".to_string(),
                source: SkillSource {
                    url: None,
                    content: Some("Y=eco".to_string()),
                },
            }],
        };
        let rt = runtime_inline_only(1024);
        write_atomic(&cache_dir, &entry, &rt).await.unwrap();

        assert!(cache_dir.join("SKILL.md").exists());
        assert!(cache_dir.join("references").join("codes.md").exists());
        let body = std::fs::read_to_string(cache_dir.join("references").join("codes.md")).unwrap();
        assert_eq!(body, "Y=eco");
    }

    #[tokio::test]
    async fn write_atomic_total_cap_aborts_and_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache").join("v1").join("big");
        let entry = DeclaredSkill {
            name: "big".to_string(),
            description: "d".to_string(),
            version: "v1".to_string(),
            skill_md: SkillSource {
                url: None,
                content: Some("a".repeat(8)),
            },
            references: vec![DeclaredReference {
                name: "ref1".to_string(),
                description: "d".to_string(),
                source: SkillSource {
                    url: None,
                    content: Some("a".repeat(8)),
                },
            }],
        };
        let mut rt = runtime_inline_only(8);
        rt.config.max_total_bytes = 10;
        let err = write_atomic(&cache_dir, &entry, &rt).await.unwrap_err();
        assert!(matches!(err, PreprocessError::TotalTooLarge { .. }));
        // No leftover temp dir or final dir:
        assert!(!cache_dir.exists());
        let parent_listing: Vec<_> = std::fs::read_dir(cache_dir.parent().unwrap())
            .map(|d| d.flatten().collect())
            .unwrap_or_default();
        assert!(parent_listing.is_empty());
    }

    #[tokio::test]
    async fn write_atomic_rejects_traversal_in_reference_name() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache").join("v1").join("evil");
        let entry = DeclaredSkill {
            name: "evil".to_string(),
            description: "d".to_string(),
            version: "v1".to_string(),
            skill_md: SkillSource {
                url: None,
                content: Some(String::new()),
            },
            references: vec![DeclaredReference {
                name: "../escape".to_string(),
                description: "d".to_string(),
                source: SkillSource {
                    url: None,
                    content: Some("x".to_string()),
                },
            }],
        };
        let rt = runtime_inline_only(1024);
        let err = write_atomic(&cache_dir, &entry, &rt).await.unwrap_err();
        assert!(matches!(err, PreprocessError::InvalidIdentifier(_)));
    }
```

- [ ] **Step 6: Run tests**

Run: `cd /home/daniel-garcia4/startti/adp && cargo test -p worker skills::materializer`
Expected: PASS (12 tests: 4 sanitize + 5 normalize-related + 3 write_atomic).

- [ ] **Step 7: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/src/skills/materializer.rs
git commit -m "feat(worker/skills): atomic materializer with frontmatter normalization"
```

### Task 14: `parser` — extract `declared` and rewrite `paths`

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/src/skills/parser.rs`

- [ ] **Step 1: Implement the parser**

Replace `parser.rs` with:

```rust
//! Walks the graph JSON and orchestrates per-node skill materialization.

use super::cache;
use super::types::{parse_declared, DeclaredSkill};
use super::validator;
use super::{PreprocessError, SkillsRuntime};
use serde_json::{Map, Value};
use std::collections::HashSet;

/// Scan, validate, materialize, and rewrite. Mutates `graph_json` in place.
pub async fn preprocess_in_place(
    graph_json: &mut Value,
    agent_id: Option<&str>,
    runtime: &SkillsRuntime,
) -> Result<(), PreprocessError> {
    // 1) Discover.
    let mut any_skills_used = false;
    let mut work: Vec<(String, Vec<DeclaredSkill>)> = Vec::new();

    let nodes = graph_json
        .get("nodes")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    for (node_id, node_v) in nodes.iter() {
        let Some(skills_v) = node_v
            .get("config")
            .and_then(|c| c.get("skills"))
        else {
            continue;
        };
        if !is_llm_call(node_v) {
            continue;
        }
        if has_any_skill_field(skills_v) {
            any_skills_used = true;
        }
        let declared = parse_declared(skills_v).map_err(PreprocessError::Serde)?;
        if !declared.is_empty() {
            work.push((node_id.clone(), declared));
        }
    }

    if any_skills_used && agent_id.is_none() {
        return Err(PreprocessError::AgentIdRequired);
    }
    let agent_id = agent_id.unwrap_or("");

    // 2) Validate (no I/O).
    for (_, entries) in &work {
        let mut seen: HashSet<&str> = HashSet::new();
        for e in entries {
            if !seen.insert(e.name.as_str()) {
                return Err(PreprocessError::DuplicateName(e.name.clone()));
            }
            validator::check(e, &runtime.config)?;
        }
    }

    // 3) Materialize and collect paths per node.
    let mut paths_to_append: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for (node_id, entries) in &work {
        for entry in entries {
            let cache_dir = cache::path_for(
                &runtime.config.cache_root,
                agent_id,
                &entry.name,
                &entry.version,
            );
            cache::ensure_materialized(&cache_dir, entry, runtime).await?;
            paths_to_append
                .entry(node_id.clone())
                .or_default()
                .push(cache_dir.to_string_lossy().into_owned());
        }
    }

    // 4) Rewrite the JSON in place.
    let nodes_mut = graph_json
        .get_mut("nodes")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| {
            PreprocessError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "graph json missing 'nodes' object",
            ))
        })?;
    for (node_id, paths) in paths_to_append {
        let Some(node_v) = nodes_mut.get_mut(&node_id) else {
            continue;
        };
        let Some(config) = node_v.get_mut("config").and_then(|c| c.as_object_mut()) else {
            continue;
        };
        let skills = config
            .entry("skills".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(skills_obj) = skills.as_object_mut() else {
            continue;
        };
        let mut existing_paths: Vec<Value> = skills_obj
            .remove("paths")
            .and_then(|v| match v {
                Value::Array(a) => Some(a),
                _ => None,
            })
            .unwrap_or_default();
        for p in paths {
            existing_paths.push(Value::String(p));
        }
        skills_obj.insert("paths".to_string(), Value::Array(existing_paths));
        skills_obj.remove("declared");
    }

    Ok(())
}

fn is_llm_call(node_v: &Value) -> bool {
    node_v
        .get("type")
        .and_then(|v| v.as_str())
        .map(|t| t == "llm_call")
        .unwrap_or(false)
}

fn has_any_skill_field(skills_v: &Value) -> bool {
    let Some(obj) = skills_v.as_object() else {
        return false;
    };
    let nonempty = |key: &str| -> bool {
        obj.get(key)
            .map(|v| match v {
                Value::Array(a) => !a.is_empty(),
                _ => false,
            })
            .unwrap_or(false)
    };
    nonempty("builtin") || nonempty("paths") || nonempty("declared")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::runtime::{SkillsConfig, HostMatcher};
    use dashmap::DashMap;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::tempdir;

    fn runtime_inline(tmp: &std::path::Path) -> SkillsRuntime {
        SkillsRuntime {
            http: reqwest::Client::new(),
            locks: Arc::new(DashMap::new()),
            config: SkillsConfig {
                cache_root: tmp.to_path_buf(),
                max_file_bytes: 1024,
                max_total_bytes: 4096,
                http_timeout: Duration::from_secs(1),
                allowed_hosts: vec![HostMatcher::Exact("never.invalid".into())],
            },
        }
    }

    fn graph_with_one_llm_inline_skill() -> Value {
        json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": {
                        "skills": {
                            "paths": ["/app/skills"],
                            "declared": [{
                                "name": "amadeus",
                                "description": "d",
                                "version": "v1",
                                "skill_md": { "content": "body" }
                            }]
                        }
                    }
                }
            }
        })
    }

    #[tokio::test]
    async fn preprocess_inline_only_rewrites_paths_and_removes_declared() {
        let tmp = tempdir().unwrap();
        let rt = runtime_inline(tmp.path());
        let mut g = graph_with_one_llm_inline_skill();
        preprocess_in_place(&mut g, Some("agent-1"), &rt).await.unwrap();

        let paths = g["nodes"]["agent"]["config"]["skills"]["paths"]
            .as_array()
            .unwrap();
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], "/app/skills");
        assert!(paths[1].as_str().unwrap().ends_with("/agent-1/v1/amadeus"));
        assert!(g["nodes"]["agent"]["config"]["skills"]
            .get("declared")
            .is_none());
    }

    #[tokio::test]
    async fn preprocess_requires_agent_id_when_skills_block_present() {
        let tmp = tempdir().unwrap();
        let rt = runtime_inline(tmp.path());
        let mut g = graph_with_one_llm_inline_skill();
        let err = preprocess_in_place(&mut g, None, &rt).await.unwrap_err();
        assert!(matches!(err, PreprocessError::AgentIdRequired));
    }

    #[tokio::test]
    async fn preprocess_no_skills_no_op() {
        let tmp = tempdir().unwrap();
        let rt = runtime_inline(tmp.path());
        let mut g = json!({
            "nodes": {
                "agent": { "type": "llm_call", "config": {} }
            }
        });
        let before = g.clone();
        preprocess_in_place(&mut g, None, &rt).await.unwrap();
        assert_eq!(before, g);
    }

    #[tokio::test]
    async fn preprocess_dedups_within_node() {
        let tmp = tempdir().unwrap();
        let rt = runtime_inline(tmp.path());
        let mut g = json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": {
                        "skills": {
                            "declared": [
                                { "name": "x", "description": "d", "version": "v1", "skill_md": {"content": "a"} },
                                { "name": "x", "description": "d", "version": "v2", "skill_md": {"content": "b"} }
                            ]
                        }
                    }
                }
            }
        });
        let err = preprocess_in_place(&mut g, Some("a"), &rt).await.unwrap_err();
        assert!(matches!(err, PreprocessError::DuplicateName(s) if s == "x"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /home/daniel-garcia4/startti/adp && cargo test -p worker skills::parser`
Expected: PASS (4 tests).

- [ ] **Step 3: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/src/skills/parser.rs
git commit -m "feat(worker/skills): parser walks graph and rewrites declared->paths"
```

### Task 15: Wire preprocessor into `process_job`

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/src/main.rs`

- [ ] **Step 1: Add `SkillsRuntime` to `AppState` and build it in `main`**

Locate the `AppState` struct definition (around lines 37-41). Replace it with:

```rust
#[derive(Clone)]
struct AppState {
    redis: Arc<redis::Client>,
    engine: Arc<ColmenaEngine>,
    skills_runtime: Arc<skills::SkillsRuntime>,
}
```

In `main()`, immediately after the engine is built (right after `let engine = Arc::new(...);`), add:

```rust
    let skills_runtime = Arc::new(
        skills::SkillsRuntime::from_env()
            .map_err(|e| format!("skills runtime: {}", e))?,
    );
```

Then update the `let state = AppState { ... };` block to include the new field:

```rust
    let state = AppState {
        redis: Arc::new(client),
        engine: engine.clone(),
        skills_runtime,
    };
```

- [ ] **Step 2: Plumb the preprocessor into `process_job`**

Update the signature of `process_job` to take a `&Arc<skills::SkillsRuntime>`. Find the existing signature:

```rust
async fn process_job(
    job: &platform_shared::JobRequest,
    redis_con: &mut redis::aio::Connection,
    engine: &ColmenaEngine,
) -> Result<(), Box<dyn std::error::Error>> {
```

Replace it with:

```rust
async fn process_job(
    job: &platform_shared::JobRequest,
    redis_con: &mut redis::aio::Connection,
    engine: &ColmenaEngine,
    skills_runtime: &skills::SkillsRuntime,
) -> Result<(), Box<dyn std::error::Error>> {
```

Update the corresponding caller `process_jobs_inline`. Replace the line `if let Err(e) = process_job(&job, &mut con, &state.engine).await {` with:

```rust
                        if let Err(e) = process_job(&job, &mut con, &state.engine, &state.skills_runtime).await {
```

- [ ] **Step 3: Call the preprocessor between input injection and graph deserialization**

Inside `process_job`, locate the comment `// 1.5 Deserialize Graph` (around line 218). Insert a new block immediately above it:

```rust
    // 1.7 NEW: Materialize user-declared skills before Colmena sees the graph.
    if let Err(e) = skills::preprocess(
        &mut graph_json,
        job.agent_id.as_deref(),
        skills_runtime,
    )
    .await
    {
        let msg = format!("{}", e);
        emit_error!(msg);
        return Err(msg.into());
    }
```

Now the order in `process_job` is: inject inputs → materialize skills → deserialize Graph → execute_stream.

- [ ] **Step 4: Build to verify**

Run: `cd /home/daniel-garcia4/startti/adp && cargo build -p worker`
Expected: clean build.

- [ ] **Step 5: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/src/main.rs
git commit -m "feat(worker): wire skills preprocessor into process_job"
```

---

## Phase 4 — End-to-end integration tests

### Task 16: E2E test scaffold + inline-only happy path

**Files:**
- Create: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/tests/preprocess_e2e.rs`

- [ ] **Step 1: Write the scaffold + first test**

```rust
//! End-to-end tests for the worker's skill preprocessor. These tests do not
//! involve Redis or the Colmena engine — they exercise `skills::preprocess`
//! directly with a real on-disk cache and a `wiremock` server when URLs are
//! involved.

use dashmap::DashMap;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use worker::skills::{self, runtime::HostMatcher, runtime::SkillsConfig, SkillsRuntime};

fn runtime(cache_root: PathBuf, allowed_hosts: Vec<HostMatcher>, max_file: usize) -> SkillsRuntime {
    SkillsRuntime {
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap(),
        locks: Arc::new(DashMap::new()),
        config: SkillsConfig {
            cache_root,
            max_file_bytes: max_file,
            max_total_bytes: max_file * 8,
            http_timeout: Duration::from_secs(2),
            allowed_hosts,
        },
    }
}

#[tokio::test]
async fn inline_skill_only_writes_files_and_rewrites_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = runtime(tmp.path().to_path_buf(), vec![], 1024);

    let mut graph = json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "skills": {
                        "declared": [{
                            "name": "policies",
                            "description": "Refund policies",
                            "version": "v1",
                            "skill_md": { "content": "Body of the policy.\n" },
                            "references": [
                                { "name": "limits", "description": "Refund limits", "content": "$500 max\n" }
                            ]
                        }]
                    }
                }
            }
        }
    });

    skills::preprocess(&mut graph, Some("agent-7"), &rt).await.unwrap();

    let cache_dir = tmp.path().join("agent-7").join("v1").join("policies");
    assert!(cache_dir.join("SKILL.md").exists());
    assert!(cache_dir.join("references").join("limits.md").exists());

    // Check the rewritten JSON.
    let paths = graph["nodes"]["agent"]["config"]["skills"]["paths"]
        .as_array()
        .unwrap();
    assert_eq!(paths.len(), 1);
    assert!(paths[0]
        .as_str()
        .unwrap()
        .ends_with("/agent-7/v1/policies"));
    assert!(graph["nodes"]["agent"]["config"]["skills"]
        .get("declared")
        .is_none());
}
```

In order for this integration test to compile, the worker crate must expose its `skills` module. Replace the `mod skills;` line in `worker/src/main.rs` with `pub mod skills;`. Also replace the line `mod skills;` in the new e2e test file's `use` statement with the matching path. Specifically: ensure that `worker/src/main.rs` contains `pub mod skills;` (so integration tests can reach into `worker::skills::*`).

If the worker crate is currently a binary-only crate (no `[lib]` target), an integration test under `tests/` cannot import `worker::skills`. To make the module reachable from integration tests, add a `lib.rs` shim. Create:

`worker/src/lib.rs`:
```rust
pub mod skills;
```

And remove the `mod skills;` declaration from `main.rs`. Also update `main.rs` to import via `use worker::skills;` (or fully-qualified `crate::skills` if you prefer to keep `mod skills;` in `main.rs` AND have a `lib.rs`; pick one approach — the lib.rs shim is cleaner). Update `worker/Cargo.toml` if needed: a default `[lib]` target is implied by the presence of `src/lib.rs` and does not need explicit declaration.

If you prefer to keep `main.rs` self-contained, an alternate approach is to test the skills preprocessor only via unit tests (the `mod tests` blocks already added). This integration test file then becomes optional. Choose the lib.rs shim approach to enable the E2E suite.

- [ ] **Step 2: Run the test**

Run: `cd /home/daniel-garcia4/startti/adp && cargo test -p worker --test preprocess_e2e inline_skill_only`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/src/lib.rs apps/service/ia/platform/worker/src/main.rs apps/service/ia/platform/worker/tests/preprocess_e2e.rs
git commit -m "test(worker/skills): e2e scaffold + inline-only happy path"
```

### Task 17: E2E — URL-only happy path with `wiremock`

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/tests/preprocess_e2e.rs`

- [ ] **Step 1: Append the test**

Append to `preprocess_e2e.rs`:

```rust
use wiremock::{matchers::any, Mock, MockServer, ResponseTemplate};

fn host_of(uri: &str) -> String {
    uri.replace("http://", "")
        .replace("https://", "")
        .split(':')
        .next()
        .unwrap()
        .to_string()
}

async fn server_with_two_files(skill_md: &str, ref_body: &str) -> (MockServer, String, String) {
    let server = MockServer::start().await;
    let skill_md_url = format!("{}/skill.md", server.uri());
    let ref_url = format!("{}/ref.md", server.uri());
    Mock::given(wiremock::matchers::path("/skill.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string(skill_md))
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::path("/ref.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ref_body))
        .mount(&server)
        .await;
    let _ = any();
    (server, skill_md_url, ref_url)
}

#[tokio::test]
async fn url_skill_downloads_and_rewrites() {
    let tmp = tempfile::tempdir().unwrap();
    let (server, skill_md_url, ref_url) =
        server_with_two_files("# body of skill\n", "ref content\n").await;
    let rt = runtime(
        tmp.path().to_path_buf(),
        vec![HostMatcher::Exact(host_of(&server.uri()))],
        4096,
    );

    let mut graph = json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "skills": {
                        "declared": [{
                            "name": "amadeus",
                            "description": "Flight search",
                            "version": "v1",
                            "skill_md": { "url": skill_md_url },
                            "references": [
                                { "name": "fares", "description": "Fares", "url": ref_url }
                            ]
                        }]
                    }
                }
            }
        }
    });

    skills::preprocess(&mut graph, Some("agent-1"), &rt).await.unwrap();

    let cache_dir = tmp.path().join("agent-1").join("v1").join("amadeus");
    let skill_md = std::fs::read_to_string(cache_dir.join("SKILL.md")).unwrap();
    assert!(skill_md.contains("name: amadeus"));
    assert!(skill_md.contains("# body of skill"));
    let fares = std::fs::read_to_string(cache_dir.join("references").join("fares.md")).unwrap();
    assert_eq!(fares, "ref content\n");
}
```

- [ ] **Step 2: Run**

Run: `cd /home/daniel-garcia4/startti/adp && cargo test -p worker --test preprocess_e2e url_skill_downloads`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/tests/preprocess_e2e.rs
git commit -m "test(worker/skills): e2e url download + frontmatter wrap"
```

### Task 18: E2E — cache hit + version bump

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/tests/preprocess_e2e.rs`

- [ ] **Step 1: Append the test**

```rust
#[tokio::test]
async fn cache_hit_skips_redownload_and_version_bump_redownloads() {
    let tmp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let url_v1 = format!("{}/v1.md", server.uri());
    let url_v2 = format!("{}/v2.md", server.uri());

    let v1_mock = Mock::given(wiremock::matchers::path("/v1.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string("v1 body\n"))
        .expect(1) // exactly once even after we run preprocess twice
        .named("v1");
    let v2_mock = Mock::given(wiremock::matchers::path("/v2.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string("v2 body\n"))
        .expect(1)
        .named("v2");
    server.register(v1_mock).await;
    server.register(v2_mock).await;

    let rt = runtime(
        tmp.path().to_path_buf(),
        vec![HostMatcher::Exact(host_of(&server.uri()))],
        4096,
    );

    // First run with version v1.
    let mut g1 = json!({
        "nodes": {
            "agent": { "type": "llm_call", "config": { "skills": { "declared": [
                { "name": "s", "description": "d", "version": "v1", "skill_md": { "url": url_v1 } }
            ] } } }
        }
    });
    skills::preprocess(&mut g1, Some("agent-1"), &rt).await.unwrap();

    // Second run, same v1 (must hit cache, no extra wiremock call).
    let mut g1_again = json!({
        "nodes": {
            "agent": { "type": "llm_call", "config": { "skills": { "declared": [
                { "name": "s", "description": "d", "version": "v1", "skill_md": { "url": url_v1 } }
            ] } } }
        }
    });
    skills::preprocess(&mut g1_again, Some("agent-1"), &rt).await.unwrap();

    // Third run with v2 (must download).
    let mut g2 = json!({
        "nodes": {
            "agent": { "type": "llm_call", "config": { "skills": { "declared": [
                { "name": "s", "description": "d", "version": "v2", "skill_md": { "url": url_v2 } }
            ] } } }
        }
    });
    skills::preprocess(&mut g2, Some("agent-1"), &rt).await.unwrap();

    // Both cache dirs coexist.
    assert!(tmp.path().join("agent-1").join("v1").join("s").join("SKILL.md").exists());
    assert!(tmp.path().join("agent-1").join("v2").join("s").join("SKILL.md").exists());
    let v1_body =
        std::fs::read_to_string(tmp.path().join("agent-1").join("v1").join("s").join("SKILL.md"))
            .unwrap();
    let v2_body =
        std::fs::read_to_string(tmp.path().join("agent-1").join("v2").join("s").join("SKILL.md"))
            .unwrap();
    assert!(v1_body.contains("v1 body"));
    assert!(v2_body.contains("v2 body"));

    // wiremock asserts on Drop that each `expect(1)` was hit exactly once.
}
```

- [ ] **Step 2: Run**

Run: `cd /home/daniel-garcia4/startti/adp && cargo test -p worker --test preprocess_e2e cache_hit_skips`
Expected: PASS. wiremock will panic if either mock was called more or fewer than 1 time.

- [ ] **Step 3: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/tests/preprocess_e2e.rs
git commit -m "test(worker/skills): e2e cache hit + version bump"
```

### Task 19: E2E — concurrent same-skill jobs serialize

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/tests/preprocess_e2e.rs`

- [ ] **Step 1: Append the test**

```rust
#[tokio::test]
async fn concurrent_same_skill_jobs_download_only_once() {
    let tmp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let url = format!("{}/skill.md", server.uri());
    let m = Mock::given(wiremock::matchers::path("/skill.md"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("body")
                // a tiny sleep increases the chance of overlap
                .set_delay(std::time::Duration::from_millis(50)),
        )
        .expect(1)
        .named("only-once");
    server.register(m).await;

    let rt = Arc::new(runtime(
        tmp.path().to_path_buf(),
        vec![HostMatcher::Exact(host_of(&server.uri()))],
        4096,
    ));

    let mk_graph = |url: String| {
        json!({
            "nodes": {
                "agent": { "type": "llm_call", "config": { "skills": { "declared": [
                    { "name": "shared", "description": "d", "version": "v1", "skill_md": { "url": url } }
                ] } } }
            }
        })
    };

    let rt_a = rt.clone();
    let rt_b = rt.clone();
    let rt_c = rt.clone();
    let url_a = url.clone();
    let url_b = url.clone();
    let url_c = url.clone();
    let h_a = tokio::spawn(async move {
        let mut g = mk_graph(url_a);
        skills::preprocess(&mut g, Some("agent-1"), &rt_a).await
    });
    let h_b = tokio::spawn(async move {
        let mut g = mk_graph(url_b);
        skills::preprocess(&mut g, Some("agent-1"), &rt_b).await
    });
    let h_c = tokio::spawn(async move {
        let mut g = mk_graph(url_c);
        skills::preprocess(&mut g, Some("agent-1"), &rt_c).await
    });

    h_a.await.unwrap().unwrap();
    h_b.await.unwrap().unwrap();
    h_c.await.unwrap().unwrap();
}
```

- [ ] **Step 2: Run**

Run: `cd /home/daniel-garcia4/startti/adp && cargo test -p worker --test preprocess_e2e concurrent_same_skill`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/tests/preprocess_e2e.rs
git commit -m "test(worker/skills): e2e concurrent same-skill jobs"
```

### Task 20: E2E — failure paths (whitelist, size, HTTP, missing agent_id)

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/tests/preprocess_e2e.rs`

- [ ] **Step 1: Append the four failure tests**

```rust
#[tokio::test]
async fn whitelist_rejects_url() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = runtime(tmp.path().to_path_buf(), vec![], 1024); // empty whitelist
    let mut g = json!({
        "nodes": { "agent": { "type": "llm_call", "config": { "skills": { "declared": [
            { "name": "x", "description": "d", "version": "v1", "skill_md": { "url": "https://evil.com/x" } }
        ] } } } }
    });
    let err = skills::preprocess(&mut g, Some("a"), &rt).await.unwrap_err();
    assert!(matches!(err, skills::PreprocessError::HostNotAllowed(_)));
}

#[tokio::test]
async fn size_cap_rejects_oversized_inline() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = runtime(tmp.path().to_path_buf(), vec![], 8); // 8-byte cap
    let mut g = json!({
        "nodes": { "agent": { "type": "llm_call", "config": { "skills": { "declared": [
            { "name": "x", "description": "d", "version": "v1", "skill_md": { "content": "a".repeat(16) } }
        ] } } } }
    });
    let err = skills::preprocess(&mut g, Some("a"), &rt).await.unwrap_err();
    assert!(matches!(err, skills::PreprocessError::PayloadTooLarge { .. }));
}

#[tokio::test]
async fn http_failure_propagates_as_fetch_error() {
    let tmp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let rt = runtime(
        tmp.path().to_path_buf(),
        vec![HostMatcher::Exact(host_of(&server.uri()))],
        1024,
    );
    let url = format!("{}/x.md", server.uri());
    let mut g = json!({
        "nodes": { "agent": { "type": "llm_call", "config": { "skills": { "declared": [
            { "name": "x", "description": "d", "version": "v1", "skill_md": { "url": url } }
        ] } } } }
    });
    let err = skills::preprocess(&mut g, Some("a"), &rt).await.unwrap_err();
    match err {
        skills::PreprocessError::Fetch { reason, .. } => assert!(reason.contains("503")),
        other => panic!("expected Fetch, got {other:?}"),
    }
}

#[tokio::test]
async fn missing_agent_id_with_skills_block_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = runtime(tmp.path().to_path_buf(), vec![], 1024);
    let mut g = json!({
        "nodes": { "agent": { "type": "llm_call", "config": { "skills": {
            "paths": ["/app/skills"]   // any non-empty skills field is enough
        } } } }
    });
    let err = skills::preprocess(&mut g, None, &rt).await.unwrap_err();
    assert!(matches!(err, skills::PreprocessError::AgentIdRequired));
}
```

- [ ] **Step 2: Run all e2e tests**

Run: `cd /home/daniel-garcia4/startti/adp && cargo test -p worker --test preprocess_e2e`
Expected: all 7 e2e tests PASS.

- [ ] **Step 3: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/tests/preprocess_e2e.rs
git commit -m "test(worker/skills): e2e failure paths"
```

### Task 21: E2E — Colmena consumes the materialized cache

This test verifies that after preprocessing, Colmena's `FilesystemSkillRepository::from_paths` accepts the cache layout. It exercises the full chain on a synthetic graph but without running the engine — only the repository construction.

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/tests/preprocess_e2e.rs`

- [ ] **Step 1: Append the test**

```rust
#[tokio::test]
async fn colmena_filesystem_repo_loads_materialized_skill() {
    use colmena::skills::infrastructure::filesystem_skill_repository::FilesystemSkillRepository;

    let tmp = tempfile::tempdir().unwrap();
    let rt = runtime(tmp.path().to_path_buf(), vec![], 4096);

    let mut g = json!({
        "nodes": {
            "agent": {
                "type": "llm_call",
                "config": {
                    "skills": {
                        "declared": [{
                            "name": "policies",
                            "description": "Refund policy",
                            "version": "v1",
                            "skill_md": { "content": "Body of policy.\n" },
                            "references": [
                                { "name": "limits", "description": "Refund caps", "content": "$500\n" }
                            ]
                        }]
                    }
                }
            }
        }
    });
    skills::preprocess(&mut g, Some("agent-9"), &rt).await.unwrap();

    let path = g["nodes"]["agent"]["config"]["skills"]["paths"][0]
        .as_str()
        .unwrap()
        .to_string();

    let repo = FilesystemSkillRepository::from_paths(
        &[path],
        tmp.path(),
        &[tmp.path().to_path_buf()],
    )
    .expect("Colmena should accept the materialized skill directory");

    let names: Vec<String> = repo.list_available().into_iter().map(|e| e.name).collect();
    assert_eq!(names, vec!["policies".to_string()]);
}
```

- [ ] **Step 2: Run**

Run: `cd /home/daniel-garcia4/startti/adp && cargo test -p worker --test preprocess_e2e colmena_filesystem_repo_loads`
Expected: PASS.

If the test fails to compile because `FilesystemSkillRepository` is not re-exported by the worker's pinned Colmena dep, expose it explicitly. Verify with:

```bash
cd /home/daniel-garcia4/startti/adp
grep -n "pub use" $(find ~/.cargo -name skills -type d 2>/dev/null | head -1)/mod.rs 2>/dev/null || true
```

If the path is not public, fall back to checking the path layout from this test instead of constructing the repo:

```rust
    assert!(std::path::Path::new(&path).join("SKILL.md").exists());
    let body = std::fs::read_to_string(std::path::Path::new(&path).join("SKILL.md")).unwrap();
    assert!(body.contains("name: policies"));
```

- [ ] **Step 3: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/tests/preprocess_e2e.rs
git commit -m "test(worker/skills): e2e Colmena reads materialized skill"
```

---

## Phase 5 — Deploy and observability

### Task 22: Document deploy env in worker README

**Files:**
- Create or modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/README.md` (create if absent; otherwise append)

- [ ] **Step 1: Add a "Skills preprocessor — required env" section**

If the file does not exist, create it with:

```markdown
# ADP Worker

## Skills preprocessor — required env

User-provided skills declared in the graph JSON (`skills.declared`) are
materialized to a per-agent on-disk cache by the worker before the graph is
handed to Colmena. The following environment variables must be set on the
worker container:

| Variable | Required? | Default | Purpose |
|---|---|---|---|
| `COLMENA_SKILLS_ALLOWED_HOSTS` | yes (production) | `""` (deny all) | Comma-separated host whitelist. Wildcard `*.example.com` allowed for one left-most subdomain. |
| `COLMENA_SKILLS_CACHE_ROOT` | no | `/tmp/colmena-skills-cache` | Root of the on-disk cache. |
| `COLMENA_SKILLS_MAX_FILE_BYTES` | no | `65536` (64 KB) | Per-file size cap. **Bound by Colmena's hardcoded 64 KB**; setting higher will cause Colmena to reject the file at engine load. |
| `COLMENA_SKILLS_MAX_TOTAL_BYTES` | no | `524288` (512 KB) | Per-skill aggregate cap (SKILL.md + references). |
| `COLMENA_SKILLS_HTTP_TIMEOUT_MS` | no | `15000` | HTTP timeout per file. |
| `COLMENA_SKILLS_ALLOWED_DIRS` | yes | unset | Colmena-side check. **Must contain the worker's `COLMENA_SKILLS_CACHE_ROOT` plus any baseline skill roots** (e.g. `/app/skills`). Comma-separated on Unix becomes colon-separated for Colmena. Use `:` separator on Linux containers, `;` on Windows. |

### Cloud Run example

```yaml
env:
  - name: COLMENA_SKILLS_ALLOWED_HOSTS
    value: storage.googleapis.com
  - name: COLMENA_SKILLS_CACHE_ROOT
    value: /tmp/colmena-skills-cache
  - name: COLMENA_SKILLS_ALLOWED_DIRS
    value: /tmp/colmena-skills-cache:/app/skills
```

### How the worker scopes the cache

The cache directory layout is:

```
/tmp/colmena-skills-cache/
  <agent_id>/
    <version>/
      <skill_name>/
        SKILL.md
        references/
          <ref_name>.md
```

`agent_id` comes from `JobRequest.agent_id` (forwarded from the API's `CreateExecutionRequest`). When the same agent is invoked again on a warm instance with the same `version`, the cache hit avoids the network round trip. When `version` changes, a fresh sibling directory is created and both versions coexist on disk until the instance recycles.
```

- [ ] **Step 2: Commit**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/README.md
git commit -m "docs(worker): document skills preprocessor env vars"
```

---

## Self-Review Findings

Skimmed the spec section by section against the tasks; record any gaps caught:

- **Spec coverage:**
  - Architecture (Section: "Where the logic lives") → Tasks 4, 5, 7, 15.
  - JSON shape with URL/inline mutex → Task 8 (`SkillSource::is_well_formed`) + Task 10 (validator).
  - SKILL.md normalization → Task 13 (`normalize_skill_md`).
  - Validations table → Task 10 (validator) + Task 13 (size/total caps in `write_atomic`).
  - Whitelist syntax + limits → Task 9 (`HostMatcher`).
  - Preprocessor algorithm → Tasks 7, 14.
  - Concurrency model → Task 12 (`ensure_materialized` lock) + Task 13 (atomic rename) + Task 19 (test).
  - Multiple skill sources → Tasks 1–2 (Colmena root-scan) + Task 14 (preserves `paths`/`builtin`).
  - Required Changes section in spec → Tasks 1–22 cover every line.
  - Observability → emitted via `tracing` calls in `process_job` (existing `emit_error!` macro is reused). The spec calls out `skill-cache-hit` / `skill-materialized` Redis events — these are NOT implemented in this plan; add as a follow-up if they become needed (the `error` event already covers failures end-to-end).

- **Placeholder scan:** None remaining. Each step has either complete code, exact commands, or precise file edits.

- **Type consistency:** `SkillSource`, `DeclaredSkill`, `DeclaredReference`, `SkillsConfig`, `SkillsRuntime`, `PreprocessError`, `HostMatcher` are introduced once each and used consistently in all subsequent tasks. The `SkillSource` `flatten` deserialization on `DeclaredReference` works because `SkillSource` has only `url` and `content` fields with `default`. `parse_declared` is defined in Task 8 and used by Task 14.

- **Known follow-ups (not in this plan):**
  1. SSE events `skill-cache-hit` / `skill-materialized` — present in spec, deferred until needed by frontend.
  2. Prometheus metrics — listed as future work in spec.
  3. ADP frontend changes (signing pipeline, version bumping) — explicitly out of scope per spec.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-03-user-skills-via-signed-urls.md`.

**Two execution options:**

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task, two-stage review between each (spec compliance, then code quality), fast iteration.

2. **Inline Execution** — execute tasks in this session using executing-plans, batch with checkpoints.

Which approach?
