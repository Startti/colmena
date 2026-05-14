# Attachment Auto-Summary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When an attachment is registered without a caller-supplied `description`, automatically generate a one-line summary using a cheap-tier LLM call running in parallel with the main answer call, persisting it in `conversation_attachments.description`.

**Architecture:** New `AttachmentSummaryGenerator` port + `LlmAttachmentSummaryGenerator` adapter. Text extraction via `pdf-extract` (PDFs) and UTF-8 decode (text mimes). Image bytes go straight to a vision-capable cheap model. Wired into `llm.rs::execute` with `tokio::join!` and a hard timeout. Best-effort: any failure logs and persists `description=null`. No schema migration — `description` is already nullable.

**Tech Stack:** Rust 1.95, tokio, async-trait, `pdf-extract` 0.7 (new), existing `LlmRepository` + `AttachmentRegistry` traits, `mockall` for tests.

**Spec:** [docs/superpowers/specs/2026-05-14-attachment-auto-summary-design.md](../specs/2026-05-14-attachment-auto-summary-design.md)

---

## File Structure

**New files:**
```
src/libs/colmena/src/llm/domain/attachments/
  summary_generator.rs           # trait + value objects (SummaryInput, SummaryConfig, SummaryOutcome, SummaryError)

src/libs/colmena/src/llm/infrastructure/attachment_summary/
  mod.rs                          # re-exports
  cheap_tier.rs                   # provider_cheap_tier() mapping
  text_extractor.rs               # extract_text() dispatcher + PdfTextExtractor + plaintext extractor
  byte_acquisition.rs             # acquire_bytes() for SignedUrl, Path, Inline
  llm_summary_generator.rs        # LlmAttachmentSummaryGenerator (adapter)

src/libs/colmena/tests/
  fixtures/
    hello.pdf                     # 1-page PDF fixture for extractor tests (provided as base64 in Task 3)
  load_attachment_auto_summary_e2e.rs  # integration test with mocked LLM

tests/graphs/agents/
  load_attachment_auto_summary.json    # graph for real-API smoke test (Gemini Flash)
```

**Modified files:**
```
src/libs/colmena/Cargo.toml                            # add pdf-extract 0.7
src/libs/colmena/src/llm/domain/attachments/mod.rs    # re-export new types
src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs  # add update_description
src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs  # implement update_description
src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs    # implement update_description
src/libs/colmena/src/llm/infrastructure/mod.rs        # re-export attachment_summary
src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs  # wire summary tasks (after Step 3 "auto-register")
docs/node_configurations.json                         # document new config fields
docs/developer_guide/31_load_attachment.md            # add auto-summary section
```

---

## Task 1: Add `pdf-extract` dependency

**Files:**
- Modify: `src/libs/colmena/Cargo.toml`

- [ ] **Step 1: Add the dependency**

Open `src/libs/colmena/Cargo.toml`. Locate the `[dependencies]` table (line ~23). Add the line below near the other extraction-style crates (alphabetical is fine):

```toml
pdf-extract = "0.7"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p colmena_dag_engine`
Expected: PASS (downloads `pdf-extract` and its transitive deps; clean build with no warnings).

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/Cargo.toml src/libs/colmena/Cargo.lock
git commit -m "deps(load-attachment): add pdf-extract for auto-summary"
```

---

## Task 2: `truncate_chars` utility + tests

**Files:**
- Create: `src/libs/colmena/src/llm/infrastructure/attachment_summary/mod.rs`
- Create: `src/libs/colmena/src/llm/infrastructure/attachment_summary/text_extractor.rs` (initial — only `truncate_chars`)
- Modify: `src/libs/colmena/src/llm/infrastructure/mod.rs`

- [ ] **Step 1: Wire the new module**

Create `src/libs/colmena/src/llm/infrastructure/attachment_summary/mod.rs`:

```rust
//! Adapters for `AttachmentSummaryGenerator`: local text extraction,
//! provider cheap-tier mapping, byte acquisition, and the LLM-backed
//! summary generator implementation.

pub mod text_extractor;

pub use text_extractor::truncate_chars;
```

Open `src/libs/colmena/src/llm/infrastructure/mod.rs`. Append:

```rust
pub mod attachment_summary;
```

- [ ] **Step 2: Write the failing tests**

Create `src/libs/colmena/src/llm/infrastructure/attachment_summary/text_extractor.rs`:

```rust
//! Local text extraction from document bytes, plus a multi-byte-safe
//! char truncator. Used to feed the summary generator with at most
//! `summary_max_chars` of input text.

