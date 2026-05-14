# Attachment Auto-Summary — Design

**Status:** Approved for planning
**Date:** 2026-05-14
**Author:** Daniel Garcia (brainstormed with Claude)
**Target version:** 0.4.0
**Related:**
- [docs/superpowers/specs/2026-05-13-load-attachment-design.md](2026-05-13-load-attachment-design.md) — base feature this extends
- [docs/developer_guide/31_load_attachment.md](../../developer_guide/31_load_attachment.md) — current load_attachment guide

---

## Summary

Extend `load_attachment` so that, when the caller does **not** supply a `description` for a registered file, `llm_call` automatically generates a short 1-line summary in parallel with the main LLM call and persists it in the `conversation_attachments` registry. The summary becomes part of the catalog the LLM sees in the `load_attachment` tool description, letting the model pick the right `document_id` without the caller having to write metadata by hand.

The generation flow is:

1. Extract text from the file locally (`pdf-extract` for PDFs, UTF-8 decode for `text/*`).
2. Truncate the extracted text to a configurable char cap (default 5000 ≈ first 2 pages of prose).
3. Send the truncated text — or the image itself for image mimes — to the provider's cheap-tier model with a 1-line summary prompt.
4. Run the summary call in parallel with the answer call via `tokio::join!`, bounded by a hard timeout.
5. Persist the result in the existing `description` column. Failures are silent: `description=null` and the catalog falls back to `filename`.

## Motivation

The base `load_attachment` design assumes the caller provides a `description` per file in the `files[]` JSON entry. In practice, ADP and other integrators often upload documents straight from a UI where the only metadata available is `filename`. Filenames are sometimes useless (`Screenshot 2026-05-14.png`, `document.pdf`, `untitled.docx`) and the LLM can't pick the right `document_id` from a catalog of three "document.pdf" entries.

We need a way to populate `description` automatically, cheaply, and reliably — without forcing every caller to wire metadata logic upstream.

## Goals

- **Free for the caller.** Zero new required fields. If `description` is missing, the system fills it in.
- **Cheap.** Cost per document should be sub-cent at default settings (~$0.0003 with Gemini Flash on 5000 chars of text).
- **Latency-neutral.** Summary runs in parallel with the answer call. Turn-1 latency is `max(answer, summary)`, and with a small text-only prompt the summary is almost always faster than the answer.
- **Provider-agnostic.** Works with Google, OpenAI, and Anthropic out of the box. Same provider as the main call, so only one API key is needed.
- **Best-effort.** Summary failures never block the answer. The catalog falls back to `filename` gracefully.
- **Opt-out.** A new `summary_enabled` flag lets graph authors disable the feature when they handle metadata themselves.

## Non-goals

- **Re-summarising existing rows.** Once a row is in `conversation_attachments`, we do not regenerate the summary, even if the first attempt yielded `null`. Future flag `force_resummary` is out of scope.
- **Multi-page summaries.** The summary is a single line (~150 chars) intended for the catalog. Long-form document summarisation is a different feature.
- **OCR for image-only PDFs.** If `pdf-extract` returns empty text on a PDF (image-based, scanned), we fall back to `filename`. We do not run OCR.
- **Office formats in v1.** `docx`, `xlsx`, `pptx` get `filename` fallback initially. Extractors can be added later without rearchitecting.
- **Token-exact truncation.** We truncate by characters (cheap, deterministic) instead of by provider tokens. Estimation is "~4 chars per token", precise enough for cost prediction.
- **Caching the summary cross-session.** Each `agent_session_id` gets its own registry row; same physical file uploaded into two sessions will be summarised twice.
- **Background / fire-and-forget tasks.** Summary completes within the turn (parallel with answer, bounded by timeout). No `tokio::spawn` orphans.

## Architecture

### Data flow — turn 1, file with no caller-supplied description

```
Graph JSON → llm_call.config.files[i] (no `description` field)
   ↓
parse_file_entries → FileSource
   ↓
resolve_files (existing) → uploads via provider Files API, returns Uploaded(ProviderFileRef)
   ↓
[EXISTING] AttachmentRegistry::upsert with description=None
   ↓
[NEW] Build summary_task per file:
   - get bytes (see "Byte acquisition" below)
   - if bytes.len() > summary_max_bytes: yield SummaryOutcome::Skipped (too large)
   - extract_text(mime, bytes) → Option<String>
   - if Some(text): truncate_chars(text, summary_max_chars) → send to summary_llm (text path)
   - if mime starts_with "image/": send raw bytes → summary_llm (image path)
   - else: yield SummaryOutcome::Skipped (unsupported mime / no text extractable)
   ↓
tokio::join!(
   answer_call,
   timeout(summary_timeout, summary_tasks_grouped)
)
   ↓
For each successful summary: AttachmentRegistry::update_description(document_id, summary)
   ↓
Return answer to caller (summary errors are logged, never propagated)
```

