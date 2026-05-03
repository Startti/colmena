# Design: User-Provided Skills via Signed URLs (ADP Worker)

**Status:** Approved for planning
**Date:** 2026-05-03
**Author:** Daniel Garcia (brainstormed with Claude)
**Target component:** ADP `platform-worker` (not Colmena native)

## Summary

Allow ADP users to attach their own skills to a graph by declaring them in the JSON payload sent to `/api/v1/executions`. Each declared skill carries a `name` + `description` + a `version` cache key + content for `SKILL.md` and any `references`, where each piece of content can be either a **signed URL** (downloaded by the worker) or **inline text/markdown** (embedded in the JSON itself). The ADP worker preprocesses the JSON: it materializes every declared skill to a local cache (`/tmp/colmena-skills-cache/<agent_id>/<version>/<skill>/`), rewrites `skills.declared` into `skills.paths`, and only then hands the graph to Colmena's engine. Colmena itself is unchanged — it sees ordinary filesystem skills.

## Motivation

The skills system in Colmena (see `2026-04-20-llm-skills-design.md`) supports two sources: built-in (compiled into the crate) and filesystem paths whitelisted at process start. Both are static — they cannot be customized per agent or per user without redeploying the worker. ADP users cannot upload a domain-specific skill ("our refund policy", "the Amadeus fare codes our team uses") without involving an engineer.

Pushing user skills as files on the filesystem of every Cloud Run instance is operationally heavy. The natural fit for the ADP is to keep skill assets in object storage (GCS), generate signed URLs at execution time, and let the worker fetch them on demand. Inline content covers the small-payload case (a paragraph, a checklist) without round trips.

## Goals

- Let ADP users declare skills in the graph JSON (URL or inline) without redeploying the worker.
- Cache materialized skills per agent so multi-turn conversations do not re-download.
- Keep Colmena's skill subsystem unchanged — only the worker preprocesses.
- Enforce a whitelist of allowed download hosts and per-skill size limits via worker env config.
- Atomic cache writes to handle concurrent jobs of the same agent without races.
- Fail fast and loud on misconfiguration; never run a graph with a missing skill.

## Non-goals

- Cross-instance shared cache (Filestore, GCS-fuse, etc.). Each Cloud Run instance has its own `/tmp` cache.
- Active cache eviction / LRU. Cloud Run instance shutdown handles cleanup. Documented as future work.
- ETag / Last-Modified validation. The `version` field is the invalidation contract.
- Hash verification of downloaded content against `version`. The signed URL itself is the trust boundary; `version` is opaque.
- Per-agent host whitelist. Whitelist is process-level via env.
- Changes to Colmena (`SkillRepository`, `FilesystemSkillRepository`, `load_skill` tool).
- Changes to `platform-shared` other than the new `agent_id` field.
- Changes to the API beyond accepting and forwarding `agent_id`.

## Architecture

### Where the logic lives

```
┌─────────────────────────────────────────────────────────────────┐
│ ADP Worker (Rust)                                                │
│                                                                  │
│  POST /process                                                   │
│    ↓                                                             │
│  process_job(job)                                                │
│    ↓                                                             │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │ NEW: skills_preprocessor::preprocess(...)               │    │
│  │  1. Walk graph_json.nodes; find every llm_call with     │    │
│  │     a non-empty `skills` block                          │    │
│  │  2. If any uses `skills`, require job.agent_id          │    │
│  │  3. Validate every entry under skills.declared          │    │
│  │  4. For each entry:                                     │    │
│  │       cache_dir = /tmp/colmena-skills-cache/            │    │
│  │                   <agent_id>/<version>/<skill>/         │    │
│  │       if exists → reuse                                 │    │
│  │       else → materialize (download or write inline)     │    │
│  │  5. Rewrite the JSON: remove `declared`, append         │    │
│  │     each cache_dir into the same node's `skills.paths`  │    │
│  └─────────────────────────────────────────────────────────┘    │
│    ↓                                                             │
│  engine.execute_stream(modified_dag_json)                        │
│                                                                  │
│  Colmena sees only `skills.paths: [...]`.                       │
└─────────────────────────────────────────────────────────────────┘
```

### Workspace footprint

- `platform-shared/src/lib.rs`: add `agent_id: Option<String>` to `JobRequest`.
- `platform-api/src/handlers.rs`: add `agent_id: Option<String>` to `CreateExecutionRequest`; copy through to `JobRequest`.
- `platform-worker/src/skills/`: new module (`mod.rs`, `parser.rs`, `validator.rs`, `cache.rs`, `downloader.rs`, `materializer.rs`).
- `platform-worker/src/main.rs`: build `SkillsRuntime` at startup, attach to `AppState`, call `preprocess` from `process_job` between input injection and graph deserialization.
- `colmena/src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs`: extend `from_paths` to auto-detect single-skill vs root directories. Backward-compatible.
- `colmena/src/libs/colmena/src/skills/domain/skill_error.rs`: new `EmptyRoot` variant.
- Docker / deploy: ensure `COLMENA_SKILLS_ALLOWED_DIRS` includes `/tmp/colmena-skills-cache/` (for materialized user skills) and `/app/skills/` (for ADP baseline roots) so `FilesystemSkillRepository` accepts both shapes.

## JSON Shape

The new field lives inside the existing `skills` block of an `llm_call` node:

