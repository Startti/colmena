# ADP-Side Migration — Detailed Plan (Plans A + B + C)

**Status:** Authoritative coordination doc (2026-05-25). Supersedes the earlier `plan-b-adp-migration-notes.md` which was based on an incomplete sweep.

**Audience:** ADP team (NestJS backend + Next.js frontend). The colmena Rust worker needs no source changes — confirmed by sweep against `apps/service/ia/platform/{worker,api,shared}/src/`.

**Repos / paths involved:**
- ADP repo root: `/Users/danielgarcia/startti/adp`
- Backend (NestJS): `apps/api/src/`
- Frontend (Next.js): `apps/chat/`
- Prisma schema: `packages/database/prisma/schema.prisma`
- Prisma migrations: `packages/database/prisma/migrations/`
- Database: shared Postgres (`colmena_llm_memory`) — colmena's `conversation_attachments` and ADP's `agent_attachment` are separate tables in the same DB. Confirmed by `apps/service/ia/platform/deploy_gcp.sh` (`DATABASE_URL=postgresql://colmena:...@.../colmena_llm_memory`).

## Architecture context (read this first)

Two attachment tables, one DB:

| Table | Owner | Purpose |
|---|---|---|
| `conversation_attachments` | colmena (sqlx migrations) | Source of truth for what's attached to a conversation. Tracks `document_id`, `storage_key`, `provider`, `last_used_at`, etc. Updated by colmena's `LlmCallNode` + multimedia nodes + `AttachmentStreamResolver`. |
| `agent_attachment` | ADP (Prisma migrations) | ADP's per-message attachment record for chat rendering. Tracks `messageId`, `fileName`, `mimeType`, `sizeInBytes`, `url`, `storageKey`, `source`. Persisted by `chat.service.ts::extractGeneratedAttachments` from colmena tool outputs. |

**Plans don't unify these tables.** ADP keeps writing to `agent_attachment` for its own chat view; colmena keeps writing to `conversation_attachments` for its own resolver / catalog. The Plan B change is that ADP needs a new way to resolve `storage_key` (which used to come for free in the tool result) — by querying `conversation_attachments` after the tool runs.

---

## Plan A — Schema migration (zero ADP code change)

**Status:** Plan A is **additive and ADP-source-clean**. ADP needs no TypeScript or Rust changes. The only thing to verify:

### Step A.1: Confirm Plan A migration applied to the shared DB

Plan A's migration lives at `migrations/postgres/20260525000001_attachment_uniform_resolution.sql` in the colmena repo. It runs via `sqlx::migrate!` on the colmena worker's next startup AND from the new `attachment_gc` binary's startup. So when colmena's `workingbranch/upload_documents_with_inline` merges to `develop` and Cloud Build redeploys the worker, the migration applies automatically.

**Verify after the colmena develop bump:**

```bash
psql "$DATABASE_URL" -c "\d conversation_attachments" | grep -E "storage_key|origin|last_used_at"
```

Should show all three columns. If the migration didn't run (e.g., because the worker rolled back), apply manually:

```bash
psql "$DATABASE_URL" < /Users/danielgarcia/startti/colmena/migrations/postgres/20260525000001_attachment_uniform_resolution.sql
```

### Step A.2: Prisma drift detection (no migration needed, but good hygiene)

ADP's `schema.prisma` does NOT model `conversation_attachments` (it's not ADP's table). So Prisma is unaware of the new columns — that's fine. `prisma migrate deploy` won't try to revert them.

**However**, if anyone later runs `prisma db pull` to regenerate the schema, it WILL pick up `conversation_attachments` as an unmanaged table. Document this in `packages/database/prisma/README.md` (if it exists) or add a comment to `schema.prisma`:

```prisma
// NOTE: The `conversation_attachments` table exists in this database
// but is owned and migrated by colmena (via sqlx), not by Prisma.
// Do NOT run `prisma db pull` and commit the resulting model —
// it will conflict with colmena's schema management.
```

**Total ADP work for Plan A: ~5 minutes of verification + a doc comment. No code, no migration.**

---

## Plan B — Breaking changes (NestJS backend + Next.js frontend)

**The breakage points** in ADP, mapped to actual files:

### B.1: `apps/api/src/chat/application/chat.service.ts` — extractGeneratedAttachments

**Current code (lines 142-182):** consumes `attachment_id` and `url` from each item in `payload.images` / `payload.audio`. Both fields disappear in Plan B.

```typescript
// CURRENT (will break under Plan B):
for (const img of images) {
  if (!img?.attachment_id || !img?.url) continue;        // ← break: both fields gone
  if (!isGeneratedStorageKey(img.attachment_id)) continue; // ← break: attachment_id gone
  out.push({
    fileName: basenameFromKey(img.attachment_id),         // ← break: attachment_id gone
    mimeType: img.mime_type ?? 'application/octet-stream',
    sizeInBytes: typeof img.size_bytes === 'number' ? img.size_bytes : 0,
    url: img.url,                                          // ← break: url gone
    storageKey: img.attachment_id,                         // ← break: attachment_id gone
    source: isEdit ? 'image_edit' : 'image_gen',
  });
}
```

**After Plan B**, the tool result emits only `{ document_id, mime_type, size_bytes }`. To rebuild the `agent_attachment` row, ADP needs to look up the corresponding `conversation_attachments` row (which colmena auto-registered in Plan A Task 4-6) to get the `storage_key`, then generate a fresh signed URL.

**Recommended migration (chat.service.ts:142-182):**

```typescript
async function extractGeneratedAttachments(
  result: ColmenaResult,
  prisma: PrismaService,        // NEW: needed for the conversation_attachments lookup
  gcs: GcsService,              // NEW: needed for the signed URL
  agentSessionId: string,       // NEW: needed for the auth-scoped lookup
): Promise<NewAttachment[]> {
  if (result.errorText) return [];
  const out: NewAttachment[] = [];

  for (const tc of result.toolCalls ?? []) {
    const payload = tc?.output?.output ?? tc?.output;
    if (!payload || typeof payload !== 'object') continue;

    const isEdit =
      (typeof tc.name === 'string' && tc.name.includes('edit')) ||
      (typeof payload.model === 'string' && payload.model.includes('edit'));

    const images = Array.isArray(payload.images) ? payload.images : [];
    for (const img of images) {
      if (!img?.document_id) continue;  // Plan B: tool result uses document_id

      // Plan B: lookup the conversation_attachments row by (agent_session_id, document_id)
      // to recover the storage_key. Colmena's image_generation/image_edit nodes
      // already auto-registered this row (Plan A).
      const row = await prisma.$queryRaw<Array<{
        storage_key: string | null;
        filename: string;
      }>>`
        SELECT storage_key, filename
        FROM conversation_attachments
        WHERE agent_session_id = ${agentSessionId}
          AND document_id = ${img.document_id}
        LIMIT 1
      `;
      const ca = row[0];
      if (!ca?.storage_key) {
        console.warn('[extractGeneratedAttachments] No conversation_attachments row for', img.document_id);
        continue;
      }

      // Generate a fresh signed read URL for the storage_key.
      const { readUrl } = await gcs.generateReadSignedUrlForKey(ca.storage_key);

      out.push({
        fileName: ca.filename || basenameFromKey(ca.storage_key),
        mimeType: img.mime_type ?? 'application/octet-stream',
        sizeInBytes: typeof img.size_bytes === 'number' ? img.size_bytes : 0,
        url: readUrl,                  // Fresh signed URL
        storageKey: ca.storage_key,    // Real storage_key from conversation_attachments
        source: isEdit ? 'image_edit' : 'image_gen',
      });
    }

    // Same pattern for audio
    const audio = payload.audio;
    if (audio?.document_id) {
      const row = await prisma.$queryRaw<...>`...`;
      // ... mirror the image block ...
    }
  }
  return out;
}
```

**Notes on this migration:**
- `extractGeneratedAttachments` becomes async (was sync). Verify call sites await it.
- The function now needs `prisma` + `gcs` + `agentSessionId`. Pass them from the calling method (likely `ChatService` instance methods that have access to all three).
- The raw SQL avoids modeling `conversation_attachments` in Prisma — which is correct per the architecture note (colmena owns that table).
- The function is now I/O-heavy (1 DB query + 1 GCS signing call per generated artifact). For N artifacts, that's 2N round-trips. If perf matters, batch the queries.

