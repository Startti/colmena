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
- Colmena: no changes.
- Docker / deploy: ensure `COLMENA_SKILLS_ALLOWED_DIRS` includes `/tmp/colmena-skills-cache/` so `FilesystemSkillRepository` accepts the materialized paths.

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
| Downloaded file > `COLMENA_SKILLS_MAX_FILE_BYTES` (default 256 KB) | `PayloadTooLarge: skill 'X' file 'Y' exceeded N bytes` |
| Inline `content` > same limit | `BadRequest: inline content for skill 'X' too large` |
| Total bytes per skill > 2 MB | `PayloadTooLarge: skill 'X' total exceeded 2097152 bytes` |
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
| `COLMENA_SKILLS_MAX_FILE_BYTES` | `262144` (256 KB) | Per-file ceiling, applies to download and inline. |
| `COLMENA_SKILLS_MAX_TOTAL_BYTES` | `2097152` (2 MB) | Per-skill aggregate ceiling. |
| `COLMENA_SKILLS_HTTP_TIMEOUT_MS` | `15000` (15 s) | Per-file HTTP timeout. |
| `COLMENA_SKILLS_ALLOWED_HOSTS` | unset (deny all) | Comma-separated host whitelist. |
| `COLMENA_SKILLS_CACHE_ROOT` | `/tmp/colmena-skills-cache` | Cache base path. |

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

## References

- Existing Colmena skills design: `2026-04-20-llm-skills-design.md`
- Lazy tool loading (synthetic tool pattern): `2026-05-03-lazy-tool-loading-design.md`
- ADP `JobRequest` shape: `apps/service/ia/platform/shared/src/lib.rs`
- ADP API entry: `apps/service/ia/platform/api/src/handlers.rs`
- ADP worker entry: `apps/service/ia/platform/worker/src/main.rs`
- Colmena `FilesystemSkillRepository`: `src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs`
