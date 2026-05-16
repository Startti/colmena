# Path/Data Attachment Registration Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `path:` and `data:` (base64) attachments — which `parse_file_entries` maps to `FileSource::InlineBytes` — flow through the same upload-and-register pipeline that `url:` (signed URL) attachments use, so they land in `conversation_attachments` and the `load_attachment` + auto-summary features work uniformly across input types.

**Architecture:** Two coupled changes. (1) In `LlmCallUseCase::resolve_one` (the canonical cache-aware resolver), replace the InlineBytes pass-through with an upload-and-dedup arm that streams the bytes to the provider's Files API via `upload_streaming`. (2) In `llm.rs::execute`, extend the gate at line 779 to fire when ANY `InlineBytes` or `SignedUrl` is present, and add a parallel `InlineBytes` arm to the no-cache fallback loop at line 842. After both changes, `InlineBytes` files become `FileSource::Uploaded` before the auto-register loop, so the existing `FileSource::Uploaded => ...` match arm registers them normally.

**Tech Stack:** Rust 1.95, `tokio`, `futures::stream`, `bytes::Bytes`, existing `FileProviderRepository::upload_streaming` port.

**Spec:** [docs/superpowers/specs/2026-05-16-load-attachment-path-data-registration-issue.md](../specs/2026-05-16-load-attachment-path-data-registration-issue.md)

---

## File Structure

**Modified:**

```
src/libs/colmena/src/llm/application/llm_call_use_case.rs
  ├─ resolve_one (line ~298)                    # replace InlineBytes pass-through
  └─ resolve_files_tests (line ~530, #[cfg(test)])  # add InlineBytes upload test

src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
  ├─ gate (line ~779)                           # extend to include InlineBytes
  ├─ no-cache match (line ~842)                 # add InlineBytes arm
  └─ TODO(v2) comment (line ~779)               # delete once fixed

docs/developer_guide/31_load_attachment.md
  └─ "Limitaciones conocidas (v1)" section      # remove item #0
```

**Fixture (already exists from Task 3 of auto-summary):**
```
src/libs/colmena/tests/fixtures/hello.pdf       # 580-byte valid PDF with "Hello World"
```

**Test graph (already exists, will be re-used with path: revert):**
```
tests/graphs/agents/load_attachment_two_agents_step1_upload.json
tests/graphs/agents/load_attachment_two_agents_step2_read.json
tests/graphs/agents/load_attachment_two_agents_step3_isolated.json
```

---

## Task 1: Failing unit test for InlineBytes upload in `resolve_one`

**Files:**
- Modify: `src/libs/colmena/src/llm/application/llm_call_use_case.rs` (inside the existing `mod resolve_files_tests` at line ~530)

- [ ] **Step 1: Write the failing test**

Open `src/libs/colmena/src/llm/application/llm_call_use_case.rs`. Locate the `mod resolve_files_tests` block (starts at ~line 530). The existing `StubCache` and `StubProvider` (lines ~544–620) are already wired and counters-instrumented — reuse them. Add this test at the bottom of the module, BEFORE the closing `}` of the mod:

