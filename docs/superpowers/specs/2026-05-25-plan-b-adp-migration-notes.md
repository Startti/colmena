# Plan B — ADP Migration Notes

**Audience:** ADP frontend team (TypeScript/React surface that renders agent tool results) + ADP platform team (Rust services that pull colmena develop).
**Source of truth:** colmena's `workingbranch/upload_documents_with_inline` branch (or its merge into `develop`).

## TL;DR

- **ADP Rust services (`apps/service/ia/platform/{worker,api,shared}/src/`):** no source changes needed. Confirmed by sweep — none of these crates parse `attachment_id` or `url` from tool results. Action required: just rebuild against the new colmena revision (Cloud Build does this automatically on the next develop push). One-line `Cargo.toml` change if you want to pin a specific colmena commit instead of the floating `branch = "develop"` ref.
- **ADP frontend (TypeScript/React):** real work. Tool result JSON consumed by the frontend loses two fields (`attachment_id`, `url`) and gains semantic clarity on `document_id`. Frontend must replace direct `url` rendering with an API-mediated lookup. **This is the only blocking item.**
- **LLM behavior:** the model no longer auto-receives doc content in turn 1. Affects every graph that assumes autoinject; prompts need a one-line addition instructing the model to call `load_attachment`.

## 1. Tool result schema for image_generation / image_edit / tts

### Before (Plan A)

```json
{ "images": [{
  "document_id": "img_revenue_chart_a1b2c3",
  "attachment_id": "<storage-key-uuid>",
  "url": "https://storage.googleapis.com/.../signed-url",
  "mime_type": "image/png",
  "size_bytes": 348000
}]}
```

### After (Plan B)

```json
{ "images": [{
  "document_id": "img_revenue_chart_a1b2c3",
  "mime_type": "image/png",
  "size_bytes": 348000
}]}
```

`tts` follows the same pattern with `"audio"` as the outer key and preserves audio-specific fields like `duration_ms`.

### What ADP frontend needs to change

The frontend currently renders generated artifacts (typically images) by passing the tool result's `url` directly into an `<img src={url}>` or equivalent. After Plan B, no `url` is in the tool result. Replace with:

1. **A new ADP API endpoint** — `GET /api/attachments/:document_id/url` (or equivalent name):
   - Authenticates the user against the `agent_session_id` that owns the document.
   - Queries `conversation_attachments` (joined by `(agent_session_id, document_id)`) for the row.
   - Returns a signed URL pointing at the underlying storage backend (or proxies the bytes inline if preferred).

2. **Frontend rendering migration:**
   - Where the code reads `image.url` directly, replace with a fetch to the new endpoint using `image.document_id`.
   - Where the code reads `image.attachment_id` as a stable identifier for caching / dedup, switch to `image.document_id`.

3. **Suggested frontend implementation sketch** (React):

   ```tsx
   function AttachmentImage({ documentId, mimeType }: ImageRef) {
     const { data, error } = useSWR(
       `/api/attachments/${documentId}/url`,
       fetcher
     );
     if (error) return <Failed />;
     if (!data) return <Loading />;
     return <img src={data.url} alt="" />;
   }
   ```

### What ADP Rust services need to change

Sweep result (executed 2026-05-25):

```
attachment_id / read_url / images.*url consumers in:
  apps/service/ia/platform/worker/src/   → none (only example strings)
  apps/service/ia/platform/api/src/      → none
  apps/service/ia/platform/shared/src/   → none
```

**No code changes required.** When colmena develop bumps to include Plan B:

1. The worker's `Cargo.toml` already points at `colmena = { git = "...", branch = "develop" }` — Cloud Build picks up the new revision automatically.
2. If you want to pin to a specific Plan B commit instead of `branch = "develop"` for reproducibility, update `Cargo.toml` to `rev = "<post-plan-b-sha>"`. Optional.
3. Verify with a local `cargo check` from the worker root before pushing to ADP develop.

## 2. LLM behavior — no-autoinject + ephemeral load_attachment

### Before (Plan A)

When a graph received `inputs.files[]`, the LLM's first user message included the doc bytes via `LlmMessage::user_with_files`. The model could analyze the doc immediately on turn 1 without explicit action.

### After (Plan B)

The model sees only the catalog (in the system message). To read content, the model must call `load_attachment(document_id)`. The doc content is then injected into that turn's iteration stream — but disappears from history after the turn completes (a marker replaces it).

### What ADP needs to change

