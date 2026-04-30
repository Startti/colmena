# ADP Deploy with Externalized Skills — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deploy the latest Colmena `develop` changes to ADP's Cloud Run worker, with ADP-specific skills moved out of Colmena into the worker container.

**Architecture:** Skills baked into the worker Docker image at `/app/skills/`. Graphs reference them via absolute paths (`/app/skills/<name>`). `COLMENA_SKILLS_ALLOWED_DIRS=/app/skills` injected by the deploy script so the LLM node's path validation accepts those paths. Colmena pushed to `develop` first because the worker's `Cargo.toml` pulls Colmena via git+branch.

**Tech Stack:** Rust (Colmena, worker), Docker (Kaniko + Cloud Build), Google Cloud Run, gcloud CLI, bash.

**Spec:** [docs/superpowers/specs/2026-04-29-adp-deploy-with-skills-design.md](../specs/2026-04-29-adp-deploy-with-skills-design.md)

**Note on TDD:** This plan is a deploy/migration, not feature work. Tasks are structured as "make change → verify with explicit command → commit". No automated tests are added; verification is via build + smoke test.

---

## Repo paths used in this plan

- **Colmena:** `/home/daniel-garcia4/startti/colmena/`
- **ADP:** `/home/daniel-garcia4/startti/adp/`
- **Platform dir:** `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/`

---

## Task 1: Pre-flight checks

**Files:** none modified.

- [ ] **Step 1: Confirm Colmena workspace compiles**

Run from `/home/daniel-garcia4/startti/colmena/`:
```bash
cargo check --workspace
```
Expected: finishes with `Finished` (no errors). If it fails, **stop the plan** and fix compilation before proceeding.

- [ ] **Step 2: Confirm ADP `[patch]` for Colmena is not active**

Run:
```bash
grep -n '^\[patch\."https://github\.com/Startti/colmena"\]' /home/daniel-garcia4/startti/adp/apps/service/ia/platform/Cargo.toml || echo "OK: no active patch"
```
Expected: prints `OK: no active patch`. If it prints a line number, the `[patch]` block is uncommented — the deploy script will abort. Comment it out before proceeding.

- [ ] **Step 3: Confirm ADP develop branch is clean enough for build**

Run:
```bash
cd /home/daniel-garcia4/startti/adp && git status --short apps/service/ia/platform/
```
Expected: list of modified files including `cloudbuild.yaml`, `deploy_gcp.sh`, `worker/Cargo.toml`, `worker/src/main.rs`, etc. These are expected — they will be included in the Cloud Build context. No action needed in this step; just verify nothing unexpected is staged.

- [ ] **Step 4: Confirm Colmena local branch is `develop`**

Run from `/home/daniel-garcia4/startti/colmena/`:
```bash
git branch --show-current
```
Expected: `develop`. If different, switch with `git checkout develop`.

---

## Task 2: Commit Colmena changes (production-bound only)

**Files:** Colmena modifications listed in spec D4.

- [ ] **Step 1: Stage production-bound modifications**

Run from `/home/daniel-garcia4/startti/colmena/`:
```bash
git add \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs \
  src/libs/colmena/src/documents/application/runtime.rs \
  src/libs/colmena/src/documents/infrastructure/storage/mod.rs \
  src/libs/colmena/src/documents/infrastructure/storage/gcs_store.rs \
  docs/node_as_tools_reference.json \
  docs/node_configurations.json
```
Expected: no error.

- [ ] **Step 2: Verify only the intended files are staged**

Run:
```bash
git diff --cached --name-only
```
Expected output (exactly these 8 lines, in any order):
```
docs/node_as_tools_reference.json
docs/node_configurations.json
src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs
src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs
src/libs/colmena/src/documents/application/runtime.rs
src/libs/colmena/src/documents/infrastructure/storage/gcs_store.rs
src/libs/colmena/src/documents/infrastructure/storage/mod.rs
```
If `tests/graphs/external/skills/` or `tests/graphs/external/socketio_canvas_builder.json` appears: `git restore --staged <path>` to unstage. Those must NOT go into this commit.

- [ ] **Step 3: Commit**

Run:
```bash
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
feat: GCS storage backend, python sandbox, async DocumentRuntime

- documents/infrastructure/storage: add GCS store implementation
- documents/application/runtime: convert init to async
- dag_engine/nodes/python_node: sandboxing + reserved keys
- dag_engine/nodes/llm, document_nodes: alignment with new runtime
- docs/node_*.json: regenerated canonical schemas

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```
Expected: `[develop <hash>] feat: ...` line; `8 files changed`.

