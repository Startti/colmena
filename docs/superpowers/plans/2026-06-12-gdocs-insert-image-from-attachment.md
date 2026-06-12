# gdocs_insert_image from attachment (Approach A) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let `gdocs_insert_image_after_text` insert an image from an `attachment_id` (generated/edited/inline/signed-url) by uploading the bytes to Drive, making them public, inserting via the `lh3.googleusercontent.com/d/<id>` content URL, then deleting the temp Drive file.

**Architecture:** Approach A from the spec. A new application helper `run_insert_image_from_bytes` (in `gdocs/application/insert.rs`) orchestrates upload→public→insert→cleanup over the injected `DocsClient` (testable with the existing mockall `MockDocsClient`). The `gdocs_insert_image_after_text` dispatcher gains an `attachment_id` branch (XOR with `image_url`) routed through the executor (`fetch_attachment_bytes`). Three additive `DocsClient` methods do the Drive plumbing. Empirically corroborated 2026-06-12 (see spec §4).

**Tech Stack:** Rust, `reqwest` (HTTP), `mockall` (trait mocks), `wiremock` (HTTP mocks), `schemars` (JsonSchema), PyO3 sandbox unrelated. Cargo package `colmena_dag_engine`.

**Spec:** [`docs/superpowers/specs/2026-06-12-gdocs-insert-image-from-attachment-design.md`](../specs/2026-06-12-gdocs-insert-image-from-attachment-design.md)

---

## File Structure

- `src/libs/colmena/src/gdocs/domain/traits.rs` — +3 `DocsClient` methods.
- `src/libs/colmena/src/gdocs/infrastructure/http_client.rs` — HTTP impl of the 3 + wiremock tests.
- `src/libs/colmena/src/gdocs/application/insert.rs` — `run_insert_image_from_bytes` helper + mime helper + unit tests.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs` — args change (`image_url` → `Option`, add `attachment_id`), new `dispatch_..._via_executor`, arg-validation tests.
- `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` — route the tool through the via_executor variant (pass `self`).
- `src/libs/colmena/text/tools/gdocs.yaml` — document `attachment_id`.
- `docs/developer_guide/45_gdocs.md`, `docs/CHANGELOG_2026-06.md`, `docs/BACKLOG.md` — docs.
- `tests/graphs/agents/gdocs_insert_image_from_attachment_e2e.json` — E2E graph.

**Mockall note:** `DocsClient` is mocked with `mockall` (`MockDocsClient`, used via `expect_*` in `insert.rs` app_tests). Adding trait methods auto-generates `expect_upload_image_to_drive` / `expect_set_anyone_reader` / `expect_delete_drive_file` — no manual mock edits needed.

---

## Task 1: Add the 3 Drive plumbing methods to `DocsClient`

**Files:**
- Modify: `src/libs/colmena/src/gdocs/domain/traits.rs`
- Modify: `src/libs/colmena/src/gdocs/infrastructure/http_client.rs`
- Test: `src/libs/colmena/src/gdocs/infrastructure/http_client.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Add trait methods**

In `traits.rs`, inside `pub trait DocsClient` (after `delete_permission`), add:

```rust
    /// Upload raw image bytes to Drive as a standalone image file (NO Doc
    /// conversion). Returns the Drive `file_id`. Used to host an attachment
    /// publicly so `insertInlineImage` can fetch it. `mime` must be an image
    /// type (e.g. `image/png`). `filename` is the Drive file name.
    async fn upload_image_to_drive(
        &self,
        bytes: Vec<u8>,
        mime: &str,
        filename: &str,
    ) -> Result<String, DocsError>;

    /// Grant `anyone with the link` reader access to a Drive file (so Google
    /// can fetch it server-side for `insertInlineImage`).
    async fn set_anyone_reader(&self, file_id: &str) -> Result<(), DocsError>;

    /// Delete a Drive file by id (`files.delete`). Used to clean up the temp
    /// image after Docs has copied it into the document.
    async fn delete_drive_file(&self, file_id: &str) -> Result<(), DocsError>;
```

- [ ] **Step 2: Write failing wiremock test for `upload_image_to_drive`**