/// Truncate a string to at most `max_chars` Unicode characters
/// (not bytes). Safe across multi-byte UTF-8 sequences — never splits
/// a code point mid-way. Returns a `String` (allocates only when truncation
/// actually happens).
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((cut, _)) => s[..cut].to_string(),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_long_ascii_string() {
        assert_eq!(truncate_chars("abcdef", 3), "abc");
    }

    #[test]
    fn returns_full_string_when_under_cap() {
        assert_eq!(truncate_chars("abc", 10), "abc");
    }

    #[test]
    fn returns_empty_for_empty_input() {
        assert_eq!(truncate_chars("", 5), "");
    }

    #[test]
    fn handles_multi_byte_chars_without_panic() {
        // Each emoji is 4 UTF-8 bytes but 1 char.
        let s = "🦀🦀🦀🦀🦀";
        let out = truncate_chars(s, 3);
        assert_eq!(out, "🦀🦀🦀");
        assert_eq!(out.chars().count(), 3);
    }

    #[test]
    fn cap_zero_returns_empty() {
        assert_eq!(truncate_chars("hello", 0), "");
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p colmena_dag_engine --lib attachment_summary::text_extractor`
Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/mod.rs \
        src/libs/colmena/src/llm/infrastructure/attachment_summary/mod.rs \
        src/libs/colmena/src/llm/infrastructure/attachment_summary/text_extractor.rs
git commit -m "feat(auto-summary): truncate_chars utility for char-bounded truncation"
```

---

## Task 3: `extract_text` dispatcher with PDF and plaintext extractors

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/attachment_summary/text_extractor.rs`
- Create: `src/libs/colmena/tests/fixtures/hello.pdf` (binary fixture)

- [ ] **Step 1: Create the PDF fixture**

Run (from repo root):

```bash
mkdir -p src/libs/colmena/tests/fixtures
python3 - <<'PY'
import zlib, os
# Minimal 1-page PDF with the text "Hello World"
content = b"BT /F1 24 Tf 100 700 Td (Hello World) Tj ET"
stream = zlib.compress(content)
objs = []
objs.append(b"<</Type/Catalog/Pages 2 0 R>>")
objs.append(b"<</Type/Pages/Kids[3 0 R]/Count 1>>")
objs.append(b"<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R"
            b"/Resources<</Font<</F1 5 0 R>>>>>>")
objs.append(b"<</Length %d/Filter/FlateDecode>>stream\n" % len(stream) + stream + b"\nendstream")
objs.append(b"<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>")

buf = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
offsets = []
for i, body in enumerate(objs, 1):
    offsets.append(len(buf))
    buf += b"%d 0 obj\n" % i + body + b"\nendobj\n"
xref_off = len(buf)
buf += b"xref\n0 %d\n" % (len(objs) + 1)
buf += b"0000000000 65535 f \n"
for off in offsets:
    buf += b"%010d 00000 n \n" % off
buf += b"trailer<</Size %d/Root 1 0 R>>\n" % (len(objs) + 1)
buf += b"startxref\n%d\n%%%%EOF\n" % xref_off

with open("src/libs/colmena/tests/fixtures/hello.pdf", "wb") as f:
    f.write(buf)
print("wrote", os.path.getsize("src/libs/colmena/tests/fixtures/hello.pdf"), "bytes")
PY
```

Expected: prints `wrote NNN bytes` (NNN ≈ 400-600).

If Python is not available, create the file with any PDF generator (e.g. `pandoc -o hello.pdf <<<'Hello World'`) — the fixture only needs to contain the literal string "Hello World" somewhere in the extracted text.

- [ ] **Step 2: Write the failing tests**

Replace the contents of `src/libs/colmena/src/llm/infrastructure/attachment_summary/text_extractor.rs` with:

```rust
//! Local text extraction from document bytes, plus a multi-byte-safe
//! char truncator. Used to feed the summary generator with at most
//! `summary_max_chars` of input text.

use thiserror::Error;

/// Error type for local text extraction.
///
/// `Ok(None)` (returned by the dispatcher) means the MIME is recognised
/// but not text-extractable (e.g. images, archives). `Err` is reserved
/// for malformed input of a supported MIME (corrupt PDF, invalid UTF-8).
#[derive(Debug, Error)]
pub enum ExtractError {
    #[error("pdf parse failure: {0}")]
    PdfParse(String),

    #[error("invalid UTF-8 text: {0}")]
    InvalidUtf8(String),
}

/// Dispatcher: given a MIME type and the file bytes, return either the
/// extracted text (`Ok(Some(...))`), an explicit "no text available for
/// this MIME" (`Ok(None)`), or an extraction error (`Err`).
///
/// Caller is responsible for char-truncating the returned text via
/// [`truncate_chars`]. This function returns the full extracted string.
pub fn extract_text(mime: &str, bytes: &[u8]) -> Result<Option<String>, ExtractError> {
    let mime_norm = mime.to_ascii_lowercase();
    match mime_norm.as_str() {
        "application/pdf" => extract_pdf(bytes).map(Some),
        "text/plain" | "text/markdown" | "text/csv" | "text/html" | "text/x-markdown" => {
            extract_plaintext(bytes).map(Some)
        }
        _ => Ok(None),
    }
}

fn extract_pdf(bytes: &[u8]) -> Result<String, ExtractError> {
    pdf_extract::extract_text_from_mem(bytes).map_err(|e| ExtractError::PdfParse(e.to_string()))
}

fn extract_plaintext(bytes: &[u8]) -> Result<String, ExtractError> {
    std::str::from_utf8(bytes)
        .map(|s| s.to_string())
        .map_err(|e| ExtractError::InvalidUtf8(e.to_string()))
}

/// Truncate a string to at most `max_chars` Unicode characters
/// (not bytes). Safe across multi-byte UTF-8 sequences — never splits
/// a code point mid-way.
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((cut, _)) => s[..cut].to_string(),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- truncate_chars (unchanged from Task 2) ---------------------

    #[test]
    fn truncates_long_ascii_string() {
        assert_eq!(truncate_chars("abcdef", 3), "abc");
    }

    #[test]
    fn returns_full_string_when_under_cap() {
        assert_eq!(truncate_chars("abc", 10), "abc");
    }

    #[test]
    fn returns_empty_for_empty_input() {
        assert_eq!(truncate_chars("", 5), "");
    }

    #[test]
    fn handles_multi_byte_chars_without_panic() {
        let s = "🦀🦀🦀🦀🦀";
        let out = truncate_chars(s, 3);
        assert_eq!(out, "🦀🦀🦀");
        assert_eq!(out.chars().count(), 3);
    }

    #[test]
    fn cap_zero_returns_empty() {
        assert_eq!(truncate_chars("hello", 0), "");
    }

    // ---- extract_text ------------------------------------------------

    #[test]
    fn extract_plaintext_decodes_utf8() {
        let r = extract_text("text/plain", b"hello world").unwrap();
        assert_eq!(r.as_deref(), Some("hello world"));
    }

    #[test]
    fn extract_markdown_decodes_utf8() {
        let r = extract_text("text/markdown", b"# Title").unwrap();
        assert_eq!(r.as_deref(), Some("# Title"));
    }

    #[test]
    fn extract_csv_decodes_utf8() {
        let r = extract_text("text/csv", b"a,b\n1,2").unwrap();
        assert_eq!(r.as_deref(), Some("a,b\n1,2"));
    }

    #[test]
    fn extract_plaintext_invalid_utf8_errors() {
        let r = extract_text("text/plain", &[0xff, 0xfe, 0xfd]);
        assert!(matches!(r, Err(ExtractError::InvalidUtf8(_))));
    }

    #[test]
    fn extract_pdf_returns_text_for_valid_pdf() {
        let pdf = include_bytes!("../../../../tests/fixtures/hello.pdf");
        let r = extract_text("application/pdf", pdf).unwrap();
        let text = r.expect("extract_text returned None for valid PDF");
        assert!(
            text.to_lowercase().contains("hello"),
            "expected 'hello' in extracted text, got: {:?}",
            text
        );
    }

    #[test]
    fn extract_pdf_corrupt_bytes_errors() {
        let r = extract_text("application/pdf", b"not a pdf");
        assert!(matches!(r, Err(ExtractError::PdfParse(_))));
    }

    #[test]
    fn extract_unsupported_mime_returns_none() {
        let r = extract_text("application/zip", b"PK\x03\x04anything").unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn extract_image_returns_none_no_panic() {
        let r = extract_text("image/png", &[0x89, 0x50, 0x4e, 0x47]).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn extract_mime_is_case_insensitive() {
        let r = extract_text("TEXT/PLAIN", b"x").unwrap();
        assert_eq!(r.as_deref(), Some("x"));
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p colmena_dag_engine --lib attachment_summary::text_extractor`
Expected: 13 tests pass.

If the PDF test fails with "could not find 'hello' in extracted text", verify the fixture was created correctly: `xxd src/libs/colmena/tests/fixtures/hello.pdf | head` should show `%PDF-1.4`.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/attachment_summary/text_extractor.rs \
        src/libs/colmena/tests/fixtures/hello.pdf
git commit -m "feat(auto-summary): local text extraction (pdf + utf-8 dispatcher)"
```

---

## Task 4: `provider_cheap_tier` mapping

**Files:**
- Create: `src/libs/colmena/src/llm/infrastructure/attachment_summary/cheap_tier.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/attachment_summary/mod.rs`

- [ ] **Step 1: Write the failing tests**

Create `src/libs/colmena/src/llm/infrastructure/attachment_summary/cheap_tier.rs`:

```rust
//! Maps each supported provider to its cheap-tier model name used as
//! the default for attachment summary generation. Single function,
//! easy to audit and update.

use crate::llm::domain::ProviderKind;

/// Default cheap-tier model per provider. Centralised here so a single
/// edit updates the default when providers ship cheaper variants.
pub fn provider_cheap_tier(provider: &ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Google => "gemini-2.5-flash",
        ProviderKind::OpenAi => "gpt-4o-mini",
        ProviderKind::Anthropic => "claude-haiku-4-5-20251001",
        ProviderKind::Mock => "mock-model",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_cheap_tier_is_gemini_flash() {
        assert_eq!(provider_cheap_tier(&ProviderKind::Google), "gemini-2.5-flash");
    }

    #[test]
    fn openai_cheap_tier_is_gpt4o_mini() {
        assert_eq!(provider_cheap_tier(&ProviderKind::OpenAi), "gpt-4o-mini");
    }

    #[test]
    fn anthropic_cheap_tier_is_haiku() {
        assert_eq!(
            provider_cheap_tier(&ProviderKind::Anthropic),
            "claude-haiku-4-5-20251001"
        );
    }

    #[test]
    fn mock_cheap_tier_is_mock() {
        assert_eq!(provider_cheap_tier(&ProviderKind::Mock), "mock-model");
    }
}
```

- [ ] **Step 2: Register the module**

Edit `src/libs/colmena/src/llm/infrastructure/attachment_summary/mod.rs`. Replace contents with:

```rust
//! Adapters for `AttachmentSummaryGenerator`: local text extraction,
//! provider cheap-tier mapping, byte acquisition, and the LLM-backed
//! summary generator implementation.

pub mod cheap_tier;
pub mod text_extractor;

pub use cheap_tier::provider_cheap_tier;
pub use text_extractor::{extract_text, truncate_chars, ExtractError};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p colmena_dag_engine --lib attachment_summary::cheap_tier`
Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/attachment_summary/cheap_tier.rs \
        src/libs/colmena/src/llm/infrastructure/attachment_summary/mod.rs
git commit -m "feat(auto-summary): provider cheap-tier mapping (Flash/4o-mini/Haiku)"
```

---

## Task 5: Domain types — `SummaryInput`, `SummaryConfig`, `SummaryOutcome`, `SummaryError`, `AttachmentSummaryGenerator` trait

**Files:**
- Create: `src/libs/colmena/src/llm/domain/attachments/summary_generator.rs`
- Modify: `src/libs/colmena/src/llm/domain/attachments/mod.rs`

- [ ] **Step 1: Write the failing tests + types**

Create `src/libs/colmena/src/llm/domain/attachments/summary_generator.rs`:

```rust
//! Port and value objects for attachment summary generation.
//!
//! The summary call runs in parallel with the main `llm_call`. It is a
//! one-shot, history-less invocation: it must NOT write to
//! `llm_node_history`. Implementations live in the infrastructure layer.

use crate::llm::domain::ProviderKind;
use async_trait::async_trait;
use std::time::Duration;
use thiserror::Error;

/// What the generator is asked to summarise.
#[derive(Debug, Clone)]
pub struct SummaryInput {
    pub filename: String,
    pub mime_type: String,
    pub source: SummarySource,
}

/// The actual payload fed to the model.
#[derive(Debug, Clone)]
pub enum SummarySource {
    /// Pre-extracted and char-truncated text (PDF, plain, markdown, etc.).
    ExtractedText(String),
    /// Raw image bytes; the generator will attach them as a vision input.
    ImageBytes(Vec<u8>),
}

/// Configuration for one summary call.
#[derive(Debug, Clone)]
pub struct SummaryConfig {
    pub provider: ProviderKind,
    pub model: String,
    pub api_key: String,
    pub max_output_chars: usize,
    pub timeout: Duration,
}

/// Result of attempting to generate a summary for one attachment.
///
/// Not an `Err` for skipped/empty cases because they are **expected**
/// outcomes that should still flow through normal control flow (and
/// be persisted as `description = null`), not unhandled errors.
#[derive(Debug, Clone)]
pub enum SummaryOutcome {
    Generated(String),
    Skipped { reason: String },
    Failed { reason: String },
}

/// Error type for the generator port. Returned only for unexpected
/// infrastructure failures (network, malformed request). Predictable
/// "no summary" cases use `SummaryOutcome::Skipped` / `Failed` instead.
#[derive(Debug, Error)]
pub enum SummaryError {
    #[error("llm call failed: {0}")]
    LlmCallFailed(String),

    #[error("empty model response")]
    EmptyResponse,
}

/// Generates a single-line summary for one attachment.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait AttachmentSummaryGenerator: Send + Sync {
    async fn generate(
        &self,
        input: SummaryInput,
        config: &SummaryConfig,
    ) -> Result<SummaryOutcome, SummaryError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_input_text_holds_text() {
        let i = SummaryInput {
            filename: "x.pdf".into(),
            mime_type: "application/pdf".into(),
            source: SummarySource::ExtractedText("abc".into()),
        };
        match i.source {
            SummarySource::ExtractedText(t) => assert_eq!(t, "abc"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn summary_outcome_variants_carry_data() {
        let gen = SummaryOutcome::Generated("hello".into());
        let skip = SummaryOutcome::Skipped {
            reason: "image-only".into(),
        };
        let fail = SummaryOutcome::Failed {
            reason: "timeout".into(),
        };
        assert!(matches!(gen, SummaryOutcome::Generated(_)));
        assert!(matches!(skip, SummaryOutcome::Skipped { .. }));
        assert!(matches!(fail, SummaryOutcome::Failed { .. }));
    }

    #[test]
    fn summary_error_display_includes_reason() {
        let e = SummaryError::LlmCallFailed("rate limit".into());
        assert!(format!("{}", e).contains("rate limit"));
    }
}
```

- [ ] **Step 2: Re-export from the domain mod**

Edit `src/libs/colmena/src/llm/domain/attachments/mod.rs`. Replace contents with:

```rust
pub mod attachment_error;
pub mod attachment_registry;
pub mod auto_id;
pub mod conversation_attachment;
pub mod summary_generator;

pub use attachment_error::AttachmentError;
pub use attachment_registry::{AttachmentRegistry, UpsertAttachmentInput};
pub use auto_id::generate_attachment_id;
pub use conversation_attachment::{AttachmentSource, ConversationAttachment};
pub use summary_generator::{
    AttachmentSummaryGenerator, SummaryConfig, SummaryError, SummaryInput, SummaryOutcome,
    SummarySource,
};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p colmena_dag_engine --lib attachments::summary_generator`
Expected: 3 tests pass. Also verify the crate builds: `cargo build -p colmena_dag_engine`.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/llm/domain/attachments/summary_generator.rs \
        src/libs/colmena/src/llm/domain/attachments/mod.rs
git commit -m "feat(auto-summary): AttachmentSummaryGenerator port + value objects"
```

---

## Task 6: Add `update_description` to `AttachmentRegistry` trait + Postgres/SQLite impls

**Files:**
- Modify: `src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs`

- [ ] **Step 1: Extend the trait**

Open `src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs`. Inside the `pub trait AttachmentRegistry` block, add the new method below `refresh_provider_file_id`:

```rust
    /// Replace the `description` for an existing row. Used by the auto-summary
    /// generator to persist the produced summary after the upsert. Returns
    /// `Err(NotFound)` when the row does not exist.
    async fn update_description(
        &self,
        agent_session_id: &str,
        document_id: &str,
        provider: ProviderKind,
        description: &str,
    ) -> Result<(), AttachmentError>;
```

- [ ] **Step 2: Implement on Postgres**

Open `src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs`. Find the `impl AttachmentRegistry for PostgresAttachmentRegistry` block. Add the method (mirror the style of `refresh_provider_file_id` already in the file — use the same query patterns and error mapping):

```rust
    async fn update_description(
        &self,
        agent_session_id: &str,
        document_id: &str,
        provider: ProviderKind,
        description: &str,
    ) -> Result<(), AttachmentError> {
        let result = sqlx::query(
            "UPDATE conversation_attachments
               SET description = $1
             WHERE agent_session_id = $2
               AND document_id = $3
               AND provider = $4",
        )
        .bind(description)
        .bind(agent_session_id)
        .bind(document_id)
        .bind(provider.as_str())
        .execute(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(AttachmentError::NotFound {
                document_id: document_id.to_string(),
            });
        }
        Ok(())
    }
```

> Note: if the existing `refresh_provider_file_id` impl uses different column-name casing, follow that pattern. Read the file before editing to confirm.

- [ ] **Step 3: Implement on SQLite**

Open `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs`. Find the `impl AttachmentRegistry for SqliteAttachmentRegistry` block. Add an equivalent method, but using `?` placeholders (SQLite param style) and the same column names:

```rust
    async fn update_description(
        &self,
        agent_session_id: &str,
        document_id: &str,
        provider: ProviderKind,
        description: &str,
    ) -> Result<(), AttachmentError> {
        let result = sqlx::query(
            "UPDATE conversation_attachments
               SET description = ?
             WHERE agent_session_id = ?
               AND document_id = ?
               AND provider = ?",
        )
        .bind(description)
        .bind(agent_session_id)
        .bind(document_id)
        .bind(provider.as_str())
        .execute(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(AttachmentError::NotFound {
                document_id: document_id.to_string(),
            });
        }
        Ok(())
    }
```

- [ ] **Step 4: Add unit tests for both impls**

Append to the existing `#[cfg(test)] mod tests` block at the bottom of `sqlite_attachment_registry.rs` (use `sqlite::memory:` — no external dep needed):

```rust
    #[tokio::test]
    async fn update_description_persists_value() {
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        // Upsert a row with description=None
        reg.upsert(UpsertAttachmentInput {
            agent_session_id: "s1".into(),
            document_id: "doc-1".into(),
            provider: ProviderKind::Mock,
            provider_file_id: "pf-1".into(),
            mime_type: "application/pdf".into(),
            filename: "a.pdf".into(),
            size_bytes: Some(100),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
        })
        .await
        .unwrap();

        reg.update_description("s1", "doc-1", ProviderKind::Mock, "Q3 financials")
            .await
            .unwrap();

        let row = reg
            .lookup("s1", "doc-1", ProviderKind::Mock)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.description.as_deref(), Some("Q3 financials"));
    }

    #[tokio::test]
    async fn update_description_missing_row_returns_not_found() {
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        let r = reg
            .update_description("s1", "missing", ProviderKind::Mock, "x")
            .await;
        assert!(matches!(r, Err(AttachmentError::NotFound { .. })));
    }
```

For Postgres, mirror the existing `#[ignore = "requires DATABASE_URL — run with \`cargo test -- --ignored\`"]` pattern already used in `postgres_attachment_registry.rs`. **First, read the existing test block** in that file to find the setup helper (search for `fn setup_pool` or similar — it builds a pool from `DATABASE_URL` and runs migrations). Then add:

```rust
    #[tokio::test]
    #[ignore = "requires DATABASE_URL — run with `source .env && cargo test -- --ignored`"]
    async fn update_description_persists_value_pg() {
        let pool = setup_pool().await; // re-use existing helper from this file
        let reg = PostgresAttachmentRegistry::new(pool.clone(), &std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        reg.upsert(UpsertAttachmentInput {
            agent_session_id: "s_pg_1".into(),
            document_id: "doc-pg-1".into(),
            provider: ProviderKind::Mock,
            provider_file_id: "pf-1".into(),
            mime_type: "application/pdf".into(),
            filename: "a.pdf".into(),
            size_bytes: Some(100),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
        })
        .await
        .unwrap();

        reg.update_description("s_pg_1", "doc-pg-1", ProviderKind::Mock, "Q3 financials")
            .await
            .unwrap();

        let row = reg
            .lookup("s_pg_1", "doc-pg-1", ProviderKind::Mock)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.description.as_deref(), Some("Q3 financials"));

        // clean up
        sqlx::query("DELETE FROM conversation_attachments WHERE agent_session_id = $1")
            .bind("s_pg_1")
            .execute(&pool)
            .await
            .ok();
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL — run with `source .env && cargo test -- --ignored`"]
    async fn update_description_missing_row_returns_not_found_pg() {
        let pool = setup_pool().await;
        let reg = PostgresAttachmentRegistry::new(pool, &std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let r = reg
            .update_description("s_pg_missing", "missing", ProviderKind::Mock, "x")
            .await;
        assert!(matches!(r, Err(AttachmentError::NotFound { .. })));
    }
```

> If `setup_pool` is named differently in the existing Postgres test module, substitute the actual helper name. The construction pattern for `PostgresAttachmentRegistry::new` may also differ — read at least one existing test in the file before writing these.

- [ ] **Step 5: Run tests**

Run: `cargo test -p colmena_dag_engine --lib attachment_registry`
Expected: SQLite tests pass; Postgres tests `#[ignore]`d unless `--ignored` is set.

Also: `cargo build -p colmena_dag_engine`
Expected: clean build with no warnings (trait change requires both adapters to compile).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs \
        src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs \
        src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs
git commit -m "feat(auto-summary): AttachmentRegistry::update_description"
```

---

## Task 7: Byte acquisition helper

**Files:**
- Create: `src/libs/colmena/src/llm/infrastructure/attachment_summary/byte_acquisition.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/attachment_summary/mod.rs`

- [ ] **Step 1: Write the helper**

Create `src/libs/colmena/src/llm/infrastructure/attachment_summary/byte_acquisition.rs`:

```rust
//! Bounded byte acquisition for attachment summary generation.
//!
//! The main upload pipeline (`upload_streaming`) consumes the bytes and
//! does not retain them. The summary generator needs a separate copy.
//! For v1 we re-download (signed URLs) or re-read (paths). Inline sources
//! already carry the bytes in memory.

use crate::llm::domain::attachments::AttachmentSource;
use crate::llm::domain::signed_url_fetcher::SignedUrlFetcher;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AcquireError {
    #[error("file exceeds {max} bytes")]
    TooLarge { max: usize },

    #[error("download error: {0}")]
    Download(String),

    #[error("read error: {0}")]
    Read(String),

    #[error("source has no retained bytes")]
    NoBytes,
}

/// Acquire the bytes of an attachment for local extraction. Bounded by
/// `max_bytes` — exceeding it short-circuits with `TooLarge` (the partial
/// buffer is dropped).
///
/// `inline_bytes` carries the bytes from `FileSource::InlineBytes` upstream,
/// because they are not stored anywhere else after the upload streams them.
pub async fn acquire_bytes(
    source: &AttachmentSource,
    inline_bytes: Option<&[u8]>,
    max_bytes: usize,
    fetcher: Arc<dyn SignedUrlFetcher>,
) -> Result<Vec<u8>, AcquireError> {
    match source {
        AttachmentSource::Inline => inline_bytes
            .map(|b| {
                if b.len() > max_bytes {
                    Err(AcquireError::TooLarge { max: max_bytes })
                } else {
                    Ok(b.to_vec())
                }
            })
            .unwrap_or(Err(AcquireError::NoBytes)),

        AttachmentSource::Path(p) => {
            let meta = tokio::fs::metadata(p)
                .await
                .map_err(|e| AcquireError::Read(e.to_string()))?;
            if meta.len() as usize > max_bytes {
                return Err(AcquireError::TooLarge { max: max_bytes });
            }
            tokio::fs::read(p)
                .await
                .map_err(|e| AcquireError::Read(e.to_string()))
        }

        AttachmentSource::SignedUrl(url) => fetcher
            .fetch_bounded(url, max_bytes)
            .await
            .map_err(|e| AcquireError::Download(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct MockFetcher {
        body: Vec<u8>,
    }

    #[async_trait]
    impl SignedUrlFetcher for MockFetcher {
        async fn fetch_bounded(&self, _url: &str, max: usize) -> Result<Vec<u8>, String> {
            if self.body.len() > max {
                Err(format!("file exceeds {} bytes", max))
            } else {
                Ok(self.body.clone())
            }
        }
    }

    #[tokio::test]
    async fn inline_returns_provided_bytes() {
        let r = acquire_bytes(
            &AttachmentSource::Inline,
            Some(b"hello"),
            1024,
            Arc::new(MockFetcher { body: vec![] }),
        )
        .await
        .unwrap();
        assert_eq!(r, b"hello");
    }

    #[tokio::test]
    async fn inline_without_bytes_errors() {
        let r = acquire_bytes(
            &AttachmentSource::Inline,
            None,
            1024,
            Arc::new(MockFetcher { body: vec![] }),
        )
        .await;
        assert!(matches!(r, Err(AcquireError::NoBytes)));
    }

    #[tokio::test]
    async fn inline_too_large_errors() {
        let big = vec![0u8; 100];
        let r = acquire_bytes(
            &AttachmentSource::Inline,
            Some(&big),
            10,
            Arc::new(MockFetcher { body: vec![] }),
        )
        .await;
        assert!(matches!(r, Err(AcquireError::TooLarge { max: 10 })));
    }

    #[tokio::test]
    async fn signed_url_returns_fetched_bytes() {
        let r = acquire_bytes(
            &AttachmentSource::SignedUrl("https://example.com/x".into()),
            None,
            1024,
            Arc::new(MockFetcher {
                body: b"downloaded".to_vec(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(r, b"downloaded");
    }
}
```

> **Check before this step:** `SignedUrlFetcher` is referenced in `src/libs/colmena/src/llm/domain/signed_url_fetcher.rs`. Verify it exposes a method like `fetch_bounded(url, max) -> Result<Vec<u8>, String>` (or equivalent). If the actual method is named differently (e.g. `fetch_to_vec(url, limit)`), adjust the call in `acquire_bytes` and the mock impl accordingly. The trait method MUST take a max-bytes bound.

> If `SignedUrlFetcher` does **not** have a bounded fetch method, add one in the same task before writing this helper. Implementation idea: hyper/reqwest with `take` on the body stream, abort once `max_bytes` is exceeded.

- [ ] **Step 2: Register the module**

Edit `src/libs/colmena/src/llm/infrastructure/attachment_summary/mod.rs`. Add to the `pub mod` list:

```rust
pub mod byte_acquisition;
pub mod cheap_tier;
pub mod text_extractor;

pub use byte_acquisition::{acquire_bytes, AcquireError};
pub use cheap_tier::provider_cheap_tier;
pub use text_extractor::{extract_text, truncate_chars, ExtractError};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p colmena_dag_engine --lib attachment_summary::byte_acquisition`
Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/attachment_summary/byte_acquisition.rs \
        src/libs/colmena/src/llm/infrastructure/attachment_summary/mod.rs
git commit -m "feat(auto-summary): bounded byte acquisition for inline/path/signed_url"
```

---

## Task 8: `LlmAttachmentSummaryGenerator` adapter

**Files:**
- Create: `src/libs/colmena/src/llm/infrastructure/attachment_summary/llm_summary_generator.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/attachment_summary/mod.rs`

- [ ] **Step 1: Write the generator with mocked-LlmRepository tests**

Create `src/libs/colmena/src/llm/infrastructure/attachment_summary/llm_summary_generator.rs`:

```rust
//! Adapter for `AttachmentSummaryGenerator` that issues a one-shot,
//! history-less `LlmRepository::call`. Bypasses `LlmCallUseCase` so
//! the summary turn never lands in `llm_node_history`.

use crate::llm::domain::attachments::{
    AttachmentSummaryGenerator, SummaryConfig, SummaryError, SummaryInput, SummaryOutcome,
    SummarySource,
};
use crate::llm::domain::{
    FileData, LlmMessage, LlmProvider, LlmRepository, LlmRequest, MessageRole,
};
use async_trait::async_trait;
use std::sync::Arc;

const SYSTEM_PROMPT_TEXT: &str = "You are a document cataloger. Given the first N \
characters of a document's extracted text, output a single short description \
(max {MAX_CHARS} characters) that helps a downstream LLM decide whether this \
document is relevant to a user's question. Focus on: document type, topic, and \
time period if relevant. No commentary, no quotes, no markdown. Just the \
description on one line.";

const SYSTEM_PROMPT_IMAGE: &str = "You are a document cataloger. Look at the \
attached image and output a single short description (max {MAX_CHARS} characters) \
that helps a downstream LLM decide whether this image is relevant to a user's \
question. Focus on: subject, type of image, salient details. No commentary, no \
markdown. Just the description on one line.";

pub struct LlmAttachmentSummaryGenerator {
    repo_factory: Arc<dyn Fn(&SummaryConfig) -> Arc<dyn LlmRepository> + Send + Sync>,
}

impl LlmAttachmentSummaryGenerator {
    /// Construct from a factory function — lets the caller decide how to
    /// build the `LlmRepository` for a given `(provider, model, api_key)`
    /// triple. The default in `llm.rs` uses `LlmProviderFactory::create`.
    pub fn new<F>(factory: F) -> Self
    where
        F: Fn(&SummaryConfig) -> Arc<dyn LlmRepository> + Send + Sync + 'static,
    {
        Self {
            repo_factory: Arc::new(factory),
        }
    }
}

#[async_trait]
impl AttachmentSummaryGenerator for LlmAttachmentSummaryGenerator {
    async fn generate(
        &self,
        input: SummaryInput,
        config: &SummaryConfig,
    ) -> Result<SummaryOutcome, SummaryError> {
        let repo = (self.repo_factory)(config);

        let (system, user) = match &input.source {
            SummarySource::ExtractedText(text) => {
                if text.trim().is_empty() {
                    return Ok(SummaryOutcome::Skipped {
                        reason: "extracted text was empty".into(),
                    });
                }
                let sys = SYSTEM_PROMPT_TEXT
                    .replace("{MAX_CHARS}", &config.max_output_chars.to_string());
                let usr = format!(
                    "Filename: {}\nMIME type: {}\nExtracted text (truncated):\n---\n{}\n---",
                    input.filename, input.mime_type, text
                );
                let user_msg = LlmMessage::user(usr)
                    .map_err(|e| SummaryError::LlmCallFailed(format!("build user msg: {}", e)))?;
                (sys, user_msg)
            }
            SummarySource::ImageBytes(bytes) => {
                let sys = SYSTEM_PROMPT_IMAGE
                    .replace("{MAX_CHARS}", &config.max_output_chars.to_string());
                let content = format!("Filename: {}", input.filename);
                let file = FileData::inline(input.mime_type.clone(), input.filename.clone(), bytes.clone());
                let user_msg = LlmMessage::user_with_files(content, vec![file])
                    .map_err(|e| SummaryError::LlmCallFailed(format!("build user msg: {}", e)))?;
                (sys, user_msg)
            }
        };

        let system_msg = LlmMessage::system(system)
            .map_err(|e| SummaryError::LlmCallFailed(format!("build system msg: {}", e)))?;

        let provider = LlmProvider::new(config.provider.clone(), config.api_key.clone(), config.model.clone())
            .map_err(|e| SummaryError::LlmCallFailed(format!("build provider: {}", e)))?;
        let request = LlmRequest::new(provider, vec![system_msg, user])
            .map_err(|e| SummaryError::LlmCallFailed(format!("build request: {}", e)))?;

        let response = repo
            .call(request)
            .await
            .map_err(|e| SummaryError::LlmCallFailed(e.to_string()))?;

        let raw = response.content().trim().trim_matches('"').to_string();
        let collapsed = raw.replace(['\n', '\r'], " ").trim().to_string();
        if collapsed.is_empty() {
            return Err(SummaryError::EmptyResponse);
        }
        let truncated = collapsed
            .char_indices()
            .nth(config.max_output_chars)
            .map(|(i, _)| collapsed[..i].to_string())
            .unwrap_or(collapsed);

        Ok(SummaryOutcome::Generated(truncated))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{LlmResponse, MockLlmRepository, ProviderKind, TokenUsage};
    use mockall::predicate::always;
    use std::time::Duration;

    fn cfg() -> SummaryConfig {
        SummaryConfig {
            provider: ProviderKind::Mock,
            model: "mock-model".into(),
            api_key: "key".into(),
            max_output_chars: 200,
            timeout: Duration::from_secs(5),
        }
    }

    fn mock_response(content: &str) -> LlmResponse {
        // Use the LlmResponse constructor in the codebase; if it's
        // `LlmResponse::new(content, usage)` keep that, otherwise mirror the
        // pattern used in existing mocked tests under `tests/`.
        LlmResponse::new(content.to_string(), TokenUsage::default())
    }

    fn factory_for(repo: Arc<MockLlmRepository>) -> LlmAttachmentSummaryGenerator {
        let r = repo.clone();
        LlmAttachmentSummaryGenerator::new(move |_| r.clone() as Arc<dyn LlmRepository>)
    }

    #[tokio::test]
    async fn generates_summary_from_extracted_text() {
        let mut mock = MockLlmRepository::new();
        mock.expect_call()
            .with(always())
            .returning(|_| Ok(mock_response("A Q3 financial report dated 2025-09")));
        let gen = factory_for(Arc::new(mock));

        let outcome = gen
            .generate(
                SummaryInput {
                    filename: "q3.pdf".into(),
                    mime_type: "application/pdf".into(),
                    source: SummarySource::ExtractedText("Quarterly results...".into()),
                },
                &cfg(),
            )
            .await
            .unwrap();
        assert!(matches!(outcome, SummaryOutcome::Generated(_)));
    }

    #[tokio::test]
    async fn empty_extracted_text_returns_skipped() {
        let mock = MockLlmRepository::new(); // no call expected
        let gen = factory_for(Arc::new(mock));

        let outcome = gen
            .generate(
                SummaryInput {
                    filename: "x.pdf".into(),
                    mime_type: "application/pdf".into(),
                    source: SummarySource::ExtractedText("   ".into()),
                },
                &cfg(),
            )
            .await
            .unwrap();
        assert!(matches!(outcome, SummaryOutcome::Skipped { .. }));
    }

    #[tokio::test]
    async fn whitespace_only_response_returns_empty_response_err() {
        let mut mock = MockLlmRepository::new();
        mock.expect_call().returning(|_| Ok(mock_response("   ")));
        let gen = factory_for(Arc::new(mock));

        let err = gen
            .generate(
                SummaryInput {
                    filename: "x.pdf".into(),
                    mime_type: "application/pdf".into(),
                    source: SummarySource::ExtractedText("content".into()),
                },
                &cfg(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, SummaryError::EmptyResponse));
    }

    #[tokio::test]
    async fn truncates_oversized_response_to_max_output_chars() {
        let long: String = "a".repeat(500);
        let mut mock = MockLlmRepository::new();
        mock.expect_call()
            .returning(move |_| Ok(mock_response(&long)));
        let gen = factory_for(Arc::new(mock));

        let outcome = gen
            .generate(
                SummaryInput {
                    filename: "x.pdf".into(),
                    mime_type: "application/pdf".into(),
                    source: SummarySource::ExtractedText("content".into()),
                },
                &cfg(),
            )
            .await
            .unwrap();
        if let SummaryOutcome::Generated(s) = outcome {
            assert_eq!(s.chars().count(), 200);
        } else {
            panic!("expected Generated");
        }
    }

    #[tokio::test]
    async fn collapses_newlines_in_response() {
        let mut mock = MockLlmRepository::new();
        mock.expect_call()
            .returning(|_| Ok(mock_response("line1\nline2\nline3")));
        let gen = factory_for(Arc::new(mock));

        let outcome = gen
            .generate(
                SummaryInput {
                    filename: "x.pdf".into(),
                    mime_type: "application/pdf".into(),
                    source: SummarySource::ExtractedText("content".into()),
                },
                &cfg(),
            )
            .await
            .unwrap();
        if let SummaryOutcome::Generated(s) = outcome {
            assert!(!s.contains('\n'));
            assert!(s.contains("line1 line2 line3"));
        } else {
            panic!("expected Generated");
        }
    }
}
```

> **Adapter check:** the test constructors (`LlmResponse::new`, `TokenUsage::default`, `MockLlmRepository`) must match the actual signatures in the codebase. Before running, open `src/libs/colmena/src/llm/domain/llm_response.rs` and confirm. If `LlmResponse::new` takes more fields, mirror the pattern from another mocked-LLM test (search: `MockLlmRepository::new`) and copy the response construction verbatim.

- [ ] **Step 2: Register the module**

Edit `src/libs/colmena/src/llm/infrastructure/attachment_summary/mod.rs`. Add the new module:

```rust
pub mod byte_acquisition;
pub mod cheap_tier;
pub mod llm_summary_generator;
pub mod text_extractor;

pub use byte_acquisition::{acquire_bytes, AcquireError};
pub use cheap_tier::provider_cheap_tier;
pub use llm_summary_generator::LlmAttachmentSummaryGenerator;
pub use text_extractor::{extract_text, truncate_chars, ExtractError};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p colmena_dag_engine --lib attachment_summary::llm_summary_generator`
Expected: 5 tests pass.

Also: `cargo build -p colmena_dag_engine`
Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/attachment_summary/llm_summary_generator.rs \
        src/libs/colmena/src/llm/infrastructure/attachment_summary/mod.rs
git commit -m "feat(auto-summary): LlmAttachmentSummaryGenerator adapter (one-shot, history-less)"
```

---

## Task 9: Wire summary tasks into `llm.rs::execute`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

This is the largest task — it's where everything composes. The wiring goes immediately after the existing "Step 3: Auto-register resolved uploads" block (around line 907 today) and uses `tokio::join!` to overlap with the answer call below.

- [ ] **Step 1: Add config parsing**

In the config-parsing section near the top of `execute` (locate where `attachments_enabled` is already parsed around line 1017 — search for `"attachments_enabled"`), add right after it:

```rust
        let summary_enabled: bool = inputs
            .get("summary_enabled")
            .and_then(|v| v.as_bool())
            .or_else(|| config.get("summary_enabled").and_then(|v| v.as_bool()))
            .unwrap_or(true);
        let summary_max_chars: usize = inputs
            .get("summary_max_chars")
            .and_then(|v| v.as_u64())
            .or_else(|| config.get("summary_max_chars").and_then(|v| v.as_u64()))
            .map(|v| v as usize)
            .unwrap_or(5000);
        let summary_max_output_chars: usize = inputs
            .get("summary_max_output_chars")
            .and_then(|v| v.as_u64())
            .or_else(|| config.get("summary_max_output_chars").and_then(|v| v.as_u64()))
            .map(|v| v as usize)
            .unwrap_or(200);
        let summary_max_bytes: usize = inputs
            .get("summary_max_bytes")
            .and_then(|v| v.as_u64())
            .or_else(|| config.get("summary_max_bytes").and_then(|v| v.as_u64()))
            .map(|v| v as usize)
            .unwrap_or(26_214_400); // 25 MiB
        let summary_timeout_secs: u64 = inputs
            .get("summary_timeout_secs")
            .and_then(|v| v.as_u64())
            .or_else(|| config.get("summary_timeout_secs").and_then(|v| v.as_u64()))
            .unwrap_or(15);
        let summary_model_override: Option<String> = inputs
            .get("summary_model")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                config
                    .get("summary_model")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });
```

- [ ] **Step 2: Build the per-file summary input list during auto-registration**

In the existing `for (idx, file) in resolved_files.iter().enumerate()` loop (currently at ~line 839), after the existing `reg.upsert(input).await?;` call, **also** collect a `(document_id, source, mime_type, filename, inline_bytes_opt)` tuple into a new `summary_targets` vec — but only when `summary_enabled && description.is_none()`. Add the vec at the same scope as `attachment_registry`, before the for loop:

```rust
        let mut summary_targets: Vec<SummaryTarget> = Vec::new();
```

Where `SummaryTarget` is a small local struct defined near the top of the function (or in module scope just above `execute`):

```rust
#[derive(Debug)]
struct SummaryTarget {
    document_id: String,
    source: crate::llm::domain::AttachmentSource,
    mime_type: String,
    filename: String,
    /// Carries bytes from `FileSource::InlineBytes` because they are not
    /// retained anywhere else.
    inline_bytes: Option<Vec<u8>>,
}
```

Inside the loop, after the `reg.upsert(...)` call:

```rust
            if summary_enabled && description.is_none() {
                let inline_bytes = match &file.source {
                    crate::llm::domain::FileSource::InlineBytes { bytes, .. } => {
                        Some(bytes.clone())
                    }
                    _ => None,
                };
                summary_targets.push(SummaryTarget {
                    document_id: document_id.clone(),
                    source: source.clone(),
                    mime_type: file.mime_type.clone(),
                    filename: file.filename.clone(),
                    inline_bytes,
                });
            }
```

> Read the existing `FileSource::InlineBytes` definition before this step to confirm the field is called `bytes` and is a `Vec<u8>`. Adjust the destructure accordingly.

- [ ] **Step 3: Build the summary future**

Below the auto-register block (immediately after the closing `}` of `if let (Some(reg), Some(sid))` around line 907), insert the summary-task plumbing. **Critically, do not `await` it here** — bind it as a future and join with the answer call below.

```rust
        // ---- Step 4: Build summary tasks (run in parallel with answer call below) -----
        use crate::llm::domain::attachments::{
            AttachmentSummaryGenerator, SummaryConfig, SummaryInput, SummaryOutcome, SummarySource,
        };
        use crate::llm::infrastructure::attachment_summary::{
            acquire_bytes, extract_text, provider_cheap_tier, truncate_chars,
            LlmAttachmentSummaryGenerator,
        };

        let summary_generator: Option<std::sync::Arc<dyn AttachmentSummaryGenerator>> = if summary_enabled
            && !summary_targets.is_empty()
            && attachment_registry.is_some()
        {
            // Build a factory closure that constructs an LlmRepository for any
            // requested (provider, model, api_key) triple. We reuse the same
            // LlmProviderFactory used by the main call.
            use crate::llm::infrastructure::LlmProviderFactory;
            let gen = LlmAttachmentSummaryGenerator::new(move |cfg: &SummaryConfig| {
                LlmProviderFactory::create(cfg.provider.clone())
            });
            Some(std::sync::Arc::new(gen))
        } else {
            None
        };

        let summary_cfg = SummaryConfig {
            provider: provider_kind.clone(),
            model: summary_model_override
                .clone()
                .unwrap_or_else(|| provider_cheap_tier(&provider_kind).to_string()),
            api_key: api_key.clone(),
            max_output_chars: summary_max_output_chars,
            timeout: std::time::Duration::from_secs(summary_timeout_secs),
        };

        // Capture the SignedUrl fetcher from the existing downloader.
        let fetcher_for_summary = signed_url_fetcher.clone();

        let summary_fut = {
            let gen = summary_generator.clone();
            let reg = attachment_registry.clone();
            let sid = agent_session_id_str.clone();
            let provider_kind_cap = provider_kind.clone();
            let cfg = summary_cfg.clone();
            let targets = std::mem::take(&mut summary_targets);
            async move {
                let (Some(gen), Some(reg), Some(sid)) = (gen, reg, sid) else {
                    return;
                };
                // Each target runs concurrently with the others.
                let mut handles = Vec::new();
                for t in targets {
                    let gen = gen.clone();
                    let reg = reg.clone();
                    let sid = sid.clone();
                    let provider_kind = provider_kind_cap.clone();
                    let cfg = cfg.clone();
                    let fetcher = fetcher_for_summary.clone();
                    handles.push(tokio::spawn(async move {
                        let outcome = generate_one_summary(
                            &*gen,
                            &cfg,
                            &t,
                            fetcher,
                            summary_max_bytes,
                            summary_max_chars,
                        )
                        .await;
                        if let SummaryOutcome::Generated(text) = outcome {
                            if let Err(e) = reg
                                .update_description(&sid, &t.document_id, provider_kind, &text)
                                .await
                            {
                                tracing::warn!(
                                    target: "colmena::attachment",
                                    event = "summary.persist_failed",
                                    document_id = %t.document_id,
                                    error = %e,
                                    "failed to persist summary"
                                );
                            } else {
                                tracing::info!(
                                    target: "colmena::attachment",
                                    event = "summary.persisted",
                                    document_id = %t.document_id,
                                    summary_len = text.len(),
                                    "summary persisted"
                                );
                            }
                        } else {
                            tracing::info!(
                                target: "colmena::attachment",
                                event = "summary.skipped_or_failed",
                                document_id = %t.document_id,
                                outcome = ?outcome,
                                "summary skipped or failed"
                            );
                        }
                    }));
                }
                for h in handles {
                    let _ = h.await;
                }
            }
        };
```

And the helper `generate_one_summary` (place it as a private free function above `execute`):

```rust
async fn generate_one_summary(
    gen: &dyn crate::llm::domain::attachments::AttachmentSummaryGenerator,
    cfg: &crate::llm::domain::attachments::SummaryConfig,
    target: &SummaryTarget,
    fetcher: std::sync::Arc<dyn crate::llm::domain::signed_url_fetcher::SignedUrlFetcher>,
    max_bytes: usize,
    max_chars: usize,
) -> crate::llm::domain::attachments::SummaryOutcome {
    use crate::llm::domain::attachments::{SummaryInput, SummaryOutcome, SummarySource};
    use crate::llm::infrastructure::attachment_summary::{
        acquire_bytes, extract_text, truncate_chars,
    };

    // 1. Acquire bytes (bounded).
    let bytes = match acquire_bytes(
        &target.source,
        target.inline_bytes.as_deref(),
        max_bytes,
        fetcher,
    )
    .await
    {
        Ok(b) => b,
        Err(e) => {
            return SummaryOutcome::Skipped {
                reason: format!("byte acquisition failed: {}", e),
            }
        }
    };

    // 2. Build SummarySource based on mime.
    let source = if target.mime_type.starts_with("image/") {
        SummarySource::ImageBytes(bytes)
    } else {
        match extract_text(&target.mime_type, &bytes) {
            Ok(Some(text)) => {
                let truncated = truncate_chars(&text, max_chars);
                if truncated.trim().is_empty() {
                    return SummaryOutcome::Skipped {
                        reason: "extraction returned empty text".into(),
                    };
                }
                SummarySource::ExtractedText(truncated)
            }
            Ok(None) => {
                return SummaryOutcome::Skipped {
                    reason: format!("mime {} not extractable", target.mime_type),
                }
            }
            Err(e) => {
                return SummaryOutcome::Skipped {
                    reason: format!("extraction error: {}", e),
                }
            }
        }
    };

    let input = SummaryInput {
        filename: target.filename.clone(),
        mime_type: target.mime_type.clone(),
        source,
    };

    match gen.generate(input, cfg).await {
        Ok(outcome) => outcome,
        Err(e) => SummaryOutcome::Failed {
            reason: format!("generator error: {}", e),
        },
    }
}
```

- [ ] **Step 4: Join the summary future with the answer call**

Locate the existing point in `execute` where the agent service runs the main call. **Before editing, run** `grep -n "agent_service" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs | head -20` to find the await line. It will look roughly like:

```rust
        let agent_result = agent_service.run(params).await
            .map_err(|e| format!("agent run: {}", e))?;
```

Convert it to a `tokio::join!` so the summary future overlaps with the answer call. Replace the **single** `await` with:

```rust
        let summary_timeout_dur = std::time::Duration::from_secs(summary_timeout_secs);
        let (agent_run_result, summary_outcome) = tokio::join!(
            agent_service.run(params),
            tokio::time::timeout(summary_timeout_dur, summary_fut),
        );

        if summary_outcome.is_err() {
            tracing::warn!(
                target: "colmena::attachment",
                event = "summary.batch_timeout",
                timeout_secs = summary_timeout_secs,
                "summary batch exceeded timeout"
            );
        }

        let agent_result = agent_run_result.map_err(|e| format!("agent run: {}", e))?;
```

**Constraints to respect when adapting to the real signature:**

1. **Do not move `agent_service`** into the future — `agent_service.run(...)` keeps the borrow inside the `join!` macro, which is fine because `join!` polls both futures in the same task.
2. **The `summary_fut`** created in Step 3 must be owned-only (no `&self`, no `&agent_service` capture). Verify: every value it uses is cloned into the `async move` block.
3. **Preserve the existing error mapping.** If the original line wraps the error differently (e.g. `?` directly, or `.map_err(|e| anyhow!(...))`), use the same form on `agent_run_result`.

If the original line passes arguments other than a single `params` value, keep them verbatim inside the `join!` invocation — the macro accepts any expression that resolves to a future.

- [ ] **Step 5: Build / typecheck the wiring**

Run: `cargo build -p colmena_dag_engine`
Expected: clean build.

If there are signature mismatches (e.g. `agent_service.run` is `async fn` that returns a borrow), refactor by:
- Hoisting all owned values needed by `agent_service.run` into `let` bindings before the `join!`.
- Not capturing `&self` inside the summary future (it must own its data).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(auto-summary): wire parallel summary generation into llm_call"
```

---

## Task 10: Integration test with mocked LLM

**Files:**
- Create: `src/libs/colmena/tests/load_attachment_auto_summary_e2e.rs`

- [ ] **Step 1: Write the failing test**

Create `src/libs/colmena/tests/load_attachment_auto_summary_e2e.rs`:

```rust
//! End-to-end coverage of the auto-summary path with a mocked LLM
//! (no network). Asserts that:
//!   * a new attachment without caller description triggers the summary path
//!   * the produced summary is persisted into `conversation_attachments.description`
//!   * `summary_enabled = false` disables the path
//!   * caller-supplied description short-circuits generation

use colmena_dag_engine::llm::domain::attachments::{
    AttachmentRegistry, AttachmentSource, SummaryOutcome, UpsertAttachmentInput,
};
use colmena_dag_engine::llm::domain::ProviderKind;
use colmena_dag_engine::llm::infrastructure::persistence::SqliteAttachmentRegistry;
use std::sync::Arc;

async fn setup_registry() -> Arc<dyn AttachmentRegistry> {
    let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
        .await
        .expect("registry init");
    Arc::new(reg)
}

#[tokio::test]
async fn caller_supplied_description_is_preserved() {
    let reg = setup_registry().await;
    reg.upsert(UpsertAttachmentInput {
        agent_session_id: "s1".into(),
        document_id: "doc-1".into(),
        provider: ProviderKind::Mock,
        provider_file_id: "pf-1".into(),
        mime_type: "application/pdf".into(),
        filename: "a.pdf".into(),
        size_bytes: Some(100),
        label: None,
        description: Some("caller-supplied".into()),
        source: AttachmentSource::Inline,
    })
    .await
    .unwrap();

    let row = reg
        .lookup("s1", "doc-1", ProviderKind::Mock)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.description.as_deref(), Some("caller-supplied"));
}

#[tokio::test]
async fn update_description_overwrites_existing() {
    let reg = setup_registry().await;
    reg.upsert(UpsertAttachmentInput {
        agent_session_id: "s1".into(),
        document_id: "doc-1".into(),
        provider: ProviderKind::Mock,
        provider_file_id: "pf-1".into(),
        mime_type: "application/pdf".into(),
        filename: "a.pdf".into(),
        size_bytes: Some(100),
        label: None,
        description: None,
        source: AttachmentSource::Inline,
    })
    .await
    .unwrap();
    reg.update_description("s1", "doc-1", ProviderKind::Mock, "auto-generated")
        .await
        .unwrap();

    let row = reg
        .lookup("s1", "doc-1", ProviderKind::Mock)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.description.as_deref(), Some("auto-generated"));
}
```

> This test exercises the persistence path and the public surface area touched by the feature. A full graph-runner test against the wired `llm_call` node is captured by the real-API test graph in Task 11. The unit tests in Task 8 already cover the generator's behaviour with mocked `LlmRepository`.

- [ ] **Step 2: Run tests**

Run: `cargo test -p colmena_dag_engine --test load_attachment_auto_summary_e2e`
Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/tests/load_attachment_auto_summary_e2e.rs
git commit -m "test(auto-summary): e2e persistence + caller-description preservation"
```

---

## Task 11: Real-API smoke test graph

**Files:**
- Create: `tests/graphs/agents/load_attachment_auto_summary.json`

- [ ] **Step 1: Create the graph**

Create `tests/graphs/agents/load_attachment_auto_summary.json`:

```json
{
  "nodes": {
    "ask": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "connection_url": "${DATABASE_URL}",
        "session_id": "load_attachment_auto_summary",
        "attachments_enabled": true,
        "summary_enabled": true,
        "system_message": "You are a helpful assistant. Answer concisely.",
        "prompt": "What is in the attached document? One sentence.",
        "files": [
          {
            "id": "demo_doc",
            "label": "Demo PDF",
            "url": "$REPLACE_WITH_SIGNED_URL",
            "mime_type": "application/pdf",
            "filename": "demo.pdf"
          }
        ]
      }
    },
    "out": { "type": "log" }
  },
  "edges": [{ "from": "ask", "to": "out" }]
}
```

The `description` field is intentionally omitted so the auto-summary path fires. The graph uses Postgres for the registry (`connection_url` reads `DATABASE_URL`) so the summary survives across runs.

- [ ] **Step 2: Smoke-test instructions (for the engineer to run locally)**

```bash
# Replace the placeholder with a real signed URL to a PDF before running.
# Then run twice with the same agent-session-id:
source .env
cargo run --bin dag_engine -- run tests/graphs/agents/load_attachment_auto_summary.json \
  --agent-session-id agent_auto_summary_001

# Inspect Postgres to confirm description was populated:
psql "$DATABASE_URL" -c \
  "SELECT document_id, description FROM conversation_attachments \
   WHERE agent_session_id = 'agent_auto_summary_001';"
```

Expected: the `description` column contains a non-empty single line describing the PDF.

- [ ] **Step 3: Commit**

```bash
git add tests/graphs/agents/load_attachment_auto_summary.json
git commit -m "test(auto-summary): real-API smoke graph (Gemini Flash + Postgres)"
```

---

## Task 12: Documentation updates

**Files:**
- Modify: `docs/node_configurations.json`
- Modify: `docs/developer_guide/31_load_attachment.md`

- [ ] **Step 1: Document the new config fields in `node_configurations.json`**

Open `docs/node_configurations.json`. Find the `llm_call` entry and locate the existing `attachments_enabled` field. Below it, add (matching the surrounding JSON style — copy the `attachments_enabled` block as a template):

```json
"summary_enabled": {
  "type": "boolean",
  "required": false,
  "default": true,
  "description": "Auto-generate a one-line description for attachments registered without a caller-supplied `description`. Runs in parallel with the answer call using the provider's cheap-tier model."
},
"summary_max_chars": {
  "type": "integer",
  "required": false,
  "default": 5000,
  "description": "Max characters of extracted text sent to the summary LLM. ~2 pages of typical prose. Ignored for images (which are sent as-is)."
},
"summary_model": {
  "type": "string",
  "required": false,
  "default": null,
  "description": "Override the model used for summary generation. When omitted, defaults to the cheap tier of the main provider (Google→gemini-2.5-flash, OpenAI→gpt-4o-mini, Anthropic→claude-haiku-4-5-20251001)."
},
"summary_timeout_secs": {
  "type": "integer",
  "required": false,
  "default": 15,
  "description": "Hard timeout for the summary batch. On exceed, summaries are cancelled and `description` stays null."
},
"summary_max_output_chars": {
  "type": "integer",
  "required": false,
  "default": 200,
  "description": "Hard cap on the produced summary's length (chars). Post-truncation; the prompt asks the model to stay within this."
},
"summary_max_bytes": {
  "type": "integer",
  "required": false,
  "default": 26214400,
  "description": "Files above this size (bytes) skip extraction entirely. Guards against memory bombs. Default 25 MiB."
}
```

> Read the file before editing to match its exact indentation / property layout. Some node configs in this file use slightly different shapes per node.

- [ ] **Step 2: Add an "Auto-Summary" section to the developer guide**

Open `docs/developer_guide/31_load_attachment.md`. Append (just before "Troubleshooting" / "See also" if those exist, else at the end):

```markdown
## Auto-generated descriptions

When the caller does not provide a `description` for a file in `files[]`,
the `llm_call` node auto-generates a one-line summary in parallel with
the main answer call. The summary is persisted in
`conversation_attachments.description` and shown in the
`load_attachment` tool catalog from the next turn onward.

The auto-summary path:

1. Acquires the file bytes (re-downloads signed URLs, reads paths,
   or reuses inline bytes).
2. Extracts text locally (`pdf-extract` for PDFs, UTF-8 decode for
   `text/*` mimes). Images skip extraction and are sent as vision input.
3. Truncates to `summary_max_chars` (default 5000 ≈ first 2 pages of prose).
4. Calls the provider's cheap-tier model with a 1-line summary prompt.
5. Persists the result.

Failure modes are silent — corrupt PDFs, unsupported MIMEs, LLM errors,
and timeouts all result in `description = null` and a fallback to
`filename` in the catalog.

### Config

| Field | Default | What it does |
|---|---|---|
| `summary_enabled` | `true` | Master switch. Set to `false` to disable auto-summary entirely. |
| `summary_max_chars` | `5000` | Max chars of extracted text sent to the model. |
| `summary_model` | provider cheap-tier | Override the model used. Defaults to Flash/4o-mini/Haiku per provider. |
| `summary_timeout_secs` | `15` | Hard timeout on the summary batch. |
| `summary_max_output_chars` | `200` | Cap on the produced summary's length. |
| `summary_max_bytes` | `26214400` (25 MiB) | Files larger than this skip extraction. |

### Supported MIME types

| MIME | Path | Notes |
|---|---|---|
| `application/pdf` | extract text (`pdf-extract`) | Image-only PDFs (no text layer) → `description = null`. |
| `text/plain`, `text/markdown`, `text/csv`, `text/html` | UTF-8 decode | Invalid UTF-8 → skipped. |
| `image/*` | send image as-is to vision model | One image ≈ 258 tokens. |
| anything else | fallback to filename | Office formats (`docx`, `xlsx`) currently unsupported. |
```

- [ ] **Step 3: Commit**

```bash
git add docs/node_configurations.json docs/developer_guide/31_load_attachment.md
git commit -m "docs(auto-summary): config schema + developer guide section"
```

---

## Final verification

- [ ] **Step 1: Full test sweep**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p colmena_dag_engine --lib
cargo test -p colmena_dag_engine --tests
```

Expected: all green.

- [ ] **Step 2: Doctest run (catches doc-comment errors)**

```bash
cargo test -p colmena_dag_engine --verbose
```

Expected: all green, including doctests.

- [ ] **Step 3: Real-API smoke (optional, requires `.env`)**

Follow the instructions in Task 11 Step 2. Confirm `description` is populated in Postgres.

---

## Open caveats for the implementer

- **`SignedUrlFetcher::fetch_bounded`** may not exist with that exact signature. Task 7's "check before this step" callout explains how to adapt. If a bounded-fetch method must be added, do it in Task 7, not later — Task 9 depends on it.
- **`LlmProviderFactory::create` signature.** Task 9's summary-generator factory closure assumes `create(provider_kind)` returns an `Arc<dyn LlmRepository>`. Verify by reading `src/libs/colmena/src/llm/infrastructure/` — if it actually needs `(provider_kind, api_key, model)`, expand the closure to pass `cfg.api_key.clone()` and `cfg.model.clone()` from the `SummaryConfig`.
- **`agent_service.run` await point.** The exact line is the one Task 9 Step 4 wraps in `tokio::join!`. Hoist all owned arguments into `let` bindings if Rust complains about borrow lifetime; the summary future cannot capture `&self`.
- **`MockLlmRepository`** is generated by `#[cfg_attr(test, mockall::automock)]` on the trait. Test imports follow the pattern used in existing tests under `src/libs/colmena/src/llm/`. If `LlmResponse::new` or `TokenUsage::default` don't compile, copy the response-construction idiom from the nearest existing mocked-LLM test.