### Byte acquisition

The upload pipeline (`upload_streaming`) consumes the bytes stream and does not retain them — we need a separate way to feed `extract_text`. Per `FileSource`:

| Source | Strategy |
|---|---|
| `Inline(bytes)` | Already have the bytes in memory. Pass directly to extraction. No I/O. |
| `SignedUrl(url)` | Re-issue a `downloader.stream(url)` call **in parallel with the answer call**, collect into a `Vec<u8>` bounded by `summary_max_bytes`, then extract. |
| `Path(path)` | Read the file from disk (`tokio::fs::read`), bounded by `summary_max_bytes`. |

The re-download for `SignedUrl` is the v1 simplification. A future optimisation can tee the original upload stream so we only download once, but v1 prioritises code simplicity — the re-download happens concurrently with the answer call, so user-facing latency is unaffected. Network cost for signed URLs is negligible (typically same-region object storage).

### Size cap

`summary_max_bytes` (default 25 MB) guards against memory bombs. If the file exceeds the cap, we skip extraction entirely and persist `description=null`. The cap is applied at the byte-acquisition layer (before allocating the full `Vec<u8>`), so streaming downloads stop early. For typical document workflows 25 MB ≫ 200 pp of PDF, so this rarely triggers.

### Data flow — turn N, file already in registry

No change. Catalog read uses the persisted `description` (which may be the auto-generated string, the caller-supplied string, or `NULL`).

### Data flow — provider_file_id expired (cache miss, re-upload)

Existing recovery path is unchanged. Summary is **not** regenerated — we trust the existing `description` (or `NULL`) value.

### Layer split

| Layer | New component | Responsibility |
|---|---|---|
| Domain | `AttachmentSummaryGenerator` trait | Port: given input bytes/text + config, returns a 1-line summary or error |
| Domain | `SummaryInput`, `SummaryConfig`, `SummaryOutcome`, `SummaryError` value objects | Inputs/outputs for the generator |
| Domain | `TextExtractor` trait + `extract_text` dispatcher fn | Port: given a MIME and bytes, returns extracted text or None |
| Infrastructure | `LlmAttachmentSummaryGenerator` | Adapter using existing `LlmRepository` / `LlmCallUseCase` to make the cheap-model call |
| Infrastructure | `PdfTextExtractor` (uses `pdf-extract` crate) | PDF → text |
| Infrastructure | `PlaintextTextExtractor` | `text/plain`, `text/markdown`, `text/csv`, `text/html` → UTF-8 string |
| Infrastructure | `provider_cheap_tier()` helper | Maps `ProviderKind` → default cheap model name |
| Wiring | `llm.rs::execute` | Builds and joins summary tasks alongside the answer call |
| Persistence | (none — `description` column already nullable) | — |

## Data model

**No schema changes.** The existing `conversation_attachments.description` column (`TEXT NULL` in both Postgres and SQLite) accepts the auto-generated string transparently. Catalog rendering via `catalog_line()` already handles `description IS NULL`.

We do **not** add a `description_source` column to distinguish caller-supplied vs auto-generated descriptions. Once written, both look the same to the catalog. If we later need the distinction (e.g., for forced re-summary), we add the column then.

## Config schema

New fields on the `llm_call` node config (all optional, all top-level):

| Field | Type | Default | Description |
|---|---|---|---|
| `summary_enabled` | `bool` | `true` | Opt-out for auto-summary. When `false`, files without caller-supplied `description` stay with `description=null`. |
| `summary_max_chars` | `int` | `5000` | Maximum characters of extracted text sent to the summary LLM. Roughly equals the first 2 pages of prose. |
| `summary_model` | `string` | provider cheap-tier | Override the model used for summarisation. Empty/missing → use the cheap-tier default for the main provider. |
| `summary_timeout_secs` | `int` | `15` | Hard timeout on the summary call. On exceed, the summary task is cancelled and `description` stays null. |
| `summary_max_output_chars` | `int` | `200` | The summary prompt instructs the model to keep output under this many chars. Soft cap; we also truncate post-hoc. |
| `summary_max_bytes` | `int` | `26214400` (25 MB) | Hard ceiling on file size for which extraction is attempted. Above this, summary is skipped and `description=null`. |

`docs/node_configurations.json` will be updated with these fields.

### Cheap-tier mapping (hardcoded in `provider_cheap_tier`)

| Provider | Cheap-tier model |
|---|---|
| Google | `gemini-2.5-flash` |
| OpenAI | `gpt-4o-mini` |
| Anthropic | `claude-haiku-4-5-20251001` |