In `http_client.rs` `#[cfg(test)] mod tests`, add (mirrors existing `create_from_docx` wiremock tests — check the file for the `for_tests` constructor + `token_test_seed` helper names and reuse them exactly):

```rust
    #[tokio::test]
    async fn upload_image_to_drive_posts_multipart_and_returns_id() {
        let server = wiremock::MockServer::start().await;
        // base_docs, base_drive, base_drive_upld all point at the mock server.
        let client = GoogleDocsHttpClient::for_tests(
            &server.uri(), &server.uri(), &server.uri(), /* match the real arity */
        );
        client.token_test_seed("fake-token").await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path_regex(r"/files$"))
            .and(wiremock::matchers::query_param("uploadType", "multipart"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id": "drvFID123"})),
            )
            .mount(&server)
            .await;
        let id = client
            .upload_image_to_drive(vec![1, 2, 3], "image/png", "colmena-tmp-img-x.png")
            .await
            .unwrap();
        assert_eq!(id, "drvFID123");
    }
```

NOTE: before writing, open `http_client.rs` and copy the EXACT `for_tests(...)` signature and the token-seed helper name from a neighboring test (e.g. the `create_from_docx` or `share` test). Match arity/names exactly.

- [ ] **Step 3: Run test — verify it fails to compile (method missing)**

Run: `cargo test --lib gdocs::infrastructure::http_client::tests::upload_image_to_drive 2>&1 | tail -20`
Expected: compile error — `upload_image_to_drive` not implemented for `GoogleDocsHttpClient`.

- [ ] **Step 4: Implement the 3 methods in `http_client.rs`**

Inside `impl DocsClient for GoogleDocsHttpClient`, add (mirrors `create_from_docx` upload + `share` + `delete_permission`; uses the existing `send_with_retry` + `map_status` helpers):

```rust
    async fn upload_image_to_drive(
        &self,
        bytes: Vec<u8>,
        mime: &str,
        filename: &str,
    ) -> Result<String, DocsError> {
        let metadata = serde_json::json!({ "name": filename });
        let boundary = "colmena_gdocs_img_boundary";
        let metadata_part = format!(
            "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{}\r\n",
            serde_json::to_string(&metadata).expect("valid metadata json")
        );
        let media_part_header =
            format!("--{boundary}\r\nContent-Type: {mime}\r\n\r\n");
        let trailer = format!("\r\n--{boundary}--");
        let mut body = Vec::new();
        body.extend_from_slice(metadata_part.as_bytes());
        body.extend_from_slice(media_part_header.as_bytes());
        body.extend_from_slice(&bytes);
        body.extend_from_slice(trailer.as_bytes());

        let resp = self
            .send_with_retry(|c, t| {
                c.request(
                    Method::POST,
                    format!("{}/files?uploadType=multipart&fields=id", self.base_drive_upld),
                )
                .bearer_auth(t)
                .header("Content-Type", format!("multipart/related; boundary={boundary}"))
                .body(body.clone())
            })
            .await?;
        let resp = self.map_status(resp, "upload_image_to_drive").await?;
        let j: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DocsError::Http(format!("upload image json: {e}")))?;
        j.get("id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| DocsError::Http("upload image: missing id".into()))
    }

    async fn set_anyone_reader(&self, file_id: &str) -> Result<(), DocsError> {
        let url = format!("{}/files/{}/permissions", self.base_drive, file_id);
        let body = serde_json::json!({ "role": "reader", "type": "anyone" });
        let resp = self
            .send_with_retry(|c, t| c.request(Method::POST, &url).bearer_auth(t).json(&body))
            .await?;
        self.map_status(resp, "set_anyone_reader").await?;
        Ok(())
    }

    async fn delete_drive_file(&self, file_id: &str) -> Result<(), DocsError> {
        let url = format!("{}/files/{}", self.base_drive, file_id);
        let resp = self
            .send_with_retry(|c, t| {
                c.request(Method::DELETE, &url)
                    .bearer_auth(t)
                    .query(&[("supportsAllDrives", "true")])
            })
            .await?;
        self.map_status(resp, "delete_drive_file").await?;
        Ok(())
    }
```