```jsonc
{
  "type": "llm_call",
  "config": {
    "skills": {
      "builtin": ["..."],            // existing, unchanged
      "paths":   ["..."],            // existing, unchanged
      "declared": [                  // NEW
        {
          "name": "amadeus-flights",
          "description": "Use when the user asks about flight searches, fares, or airlines.",
          "version": "a1b2c3d4e5f6",          // opaque cache key chosen by ADP
          "skill_md": {
            "url": "https://storage.googleapis.com/...sig=abc"
          },
          "references": [
            {
              "name": "fare_codes",
              "description": "IATA fare class codes",
              "content": "# Fare codes\n\n- Y: Economy\n- J: Business..."
            },
            {
              "name": "airports",
              "description": "Airport code lookup",
              "url": "https://storage.googleapis.com/...sig=ghi"
            }
          ]
        }
      ]
    }
  }
}
```

### Field rules

| Field | Required | Notes |
|---|---|---|
| `name` | yes | Unique across all skills in the same `llm_call` (across `builtin`, `paths`, `declared`). |
| `description` | yes | Surfaced to the LLM as the catalog entry hint. |
| `version` | yes | Opaque cache key. ADP must change it whenever the skill content changes. |
| `skill_md` | yes | Object with **exactly one** of `url` (string) or `content` (string). |
| `references` | optional | Array. Each item: `name`, `description` required; `url` xor `content`. |

### `version` semantics

`version` is **opaque** to the worker. It is not verified against downloaded content, not parsed, not hashed by the worker. Its only role is to be the cache-key suffix:

- Cache hit (directory `<agent_id>/<version>/<skill>/` exists) → reuse.
- Cache miss → download URLs / write inline content into a fresh `<agent_id>/<version>/<skill>/`.

The ADP is responsible for changing `version` whenever any byte of the skill changes (recommended: SHA-256 of the concatenated content, truncated to 12 hex chars). If the ADP fails to bump it, warm worker instances will keep serving stale content until they are recycled. This is a deliberate trust boundary: the integrity contract is the **signed URL**, not the worker.

### SKILL.md normalization

Colmena's `parse_skill_md` requires every `SKILL.md` to begin with a YAML frontmatter block (`---\n...\n---\n`) declaring at least `name` and `description`, and the frontmatter `name` must match the directory name on disk. The preprocessor normalizes incoming content to satisfy this contract regardless of where it came from:

- **If the SKILL.md content already has frontmatter:** validate that frontmatter `name` equals the JSON-level `name` (otherwise `BadRequest`). Use as-is.
- **If the SKILL.md content has no frontmatter:** the preprocessor prepends an auto-generated header from the JSON's `name`, `description`, and `references[]` metadata before writing to disk:
  ```
  ---
  name: <json.name>
  description: <json.description>
  references:
    - name: <ref1.name>
      description: <ref1.description>
    - name: <ref2.name>
      description: <ref2.description>
  ---
  <user content>
  ```

Reference files (`references/*.md`) are plain markdown — Colmena does not parse them as SKILL.md and does not require frontmatter. The preprocessor writes the bytes directly.

The `description` field at the JSON level is the **canonical** description surfaced to the LLM catalog. If frontmatter is present and its `description` differs, the preprocessor logs a warning but does not fail; Colmena will use the frontmatter description because that is what its loader reads. Recommendation: when the JSON includes frontmatter, keep both descriptions in sync at the ADP layer.

## Validations

The preprocessor performs all validations **before** any download. If any fails, the job fails with an `error` event before the engine is invoked.

| Check | Failure |
|---|---|
| Any `llm_call` declares any `skills` (builtin, paths, or declared) and `job.agent_id` is `None` | `BadRequest: agent_id required when an llm_call uses skills` |
| Skill `name` collides with another skill in the same node | `BadRequest: skill name 'X' duplicated` |
| `skill_md` or any reference has both `url` and `content`, or has neither | `BadRequest: skill 'X' source must be exactly one of url/content` |
| `url` is malformed or scheme is not `https` | `BadRequest: invalid url for skill 'X'` |
| URL host is not in `COLMENA_SKILLS_ALLOWED_HOSTS` | `Forbidden: host '<host>' not allowed` |
| Downloaded file > `COLMENA_SKILLS_MAX_FILE_BYTES` (default 64 KB, bound by Colmena) | `PayloadTooLarge: skill 'X' file 'Y' exceeded N bytes` |
| Inline `content` > same limit | `BadRequest: inline content for skill 'X' too large` |
| Total bytes per skill > 512 KB | `PayloadTooLarge: skill 'X' total exceeded 524288 bytes` |
| HTTP GET fails (non-2xx, timeout, network error) | `BadGateway: failed to fetch skill 'X' file 'Y' from <host>: <reason>` |
| `version` is empty or > 64 chars | `BadRequest: skill 'X' version invalid` |
| SKILL.md content has frontmatter and its `name` ≠ JSON `name` | `BadRequest: skill 'X' frontmatter name mismatch` |

### Whitelist syntax

`COLMENA_SKILLS_ALLOWED_HOSTS` is a comma-separated list of hostnames. A leading `*.` is a wildcard for the left-most subdomain only:

- `storage.googleapis.com` → exact match.
- `*.r2.cloudflarestorage.com` → matches `acme.r2.cloudflarestorage.com` but not `acme.staging.r2.cloudflarestorage.com`.