- [ ] **Step 4: Push to GitHub `develop`**

Run:
```bash
git push origin develop
```
Expected: `develop -> develop` in output. Pre-commit/pre-push hooks may run; if any fails, **stop and fix** rather than skipping with `--no-verify`.

- [ ] **Step 5: Verify GitHub `develop` matches local HEAD**

Run:
```bash
LOCAL_HASH=$(git rev-parse HEAD)
REMOTE_HASH=$(git ls-remote https://github.com/Startti/colmena.git refs/heads/develop | cut -f1)
echo "Local:  $LOCAL_HASH"
echo "Remote: $REMOTE_HASH"
[[ "$LOCAL_HASH" == "$REMOTE_HASH" ]] && echo "OK: in sync" || echo "MISMATCH"
```
Expected: `OK: in sync`. Save the hash — Task 8 verifies the deploy uses it.

---

## Task 3: Move skills directory from Colmena to ADP

**Files:**
- Move: `/home/daniel-garcia4/startti/colmena/tests/graphs/external/skills/adp-node-catalog/` → `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/skills/adp-node-catalog/`

- [ ] **Step 1: Confirm source skill exists with expected layout**

Run:
```bash
ls /home/daniel-garcia4/startti/colmena/tests/graphs/external/skills/adp-node-catalog/
```
Expected:
```
references
SKILL.md
```
Then:
```bash
ls /home/daniel-garcia4/startti/colmena/tests/graphs/external/skills/adp-node-catalog/references/
```
Expected: 7 `.md` files (`agent.md`, `apiCall.md`, `chatInput.md`, `chatOutput.md`, `databaseQuery.md`, `llmCall.md`, `webSearch.md`).

- [ ] **Step 2: Confirm destination does NOT exist yet**

Run:
```bash
ls /home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/skills/ 2>&1
```
Expected: `ls: cannot access ...: No such file or directory`. If the directory already exists, **stop** and reconcile manually.

- [ ] **Step 3: Create destination parent and move the skill**

Run:
```bash
mkdir -p /home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/skills/
mv /home/daniel-garcia4/startti/colmena/tests/graphs/external/skills/adp-node-catalog \
   /home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/skills/adp-node-catalog
```
Expected: no output (success).

- [ ] **Step 4: Verify move succeeded**

Run:
```bash
ls /home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/skills/adp-node-catalog/
ls /home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/skills/adp-node-catalog/references/ | wc -l
```
Expected: first command shows `references` and `SKILL.md`; second prints `7`.

- [ ] **Step 5: Remove the (now empty) skills dir from Colmena**

Run:
```bash
rmdir /home/daniel-garcia4/startti/colmena/tests/graphs/external/skills/ 2>&1 || \
  rm -rf /home/daniel-garcia4/startti/colmena/tests/graphs/external/skills/
```
Expected: no error. `rmdir` succeeds if empty (the safe path); `rm -rf` is the fallback only if there were leftover files.

- [ ] **Step 6: Verify Colmena no longer has the skills dir**

Run from `/home/daniel-garcia4/startti/colmena/`:
```bash
[[ ! -e tests/graphs/external/skills ]] && echo "OK: removed" || echo "STILL EXISTS"
git status --short tests/graphs/external/
```
Expected: `OK: removed`. The `git status` line should no longer show `tests/graphs/external/skills/` as untracked.

---

## Task 4: Update worker Dockerfile to bake skills into image

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/Dockerfile`

- [ ] **Step 1: Add `COPY worker/skills /app/skills` to runtime stage**

Edit `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/Dockerfile`. Replace this block:

```dockerfile
# Stage 2: Minimal runtime
FROM debian:bookworm-slim
WORKDIR /app