```rust
    #[tokio::test]
    async fn resolve_files_uploads_inline_bytes_and_marks_uploaded() {
        // GIVEN a single InlineBytes file (e.g. from path: or data: input)
        let bytes = b"%PDF-1.4 hello world".to_vec();
        let mut files = vec![FileData {
            document_id: Some("doc-inline-1".to_string()),
            mime_type: "application/pdf".to_string(),
            filename: "hello.pdf".to_string(),
            size_hint: Some(bytes.len() as u64),
            source: FileSource::InlineBytes { bytes },
        }];

        // WHEN we run resolve_files with stub provider + stub cache
        let provider: Arc<dyn FileProviderRepository> = Arc::new(StubProvider::new());
        let cache: Arc<dyn FileCacheRepository> = Arc::new(StubCache::new());
        let fetcher = SignedUrlDownloader::new();

        LlmCallUseCase::resolve_files(
            &mut files,
            ProviderKind::Anthropic,
            provider.clone(),
            cache,
            &fetcher,
        )
        .await
        .expect("resolve_files should succeed");

        // THEN the file's source should be Uploaded with the stub-provided id
        assert_eq!(files.len(), 1);
        match &files[0].source {
            FileSource::Uploaded(r) => {
                assert_eq!(r.provider_file_id, "uploaded-1");
                assert_eq!(r.mime_type, "application/pdf");
                assert_eq!(r.filename, "hello.pdf");
            }
            other => panic!("expected Uploaded, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn resolve_files_inline_bytes_intra_request_dedup() {
        // GIVEN two InlineBytes entries with the SAME document_id
        let bytes = b"same content".to_vec();
        let mut files = vec![
            FileData {
                document_id: Some("doc-dedup".to_string()),
                mime_type: "application/pdf".to_string(),
                filename: "a.pdf".to_string(),
                size_hint: Some(bytes.len() as u64),
                source: FileSource::InlineBytes {
                    bytes: bytes.clone(),
                },
            },
            FileData {
                document_id: Some("doc-dedup".to_string()),
                mime_type: "application/pdf".to_string(),
                filename: "b.pdf".to_string(),
                size_hint: Some(bytes.len() as u64),
                source: FileSource::InlineBytes { bytes },
            },
        ];

        let stub_provider = Arc::new(StubProvider::new());
        let provider: Arc<dyn FileProviderRepository> = stub_provider.clone();
        let cache: Arc<dyn FileCacheRepository> = Arc::new(StubCache::new());
        let fetcher = SignedUrlDownloader::new();

        LlmCallUseCase::resolve_files(
            &mut files,
            ProviderKind::Anthropic,
            provider,
            cache,
            &fetcher,
        )
        .await
        .unwrap();

        // THEN only ONE upload was issued (second was deduped)
        let upload_count = *stub_provider.upload_count.lock().unwrap();
        assert_eq!(
            upload_count, 1,
            "expected exactly 1 upload for two files with same document_id, got {}",
            upload_count
        );

        // Both entries should be Uploaded with the same provider_file_id
        match (&files[0].source, &files[1].source) {
            (FileSource::Uploaded(a), FileSource::Uploaded(b)) => {
                assert_eq!(a.provider_file_id, b.provider_file_id);
            }
            _ => panic!("expected both Uploaded"),
        }
    }
```

> `StubProvider.upload_count` is already a `Mutex<usize>` field on the existing stub (line ~588). You're reading it via the same `Arc` pattern used elsewhere.

- [ ] **Step 2: Run tests to verify they FAIL**

```bash
cargo test -p colmena_dag_engine --lib \
  llm::application::llm_call_use_case::resolve_files_tests::resolve_files_uploads_inline_bytes_and_marks_uploaded \
  llm::application::llm_call_use_case::resolve_files_tests::resolve_files_inline_bytes_intra_request_dedup
```

Expected: **FAIL.** The current `resolve_one` arm for `InlineBytes` just logs and returns the file unchanged. The first test will hit `panic!("expected Uploaded, got InlineBytes {...}")`. The second will see `upload_count == 0`.

- [ ] **Step 3: Commit the failing tests**

```bash
git add src/libs/colmena/src/llm/application/llm_call_use_case.rs
git commit -m "test(load-attachment): failing tests for InlineBytes upload path"
```

---

## Task 2: Implement `InlineBytes` upload arm in `resolve_one`

**Files:**
- Modify: `src/libs/colmena/src/llm/application/llm_call_use_case.rs` (the `InlineBytes` arm of `resolve_one`, around line 299)

- [ ] **Step 1: Replace the pass-through arm**

