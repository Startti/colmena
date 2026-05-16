# Issue — `load_attachment` registration skipped for `path:` / `data:` (base64) files

**Status:** Fixed in [docs/superpowers/plans/2026-05-16-load-attachment-path-data-fix.md](../plans/2026-05-16-load-attachment-path-data-fix.md)
**Date raised:** 2026-05-16
**Discovered during:** Two-agent integration testing of the auto-summary feature
**Affects:** Both `load_attachment` (the base feature) and the new auto-summary path. They share the same registration code so neither works for inline-source files.

---

## Summary

When a `files[]` entry uses `path:` (read from disk) or `data:` (base64 inline) instead of `url:` (signed URL), the file **never lands in the `conversation_attachments` table**. The LLM in the current turn DOES see the file (the bytes are passed to the provider SDK through the regular `LlmMessage::user_with_files` path) — but the entry is not visible to subsequent turns via `load_attachment`, and no auto-summary is generated.

In production this is mostly invisible because the typical caller (ADP) uses signed URLs. Local testing with `path:` to a fixture file, and any caller passing base64 inline data, hits the gap immediately.

## Reproducer

Graph (`tests/graphs/agents/load_attachment_two_agents_step1_upload.json` in the path variant):

```json
{
  "files": [{
    "id": "shared_doc",
    "path": "src/libs/colmena/tests/fixtures/hello.pdf",
    "mime_type": "application/pdf",
    "filename": "hello.pdf"
  }]
}
```

Run:

```bash
source .env
cargo run --bin dag_engine -- run <graph> --agent-session-id agent_two_002
```

Expected: row appears in `conversation_attachments` for `agent_session_id = 'agent_two_002'`.

Actual: zero rows. LLM responds normally ("Documento recibido"), but nothing was registered.

```sql
SELECT count(*) FROM conversation_attachments WHERE agent_session_id = 'agent_two_002';
-- 0
```

## Root cause

Two coupled gates in `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`:

### Gate 1 — upload-to-provider only fires when `SignedUrl` is present (line 779)

```rust
if resolved_files
    .iter()
    .any(|f| matches!(f.source, FileSource::SignedUrl(_)))
{
    // ... download (signed URL) + upload to provider Files API ...
    // ... convert FileSource::SignedUrl → FileSource::Uploaded ...
}
```

`parse_file_entries` (line 2141-2192) maps:
- `data:` → `FileSource::InlineBytes { bytes }`
- `path:` → `FileSource::InlineBytes { bytes }` (after `fs::read`)
- `url:` → `FileSource::SignedUrl(url)`

So a `files[]` with ONLY `path` or `data` entries leaves `resolved_files` full of `InlineBytes`, the gate is `false`, and the upload block is skipped entirely.

The file's bytes still flow to the provider's SDK at the regular `LlmMessage::user_with_files` step (line 1044), so the LLM call works for THIS turn. But the bytes never get a `provider_file_id` they can be referenced by in future turns.

### Gate 2 — auto-register only handles `FileSource::Uploaded` (line 988)

```rust
let provider_file_id = match &file.source {
    FileSource::Uploaded(r) => r.provider_file_id.clone(),
    _ => continue, // Not uploaded yet — skip registration this pass.
};
```

Even if gate 1 happened to be permissive, an `InlineBytes` file would still hit the `_ => continue` and skip the `reg.upsert(...)` call.

### Combined effect

| Source kind | Gate 1 (upload) | Gate 2 (register) | Result |
|---|---|---|---|
| `SignedUrl` | ✅ fires, → `Uploaded` | ✅ registers | works |
| `path:` (→ `InlineBytes`) | ❌ skipped | ❌ skipped | no registration, no auto-summary, no cross-turn `load_attachment` |
| `data:` (→ `InlineBytes`) | ❌ skipped | ❌ skipped | same as above |

## Why this slipped through

- The original `load_attachment` spec assumed signed URLs as the primary use case (ADP-driven workflow). v1 tests used signed URLs.
- The auto-summary feature inherited the same scope — its spec mentioned an `AttachmentSource::Inline` v1 skip but that's a different concern (no bytes retained after upload). What was missed is the upstream gap: `InlineBytes` files never even reach the registration loop in a state where registration would succeed.
- The base feature's `_ => continue` was justified at the time as "not uploaded yet — skip" — assuming the upload happens elsewhere for non-URL sources. In fact it does NOT, for the auto-register pre-LlmCall window.

## Proposed fix (v2)

Two-step change in `llm.rs`:

### Step A — extend gate 1 to include `InlineBytes`

```rust
if resolved_files.iter().any(|f| matches!(
    f.source,
    FileSource::SignedUrl(_) | FileSource::InlineBytes { .. }
)) {
    // ... existing block, but also handle InlineBytes inside ...
}
```

### Step B — add an `InlineBytes` arm inside the upload loop

Both the with-cache branch (`LlmCallUseCase::resolve_files`) and the no-cache branch (the explicit `match &file.source` loop at line 842) need to handle `InlineBytes`:

```rust
FileSource::InlineBytes { bytes } => {
    let stream = futures::stream::once(async move {
        Ok::<Bytes, std::io::Error>(Bytes::from(bytes))
    });
    let boxed: BoxedByteStream = Box::pin(stream);
    match file_provider.upload_streaming(boxed, &mime_type, &filename).await {
        Ok(provider_ref) => {
            new_files.push(FileData {
                document_id,
                mime_type,
                filename,
                size_hint,
                source: FileSource::Uploaded(provider_ref),
            });
        }
        Err(e) => { /* warn */ }
    }
}
```

The `LlmCallUseCase::resolve_files` orchestrator probably needs the same arm — check its current implementation and add symmetric handling.

### Why no AttachmentSource problem after the fix

After step A+B, `InlineBytes` files become `Uploaded` BEFORE the registration loop. The existing `AttachmentSource` mapping (line 962-975) already covers this case:
- `FileSource::Uploaded(_)` + raw has `path` → `AttachmentSource::Path(path)` ← summary can re-read from disk
- `FileSource::Uploaded(_)` + raw has `data` (no url/path) → `AttachmentSource::Inline` ← summary still skipped (matches the v1 "inline-bytes lost" non-goal, expected behavior)

So `path:` files get full feature support after the fix. `data:` files get registration but no auto-summary (acceptable — the v1 inline-skip applies).

## Testing plan for the fix

1. Add a unit test against the no-cache branch: feed `InlineBytes`, assert `Uploaded` after.
2. Add an integration test that mirrors the existing two-agent scenario but with `path:` instead of `url:`. Assert:
   - Row exists in `conversation_attachments` after step 1.
   - `description` is non-null after auto-summary completes.
   - Step 2 (reader) sees the doc via `load_attachment`.
3. Re-run the local two-agent test graphs (`load_attachment_two_agents_step1_upload.json` etc.) with `path:` to the local fixture.

## Workaround for current users (v1)

Pass `url:` (signed URL) instead of `path:` / `data:`. ADP already does this in production. For local testing, generate a signed URL with `gsutil signurl` against any GCS bucket and use it in the `url:` field.

## Affected docs

- `docs/developer_guide/31_load_attachment.md` — update the "Limitaciones conocidas (v1)" section to flag this prominently (currently mentions only `AttachmentSource::Inline` lost-bytes case, but the upstream `InlineBytes` gate is also broken).
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs:779-781` — add a `TODO(v2)` comment pointing at this issue file.