If the env var is unset, the whitelist is empty and **all URL-based skills fail** (deny-by-default for production safety). For local development, set the var explicitly.

### Limits

| Variable | Default | Purpose |
|---|---|---|
| `COLMENA_SKILLS_MAX_FILE_BYTES` | `65536` (64 KB) | Per-file ceiling, applies to download and inline. **Bound by Colmena's hardcoded `MAX_FILE_SIZE_BYTES = 64 KB` in `FilesystemSkillRepository`** — setting the worker cap higher would cause Colmena to reject the materialized file after the worker accepted it. Raising this requires bumping the Colmena constant first (out of scope for this design). |
| `COLMENA_SKILLS_MAX_TOTAL_BYTES` | `524288` (512 KB) | Per-skill aggregate ceiling (SKILL.md + all references). |
| `COLMENA_SKILLS_HTTP_TIMEOUT_MS` | `15000` (15 s) | Per-file HTTP timeout. |
| `COLMENA_SKILLS_ALLOWED_HOSTS` | unset (deny all) | Comma-separated host whitelist. |
| `COLMENA_SKILLS_CACHE_ROOT` | `/tmp/colmena-skills-cache` | Cache base path. Must also be listed in Colmena's `COLMENA_SKILLS_ALLOWED_DIRS` env var on the same container. |

## Preprocessor Algorithm

### Module layout

```
platform-worker/src/skills/
├── mod.rs              // pub fn preprocess(graph_json, agent_id, runtime)
├── parser.rs           // walk JSON, extract per-node `declared` lists
├── validator.rs        // validations from the previous section
├── cache.rs            // path layout, atomic rename, in-process locking
├── downloader.rs       // reqwest client, whitelist, size cap, timeout
└── materializer.rs     // write SKILL.md + references/*.md into cache
```

### `preprocess` entry point

```rust
pub async fn preprocess(
    graph_json: &mut Value,
    agent_id: Option<&str>,
    runtime: &SkillsRuntime,
) -> Result<(), PreprocessError> {
    // 1. Parse: collect (node_id, &mut node_config, declared_entries) from each llm_call.
    let collected = parser::extract(graph_json)?;

    // 2. Require agent_id only if any llm_call has any `skills` (builtin/paths/declared).
    if collected.any_skills_used() && agent_id.is_none() {
        return Err(PreprocessError::AgentIdRequired);
    }

    // 3. Validate every entry up front (before any I/O).
    for (_, _, entries) in collected.iter() {
        for entry in entries {
            validator::check(entry, &runtime.config)?;
        }
    }

    let agent_id = agent_id.unwrap_or(""); // safe: only entered if some skill is declared

    // 4. Materialize each unique (skill_name, version) once per request.
    //    Multiple llm_call nodes declaring the same (name, version) share the same path.
    let mut paths_per_node: HashMap<NodeId, Vec<String>> = HashMap::new();
    for (node_id, _, entries) in collected.iter() {
        for entry in entries {
            let cache_dir = cache::path_for(&runtime.config.cache_root, agent_id, &entry.name, &entry.version);
            cache::ensure_materialized(&cache_dir, entry, runtime).await?;
            paths_per_node.entry(node_id.clone())
                .or_default()
                .push(cache_dir.to_string_lossy().into_owned());
        }
    }

    // 5. Rewrite JSON: drop `declared`, append paths into `skills.paths`.
    parser::rewrite(graph_json, paths_per_node)?;

    Ok(())
}
```

### `cache::ensure_materialized` (atomic, locked)

```rust
pub async fn ensure_materialized(
    cache_dir: &Path,
    entry: &DeclaredSkill,
    runtime: &SkillsRuntime,
) -> Result<(), PreprocessError> {
    if cache_dir.exists() {
        runtime.metrics.cache_hit(entry);
        return Ok(());
    }

    // In-process serialization: prevent two concurrent jobs from racing the same skill.
    let lock = runtime.locks
        .entry(cache_dir.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;

    // Double-check after acquiring the lock.
    if cache_dir.exists() {
        return Ok(());
    }

    materializer::write_atomic(cache_dir, entry, runtime).await?;
    runtime.metrics.cache_miss(entry);
    Ok(())
}
```

### `materializer::write_atomic` (temp-dir + rename)

```rust
pub async fn write_atomic(
    cache_dir: &Path,
    entry: &DeclaredSkill,
    runtime: &SkillsRuntime,
) -> Result<(), PreprocessError> {
    let tmp_dir = cache_dir.with_extension(format!("tmp.{}", random_suffix()));
    fs::create_dir_all(tmp_dir.join("references")).await?;

    // SKILL.md
    let raw = match &entry.skill_md {
        SkillSource::Url(u)     => downloader::fetch(u, runtime).await?,
        SkillSource::Content(c) => c.as_bytes().to_vec(),
    };
    enforce_size(raw.len(), runtime)?;
    let normalized = normalize_skill_md(&raw, entry)?; // injects frontmatter if absent;
                                                       // validates frontmatter name match.
    fs::write(tmp_dir.join("SKILL.md"), &normalized).await?;
    let mut total_bytes = normalized.len();

    // references
    for r in &entry.references {
        let body = match &r.source {
            SkillSource::Url(u)     => downloader::fetch(u, runtime).await?,
            SkillSource::Content(c) => c.as_bytes().to_vec(),
        };
        enforce_size(body.len(), runtime)?;
        let safe_name = sanitize_reference_name(&r.name)?;
        fs::write(tmp_dir.join("references").join(format!("{safe_name}.md")), &body).await?;
        total_bytes += body.len();
        if total_bytes > runtime.config.max_total_bytes {
            cleanup(&tmp_dir).await;
            return Err(PreprocessError::PayloadTooLarge);
        }
    }

    // Atomic publish
    match fs::rename(&tmp_dir, cache_dir).await {
        Ok(_) => Ok(()),
        Err(_) if cache_dir.exists() => {
            cleanup(&tmp_dir).await;
            Ok(())
        }
        Err(e) => {
            cleanup(&tmp_dir).await;
            Err(PreprocessError::Io(e))
        }
    }
}
```