### B.2: New backend endpoint `GET /api/attachments/:documentId/url`

The frontend (B.3) will need a way to fetch a fresh signed URL after the initial render — both for newly generated artifacts (signed URLs expire) and for any attachment that the user revisits later.

**Why a new endpoint and not just store the URL?** Signed URLs expire (typically 7 days in colmena's config). Storing them in `agent_attachment.url` means they go stale. The existing pattern at `chat.service.ts:232-256` (`getSessionMessages`) ALREADY regenerates URLs on every read by calling `gcs.generateReadSignedUrlForKey(att.storageKey)`. The frontend gets fresh URLs only when fetching the message list. For real-time scenarios (live image rendering during streaming, image opening in a new tab, etc.) the frontend benefits from an on-demand endpoint.

**File:** Create `apps/api/src/chat/attachment.controller.ts` (or extend `gcs.controller.ts` which already has a similar `read-url` endpoint at line 86):

```typescript
import { Controller, Get, NotFoundException, Param, Req, UseGuards } from '@nestjs/common';
import { JwtAuthGuard } from '../auth/infrastructure/guards/jwt-auth.guard';  // or whatever your session guard is
import { PrismaService } from '../shared/infrastructure/prisma/prisma.service';
import { GcsService } from '../gcs/gcs.service';

@Controller('attachments')
@UseGuards(JwtAuthGuard)  // session-authenticated, NOT internal-token
export class AttachmentController {
  constructor(
    private readonly prisma: PrismaService,
    private readonly gcs: GcsService,
  ) {}

  /**
   * Resolve a colmena `document_id` to a fresh signed read URL.
   * Authorization: caller must own the agent_session that owns the document.
   */
  @Get(':documentId/url')
  async getAttachmentUrl(
    @Param('documentId') documentId: string,
    @Req() req: any,
  ): Promise<{ url: string; mime_type: string; filename: string }> {
    const userId = req.user.id;

    // Find the conversation_attachments row + verify the user owns its session.
    const rows = await this.prisma.$queryRaw<Array<{
      storage_key: string | null;
      mime_type: string;
      filename: string;
      agent_session_id: string;
    }>>`
      SELECT ca.storage_key, ca.mime_type, ca.filename, ca.agent_session_id
      FROM conversation_attachments ca
      JOIN agent_session asess ON asess.id = ca.agent_session_id
      WHERE ca.document_id = ${documentId}
        AND asess."userId" = ${userId}
      LIMIT 1
    `;

    const row = rows[0];
    if (!row) {
      throw new NotFoundException('Attachment not found or not owned by caller');
    }
    if (!row.storage_key) {
      throw new NotFoundException('Attachment has no storage_key (pre-Plan-A row)');
    }

    const { readUrl } = await this.gcs.generateReadSignedUrlForKey(row.storage_key);
    return { url: readUrl, mime_type: row.mime_type, filename: row.filename };
  }
}
```

Register the controller in `apps/api/src/chat/chat.module.ts` (or wherever module composition happens for this domain).

**Note:** the joined column name on `agent_session` is `userId` (camelCase) per the Prisma schema, but the actual DB column name is what Prisma maps to. The raw SQL above assumes Prisma's default behavior of using the field name. If your Prisma map differs, adjust.

### B.3: Frontend changes — `apps/chat/components/chat/ChatMessage.tsx`

**Current breakage points:**

- Line 81: `ImageAttachmentPreview` reads `att.url` directly.
- Line 89, 203: `window.open(att.url, '_blank')` on click.
- Line 96: `<img src={att.url} ...>`.

For freshly generated artifacts:
- If chat.service.ts's `extractGeneratedAttachments` (B.1) populates a fresh `att.url` at extraction time, the frontend works without changes.
- BUT: the signed URL expires. If the user revisits the message after the URL expires, `att.url` 404s.

**Two options:**

**Option 1: server-refresh (minimal frontend change).** `getSessionMessages` already refreshes URLs on every read (line 232-256). For real-time / streaming scenarios where messages arrive mid-session, ensure the streaming pipeline also refreshes URLs before sending to the frontend.

**Option 2: client-fetch on demand (more robust).** Add a new hook `apps/chat/hooks/useAttachmentUrl.ts`:

```typescript
import useSWR from 'swr';

export function useAttachmentUrl(documentId: string | undefined) {
  const { data, error } = useSWR(
    documentId ? `/api/attachments/${documentId}/url` : null,
    (key) => fetch(key, { credentials: 'include' }).then(r => {
      if (!r.ok) throw new Error('Failed to load attachment URL');
      return r.json() as Promise<{ url: string; mime_type: string; filename: string }>;
    }),
  );
  return {
    url: data?.url,
    mime: data?.mime_type,
    filename: data?.filename,
    isLoading: !data && !error,
    error,
  };
}
```

Then modify `ImageAttachmentPreview` (`apps/chat/components/chat/ChatMessage.tsx:81-103`):

```tsx
const ImageAttachmentPreview = ({ att, name }: { att: any; name: string }) => {
  const [isLoading, setIsLoading] = useState(true);

  // Plan B: if the att has a `url`, use it directly (legacy / server-refreshed).
  // Otherwise, fetch on demand using the document_id.
  const inlineUrl = att.url as string | undefined;
  const { url: fetchedUrl } = useAttachmentUrl(inlineUrl ? undefined : att.document_id);
  const finalUrl = inlineUrl ?? fetchedUrl;

  return (
    <div
      className="..."
      onClick={(e) => {
        e.stopPropagation();
        if (finalUrl) window.open(finalUrl, '_blank');
      }}
    >
      {(isLoading || !finalUrl) && (
        <div className="absolute inset-0 animate-pulse bg-foreground/10" />
      )}
      {finalUrl && (
        <img
          src={finalUrl}
          alt={name}
          className="..."
          onLoad={() => setIsLoading(false)}
        />
      )}
    </div>
  );
};
```

(Apply the same pattern to the other `att.url` usage at line 203.)

**Pick Option 1 if the streaming pipeline reliably populates `att.url` at all read points.**
**Pick Option 2 (recommended) if you want client-resilience to expired URLs and don't want to chase every read path.**

### B.4: Graph prompts — instruct the model to call `load_attachment`

Plan B's no-autoinject means graphs that previously assumed the model would auto-see files now need a one-line system_prompt update.

**Sweep:**

```bash
rg -l "files\[\]|files\.0\.|\"files\"" /Users/danielgarcia/startti/adp/apps/service/ia/platform/worker/src/skills/
```

For each graph that takes `inputs.files`, audit its `system_prompt` for any mention of "the user has attached" / "the document above" / etc. If the prompt assumes the model sees content automatically, add an explicit instruction:

> "When the user references an attached document, call `load_attachment(document_id)` first to read its contents. The available document_ids are listed in the catalog at the top of this message."

If your ADP-canvas-owned graphs live elsewhere (e.g., in the database as user-editable templates), do the equivalent sweep there.

---

## Plan C — New internal endpoint `POST /internal/gcs/delete`

Plan C's `attachment_gc` binary calls a new endpoint on the ADP backend to delete GCS blobs. ADP needs to implement it before the cron job runs in prod.

### C.1: Extend `apps/api/src/gcs/internal-gcs.controller.ts`

Add a `@Post('delete')` method to the existing `InternalGcsController`. The controller already has `@UseGuards(InternalServiceGuard)` so the new endpoint is automatically protected by the shared-secret check.

```typescript
import {
  Body,
  Controller,
  HttpCode,
  NotFoundException,
  Post,
  UseGuards,
} from '@nestjs/common';
import { InternalServiceGuard } from '../auth/infrastructure/guards/internal-service.guard';
import { PrismaService } from '../shared/infrastructure/prisma/prisma.service';
import { GcsService, buildGeneratedStorageKey } from './gcs.service';
import { SignPutDto } from './dto/internal-gcs.dto';

@Controller('internal/gcs')
@UseGuards(InternalServiceGuard)
export class InternalGcsController {
  constructor(
    private readonly prisma: PrismaService,
    private readonly gcs: GcsService,
  ) {}

  @Post('sign-put')
  async signPut(@Body() body: SignPutDto): Promise<{...}> {
    // ... existing impl ...
  }

  /**
   * Plan C: delete a GCS blob by its storage_key. Called by colmena's
   * attachment_gc binary as part of TTL cleanup. The binary deletes the
   * conversation_attachments row separately (after this endpoint succeeds).
   *
   * IMPORTANT: this endpoint must ONLY delete the GCS blob. It must NOT
   * touch any DB row — colmena owns conversation_attachments cleanup.
   *
   * @returns 204 on success or if the blob didn't exist (idempotent).
   *          5xx on transient backend failure (caller will retry).
   */
  @Post('delete')
  @HttpCode(204)
  async deleteBlob(@Body() body: { storage_key: string }): Promise<void> {
    if (!body?.storage_key || typeof body.storage_key !== 'string') {
      throw new Error('storage_key is required');
    }

    try {
      await this.gcs.deleteByKey(body.storage_key);
    } catch (err: any) {
      // GCS "not found" → idempotent success.
      if (err.code === 404 || err.message?.includes('No such object')) {
        return;
      }
      console.error('[InternalGcsController] delete failed:', err);
      throw err;  // 500 — colmena gc will retry next run
    }
  }
}
```

### C.2: Add `deleteByKey` to `GcsService`

Read `apps/api/src/gcs/gcs.service.ts` and find the existing `generateReadSignedUrlForKey` / `generateUploadSignedUrlForKey` methods to understand the GCS client pattern. Add a `deleteByKey` method:

```typescript
async deleteByKey(storageKey: string): Promise<void> {
  const bucket = this.storage.bucket(this.bucketName);
  await bucket.file(storageKey).delete({ ignoreNotFound: true });
}
```

(Adjust based on the actual GCS client library — `@google-cloud/storage` Node.js client supports `ignoreNotFound`; if your wrapper is different, mirror its pattern.)

### C.3: That's it for Plan C ADP-side

No frontend changes. No Prisma changes. Just one new POST handler + the GCS delete helper.

---

## Deployment sequence (recommended)

The colmena branch contains all three plans. Suggested rollout:

1. **Pre-merge audit** (this week):
   - Re-run the sweep against `apps/api/src/` for `attachment_id` / `url` to be sure nothing new appeared.
   - Confirm `conversation_attachments` exists in staging DB (verify Plan A migration applied on a prior colmena develop bump).

2. **Frontend leads** (week 1, behind a feature flag):
   - Ship the new `AttachmentImage` / `useAttachmentUrl` hook and the new `GET /api/attachments/:documentId/url` endpoint.
   - The frontend reads `att.document_id` if present, falls back to `att.url`. Both code paths coexist.
   - At this stage colmena is still on Plan A (tool result has both `document_id` AND `attachment_id` AND `url`). Verify the new code path renders correctly using the legacy `attachment_id`+`url` data → no behavior change for users, but the new path is exercised.

3. **NestJS API ships the new endpoints + chat.service.ts migration** (week 2):
   - Deploy the new `POST /internal/gcs/delete` (Plan C) — colmena's attachment_gc binary will be the consumer.
   - Deploy the new `GET /api/attachments/:documentId/url` (Plan B) — frontend feature flag flip target.
   - **Critical:** the chat.service.ts change (B.1) — `extractGeneratedAttachments` becomes async + queries `conversation_attachments`. Ship this with the legacy `if (img.attachment_id && img.url)` path still present as fallback. So:
     ```typescript
     if (img.document_id) {
       // new Plan B path
     } else if (img.attachment_id && img.url) {
       // legacy Plan A fallback
     }
     ```
   - At this stage ADP can handle both Plan A and Plan B colmena tool results.

4. **Colmena Plan A + B + C merges to develop** (week 3):
   - Worker auto-rebuilds. New tool result schema kicks in. Legacy fallback in chat.service.ts becomes dead code.
   - Frontend feature flag flipped to 100% (the new `document_id` path is the only one that matters now).
   - Schedule the `attachment_gc` Cloud Run Job to run nightly.

5. **Cleanup** (week 4):
   - Delete the legacy fallback in chat.service.ts.
   - Delete the frontend feature flag (the new path is the only path).

## Validation checklist (before flipping anything to 100%)

- [ ] `conversation_attachments` table has `storage_key`, `origin`, `last_used_at` columns in staging + prod.
- [ ] `POST /api/internal/gcs/delete` endpoint deployed and returns 204 for an existing key, 204 for a non-existent key, 500 for a GCS transient failure.
- [ ] `GET /api/attachments/:documentId/url` deployed, authenticated, returns 200 with a working signed URL for an owned document, 404 for a non-owned document.
- [ ] Frontend canary (~5% users) renders generated images correctly via the new endpoint. No 404 spike in browser network panel.
- [ ] At least one E2E test passes:
  - Spawn an agent session.
  - Run an image_generation tool.
  - Confirm the chat message renders the generated image (using the new `document_id` path).
  - Wait 8 days (or backdate manually via SQL).
  - Run `attachment_gc` manually with `--dry-run`. Confirm it identifies the row.
  - Run without `--dry-run`. Confirm GCS blob is gone + `conversation_attachments` row is gone + `agent_attachment` row may still exist (ADP-owned; colmena doesn't touch it).
- [ ] Graph prompts that take `inputs.files` have been updated to instruct `load_attachment` calls.

## Rollback plan

If anything breaks after the colmena merge:

1. **Frontend rollback** alone won't help: once colmena ships, `attachment_id` and `url` are gone from the tool result. Rolling back the frontend to read `url` will just see undefined.
2. **The actual rollback** is reverting colmena `develop` to the pre-Plan-B SHA. ADP's worker re-builds against the older colmena, tool results regain `attachment_id`/`url`, both paths in chat.service.ts work, frontend renders normally.
3. **Plan A foundation stays** — `conversation_attachments` columns aren't touched by a colmena rollback, no migration to undo.
4. **`attachment_gc` schedule** should be paused before any rollback so it doesn't delete data while the system is in an inconsistent state.

---

## Files-to-modify summary (ADP repo)

| File | Plan | Change |
|---|---|---|
| `apps/api/src/chat/application/chat.service.ts` (lines 142-182) | B | Make `extractGeneratedAttachments` async; lookup `conversation_attachments` by `document_id`; generate fresh signed URL; keep fallback for legacy `attachment_id` during transition |
| `apps/api/src/chat/attachment.controller.ts` (NEW) | B | `GET /api/attachments/:documentId/url` endpoint |
| `apps/api/src/chat/chat.module.ts` | B | Register the new controller |
| `apps/api/src/gcs/internal-gcs.controller.ts` (extend) | C | `@Post('delete')` handler |
| `apps/api/src/gcs/gcs.service.ts` (extend) | C | `deleteByKey(storageKey: string)` method |
| `apps/chat/hooks/useAttachmentUrl.ts` (NEW) | B | SWR hook for client-side URL fetching |
| `apps/chat/components/chat/ChatMessage.tsx` (lines 81-103, 203) | B | Use the hook for fallback rendering when `att.url` absent |
| `apps/chat/components/chat/ChatInput.tsx` (lines 183, 408-410) | B | Audit for the same `att.url` pattern; apply hook if needed |
| `packages/database/prisma/schema.prisma` | A | Add comment noting `conversation_attachments` is colmena-owned |
| `apps/service/ia/platform/worker/src/skills/**/*.json` (or wherever graphs live) | B | Update system_prompts that depend on autoinject |

**No new Prisma migrations needed.** The `conversation_attachments` table is colmena-owned; Plan A's columns are added via sqlx, not Prisma.

## ADP Rust services — confirmed clean

Sweep performed against `apps/service/ia/platform/{worker,api,shared}/src/`:

```
attachment_id / read_url / images.*url / "url" consumers → none in production code
```

The Rust services consume colmena via the engine API (`cargo` dep on colmena develop) and pass tool results through as opaque JSON. **No Rust source changes needed.** Cloud Build re-runs the worker against the new colmena revision automatically.

---

## Estimated effort

| Plan | Backend (TypeScript) | Frontend (React) | Total |
|---|---|---|---|
| A | 0 (just verify) | 0 | ~30 min verification |
| B | ~4-6 hours (chat.service.ts migration, new endpoint, tests) | ~3-4 hours (hook + component update + canary flag) | ~7-10 hours |
| C | ~1-2 hours (extend existing controller + service) | 0 | ~1-2 hours |

**Total ADP effort:** ~10-12 hours of focused work, spread across backend + frontend devs.