In `src/libs/colmena/src/llm/application/llm_call_use_case.rs`, find this exact block (it's the current `InlineBytes` arm of `resolve_one`):

```rust
            FileSource::InlineBytes { .. } => {
                crate::colmena_log!(
                    "[file-resolve] '{}' is inline bytes ({}), passing through unchanged",
                    file.filename,
                    file.mime_type
                );
                Ok(file)
            }
```

Replace it with:

```rust
            FileSource::InlineBytes { bytes } => {
                let bytes_owned = bytes.clone();
                crate::colmena_log!(
                    "[file-resolve] '{}' is inline bytes ({}, {} B), uploading to {} Files API",
                    file.filename,
                    file.mime_type,
                    bytes_owned.len(),
                    provider_kind
                );

                // Intra-request dedup when the caller supplied a document_id.
                if let Some(doc_id) = file.document_id.as_deref() {
                    if let Some(r) = dedup.get(doc_id) {
                        crate::colmena_log!(
                            "[file-resolve] '{}' (id={}) inline-bytes intra-request dedup HIT — reusing file_id {}",
                            file.filename,
                            doc_id,
                            r.provider_file_id
                        );
                        file.source = FileSource::Uploaded(r.clone());
                        return Ok(file);
                    }
                }

                // Cross-request cache is intentionally NOT consulted for InlineBytes:
                // the cache key is (document_id, provider) and does not include a
                // content hash, so a stale entry could hand out a file_id pointing
                // at outdated content. The conversation_attachments registry covers
                // cross-turn reuse via load_attachment.

                let stream: BoxedByteStream = Box::pin(futures::stream::once(async move {
                    Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from(bytes_owned))
                }));
                let provider_ref = provider
                    .upload_streaming(stream, &file.mime_type, &file.filename)
                    .await?;

                crate::colmena_log!(
                    "[file-resolve] '{}' inline-bytes upload complete (file_id={})",
                    file.filename,
                    provider_ref.provider_file_id
                );

                if let Some(doc_id) = file.document_id.as_deref() {
                    dedup.insert(doc_id.to_string(), provider_ref.clone());
                }
                file.source = FileSource::Uploaded(provider_ref);
                Ok(file)
            }
```

> Key choices to preserve:
> - `bytes.clone()` is required because the closure captured by `stream::once` is `async move`, and `Bytes::from(Vec<u8>)` needs ownership.
> - We do NOT call `cache.upsert` after upload (see comment in code). The conversation_attachments registry handles cross-turn caching for InlineBytes.
> - `provider.upload_streaming(...)` returns `Result<ProviderFileRef, LlmError>` so `?` propagates correctly.

- [ ] **Step 2: Run the two new tests to verify they PASS**

```bash
cargo test -p colmena_dag_engine --lib \
  llm::application::llm_call_use_case::resolve_files_tests::resolve_files_uploads_inline_bytes_and_marks_uploaded \
  llm::application::llm_call_use_case::resolve_files_tests::resolve_files_inline_bytes_intra_request_dedup
```

Expected: **2 passed.**

- [ ] **Step 3: Run the full `resolve_files_tests` module to make sure existing tests still pass**

```bash
cargo test -p colmena_dag_engine --lib llm::application::llm_call_use_case::resolve_files_tests
```

Expected: **all green.** No SignedUrl regression.

- [ ] **Step 4: Run the full lib suite for broader regression check**

```bash
cargo test -p colmena_dag_engine --lib
```

Expected: ~750+ passed, 0 failed.

- [ ] **Step 5: Verify build is clean (deny-warnings is in effect)**

```bash
cargo build -p colmena_dag_engine
```

Expected: clean build, zero warnings.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/llm/application/llm_call_use_case.rs
git commit -m "fix(load-attachment): upload InlineBytes to provider in resolve_one"
```

---

## Task 3: Extend the upload gate in `llm.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (line ~779)

- [ ] **Step 1: Update the gate condition**

Find the existing gate (it has a TODO(v2) comment block above it that was added when the issue was first documented):

```rust
        // TODO(v2): this gate only fires for SignedUrl, leaving path: and data: (base64)
        // files un-uploaded and therefore un-registered in conversation_attachments. See
        // docs/superpowers/specs/2026-05-16-load-attachment-path-data-registration-issue.md
        // for the full diagnosis and fix plan. Workaround for v1: use `url:` (signed URL).
        if resolved_files
            .iter()
            .any(|f| matches!(f.source, crate::llm::domain::FileSource::SignedUrl(_)))
        {
```

Replace those FIVE lines with:

```rust
        if resolved_files.iter().any(|f| {
            matches!(
                f.source,
                crate::llm::domain::FileSource::SignedUrl(_)
                    | crate::llm::domain::FileSource::InlineBytes { .. }
            )
        }) {
```

> The TODO block is removed because the fix is landing in this commit. The `match` on `SignedUrl(_) | InlineBytes { .. }` is the correct union: the upload block fires for either input source, and the inner code (which goes through `LlmCallUseCase::resolve_files` when cache is available) now handles both correctly thanks to Task 2.

- [ ] **Step 2: Build (no new tests for this isolated change — Task 6 covers behavior end-to-end)**

```bash
cargo build -p colmena_dag_engine
```

Expected: clean build, zero warnings.

- [ ] **Step 3: Run the full lib suite to confirm nothing regressed**

```bash
cargo test -p colmena_dag_engine --lib
```

Expected: ~750+ passed, 0 failed.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "fix(load-attachment): fire upload gate for InlineBytes too"
```

---

## Task 4: Add `InlineBytes` arm to the no-cache fallback loop

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (line ~842, inside the `for file in resolved_files.drain(..)` loop in the no-cache branch)

- [ ] **Step 1: Add the InlineBytes upload arm**

In the same file, locate the no-cache fallback loop. It currently has a match like this (around line 842):

```rust
                for file in resolved_files.drain(..) {
                    match &file.source {
                        FileSource::SignedUrl(url) => {
                            // ... existing download + upload code ...
                        }
                        _ => new_files.push(file),
                    }
                }
```

Add a NEW arm for `FileSource::InlineBytes { bytes }` BEFORE the `_ => new_files.push(file)` catch-all. Place it as the second arm (so the order is SignedUrl, InlineBytes, _):

```rust
                        FileSource::InlineBytes { bytes } => {
                            let bytes_owned = bytes.clone();
                            let mime_type = file.mime_type.clone();
                            let filename = file.filename.clone();
                            let document_id = file.document_id.clone();
                            let size_hint = file.size_hint;

                            crate::colmena_log!(
                                "[file-resolve-no-cache] '{}' (inline, {} B) uploading to {} Files API",
                                filename,
                                bytes_owned.len(),
                                provider_kind
                            );

                            let stream: crate::llm::domain::BoxedByteStream =
                                Box::pin(futures::stream::once(async move {
                                    Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from(
                                        bytes_owned,
                                    ))
                                }));
                            match file_provider
                                .upload_streaming(stream, &mime_type, &filename)
                                .await
                            {
                                Ok(provider_ref) => {
                                    crate::colmena_log!(
                                        "[file-resolve-no-cache] '{}' (inline) uploaded as id '{}'",
                                        filename,
                                        provider_ref.provider_file_id
                                    );
                                    new_files.push(crate::llm::domain::FileData {
                                        document_id,
                                        mime_type,
                                        filename,
                                        size_hint,
                                        source: FileSource::Uploaded(provider_ref),
                                    });
                                }
                                Err(e) => {
                                    crate::colmena_log!(
                                        "[file-resolve-no-cache] WARN inline upload failed for '{}': {}",
                                        filename,
                                        e
                                    );
                                }
                            }
                        }