- **Graph prompts** — any graph in `apps/service/ia/platform/worker/src/skills/` (or wherever ADP-owned canvas graphs live) that assumed the model would automatically see attached docs needs its `system_prompt` updated. Recommended one-liner:

  > *"To analyze attached documents, call `load_attachment(document_id)` — the document IDs are listed in the catalog block at the top of this message."*

  Run a sweep to identify affected graphs:

  ```bash
  rg -l "files\[\]|attachment|input.*file" \
     /Users/danielgarcia/startti/adp/apps/service/ia/platform/worker/src/skills/
  ```

  For each hit, audit the system_prompt and append the load_attachment instruction if the graph routinely receives files.

- **Frontend UX** — if ADP shows "the agent is processing your document..." during turn 1, that flow may now show an extra round-trip (turn 1: model calls load_attachment; turn 2: model responds). Two options:
  1. **Accept the latency increase** (typical: +1-3s for the extra round-trip). For most users this is invisible inside the existing "thinking..." spinner.
  2. **Add a UI state** for "loading document" between turns 1 and 2 if the spinner UX would feel stuck.

- **Long-lived session cost monitoring** — Plan B reduces input-token cost on sessions where the same doc is referenced across many turns. ADP's cost dashboards may show a step-down for multi-turn doc-Q&A workloads. This is the intended effect of Plan B.

## 3. Database migration

Plan A introduced the migration `20260525000001_attachment_uniform_resolution.sql` (additive columns on `conversation_attachments`). **Plan B adds NO new migration — schema unchanged from Plan A.**

Confirm the Plan A migration ran cleanly in ADP's environments before deploying Plan B colmena:

```bash
psql $DATABASE_URL -c "\d conversation_attachments" | grep -E "storage_key|origin|last_used_at"
```

All three columns should be present. If they aren't, apply manually before deploying Plan B:

```bash
psql $DATABASE_URL < src/libs/colmena/migrations/postgres/20260525000001_attachment_uniform_resolution.sql
```

(Or, since ADP uses `prisma migrate deploy` exclusively, hand-author an equivalent Prisma migration with the same DDL and run `pnpm prisma migrate deploy`.)

## 4. Rollout order

Recommended sequencing — frontend leads, colmena follows:

1. **ADP frontend** ships the new `AttachmentImage` component (or equivalent) reading `document_id` and calling the new API endpoint. Behind a feature flag for canary users. **Pre-condition: this must work against Plan A tool results, which still include `url`** — so the new path reads `document_id` and ignores `url` from the start.

2. **ADP API** ships the `GET /api/attachments/:document_id/url` endpoint. Mounted regardless of feature flag state.

3. **Canary validation** — flip the feature flag for ~5% of users. Verify generated images render correctly via the new path.

4. **Colmena Plan B merges to develop.** ADP worker's next Cloud Build auto-pulls the new colmena revision. Now tool results no longer include `url` or `attachment_id`.

5. **Frontend feature flag rolls to 100%.** Old code path (reading `url` directly) is removed in a follow-up release.

6. **Graph prompt sweep** — happens in parallel with steps 1-5; ADP team owns the schedule. Each affected graph gets the load_attachment instruction.

## 5. Validation checklist (before promoting Plan B colmena to production)

- [ ] ADP frontend `AttachmentImage` component (or equivalent) reads `document_id` and resolves the URL via the new endpoint. Flagged ON for canary users.
- [ ] `GET /api/attachments/:document_id/url` endpoint deployed, authenticated, and queryable against `conversation_attachments`.
- [ ] At least one canary graph runs `image_generation` + downstream `http_request` multipart with `$attachment:<document_id>` and confirms the image arrives at the destination intact.
- [ ] Cost dashboards: monitored for ~24h after Plan B colmena rolls out. Expected: small bump in turn-1 catalog tokens, offset by savings on multi-turn doc references. Net should be flat or slightly down.
- [ ] At least 3 affected graphs have their system_prompt updated to instruct the model to call `load_attachment`. If no graphs are affected, confirm explicitly with a sweep.
- [ ] Worker `cargo check` clean against post-Plan-B colmena (no signature drift).

## 6. Rollback plan

If Plan B breaks production after rollout:

1. **Frontend rollback** alone is insufficient — once Plan B colmena is live, the old `url` field is gone from tool results regardless of frontend state. The frontend feature flag toggles whether the frontend reads `document_id` or `url`; rolling it off makes the frontend look for `url` which no longer exists.
2. **The real rollback is reverting colmena `develop`** to the pre-Plan-B SHA and force-pushing. ADP Cloud Build re-runs the worker against the older colmena. Frontend feature flag stays in whatever state it was.
3. **Plan A foundation remains intact** during rollback — no data loss in `conversation_attachments`, no migration to undo. The `storage_key`/`origin`/`last_used_at` columns just become unused by the older colmena code paths.
4. **Frontend feature flag** can stay ON during a rollback because it reads `document_id`, which is also present in Plan A tool results. The frontend remains functional in both Plan A and Plan B modes.