- [ ] **Step 5: Add wiremock tests for `set_anyone_reader` + `delete_drive_file`**

```rust
    #[tokio::test]
    async fn set_anyone_reader_posts_permission() {
        let server = wiremock::MockServer::start().await;
        let client = GoogleDocsHttpClient::for_tests(/* same arity as Step 2 */);
        client.token_test_seed("fake-token").await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path_regex(r"/files/drvFID/permissions$"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({"id":"p1"})))
            .mount(&server)
            .await;
        client.set_anyone_reader("drvFID").await.unwrap();
    }

    #[tokio::test]
    async fn delete_drive_file_sends_delete() {
        let server = wiremock::MockServer::start().await;
        let client = GoogleDocsHttpClient::for_tests(/* same arity as Step 2 */);
        client.token_test_seed("fake-token").await;
        wiremock::Mock::given(wiremock::matchers::method("DELETE"))
            .and(wiremock::matchers::path_regex(r"/files/drvFID$"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&server)
            .await;
        client.delete_drive_file("drvFID").await.unwrap();
    }
```

- [ ] **Step 6: Run the 3 tests — verify pass**

Run: `cargo test --lib gdocs::infrastructure::http_client::tests::upload_image_to_drive gdocs::infrastructure::http_client::tests::set_anyone_reader gdocs::infrastructure::http_client::tests::delete_drive 2>&1 | tail -15`
Expected: 3 passed. (Run each filter separately if the multi-filter form doesn't match.)

- [ ] **Step 7: Confirm the whole crate still builds (mock auto-generated)**

Run: `cargo build --lib 2>&1 | tail -5`
Expected: Finished. (mockall auto-adds the 3 `expect_*` to `MockDocsClient`; if any OTHER hand-written `DocsClient` impl exists it will fail here — grep `impl DocsClient for` and add the 3 methods there too.)

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/gdocs/domain/traits.rs src/libs/colmena/src/gdocs/infrastructure/http_client.rs
git commit -m "feat(gdocs): DocsClient upload_image_to_drive + set_anyone_reader + delete_drive_file"
```

---

## Task 2: `run_insert_image_from_bytes` application helper

**Files:**
- Modify: `src/libs/colmena/src/gdocs/application/insert.rs`
- Test: same file `#[cfg(test)] mod app_tests`

- [ ] **Step 1: Write the failing happy-path test**

In `insert.rs` `app_tests` (reuse the existing `TestRig` / `snap` / `expect_get_sequence` / `make_batch_update_ok` helpers — copy their names exactly from the existing `insert_image_after_text_happy` test):

```rust
    #[tokio::test]
    async fn insert_image_from_bytes_uploads_makes_public_inserts_then_deletes() {
        let mut rig = TestRig::new();
        let s = snap("r1", vec![(1, ParagraphKind::Paragraph, "intro anchor texto", 1, 20)]);
        let s2 = snap("r2", vec![(1, ParagraphKind::Paragraph, "intro anchor texto", 1, 20)]);
        rig.client.expect_upload_image_to_drive()
            .returning(|_, _, _| Ok("FID9".to_string()));
        rig.client.expect_set_anyone_reader()
            .withf(|f| f == "FID9").returning(|_| Ok(()));
        expect_get_sequence(&mut rig.client, vec![s, s2]);
        make_batch_update_ok(&mut rig.client, "r2");
        rig.client.expect_delete_drive_file()
            .withf(|f| f == "FID9").returning(|_| Ok(()));
        let ctx = GuardContext {
            client: &rig.client, cache: &rig.cache, revisions: &rig.revisions,
            session_id: "s1", sa_email: None,
        };
        let (result, warnings) = super::run_insert_image_from_bytes(
            &ctx, &doc_id(),
            InsertImageAfterTextInput {
                anchor: "anchor".into(), image_url: String::new(),
                occurrence: None, width_pt: None, height_pt: None,
            },
            vec![1, 2, 3], "image/png",
        ).await.unwrap();
        assert_eq!(result.changes.len(), 1);
        assert!(warnings.is_empty());
    }
```

NOTE: `InsertImageAfterTextInput` already exists (shipped). The helper builds its own `uri` and overwrites `image_url`; passing `String::new()` is fine — the helper ignores the incoming `image_url`. (If cleaner, add a dedicated `InsertImageFromBytesInput { anchor, occurrence, width_pt, height_pt }` — your call; keep it consistent across this task.)

- [ ] **Step 2: Run — verify it fails (function missing)**

Run: `cargo test --lib gdocs::application::insert::app_tests::insert_image_from_bytes 2>&1 | tail -15`
Expected: compile error — `run_insert_image_from_bytes` not found.

- [ ] **Step 3: Implement the helper + mime→ext helper**

In `insert.rs` (near `run_insert_image_after_text`):

```rust
/// Map an image mime to a Drive filename extension. Returns `None` for
/// non-image / unsupported mimes (Docs only accepts PNG/JPEG/GIF).
pub(crate) fn image_ext_for_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        _ => None,
    }
}

/// Insert an image from raw bytes (Approach A): upload to Drive → make public
/// → insert via the lh3 content URL → delete the temp Drive file. Returns the
/// `EditResult` plus any soft warnings (e.g. temp-file cleanup failed — the
/// insert still succeeded). Transactional: if a step AFTER the upload fails,
/// the temp Drive file is deleted before returning the error.
pub async fn run_insert_image_from_bytes(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    input: InsertImageAfterTextInput,
    bytes: Vec<u8>,
    mime: &str,
) -> Result<(EditResult, Vec<String>), DocsError> {
    let ext = image_ext_for_mime(mime).ok_or_else(|| {
        DocsError::InvalidArgs(format!(
            "attachment mime '{mime}' is not a supported image (need image/png, image/jpeg, or image/gif)"
        ))
    })?;
    let filename = format!("colmena-tmp-img-{}.{ext}", short_token());
    let file_id = ctx.client.upload_image_to_drive(bytes, mime, &filename).await?;

    // From here on, clean up the uploaded file if anything fails.
    if let Err(e) = ctx.client.set_anyone_reader(&file_id).await {
        let _ = ctx.client.delete_drive_file(&file_id).await;
        return Err(e);
    }
    let uri = format!("https://lh3.googleusercontent.com/d/{file_id}");
    let image_input = InsertImageAfterTextInput {
        anchor: input.anchor,
        image_url: uri,
        occurrence: input.occurrence,
        width_pt: input.width_pt,
        height_pt: input.height_pt,
    };
    let result = match run_insert_image_after_text(ctx, doc_id, image_input).await {
        Ok(r) => r,
        Err(e) => {
            let _ = ctx.client.delete_drive_file(&file_id).await;
            return Err(e);
        }
    };
    // Insert succeeded — Google has copied the image. Best-effort cleanup.
    let mut warnings = Vec::new();
    if let Err(e) = ctx.client.delete_drive_file(&file_id).await {
        warnings.push(format!(
            "image inserted OK, but the temp Drive file '{file_id}' could not be deleted: {e}"
        ));
    }
    Ok((result, warnings))
}

/// Short non-secret token for temp filenames. Uses the process clock; collision
/// is harmless (filenames need not be unique). Avoids adding an RNG dep.
fn short_token() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:x}", n & 0xffff_ffff)
}
```

NOTE: `validate_image_url` (existing) runs inside `run_insert_image_after_text` and will accept the `lh3...` URL (it's https + < 2000 chars). No change needed there.

- [ ] **Step 4: Run the happy-path test — verify pass**

Run: `cargo test --lib gdocs::application::insert::app_tests::insert_image_from_bytes_uploads 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Add the transactional-cleanup + soft-warning + bad-mime tests**

```rust
    #[tokio::test]
    async fn insert_image_from_bytes_deletes_temp_file_when_insert_fails() {
        let mut rig = TestRig::new();
        let s = snap("r1", vec![(1, ParagraphKind::Paragraph, "no match here", 1, 14)]);
        rig.client.expect_upload_image_to_drive().returning(|_, _, _| Ok("FIDx".into()));
        rig.client.expect_set_anyone_reader().returning(|_| Ok(()));
        expect_get_sequence(&mut rig.client, vec![s]); // anchor not found -> insert errors
        // MUST clean up the uploaded file:
        rig.client.expect_delete_drive_file().withf(|f| f == "FIDx").times(1).returning(|_| Ok(()));
        let ctx = GuardContext { client: &rig.client, cache: &rig.cache, revisions: &rig.revisions, session_id: "s1", sa_email: None };
        let err = super::run_insert_image_from_bytes(
            &ctx, &doc_id(),
            InsertImageAfterTextInput { anchor: "anchor".into(), image_url: String::new(), occurrence: None, width_pt: None, height_pt: None },
            vec![1], "image/png",
        ).await.unwrap_err();
        assert!(matches!(err, DocsError::TextNotFound { .. }));
    }

    #[tokio::test]
    async fn insert_image_from_bytes_soft_warns_when_cleanup_fails() {
        let mut rig = TestRig::new();
        let s = snap("r1", vec![(1, ParagraphKind::Paragraph, "intro anchor texto", 1, 20)]);
        let s2 = snap("r2", vec![(1, ParagraphKind::Paragraph, "intro anchor texto", 1, 20)]);
        rig.client.expect_upload_image_to_drive().returning(|_, _, _| Ok("FIDz".into()));
        rig.client.expect_set_anyone_reader().returning(|_| Ok(()));
        expect_get_sequence(&mut rig.client, vec![s, s2]);
        make_batch_update_ok(&mut rig.client, "r2");
        rig.client.expect_delete_drive_file().returning(|_| Err(DocsError::Http("boom".into())));
        let ctx = GuardContext { client: &rig.client, cache: &rig.cache, revisions: &rig.revisions, session_id: "s1", sa_email: None };
        let (result, warnings) = super::run_insert_image_from_bytes(
            &ctx, &doc_id(),
            InsertImageAfterTextInput { anchor: "anchor".into(), image_url: String::new(), occurrence: None, width_pt: None, height_pt: None },
            vec![1], "image/png",
        ).await.unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("FIDz"));
    }

    #[test]
    fn image_ext_for_mime_rejects_non_image() {
        assert_eq!(super::image_ext_for_mime("image/png"), Some("png"));
        assert_eq!(super::image_ext_for_mime("image/jpeg"), Some("jpg"));
        assert_eq!(super::image_ext_for_mime("application/pdf"), None);
    }
```

- [ ] **Step 6: Run all insert tests — verify pass**

Run: `cargo test --lib gdocs::application::insert 2>&1 | tail -8`
Expected: all PASS (existing + 4 new).

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/gdocs/application/insert.rs
git commit -m "feat(gdocs): run_insert_image_from_bytes — upload/public/insert/cleanup helper"
```

---

## Task 3: Tool args + dispatcher + router wire

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`
- Test: `gdocs_tools.rs` inline tests

- [ ] **Step 1: Change `InsertImageAfterTextArgs` (image_url → Option, add attachment_id)**

Replace the struct:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct InsertImageAfterTextArgs {
    pub doc_id: String,
    /// Anchor text to locate; the image is inserted right after the match.
    pub anchor: String,
    /// Public http(s) image URL. Mutually exclusive with `attachment_id`.
    pub image_url: Option<String>,
    /// Attachment id (generated/edited/uploaded image). The bytes are hosted
    /// on Drive transiently and inserted. Mutually exclusive with `image_url`.
    pub attachment_id: Option<String>,
    pub occurrence: Option<u32>,
    pub width_pt: Option<f64>,
    pub height_pt: Option<f64>,
    pub mode: Option<String>,
}
```

- [ ] **Step 2: Write failing XOR-validation unit tests**

```rust
    #[test]
    fn insert_image_args_require_exactly_one_source() {
        // both present -> error
        let both: InsertImageAfterTextArgs = serde_json::from_value(serde_json::json!({
            "doc_id":"d","anchor":"a","image_url":"https://x/y.png","attachment_id":"att1"
        })).unwrap();
        assert!(super::insert_image_source(&both).is_err());
        // neither -> error
        let neither: InsertImageAfterTextArgs = serde_json::from_value(serde_json::json!({
            "doc_id":"d","anchor":"a"
        })).unwrap();
        assert!(super::insert_image_source(&neither).is_err());
        // url only -> Ok(Url)
        let url: InsertImageAfterTextArgs = serde_json::from_value(serde_json::json!({
            "doc_id":"d","anchor":"a","image_url":"https://x/y.png"
        })).unwrap();
        assert!(matches!(super::insert_image_source(&url), Ok(super::ImageSource::Url(_))));
        // attachment only -> Ok(Attachment)
        let att: InsertImageAfterTextArgs = serde_json::from_value(serde_json::json!({
            "doc_id":"d","anchor":"a","attachment_id":"att1"
        })).unwrap();
        assert!(matches!(super::insert_image_source(&att), Ok(super::ImageSource::Attachment(_))));
    }
```

- [ ] **Step 3: Run — verify fails (helper missing)**

Run: `cargo test --lib gdocs_tools::tests::insert_image_args_require_exactly_one_source 2>&1 | tail -10`
Expected: compile error — `insert_image_source` / `ImageSource` not found.

- [ ] **Step 4: Add the `ImageSource` enum + `insert_image_source` helper**

In `gdocs_tools.rs` (near the args struct):

```rust
pub(crate) enum ImageSource {
    Url(String),
    Attachment(String),
}

/// Resolve the XOR of `image_url` / `attachment_id`. Exactly one is required.
pub(crate) fn insert_image_source(args: &InsertImageAfterTextArgs) -> Result<ImageSource, String> {
    match (&args.image_url, &args.attachment_id) {
        (Some(u), None) => Ok(ImageSource::Url(u.clone())),
        (None, Some(a)) => Ok(ImageSource::Attachment(a.clone())),
        (Some(_), Some(_)) => Err("provide exactly one of image_url or attachment_id, not both".into()),
        (None, None) => Err("provide one of image_url or attachment_id".into()),
    }
}
```

- [ ] **Step 5: Run the XOR test — verify pass**

Run: `cargo test --lib gdocs_tools::tests::insert_image_args_require_exactly_one_source 2>&1 | tail -8`
Expected: PASS.

- [ ] **Step 6: Replace the dispatcher with the via_executor variant**

Replace `dispatch_insert_image_after_text` (the current URL-only dispatcher) with one that takes the executor and branches. Keep the function name expected by the router alias, OR add a new `dispatch_insert_image_after_text_via_executor` and update the router (Step 8). Implementation:

```rust
pub async fn dispatch_insert_image_after_text_via_executor(
    executor: &crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor,
    args: serde_json::Value,
    session_id: &str,
) -> serde_json::Value {
    let parsed: InsertImageAfterTextArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let source = match insert_image_source(&parsed) {
        Ok(s) => s,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e}),
    };
    let client = match shared_client().await { Ok(c) => c, Err(e) => return e };
    let cache = shared_cache().await;
    let revisions = match shared_revs().await { Ok(r) => r, Err(e) => return e };
    let sa = std::env::var("COLMENA_GDOCS_SA_EMAIL").ok();
    let ctx = co_edit_guard::GuardContext {
        client: client.as_ref(), cache: cache.as_ref(), revisions: revisions.as_ref(),
        session_id, sa_email: sa.as_deref(),
    };
    let doc_id = DocumentId(parsed.doc_id.clone());
    match source {
        ImageSource::Url(url) => {
            let input = insert::InsertImageAfterTextInput {
                anchor: parsed.anchor, image_url: url,
                occurrence: parsed.occurrence, width_pt: parsed.width_pt, height_pt: parsed.height_pt,
            };
            match insert::run_insert_image_after_text(&ctx, &doc_id, input).await {
                Ok(r) => edit_result_to_json(r),
                Err(e) => error_to_json(e),
            }
        }
        ImageSource::Attachment(att) => {
            let stored = match executor.fetch_attachment_bytes(&att).await {
                Ok(b) => b,
                Err(e) => return serde_json::json!({"error":"attachment_fetch_failed","message":e,"attachment_id":att}),
            };
            let input = insert::InsertImageAfterTextInput {
                anchor: parsed.anchor, image_url: String::new(),
                occurrence: parsed.occurrence, width_pt: parsed.width_pt, height_pt: parsed.height_pt,
            };
            match insert::run_insert_image_from_bytes(&ctx, &doc_id, input, stored.bytes, &stored.mime_type).await {
                Ok((r, warnings)) => {
                    let mut v = edit_result_to_json(r);
                    if !warnings.is_empty() {
                        v["soft_warnings"] = serde_json::json!(warnings);
                    }
                    v
                }
                Err(e) => error_to_json(e),
            }
        }
    }
}
```

NOTE: confirm `shared_client`/`shared_cache`/`shared_revs`/`edit_result_to_json`/`error_to_json`/`invalid_args` are the exact names used by the existing `dispatch_insert_after_text` in this file; copy them verbatim. Delete the old `dispatch_insert_image_after_text` (URL-only) if you renamed; otherwise have the router call the new name.

- [ ] **Step 7: Update the mod.rs re-export**

In `llm_synthetic_tools/mod.rs`, change the gdocs dispatch re-export for the image tool to the new name:

```rust
    dispatch_insert_image_after_text_via_executor as dispatch_gdocs_insert_image_after_text,
```

(Replace the existing `dispatch_insert_image_after_text as dispatch_gdocs_insert_image_after_text` line.)

- [ ] **Step 8: Wire the router to pass `self` (the executor)**

In `dag_tool_executor.rs`, change the match arm:

```rust
                    n if n == GDOCS_INSERT_IMAGE_AFTER_TEXT_TOOL => {
                        dispatch_gdocs_insert_image_after_text(self, args, session_id).await
                    }
```

(Was `dispatch_gdocs_insert_image_after_text(args, session_id)`.) `self` is in scope in this method (it's `DagToolExecutor::...`). Confirm by checking neighboring arms / the method signature.

- [ ] **Step 9: Build + run gdocs_tools tests**

Run: `cargo build --lib 2>&1 | tail -5 && cargo test --lib gdocs_tools 2>&1 | tail -8`
Expected: Finished + tests PASS.

- [ ] **Step 10: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs
git commit -m "feat(gdocs): gdocs_insert_image_after_text accepts attachment_id (Approach A)"
```

---

## Task 4: Docs (YAML, dev guide, CHANGELOG, BACKLOG)

**Files:**
- Modify: `src/libs/colmena/text/tools/gdocs.yaml`
- Modify: `docs/developer_guide/45_gdocs.md`, `docs/CHANGELOG_2026-06.md`, `docs/BACKLOG.md`

- [ ] **Step 1: Update `gdocs.yaml` `gdocs_insert_image_after_text` description**

Add to the description body: that `attachment_id` is now an alternative to `image_url` (exactly one), works for generated/edited/uploaded images, hosts the bytes on Drive transiently then deletes the temp file, and that for `attachment_id` only image/png|jpeg|gif are accepted. Update the summary to mention "URL or attachment".

- [ ] **Step 2: Update dev guide §45 row + the v1.1 note**

In `docs/developer_guide/45_gdocs.md`, update the `gdocs_insert_image_after_text` table row to note both modes (URL + attachment). Remove "Path (i) URL-only" wording; note attachment mode uploads to Drive (app-created → works under drive.file) then cleans up.

- [ ] **Step 3: Add CHANGELOG section**

Append a `## NN. gdocs_insert_image — attachment_id (Approach A)` section: what shipped, the upload→public→insert→cleanup flow, the empirical corroboration reference (spec §4), and the test/E2E summary.

- [ ] **Step 4: Update BACKLOG**

In the `Subsystem G v1.1` `gdocs_insert_image_after_text` item, mark paths ii/iii as SHIPPED via Approach A (Drive-upload covers ALL sources uniformly, superseding the per-source ii/iii split). Strike the prioritization-table row if present.

- [ ] **Step 5: Verify text registry still parses**

Run: `cargo test --lib text:: 2>&1 | tail -5`
Expected: PASS (no orphan yaml / parse errors).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/text/tools/gdocs.yaml docs/developer_guide/45_gdocs.md docs/CHANGELOG_2026-06.md docs/BACKLOG.md
git commit -m "docs(gdocs): document insert_image attachment_id mode"
```

---

## Task 5: Live E2E

**Files:**
- Create: `tests/graphs/agents/gdocs_insert_image_from_attachment_e2e.json`

- [ ] **Step 1: Write the E2E graph (placeholder doc id)**

Create the graph: an agent with `enabled_tools: ["gdocs", "image_generation"]` (or a graph that first produces an image attachment, e.g. via `image_generation`, OR accepts an inline image), reads the doc outline (Docs API — NOT read_as_markdown), then calls `gdocs_insert_image_after_text` with `attachment_id=<the generated image's document_id>` and an anchor. Use `<YOUR_DOC_ID>` placeholder. Prompt must instruct: use `gdocs_read_outline` for the anchor, then insert the attachment image.

- [ ] **Step 2: Build release binary**

Run: `cargo build --release --bin dag_engine 2>&1 | tail -2`
Expected: Finished.

- [ ] **Step 3: Run live (inject OAuth creds in-memory)**

Use the runbook in `docs/developer_guide/47_google_oauth.md` §"Runbook — E2E local". Inject `COLMENA_GOOGLE_OAUTH_*` from Secret Manager via command-substitution (no echo, no file), set `PYTHONPATH=$PWD/.venv/lib/python3.14/site-packages`, `unset ANTHROPIC_BASE_URL`. Substitute the real doc id into a `/tmp` copy of the graph (keep the committed graph with the placeholder). Save SSE to `/tmp/colmena_e2e/`.

- [ ] **Step 4: Verify (3 checks)**

Confirm in the SSE / via a `documents.get`:
1. The tool returned an `EditResult` (change `kind:insert`, `revision_id_after`) — no error.
2. The doc has the inserted image (inlineObject with `contentUri` host `googleusercontent`).
3. No leftover temp file: list Drive for `name contains 'colmena-tmp-img'` — should be empty (cleanup ran).

Present a friendly report (input, tool result, tokens, summary). If cleanup left a temp file (soft warning present), note it.

- [ ] **Step 5: Commit the graph**

```bash
git add tests/graphs/agents/gdocs_insert_image_from_attachment_e2e.json
git commit -m "test(gdocs): live E2E for insert_image from attachment"
```

---

## Task 6: Sweep + push + CI

- [ ] **Step 1: Full verbose test + fmt + clippy**

Run: `cargo fmt && cargo clippy --lib 2>&1 | tail -3 && cargo test --lib 2>&1 | tail -4`
Expected: fmt clean, clippy 0 warnings, all tests pass.

- [ ] **Step 2: ADP sweep (breaking-change discipline)**

The `DocsClient` trait is colmena-internal (no impls in ADP). Confirm: `grep -rn "impl DocsClient" /Users/danielgarcia/startti/adp/apps/service/ia/platform/` → empty. (If non-empty, the 3 new methods must be added there too before pushing.)

- [ ] **Step 3: Push + watch CI**

```bash
git push origin develop
```
Run: `gh run list --branch develop --limit 1` then `gh run watch <id> --exit-status`.
Expected: success.

---

## Self-Review (done by plan author)

- **Spec coverage:** §5.1 args→Task 3; §5.2 flow→Task 2; §5.3 routing→Task 3 Step 8; §6 methods→Task 1; §7 error handling→Task 2 Steps 3/5; §8 testing→Tasks 1/2/3/5; §9 files→all tasks; §10 out-of-scope (no task, correct). Covered.
- **Placeholders:** the `for_tests(...)` arity and `token_test_seed`/`TestRig`/`expect_get_sequence`/`make_batch_update_ok`/`shared_client`/`edit_result_to_json` names are flagged as "copy exact from neighboring code" — they exist in the codebase; the implementer must match them. This is the one unavoidable lookup (the plan can't invent private helper arities).
- **Type consistency:** `run_insert_image_from_bytes(ctx, doc_id, InsertImageAfterTextInput, bytes, mime) -> Result<(EditResult, Vec<String>)>` used identically in Task 2 and Task 3. `ImageSource`/`insert_image_source` used identically in Task 3. `image_ext_for_mime` defined Task 2, not reused elsewhere. Consistent.