COPY --from=builder /app/target/release/worker .

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates libssl-dev python3 libpython3.11 && \
    rm -rf /var/lib/apt/lists/*

CMD ["./worker"]
```

with:

```dockerfile
# Stage 2: Minimal runtime
FROM debian:bookworm-slim
WORKDIR /app

COPY --from=builder /app/target/release/worker .

# ADP-specific skills consumed by the LLM node at runtime.
# Build context is `apps/service/ia/platform/`, so source is `worker/skills`.
COPY worker/skills /app/skills

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates libssl-dev python3 libpython3.11 && \
    rm -rf /var/lib/apt/lists/*

CMD ["./worker"]
```

- [ ] **Step 2: Verify Dockerfile parses (smoke check)**

Run:
```bash
docker --version >/dev/null 2>&1 && \
  grep -n 'COPY worker/skills /app/skills' /home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/Dockerfile
```
Expected: prints one matching line. If `docker` is unavailable, the `grep` alone is sufficient.

- [ ] **Step 3: (Optional) Local Docker build test**

Run from `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/`:
```bash
docker build --build-arg DEPS_IMAGE=rust:1-slim-bookworm -f worker/Dockerfile -t colmena-worker-local-test . 2>&1 | tail -5
```
Expected: this will likely fail at the cargo build stage (no real deps cache locally) — that is fine. We only need to confirm the COPY does not error with "no such file or directory". If you see `=> ERROR [stage-1 X/X] COPY worker/skills /app/skills`, the path is wrong; fix it. If the COPY succeeds and a later stage fails, OK.

You may skip this step if Docker is not installed locally; Cloud Build will catch a wrong COPY path.

---

## Task 5: Update `deploy_gcp.sh` to inject `COLMENA_SKILLS_ALLOWED_DIRS`

**Files:**
- Modify: `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/deploy_gcp.sh`

- [ ] **Step 1: Add default for `COLMENA_SKILLS_ALLOWED_DIRS`**

Edit `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/deploy_gcp.sh`. Find this block (around line 28-31):

```bash
# ----- Runtime defaults (override via .env or shell exports) ------------------
REDIS_URL=${REDIS_URL:-"redis://10.157.108.227:6379"}
DATABASE_URL=${DATABASE_URL:-"postgresql://colmena:qwerty123@34.172.146.67:5432/colmena_llm_memory?sslmode=require"}
RUST_LOG=${RUST_LOG:-"info"}
```

Replace with:

```bash
# ----- Runtime defaults (override via .env or shell exports) ------------------
REDIS_URL=${REDIS_URL:-"redis://10.157.108.227:6379"}
DATABASE_URL=${DATABASE_URL:-"postgresql://colmena:qwerty123@34.172.146.67:5432/colmena_llm_memory?sslmode=require"}
RUST_LOG=${RUST_LOG:-"info"}
# Where the worker container expects ADP-specific skills (baked into the image).
COLMENA_SKILLS_ALLOWED_DIRS=${COLMENA_SKILLS_ALLOWED_DIRS:-"/app/skills"}
```

- [ ] **Step 2: Add `COLMENA_SKILLS_ALLOWED_DIRS` to the `build_env_vars` loop**

In the same file, find this block (around line 62-66):

```bash
    for var in OPENAI_API_KEY ANTHROPIC_API_KEY GEMINI_API_KEY \
               AMADEUS_CLIENT_ID AMADEUS_CLIENT_SECRET \
               COLMENA_POOL_MAX_ENTRIES COLMENA_POOL_MAX_CONN_PER_URL \
               COLMENA_POOL_MIN_CONN_PER_URL COLMENA_POOL_IDLE_TIMEOUT_SEC \
               COLMENA_POOL_MAX_LIFETIME_SEC COLMENA_POOL_ACQUIRE_TIMEOUT_SEC; do
```

Replace with:

```bash
    for var in OPENAI_API_KEY ANTHROPIC_API_KEY GEMINI_API_KEY \
               AMADEUS_CLIENT_ID AMADEUS_CLIENT_SECRET \
               COLMENA_POOL_MAX_ENTRIES COLMENA_POOL_MAX_CONN_PER_URL \
               COLMENA_POOL_MIN_CONN_PER_URL COLMENA_POOL_IDLE_TIMEOUT_SEC \
               COLMENA_POOL_MAX_LIFETIME_SEC COLMENA_POOL_ACQUIRE_TIMEOUT_SEC \
               COLMENA_SKILLS_ALLOWED_DIRS; do
```

- [ ] **Step 3: Verify both edits applied**

Run:
```bash
grep -n 'COLMENA_SKILLS_ALLOWED_DIRS' /home/daniel-garcia4/startti/adp/apps/service/ia/platform/deploy_gcp.sh
```
Expected: exactly two lines — one in the "Runtime defaults" block (with `:-"/app/skills"`), one in the `for var in` loop.

- [ ] **Step 4: Bash syntax check**

Run:
```bash
bash -n /home/daniel-garcia4/startti/adp/apps/service/ia/platform/deploy_gcp.sh && echo "OK: syntax"
```
Expected: `OK: syntax`. If a syntax error is reported, fix it before continuing.

---

## Task 6: Commit ADP changes

**Files:** Move + Dockerfile + deploy_gcp.sh.

- [ ] **Step 1: Stage worker skills + deploy changes**

Run from `/home/daniel-garcia4/startti/adp/`:
```bash
git add \
  apps/service/ia/platform/worker/skills/ \
  apps/service/ia/platform/worker/Dockerfile \
  apps/service/ia/platform/deploy_gcp.sh
```
Expected: no error.

- [ ] **Step 2: Verify staged set**

Run:
```bash
git diff --cached --name-only | grep -E 'worker/skills|worker/Dockerfile|deploy_gcp\.sh'
```
Expected output (at minimum):
```
apps/service/ia/platform/deploy_gcp.sh
apps/service/ia/platform/worker/Dockerfile
apps/service/ia/platform/worker/skills/adp-node-catalog/SKILL.md
apps/service/ia/platform/worker/skills/adp-node-catalog/references/agent.md
apps/service/ia/platform/worker/skills/adp-node-catalog/references/apiCall.md
apps/service/ia/platform/worker/skills/adp-node-catalog/references/chatInput.md
apps/service/ia/platform/worker/skills/adp-node-catalog/references/chatOutput.md
apps/service/ia/platform/worker/skills/adp-node-catalog/references/databaseQuery.md
apps/service/ia/platform/worker/skills/adp-node-catalog/references/llmCall.md
apps/service/ia/platform/worker/skills/adp-node-catalog/references/webSearch.md
```

- [ ] **Step 3: Commit**

Run:
```bash
git -c commit.gpgsign=false commit -m "$(cat <<'EOF'
feat(worker): bake adp-node-catalog skill into image

Move adp-node-catalog out of colmena (where it didn't belong) into
worker/skills/. Dockerfile copies it to /app/skills at build time.
deploy_gcp.sh sets COLMENA_SKILLS_ALLOWED_DIRS=/app/skills so the
LLM node accepts /app/skills/<name> paths from incoming graphs.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```
Expected: `[develop <hash>] feat(worker): ...` and 10+ files changed.

- [ ] **Step 4: Push (optional but recommended for traceability)**

Run:
```bash
git push origin develop
```
Expected: `develop -> develop`. If hooks fail, **stop and fix**.

> Note: the deploy script uploads the local working tree to Cloud Build via `gcloud builds submit .`, so an unpushed commit would still be deployed correctly. Pushing is for traceability only.

---

## Task 7: Run the deploy

**Files:** none modified.

- [ ] **Step 1: Confirm `.env` is present and sourced**

Run from `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/`:
```bash
[[ -f .env ]] && echo "OK: .env present" || echo "MISSING .env"
```
Expected: `OK: .env present`. If missing, get it from your secrets store before proceeding (the deploy script auto-loads it but only if it exists).

- [ ] **Step 2: Verify the colmena commit visible from GitHub matches what we pushed in Task 2**

Run:
```bash
EXPECTED=$(cd /home/daniel-garcia4/startti/colmena && git rev-parse HEAD | cut -c1-8)
ACTUAL=$(git ls-remote https://github.com/Startti/colmena.git refs/heads/develop | cut -f1 | cut -c1-8)
echo "Expected: $EXPECTED"
echo "Actual:   $ACTUAL"
[[ "$EXPECTED" == "$ACTUAL" ]] && echo "OK: develop is at our commit" || echo "MISMATCH — re-push before deploying"
```
Expected: `OK: develop is at our commit`.

- [ ] **Step 3: Run the deploy script**

Run from `/home/daniel-garcia4/startti/adp/apps/service/ia/platform/`:
```bash
./deploy_gcp.sh
```

Expected milestones in stdout (in order):
- `🔑 Loaded env from <path>/.env`
- `🔍 Obteniendo último commit de colmena develop desde GitHub...`
- `Colmena commit: <8-char hash matching Task 2 step 5>`
- `🔨 Building API + Worker images (single submit, parallel build)...`
- `☁️ Deploying Worker Service to Cloud Run...`
- `Worker URL: https://colmena-worker-<hash>.us-central1.run.app`
- `🚀 Deploying API Service to Cloud Run...`
- `✅ Deployment Complete!`

If `❌ ERROR: Cargo.toml tiene el [patch] local` appears: **stop**, fix `apps/service/ia/platform/Cargo.toml` (re-comment the `[patch]` block), and re-run.

If the build fails on `COPY worker/skills /app/skills`: revisit Task 4 — the path or context is wrong.

- [ ] **Step 4: Capture worker URL**

Run:
```bash
WORKER_URL=$(gcloud run services describe colmena-worker --region us-central1 --format 'value(status.url)')
echo "$WORKER_URL"
```
Expected: a `https://...` URL. Save it for Task 8.

---

## Task 8: Post-deploy verification

**Files:** none modified.

- [ ] **Step 1: Health check**

Run:
```bash
curl -sS -o /dev/null -w "%{http_code}\n" "$WORKER_URL/"
```
Expected: `200`.

- [ ] **Step 2: Confirm deployed image carries the skills**

Run:
```bash
gcloud run services describe colmena-worker --region us-central1 \
  --format 'value(spec.template.spec.containers[0].env)' | tr ',' '\n' | grep COLMENA_SKILLS_ALLOWED_DIRS
```
Expected: a line containing `COLMENA_SKILLS_ALLOWED_DIRS=/app/skills` (exact format depends on gcloud version; the key + value must both appear).

- [ ] **Step 3: Smoke test the skill via API**

Send a graph to the API that uses:
```json
{
  "type": "llm_call",
  "config": {
    "skills": { "paths": ["/app/skills/adp-node-catalog"] },
    "system_message": "Use load_skill('adp-node-catalog', 'agent') and reply with the first line of the reference.",
    "enabled_tools": "*"
  }
}
```

How: take `tests/graphs/external/socketio_canvas_builder.json` from Colmena, replace its `skills.paths` entry with `"/app/skills/adp-node-catalog"`, and POST it through the ADP API endpoint that enqueues a job. Use whichever invocation the team normally uses (e.g. `test_stream_cloud.html` or `curl` to the API). The exact API request shape is out of scope for this plan — use the existing tooling.

Expected: streamed events include at least one `skill_loaded` event with `skill_name: "adp-node-catalog"` and `source: "filesystem"`. No `loading filesystem skills:` error in the worker logs.

- [ ] **Step 4: Inspect worker logs**

Run:
```bash
gcloud run services logs read colmena-worker --region us-central1 --limit 100 \
  | grep -iE 'skill|error' | head -30
```
Expected:
- At least one log line referencing `skill_loaded` or `adp-node-catalog`.
- No `loading filesystem skills:` errors.
- No panic / `pyo3` initialization errors (the worker calls `prepare_freethreaded_python` early; if that fails, python_script nodes won't run, but skill loading is independent).

- [ ] **Step 5: Final verification — Colmena commit hash on the live worker**

Run:
```bash
gcloud run revisions list --service colmena-worker --region us-central1 \
  --format 'value(metadata.name,metadata.creationTimestamp)' --limit 1
```
Expected: a revision created within the last few minutes. Cross-check with the `Colmena commit` printed in Task 7 step 3 — that hash is what's running.

---

## Self-Review Notes

**Spec coverage:**
- D1 (skills location in ADP) → Task 3.
- D2 (absolute path + env var) → Task 4 + Task 5.
- D3 (delete from Colmena) → Task 3 step 5.
- D4 (push Colmena develop) → Task 2.
- Architecture flow diagram → Tasks 3, 4, 5 together implement it.
- Riesgos: `[patch]` check → Task 1 step 2; commit hash check → Task 2 step 5 + Task 7 step 2; smoke test → Task 8 step 3.
- Criterios de éxito (1-5 in spec) → Task 8 covers all five.

**Placeholder scan:** No "TBD", "TODO", "implement later", or "similar to". Step 3 of Task 8 references "the existing tooling" for API invocation — that is intentional, since the API request shape varies by team workflow and is out of scope; the test_stream HTML files in `apps/service/ia/platform/` already exist for this.

**Type consistency:** The skill name `adp-node-catalog`, the absolute path `/app/skills/adp-node-catalog`, and the env var `COLMENA_SKILLS_ALLOWED_DIRS=/app/skills` appear identically across Tasks 3, 4, 5, 6, 7, and 8.