`sanitize_reference_name` rejects path traversal (`..`, `/`, `\`) and limits length, ensuring the reference filename cannot escape the `references/` folder.

### Concurrency model

| Scenario | Mechanism |
|---|---|
| Same skill, same version, two concurrent jobs in the same instance | `tokio::Mutex` keyed by cache path; one downloads, the other waits. |
| Same skill, same version, two concurrent jobs in **different** instances | Each instance materializes independently into its own `/tmp`. Acceptable cost. |
| Same skill, different versions | Different cache directories — no contention. |
| Two `llm_call` nodes in the same graph declaring the same skill | The same `(name, version)` resolves to the same cache_dir; both nodes get the same path string. |
| LLM reading SKILL.md while another job triggers materialization | Reads always hit the published `cache_dir` (renamed atomically). The temp dir is invisible until rename completes. |

### `AppState` wiring

```rust
pub struct SkillsRuntime {
    pub http: reqwest::Client,
    pub locks: Arc<DashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    pub config: SkillsConfig,
}

#[derive(Clone)]
struct AppState {
    redis: Arc<redis::Client>,
    engine: Arc<ColmenaEngine>,
    skills_runtime: Arc<SkillsRuntime>,   // NEW
}

// in main()
let skills_runtime = Arc::new(SkillsRuntime::from_env()?);
let state = AppState { redis, engine, skills_runtime };
```

### `process_job` integration

```rust
async fn process_job(job, redis_con, engine, skills_runtime) {
    let mut graph_json = job.dag_json.clone();
    if !is_resume { /* inject inputs into input nodes (existing) */ }

    // NEW: skills preprocessing
    if let Err(e) = skills_preprocessor::preprocess(
        &mut graph_json,
        job.agent_id.as_deref(),
        &skills_runtime,
    ).await {
        emit_error!(e.to_string());
        return Err(e.into());
    }

    let graph: Graph = serde_json::from_value(graph_json)?;
    // ... existing execute_stream call ...
}
```

## Observability

### Stream events

The preprocessor publishes two new event kinds to the Redis stream **before** the engine starts:

```jsonc
// On cache hit (no I/O performed)
{
  "type": "skill-cache-hit",
  "agentId": "abc-123",
  "skillName": "amadeus-flights",
  "version": "a1b2c3d4e5f6"
}

// On cache miss (after successful materialization)
{
  "type": "skill-materialized",
  "agentId": "abc-123",
  "skillName": "amadeus-flights",
  "version": "a1b2c3d4e5f6",
  "filesDownloaded": 2,
  "filesInline": 1,
  "totalBytes": 18432,
  "durationMs": 412
}
```

Errors reuse the existing `error` event with `errorText` carrying the typed prefix from the validations table (`BadRequest:`, `BadGateway:`, etc.).

### Logging

`tracing` with structured fields (the worker already uses `tracing` everywhere):

```rust
tracing::info!(
    agent_id, skill = %entry.name, version = %entry.version,
    cache_hit, files, bytes_total, duration_ms,
    "skill materialized"
);

tracing::warn!(agent_id, skill, "host not in whitelist: {}", host);
tracing::error!(agent_id, skill, "download failed: {}", err);
```

### Future: Prometheus

Documented as future work, not implemented:

- `skill_preprocessor_duration_ms` (histogram, labeled by `cache_hit=true|false`)
- `skill_preprocessor_cache_hits_total` / `_misses_total`
- `skill_preprocessor_bytes_downloaded_total`
- `skill_preprocessor_errors_total{kind="..."}`

## Error Handling

| Class | Behavior |
|---|---|
| Validation (shape, missing fields, host whitelist, sizes) | Job fails before engine. `error` event. |
| Network (timeout, non-2xx, connection error) | Job fails before engine. `error` event. The cache is not partially populated thanks to atomic rename. |
| Cache I/O (filesystem error) | Job fails before engine. `error` event. |
| Corrupted cache directory (existed but unreadable for some reason) | Treated as "exists" → reused. If the LLM later fails to load the skill, the existing `load_skill` error path handles it. (Non-issue in practice given the rename atomicity.) |
| `agent_id` missing when skills are present | Validation error in the worker preprocessor (the API does not parse `dag_json` to know whether skills are used, so the check is always at worker level). |

**No graceful degradation:** if any declared skill cannot be materialized, the entire run fails. Reasoning: an LLM with a different skill set may produce a different and silently wrong answer; the user must know.

## Testing Strategy

### Colmena unit tests (`src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs`)

Net new behaviors of `from_paths` to cover:

- **Single skill at the path** (current behavior, regression coverage): `path/SKILL.md` exists → loaded.
- **Root with N skills**: `path` has no `SKILL.md`; `path/a/SKILL.md`, `path/b/SKILL.md` both exist → both loaded; LLM catalog has 2 entries.
- **Root with mixed children**: `path` has subdirs `a/` (with SKILL.md) and `b/` (without) and a stray file `notes.txt` → only `a/` loaded; no error.
- **Empty root**: `path` is a directory with no skill children → `SkillError::EmptyRoot`.
- **Mixed root + per-skill in the same `paths[]` array**: the array `["/app/skills", "/tmp/.../amadeus"]` works — root is scanned, single skill is loaded directly, all merged.
- **Name collision across roots and per-skill entries**: same skill name from a root and from a per-skill path → `SkillNameCollision`. Existing dedup logic must apply across the expanded set, not just the literal path entries.
- **Non-existent root**: `path` does not exist → `Io { source: NotFound }` (existing canonicalize behavior).

### Unit tests (`platform-worker/src/skills/`)

- `parser`: extract `declared` from various graph shapes (single `llm_call`, multiple, nested in subgraphs, absent).
- `validator`: each rule has a passing case and a failing case (table-driven where possible).
- `cache::ensure_materialized`: cache hit returns without I/O; double-check after lock acquisition; concurrent calls serialized.
- `cache::path_for`: agent_id, name, version are joined safely; path traversal in name/version is rejected.
- `downloader::fetch`: respects whitelist, max size, timeout — verified with `wiremock` or a local hyper server.
- `materializer::write_atomic`: happy path writes SKILL.md + references; total-size cap aborts mid-write and cleans up; rename collision discards temp.
- `normalize_skill_md`: prepends frontmatter when absent; preserves content when present; rejects mismatched frontmatter `name` vs JSON `name`.
- `sanitize_reference_name`: rejects `..`, `/`, `\`, empty.

### Integration tests (`platform-worker/tests/`)

End-to-end preprocessor tests with a `wiremock` server serving "signed" URLs. Each case spins up a server, builds a graph JSON, runs `preprocess`, and asserts on (a) the rewritten JSON, (b) the cache directory contents, (c) wiremock call counts.

1. **Inline only**: `declared` with only `content`. No HTTP traffic. Files written, paths injected into `skills.paths`.
2. **URL only**: `declared` with only `url`. Files downloaded, paths injected.
3. **Mixed**: `skill_md` URL + one inline reference + one URL reference.
4. **Cache hit**: run preprocess twice with the same `version`. Second run: zero `wiremock` calls; rewritten JSON identical (modulo path ordering).
5. **Version bump → re-fetch**: first run with `version: "v1"`, second with `version: "v2"` and different mocked content. Both directories coexist; second run re-downloads.
6. **Missing `agent_id`**: `declared` non-empty, `agent_id = None` → returns `AgentIdRequired` error.
7. **Whitelist rejection**: URL host not in `COLMENA_SKILLS_ALLOWED_HOSTS` → `Forbidden`.
8. **Size cap**: server returns oversized body → `PayloadTooLarge`, no partial cache.
9. **HTTP failure**: server returns 503 → `BadGateway`, no partial cache.
10. **Concurrent same-skill jobs**: spawn 5 `tokio::spawn`s of `preprocess` with the same skill; verify exactly one `wiremock` call.
11. **Two `llm_call` nodes, same skill**: the same `(name, version)` resolves to the same cache path; both nodes' `skills.paths` end up containing it.

### Manual / staging verification

After deploy:

1. Upload a SKILL.md and one reference to a GCS bucket. Generate signed URLs.
2. Build a graph JSON declaring the skill with both URL and inline cases.
3. POST `/api/v1/executions` with `agent_id` set.
4. Subscribe to the SSE stream; confirm `skill-materialized` event with expected counts.
5. Inspect `/tmp/colmena-skills-cache/<agent_id>/` on the running container; confirm structure.
6. POST a second execution with the same `agent_id` and `version`; confirm `skill-cache-hit` event and zero new bytes downloaded.
7. POST a third execution with bumped `version`; confirm new directory and re-download.

## Security

- **Trust boundary:** signed URL validity. The worker assumes that anything the URL serves was placed there by the ADP. There is no second-channel integrity check.
- **Tenant isolation:** the path prefix `<agent_id>/` is opaque to other agents. The cache root is shared but Colmena's `FilesystemSkillRepository` only sees the per-skill directory passed in via `skills.paths`. There is no API surface that lists or reads other agents' cache entries.
- **Path traversal:** `name`, `version`, and reference `name` are sanitized before joining into paths. Whitelisted character set: `[a-zA-Z0-9_-]+`. Length capped (name ≤ 64, version ≤ 64). Empty values rejected.
- **Filesystem confinement:** Colmena's `FilesystemSkillRepository` already enforces a whitelist of allowed roots via `COLMENA_SKILLS_ALLOWED_DIRS`. Adding `/tmp/colmena-skills-cache/` to that list is a deploy-time configuration; the worker does not loosen Colmena's check.
- **Resource exhaustion:** per-file (256 KB), per-skill total (2 MB), and per-file timeout (15 s) caps protect against malicious or buggy URLs that hang or stream gigabytes.
- **No code execution:** skills are text. Same model as the existing skills feature.
- **Prompt injection:** out of scope. Same model as the existing skills feature; the agent operator is the trust authority.

## Cloud Run Deployment Notes

- `/tmp` is writable on Cloud Run; capacity is shared with the in-memory budget (typically 1-8 GB depending on the instance type).
- Cache survives across requests on warm instances. Within a multi-turn conversation, only the cold start incurs downloads.
- Instance shutdown reclaims `/tmp`. Next cold start re-downloads. This is the normal serverless cost model.
- No need for Filestore, GCS-fuse, Memorystore, or any shared storage. The design intentionally keeps cache instance-local.
- Set `COLMENA_SKILLS_ALLOWED_HOSTS` and ensure `/tmp/colmena-skills-cache/` is included in `COLMENA_SKILLS_ALLOWED_DIRS` in the worker's deploy YAML.

## Future Improvements

Documented for tracking, not part of this scope:

1. **Switch from `agent_session_id` to `agent_id`-only scoping in upstream APIs.** Today the `agent_session_id` is also available; future work can decide whether to use it as a finer-grained scope (e.g., for "preview" sessions that should not pollute the agent cache).
2. **Active cache eviction (LRU).** Useful if instances live long enough that many distinct `agent_id`s pass through and `/tmp` becomes pressured.
3. **Pre-warm on deploy / cold start.** The worker could pre-fetch the most-used skills of the most-active agents at boot.
4. **ETag / Last-Modified validation.** A HEAD request before reuse would catch ADP versioning bugs (content changed but `version` not bumped). Adds one round-trip per skill per request.
5. **Compression at rest.** Gzip the `.md` files in the cache if the disk budget becomes tight.
6. **Prometheus metrics** as listed in the Observability section.
7. **Per-agent host whitelist override.** Useful if different ADP tenants need different storage backends.
8. **Cross-instance shared cache.** Out of scope unless a high-traffic agent demonstrates that the warm-cache hit ratio is below acceptable.

## Multiple Skill Sources

A graph can pull skills from three independent origins. All three coexist in the same `skills` block of an `llm_call`, and Colmena merges them transparently into a single catalog the LLM sees through `load_skill`.

### The three origins

| Origin | Where it lives | How it lands in the JSON |
|---|---|---|
| **Built-in (Rust-compiled into Colmena)** | Inside the `colmena` crate (`src/libs/colmena/src/skills/builtin/...`) | Listed by name in `skills.builtin: ["python-expert", ...]`. The ADP frontend chooses which ones apply. |
| **ADP baseline (filesystem of the worker container)** | Files baked into the worker image at build time, e.g. `/app/skills/adp-node-catalog/`, `/app/skills/another-baseline/` | Listed in `skills.paths`. Two equivalent forms: enumerate each skill (`["/app/skills/adp-node-catalog", "/app/skills/another-baseline"]`) **or** point at the root (`["/app/skills"]`) and let Colmena's auto-detection load every immediate child that has `SKILL.md`. The ADP repo guarantees the files exist and that versions/contents are correct. |
| **User-declared (signed URL or inline)** | Per-agent content in GCS or directly inline in the JSON | Listed in the new `skills.declared: [...]` block (this design). The worker preprocessor materializes each entry to `<cache_root>/<agent_id>/<version>/<skill>/` and appends those paths into `skills.paths`. |

### How the assembled JSON looks before vs after preprocessing

**Before** (what the ADP frontend POSTs to `/api/v1/executions`):

```jsonc
"skills": {
  "builtin": ["python-expert"],
  "paths":   ["/app/skills"],          // root → Colmena scans for SKILL.md children
  "declared": [
    { "name": "amadeus-flights", "version": "v1", "skill_md": {"url":"..."}, "references": [...] },
    { "name": "internal-policies", "version": "v1", "skill_md": {"content": "..."} }
  ]
}
```

**After** (what Colmena's `LlmNode` sees, post-preprocessor):

```jsonc
"skills": {
  "builtin": ["python-expert"],
  "paths": [
    "/app/skills",                                                  // root, Colmena expands
    "/tmp/colmena-skills-cache/<agent_id>/v1/amadeus-flights",     // per-skill (worker)
    "/tmp/colmena-skills-cache/<agent_id>/v1/internal-policies"    // per-skill (worker)
  ]
  // "declared" removed
}
```

Colmena's `FilesystemSkillRepository::from_paths`:
- Path `/app/skills` has no `SKILL.md` directly → it's a root → scan immediate children → load `adp-node-catalog`, `another-baseline`, etc.
- The two `<cache>/...` paths have `SKILL.md` directly → loaded as single skills (one each).
- All loaded skills merge into one `HashMap<String, SkillEntry>`.

`CompositeSkillRepository` then merges `builtin` + the resulting filesystem set into a unified catalog visible to the LLM.

### Responsibility split

| Concern | Owner |
|---|---|
| Choosing which built-in skills apply per graph type | ADP frontend |
| Choosing which baseline skills (filesystem of worker) apply per graph type | ADP frontend |
| Sending `agent_id` and per-user `declared` skills | ADP frontend |
| Materializing each `declared` entry to a unique cache path | Worker preprocessor |
| Preserving the user-supplied `builtin` and `paths` untouched | Worker preprocessor |
| Merging all three origins into a unified catalog | Colmena (`CompositeSkillRepository`) |
| Enforcing the 50-skill global cap (`MAX_ACTIVE_SKILLS`) | Colmena |

### Preprocessor invariant

After preprocessing, the rewritten JSON satisfies:

```
out.builtin == in.builtin                                  # untouched
out.paths   == in.paths ++ [materialized_path_per_declared] # appended, in declared order
out.declared is absent                                      # removed
```

Implementation hint:

```rust
// node.config.skills is a Value object
let mut skills_obj = node.config.skills.as_object_mut()?;
let mut existing_paths: Vec<Value> = skills_obj
    .remove("paths")
    .and_then(|v| v.as_array().cloned())
    .unwrap_or_default();
for entry in &declared_entries_for_this_node {
    let cache_path = cache::path_for(...).to_string_lossy().into_owned();
    existing_paths.push(Value::String(cache_path));
}
skills_obj.insert("paths".to_string(), Value::Array(existing_paths));
skills_obj.remove("declared");
```

### What happens if the same skill name comes from two origins

Colmena's `CompositeSkillRepository::new` rejects builtin/filesystem name collisions at construction time (`SkillError::SkillNameCollision`), and `FilesystemSkillRepository::from_paths` rejects collisions within `paths`. The graph fails to start with a clear error. The validator in the preprocessor mirrors this rule for `declared` (a duplicate `name` across the same node's `declared`, `paths`, or `builtin` is rejected before materialization) so the failure surfaces earlier, before any I/O.

## Required Changes (verified against code)

This section enumerates every file/module/env change required to land the feature, separated by repo. The list was cross-checked against the actual code as of `develop` on 2026-05-03.

### Colmena repo (`/home/daniel-garcia4/startti/colmena/`) — **one small change**

A single, contained change to `FilesystemSkillRepository::from_paths`: each `paths[]` entry is now auto-detected as either a **single skill directory** or a **root containing many skill directories**. This lets the ADP frontend point at `"/app/skills"` instead of enumerating every baseline by hand. Backward-compatible: existing graphs pointing directly at a skill dir keep working unchanged.

**File:** `src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs`

**New behavior per path entry:**

| Disk shape | Treatment |
|---|---|
| `<path>/SKILL.md` exists | Single skill directory — load it (existing behavior). |
| `<path>` is a directory and at least one immediate child has `SKILL.md` | Root — load each child that has `SKILL.md`. Children without `SKILL.md` are silently ignored (allows a root to coexist with helper files). |
| `<path>` is a directory and no child has `SKILL.md` | New error `SkillError::EmptyRoot(path)` — surfaces a misconfiguration loudly. |

**Sketch of the new loop:**

```rust
for raw_path in paths {
    let canonical = path_buf.canonicalize()?;
    if !canonical.is_dir() { return Err(SkillError::NotADirectory(...)); }
    if !is_allowed(&canonical, &allowed) { return Err(SkillError::PathNotAllowed(...)); }

    if canonical.join("SKILL.md").exists() {
        load_skill_dir(&canonical, &mut skills)?;          // single
    } else {
        let mut count = 0;
        for entry in std::fs::read_dir(&canonical)? {
            let sub = entry?.path();
            if sub.is_dir() && sub.join("SKILL.md").exists() {
                load_skill_dir(&sub, &mut skills)?;        // expanded
                count += 1;
            }
        }
        if count == 0 {
            return Err(SkillError::EmptyRoot(canonical.display().to_string()));
        }
    }
}
```

`load_skill_dir` is the existing per-skill validation block extracted into a helper (frontmatter parse, name == dir_name check, references presence + size, dedup against the `skills` map). No semantics change — just shared between the two branches.

**Why root-scan is restricted to the immediate children:** keeps the contract obvious. A root is "a directory of skills"; recursive scanning invites confusion about how deep to go and what to do when SKILL.md collides at multiple levels. Immediate-children-only is unambiguous and matches how the ADP and the worker actually lay out their files.

**New error variant:**

```rust
// src/libs/colmena/src/skills/domain/skill_error.rs
#[error("path '{0}' has no SKILL.md and contains no skill subdirectories")]
EmptyRoot(String),
```

**Why user-declared cache cannot use root-scan:** the worker materializes each user skill at `<cache_root>/<agent_id>/<version>/<skill>/`. Different skills can have different versions and must coexist (warm cache + a freshly bumped version). Collapsing them under a single scan-able root would lose version coexistence. The worker therefore still enumerates one `paths[]` entry per declared skill. Root-scan is the right tool for **ADP baselines** (one directory of unversioned files), not for the worker's per-skill cache.

The other Colmena requirements remain identical and are still satisfied by the materialized layout:

| Colmena requirement | Where checked | How the spec satisfies it |
|---|---|---|
| Each `path` entry is a directory containing `SKILL.md` | `filesystem_skill_repository.rs:73-80` | Worker writes one cache directory per skill containing `SKILL.md` + `references/`. |
| Path is absolute or relative to graph dir | `filesystem_skill_repository.rs:55-60` | Worker injects absolute paths. |
| Path is canonicalized and `starts_with` an allowed root | `filesystem_skill_repository.rs:62-71` | Operator must add `/tmp/colmena-skills-cache` (or whatever `COLMENA_SKILLS_CACHE_ROOT` is set to) to `COLMENA_SKILLS_ALLOWED_DIRS`. **Deploy-time config, no code change.** |
| Frontmatter `name` matches the directory leaf name | `filesystem_skill_repository.rs:103-115` | Layout `<cache_root>/<agent_id>/<version>/<skill>/` makes `<skill>` the leaf. The worker's `normalize_skill_md` either validates that user-provided frontmatter name == `<skill>` or generates a frontmatter using `<skill>`. |
| Each declared reference exists as `references/{name}.md` | `filesystem_skill_repository.rs:117-138` | Worker writes one `.md` per reference under `references/` named after the JSON's `references[i].name`. |
| Each file ≤ 64 KB (`MAX_FILE_SIZE_BYTES = 64 * 1024`) | `filesystem_skill_repository.rs:88-94` | Worker's `COLMENA_SKILLS_MAX_FILE_BYTES` defaults to **64 KB exactly**, ensuring no file passes the worker only to be rejected by Colmena. |

**Note on `graph_dir`:** When the worker invokes `engine.execute_stream`, no `__colmena_graph_path` is injected, so Colmena falls back to `cwd()` for `graph_dir`. This is irrelevant to absolute cache paths but documented here so future maintainers do not assume `graph_dir` is meaningful in worker context.

### ADP repo (`/home/daniel-garcia4/startti/adp/apps/service/ia/platform/`)

#### Code changes

| File | Change |
|---|---|
| `shared/src/lib.rs` | Add `pub agent_id: Option<String>` (with `#[serde(default)]`) to `JobRequest`. |
| `api/src/handlers.rs` | Add `pub agent_id: Option<String>` to `CreateExecutionRequest`. In `create_execution`, copy `payload.agent_id` into the constructed `JobRequest`. |
| `worker/src/skills/mod.rs` | New module. Public `preprocess(graph_json, agent_id, runtime)` entry. |
| `worker/src/skills/parser.rs` | Walk `graph_json["nodes"]`, extract per-`llm_call` `skills.declared` lists; rewrite to `skills.paths` after materialization. |
| `worker/src/skills/validator.rs` | All shape and limit validations from the **Validations** section. |
| `worker/src/skills/cache.rs` | `path_for(...)`, `ensure_materialized(...)` with `DashMap<PathBuf, Arc<Mutex<()>>>` for in-process serialization. |
| `worker/src/skills/downloader.rs` | `reqwest`-based fetcher; whitelist check; size cap; timeout. |
| `worker/src/skills/materializer.rs` | `write_atomic(...)` using temp-dir + `fs::rename`; `normalize_skill_md(...)`; `sanitize_reference_name(...)`. |
| `worker/src/skills/error.rs` (or in `mod.rs`) | `PreprocessError` enum mapping to the typed error prefixes. |
| `worker/src/main.rs` | Build `SkillsRuntime::from_env()` once at startup; add to `AppState`. In `process_job`, call `skills::preprocess(&mut graph_json, job.agent_id.as_deref(), &state.skills_runtime)` between input injection and graph deserialization. |
| `worker/Cargo.toml` | Add `dashmap`, `tempfile` (or `rand` for suffix), and confirm `reqwest` already present. `sha2` only if a hash helper is needed for tests. |

#### Deploy / env config (Cloud Run)

The container running the worker must export both worker-side and Colmena-side variables. These are deploy-time, not code:

```yaml
# worker container env
COLMENA_SKILLS_CACHE_ROOT: /tmp/colmena-skills-cache
COLMENA_SKILLS_ALLOWED_HOSTS: storage.googleapis.com
COLMENA_SKILLS_MAX_FILE_BYTES: "65536"        # optional override (default = 64 KB)
COLMENA_SKILLS_MAX_TOTAL_BYTES: "524288"      # optional override (default = 512 KB)
COLMENA_SKILLS_HTTP_TIMEOUT_MS: "15000"       # optional override

# Colmena's own check; worker's cache root MUST be inside this set
COLMENA_SKILLS_ALLOWED_DIRS: /tmp/colmena-skills-cache
```

If `COLMENA_SKILLS_ALLOWED_DIRS` does **not** include the cache root, every materialized skill will fail Colmena's path-allow check with `PathNotAllowed` after the worker successfully wrote the files. This is the single most important deploy-time configuration item.

#### Frontend (out of scope for this implementation)

- The ADP frontend that calls `POST /api/v1/executions` must include `agent_id` in the request body. Existing top-level fields are accepted; this is a new typed field.
- The skill editor UI must compute or assign a `version` value when the user saves a skill, and bump it on every content change.
- Signed-URL generation pipeline at the ADP backend (the agent-create / agent-update path) must produce URLs that point to a host listed in `COLMENA_SKILLS_ALLOWED_HOSTS`.

## References

- Existing Colmena skills design: `2026-04-20-llm-skills-design.md`
- Lazy tool loading (synthetic tool pattern): `2026-05-03-lazy-tool-loading-design.md`
- ADP `JobRequest` shape: `apps/service/ia/platform/shared/src/lib.rs`
- ADP API entry: `apps/service/ia/platform/api/src/handlers.rs`
- ADP worker entry: `apps/service/ia/platform/worker/src/main.rs`
- Colmena `FilesystemSkillRepository`: `src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs`