These are intentionally hardcoded in a single Rust function. When a cheaper model is released for a provider, we update one line. No env vars, no DB config.

## Text extraction

### Dispatch by MIME

```rust
pub fn extract_text(mime: &str, bytes: &[u8]) -> Result<Option<String>, ExtractError> {
    match mime {
        "application/pdf" => extract_pdf(bytes),
        "text/plain" | "text/markdown" | "text/csv" | "text/html"
            => extract_utf8(bytes),
        _ => Ok(None), // unsupported -> skip summary, fall back to filename
    }
}
```

Returns `Ok(None)` (not `Err`) for unsupported MIMEs — extraction "succeeded" in saying "no text available". `Err` is reserved for **malformed input** of a supported MIME (corrupt PDF, invalid UTF-8).

### Truncation

After extraction, the caller truncates by **`char_indices`** (not raw byte slice) to avoid splitting multi-byte UTF-8 characters:

```rust
fn truncate_chars(s: &str, max_chars: usize) -> String {
    s.char_indices()
        .nth(max_chars)
        .map(|(i, _)| s[..i].to_string())
        .unwrap_or_else(|| s.to_string())
}
```

### Image path (no extraction)

For MIME types matching `image/*` (`image/png`, `image/jpeg`, `image/webp`, `image/gif`), we skip the extractor entirely and pass the image bytes to the summary LLM as a vision input. Gemini Flash, GPT-4o-mini, and Claude Haiku all support vision. One image ≈ 258 tokens — cost is negligible.

### PDF specifics

`pdf-extract` is pure Rust, no native dependencies. It returns a single `String` containing all extracted text. We **do not** stop at page 2 inside the crate — we extract everything and then char-truncate. The `pdf-extract` runtime is fast enough that this is acceptable for the document sizes we expect (~500 pages or less). If we ever need true page-bounded extraction, we swap the adapter for `lopdf`.

Empty result from `pdf-extract` (image-only PDF, no text layer, encrypted) → `Ok(None)` → fallback to `filename`.

## Summary LLM call

The summary call is a **one-shot, history-less** invocation. It bypasses `LlmCallUseCase` (which persists turns in `llm_node_history`) and goes directly through `LlmRepository::complete` so that:

- No conversation history is built or stored for the summary call.
- The summary turn does not appear in the answer call's context.
- It uses a `node_id` like `__summary_<doc_id>__` for telemetry/logging only; nothing is written to `llm_node_history`.

### Prompt template (text-extracted path)

```
SYSTEM:
You are a document cataloger. Given the first N characters of a document's
extracted text, output a single short description (max {max_output_chars}
characters) that helps a downstream LLM decide whether this document is
relevant to a user's question. Focus on: document type, topic, and time
period if relevant. No commentary, no quotes, no markdown. Just the
description on one line.

USER:
Filename: {filename}
MIME type: {mime_type}
Extracted text (truncated to {max_chars} chars):
---
{extracted_text}
---
```

### Prompt template (image path)

```
SYSTEM:
You are a document cataloger. Look at the attached image and output a single
short description (max {max_output_chars} characters) that helps a
downstream LLM decide whether this image is relevant to a user's question.
Focus on: subject, type of image, salient details. No commentary, no
markdown. Just the description on one line.

USER:
Filename: {filename}
[image attached]
```

### Output validation

Post-process the model output:
- Trim whitespace and surrounding quotes.
- Collapse internal newlines to spaces.
- Truncate to `summary_max_output_chars` (default 200) by char count.
- If empty after trim → `SummaryError::EmptyResponse` → persist `description=null`.

## Concurrency model

### Parallel execution

The summary tasks and the answer call run via `tokio::join!`:

```rust
let summary_fut = tokio::time::timeout(
    summary_timeout,
    run_summaries(summary_inputs, &summary_generator),
);
let (answer, summary_outcomes) = tokio::join!(answer_fut, summary_fut);
```

If multiple files need summarising in the same turn, their summary calls run **concurrently** with each other (not sequentially) via `futures::future::join_all`, so total time is `max(individual_summary_calls)` not the sum.

### Timeout

`tokio::time::timeout(summary_timeout_secs, ...)` wraps the whole batch of summary tasks. On timeout, the batch is cancelled (drops the futures); the answer call is unaffected because the two are joined, not raced.

Worst-case turn-1 latency: `max(answer_latency, summary_timeout_secs)`. With the default 15s timeout, this is bounded.

### Persistence ordering

After `tokio::join!` returns:

1. If `summary_outcomes` is `Ok(results)`: for each result with a non-empty summary, call `registry.update_description(document_id, summary)`. Failures (per-file) are logged; we continue to the next file.
2. If `summary_outcomes` is `Err(_)` (timeout): no updates. Rows stay with `description=null`.
3. If `answer` is `Err(_)`: we **still** persist any successful summaries before returning the answer error. The summary helps turn 2 regardless of turn-1 success.