```

> This arm mirrors the existing SignedUrl arm above it, but skips the `downloader.stream(url)` step because the bytes are already in memory. The `Box::pin(futures::stream::once(...))` constructs the same `BoxedByteStream` shape that `upload_streaming` expects.

> The catch-all `_ => new_files.push(file)` remains last and now only handles `FileSource::Uploaded` (pre-uploaded files that pass through unchanged).

- [ ] **Step 2: Build**

```bash
cargo build -p colmena_dag_engine
```

Expected: clean build, zero warnings.

- [ ] **Step 3: Run the full lib suite**

```bash
cargo test -p colmena_dag_engine --lib
```

Expected: all pre-existing tests still pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "fix(load-attachment): upload InlineBytes in no-cache fallback path"
```

---

## Task 5: Update docs to reflect the fix

**Files:**
- Modify: `docs/developer_guide/31_load_attachment.md` (the "Limitaciones conocidas (v1)" section, item #0)
- Modify: `docs/superpowers/specs/2026-05-16-load-attachment-path-data-registration-issue.md` (status header)

- [ ] **Step 1: Remove the item #0 limitation from the developer guide**

Open `docs/developer_guide/31_load_attachment.md`. Locate the section starting with `### Limitaciones conocidas (v1)`. The current item #0 reads:

```markdown
0. **`path:` y `data:` (base64) NO se registran en `conversation_attachments`.** Issue conocido del engine v1: el bloque de upload-al-provider en `llm.rs` está gateado por la presencia de `FileSource::SignedUrl` entre los `resolved_files`, y `parse_file_entries` mapea TANTO `path:` como `data:` a `FileSource::InlineBytes`. Como `InlineBytes ≠ SignedUrl`, el gate no se dispara → los archivos nunca se vuelven `Uploaded` → la registración los skipea (`_ => continue` en la auto-register loop). El LLM en el turno actual igual ve el archivo (los bytes fluyen por la SDK del provider via `LlmMessage::user_with_files`), pero los turnos siguientes no lo ven via `load_attachment` y el auto-summary no se dispara. **Esto afecta a `load_attachment` base, no solo al auto-summary.** Spec del fix: [docs/superpowers/specs/2026-05-16-load-attachment-path-data-registration-issue.md](../superpowers/specs/2026-05-16-load-attachment-path-data-registration-issue.md). **Workaround actual:** usá signed URLs (`url:` field) — `gsutil signurl` contra cualquier bucket GCS sirve para testing local.

1. **Archivos inline (base64) no se summarizan.** Cuando el integrator sube un archivo embebido en el JSON (campo `data`) sin un `url` o `path` que lo respalde, los bytes se consumen durante el upload streaming al provider y no se retienen para una segunda lectura. En esos casos `AttachmentSource::Inline` se guarda en el registry pero el path de summary salta esa fila. (Solo aplica DESPUÉS de que el issue v1 #0 esté arreglado — actualmente ni siquiera llegan al registry.) **Workaround:** pasá `description` manualmente en el `files[]` entry. **Plan v2:** tee el stream de upload para retener bytes sin doble-descarga.
```

Replace those two paragraphs with:

```markdown
1. **Archivos `data:` (base64 inline) no se summarizan.** Cuando el integrator sube un archivo embebido en el JSON (campo `data`) sin un `url` o `path` que lo respalde, los bytes se consumen durante el upload streaming al provider y no se retienen para una segunda lectura. En esos casos `AttachmentSource::Inline` se guarda en el registry pero el path de summary salta esa fila. Los archivos con `path:` SÍ se summarizan correctamente — el summary path re-lee del disco via `AttachmentSource::Path`. **Workaround para `data:`:** pasá `description` manualmente en el `files[]` entry. **Plan v2:** tee el stream de upload para retener bytes sin doble-descarga.
```

> Item #0 is removed entirely (the bug is fixed by this plan). Item #1 stays but is reworded to clarify it only affects the `data:` (base64) case — `path:` files now work end-to-end.

- [ ] **Step 2: Mark the issue spec as fixed**

Open `docs/superpowers/specs/2026-05-16-load-attachment-path-data-registration-issue.md`. Find this line near the top:

```markdown
**Status:** Known v1 limitation — fix deferred to v2
```

Replace with:

```markdown
**Status:** Fixed in [docs/superpowers/plans/2026-05-16-load-attachment-path-data-fix.md](../plans/2026-05-16-load-attachment-path-data-fix.md)
```

- [ ] **Step 3: Commit**

```bash
git add docs/developer_guide/31_load_attachment.md docs/superpowers/specs/2026-05-16-load-attachment-path-data-registration-issue.md
git commit -m "docs(load-attachment): mark path/data registration issue fixed"
```

---

## Task 6: End-to-end verification with path-based two-agent flow

**Files:**
- Modify: `tests/graphs/agents/load_attachment_two_agents_step1_upload.json` (revert to local fixture path)

- [ ] **Step 1: Revert step1 to use `path:` instead of placeholder URL**

Open `tests/graphs/agents/load_attachment_two_agents_step1_upload.json`. The `files[]` entry currently looks like:

```json
        "files": [
          {
            "id": "shared_doc",
            "label": "Shared invoice",
            "url": "$REPLACE_WITH_SIGNED_URL",
            "mime_type": "application/pdf",
            "filename": "DelayedReceipt.pdf"
          }
        ]
```

Replace with the `path:` version pointing at the local fixture committed in the auto-summary plan (Task 3):

```json
        "files": [
          {
            "id": "shared_doc",
            "label": "Hello PDF fixture",
            "path": "src/libs/colmena/tests/fixtures/hello.pdf",
            "mime_type": "application/pdf",
            "filename": "hello.pdf"
          }
        ]
```

- [ ] **Step 2: Validate JSON**

```bash
python3 -c "import json; json.load(open('tests/graphs/agents/load_attachment_two_agents_step1_upload.json')); print('valid')"
```

Expected: `valid`.

- [ ] **Step 3: Run the three-step end-to-end test against Postgres**

Pre-requisite: `.env` set with `DATABASE_URL` (Postgres) and `GEMINI_API_KEY`. `psql` available on PATH (libpq). The fixture `src/libs/colmena/tests/fixtures/hello.pdf` exists.

Use a fresh `--agent-session-id` so the run starts clean:

```bash
source .env
export PATH="/opt/homebrew/opt/libpq/bin:$PATH"
SID=agent_path_fix_001

# Step 1: upload via path
cargo run --bin dag_engine -- run \
  tests/graphs/agents/load_attachment_two_agents_step1_upload.json \
  --agent-session-id $SID

# DB check: row should exist with description set
psql "$DATABASE_URL" -c \
  "SELECT document_id, source_kind, description IS NOT NULL AS has_summary \
   FROM conversation_attachments WHERE agent_session_id = '$SID';"
```

Expected output (sample):

```
 document_id |  source_kind  | has_summary
-------------+---------------+-------------
 shared_doc  | path          | t
(1 row)
```

> The `source_kind` should now be `path` (not `signed_url`) and `has_summary` should be `t` (auto-summary ran and persisted).

- [ ] **Step 4: Run step 2 (reader with access)**

```bash
cargo run --bin dag_engine -- run \
  tests/graphs/agents/load_attachment_two_agents_step2_read.json \
  --agent-session-id $SID
```

Expected: the SSE log contains a `tool-input-available` event with `"toolName":"load_attachment","input":{"document_id":"shared_doc"}`, and the final assistant message describes the document contents.

- [ ] **Step 5: Run step 3 (isolated reader)**

```bash
cargo run --bin dag_engine -- run \
  tests/graphs/agents/load_attachment_two_agents_step3_isolated.json \
  --agent-session-id $SID
```

Expected: NO `tool-input-available` event (the isolated reader doesn't have the tool). The final assistant message says it has no access to any document.

- [ ] **Step 6: Final DB check — confirm single registration and no duplicates**

```bash
psql "$DATABASE_URL" -c \
  "SELECT count(*) AS rows FROM conversation_attachments WHERE agent_session_id = '$SID';"
```

Expected: `rows = 1`.

- [ ] **Step 7: Commit the graph revert**

```bash
git add tests/graphs/agents/load_attachment_two_agents_step1_upload.json
git commit -m "test(load-attachment): revert two-agent step1 to local path fixture"
```

---

## Final verification

- [ ] **Step 1: Full test sweep**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p colmena_dag_engine --verbose
```

Expected: all green (including doctests).

- [ ] **Step 2: Confirm the spec is up to date**

```bash
git log --oneline -10
```

Look for the commit chain: failing tests → resolve_one fix → gate → no-cache arm → docs → graph revert.

---

## Open caveats for the implementer

- **`bytes::Bytes` import.** Both Task 2 and Task 4 use `bytes::Bytes` to build the stream. The `bytes` crate is already a transitive dep (the `BoxedByteStream` type uses it). If `bytes` is not directly listed in `src/libs/colmena/Cargo.toml` and the build complains about an unresolved path, add `bytes = "1"` to the `[dependencies]` table. Most likely it's already there because `BoxedByteStream` is widely used.
- **`futures::stream::once`.** Confirm `futures = "0.3"` is in `Cargo.toml` (it already is — used elsewhere in the file).
- **Provider-specific image passthrough.** `resolve_one` has a special branch around line 332 where Anthropic and OpenAI image-mime SignedUrls are passed through without upload. This does NOT need a parallel for InlineBytes — inline image bytes have no URL to pass, so they MUST be uploaded. Don't add an early-return for InlineBytes + image mimes; the Files API path works for both providers.
- **Test fixture vs real LLM.** Task 6 runs against a real Gemini API key. If you only want to test the upload path locally without spending tokens, you can skip step 4 and step 5 — the DB check in step 3 alone confirms the fix worked (row registered + summary populated). The unit tests in Task 1 cover the upload logic in isolation.