### Cancellation

If the parent `execute()` future is dropped (user aborts the request), both `answer_fut` and `summary_fut` are dropped — Rust's `Drop` for futures cancels the in-flight tasks naturally. No orphans.

## Error handling matrix

| Scenario | Behaviour | Persisted state |
|---|---|---|
| `summary_enabled = false` | Skip extraction and LLM call entirely. | `description = caller-supplied or null` |
| Caller passed non-empty `description` | Skip generation. Use the supplied value. | `description = caller value` |
| File size exceeds `summary_max_bytes` | Skip extraction entirely. Log info. | `description = null` |
| MIME unsupported (e.g. `application/zip`) | Skip generation. Log info. | `description = null` |
| `pdf-extract` returns empty (image-only PDF) | Skip LLM call. Log info. | `description = null` |
| `pdf-extract` returns `Err` (corrupt PDF) | Skip LLM call. Log warn. | `description = null` |
| Byte acquisition fails (`download.stream` error, file read error) | Skip LLM call. Log warn. | `description = null` |
| Summary LLM call fails (network, 5xx, parse) | Log warn. | `description = null` |
| Summary LLM returns empty / whitespace-only | Log warn. | `description = null` |
| Summary batch exceeds timeout | Cancel batch. Log warn. | All rows: `description = null` |
| Answer succeeds, summary succeeds | Persist summary. Return answer. | `description = summary` |
| Answer fails, summary succeeds | Persist summary before returning answer error. | `description = summary` |
| Provider file expired (re-upload path) | Existing logic re-uploads. **Summary is NOT regenerated.** | `description` unchanged |

## Testing strategy

### Unit tests

- `extract_text`: dispatches by MIME correctly.
- `PdfTextExtractor`: extracts from a sample PDF; returns `Ok(None)` for image-only PDF; returns `Err` for corrupt bytes.
- `PlaintextTextExtractor`: round-trips UTF-8, returns `Err` for invalid UTF-8.
- `truncate_chars`: handles multi-byte chars correctly, no panics at boundaries.
- `provider_cheap_tier`: returns the expected default per `ProviderKind`.
- `LlmAttachmentSummaryGenerator` (with mocked `LlmRepository`):
  - Builds the right prompt for text input.
  - Builds the right prompt for image input.
  - Validates and truncates output.
  - Returns `EmptyResponse` on whitespace-only output.

### Integration tests

- Mocked LLM end-to-end:
  - Register a file with no `description` → assert `description` column populated after turn 1.
  - Register two files in one turn → both summaries persist; calls ran concurrently.
  - Summary times out → answer still returns successfully; `description` stays null.
  - `summary_enabled = false` → no summary call made; `description` stays null.
  - Caller-supplied `description` present → summary call skipped; description preserved.
- Real LLM (gated with `#[ignore]`):
  - `tests/graphs/agents/load_attachment_with_auto_summary.json` — Gemini Flash, real PDF, asserts catalog has non-empty description in turn 2.

### CI considerations

All `#[ignore]` tests must be runnable with `source .env && cargo test -- --ignored`. The new test graph uses `provider: "google"` + `gemini-2.5-flash` and reads `GEMINI_API_KEY` and `DATABASE_URL` from `.env`.

## Dependencies

Add to `src/libs/colmena/Cargo.toml`:

```toml
pdf-extract = "0.7"  # pin to a recent stable
```

No other new crates. `tokio::time::timeout` and `futures::future::join_all` are already in the tree.

## Open questions

1. **Should image summaries be opt-out separately?** Today, `summary_enabled = false` disables everything. Some graphs may want text summaries enabled but skip image summaries (vision-token cost). Defer until requested.
2. **Should we cap concurrent summary calls?** If a turn registers 20 files, we'd fire 20 parallel summary requests. With small payloads this is fine for Gemini's quota but could trip rate limits on stricter providers. For v1, no cap; revisit if it becomes an issue.
3. **Should `summary_max_chars` accept tokens instead of chars in a future version?** Char-based is cheap and deterministic. If integrators want token-precision, we add a separate `summary_max_tokens` field later (mutually exclusive with `summary_max_chars`).

## Out-of-scope follow-ups

- Office-format extractors (`docx`, `xlsx`, `pptx`) — separate spec when needed.
- OCR for image-only PDFs (Tesseract or vision-LLM passthrough).
- Caller-controlled prompt template override (`summary_prompt`).
- Caching summaries across sessions (deduplicated by content hash).
- Background re-summary worker for rows with `description=null`.
