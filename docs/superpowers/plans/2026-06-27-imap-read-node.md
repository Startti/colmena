# Nodo `imap_read` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Un nodo `imap_read` que lee correo vía IMAP (read-only, no destructivo) con app password, búsqueda por criterios estructurados, cuerpo en texto, y adjuntos listados + descargables como attachments de Colmena.

**Architecture:** Tres archivos: `imap_search.rs` (puro: criterios → comando SEARCH), `imap_mime.rs` (puro: bytes RFC822 → email parseado vía `mail-parser`), y `imap.rs` (`ImapNode`: orquesta la sesión IMAP con `async-imap`/`tokio-rustls`, delega parseo a los puros, registra adjuntos vía `OutputStorageRepository`). La lógica testeable vive en los dos módulos puros; el glue de red se verifica E2E contra Gmail real.

**Tech Stack:** Rust, `async-imap` 0.11, `tokio-rustls` 0.26, `mail-parser` 0.11, `chrono` (ya presente), `tokio`. TDD en los módulos puros; E2E `#[ignore]` para el round-trip de protocolo.

**Spec:** [`docs/superpowers/specs/2026-06-27-imap-read-node-design.md`](../specs/2026-06-27-imap-read-node-design.md)

---

## File Structure

- **Modify** `src/libs/colmena/Cargo.toml` — deps `async-imap`, `tokio-rustls`, `mail-parser`.
- **Create** `src/libs/colmena/src/dag_engine/infrastructure/nodes/imap_search.rs` — `SearchCriteria` + `build_search_command` (puro).
- **Create** `src/libs/colmena/src/dag_engine/infrastructure/nodes/imap_mime.rs` — `ParsedEmail`, `AttachmentInfo`, `parse_email` (puro, sobre `mail-parser`).
- **Create** `src/libs/colmena/src/dag_engine/infrastructure/nodes/imap.rs` — `ImapNode` (`ExecutableNode`), sesión IMAP, builders `with_storage`/`with_attachment_resolver`, `schema()`.
- **Modify** `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs` — declarar los 3 módulos.
- **Modify** `src/libs/colmena/src/dag_engine/infrastructure/registry.rs` — registrar `"imap_read"`, inyectar storage/resolver.
- **Create** `tests/graphs/external/imap_read_gmail.json` — grafo E2E (manual/gated).
- **Create** `docs/developer_guide/50_imap_node.md` + **Modify** `docs/node_configurations.json`, `docs/node_as_tools_reference.json`, `docs/DEVELOPER_GUIDE.md`, `docs/CHANGELOG_2026-06.md`.

---

## Task 1: `imap_search.rs` — criterios estructurados → comando SEARCH (puro)

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/imap_search.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`

- [ ] **Step 1: Escribir el archivo con impl + tests (TDD: tests primero mentalmente, pero entregar el archivo completo)**

Primero declarar el módulo en `mod.rs`: agregar `pub mod imap_search;`.

Contenido de `imap_search.rs`:

```rust
//! Pure builder: structured search criteria -> IMAP SEARCH command string.
//! No network, no IMAP session — fully unit-testable. The node feeds the
//! resulting string to `UID SEARCH`.

use serde::Deserialize;

/// Structured search criteria, deserialized from the node config `search` object.
/// All fields optional; absent = no filter on that dimension.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchCriteria {
    #[serde(default)]
    pub unseen: bool,
    pub from: Option<String>,
    pub to: Option<String>,
    pub subject: Option<String>,
    pub body_contains: Option<String>,
    /// ISO date `YYYY-MM-DD`.
    pub since: Option<String>,
    /// ISO date `YYYY-MM-DD`.
    pub before: Option<String>,
}

/// Convert an ISO `YYYY-MM-DD` date to IMAP's `dd-Mon-yyyy` (e.g. `01-Jun-2026`).
fn iso_to_imap_date(iso: &str) -> Result<String, String> {
    let d = chrono::NaiveDate::parse_from_str(iso.trim(), "%Y-%m-%d")
        .map_err(|_| format!("imap_read: invalid date '{iso}', expected YYYY-MM-DD"))?;
    // %b is the English 3-letter month abbreviation, which IMAP expects.
    Ok(d.format("%d-%b-%Y").to_string())
}

/// Escape a string for use inside an IMAP quoted string (RFC 3501 §4.3):
/// backslash and double-quote are backslash-escaped.
fn imap_quote(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Build the IMAP SEARCH key string from the criteria. Empty criteria -> `ALL`.
/// Multiple keys are space-separated (implicit AND).
pub fn build_search_command(c: &SearchCriteria) -> Result<String, String> {
    let mut parts: Vec<String> = Vec::new();
    if c.unseen {
        parts.push("UNSEEN".to_string());
    }
    if let Some(v) = &c.from {
        parts.push(format!("FROM {}", imap_quote(v)));
    }
    if let Some(v) = &c.to {
        parts.push(format!("TO {}", imap_quote(v)));
    }
    if let Some(v) = &c.subject {
        parts.push(format!("SUBJECT {}", imap_quote(v)));
    }
    if let Some(v) = &c.body_contains {
        parts.push(format!("BODY {}", imap_quote(v)));
    }
    if let Some(v) = &c.since {
        parts.push(format!("SINCE {}", iso_to_imap_date(v)?));
    }
    if let Some(v) = &c.before {
        parts.push(format!("BEFORE {}", iso_to_imap_date(v)?));
    }
    if parts.is_empty() {
        Ok("ALL".to_string())
    } else {
        Ok(parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_criteria_is_all() {
        assert_eq!(build_search_command(&SearchCriteria::default()).unwrap(), "ALL");
    }

    #[test]
    fn unseen_only() {
        let c = SearchCriteria { unseen: true, ..Default::default() };
        assert_eq!(build_search_command(&c).unwrap(), "UNSEEN");
    }

    #[test]
    fn combines_with_implicit_and() {
        let c = SearchCriteria {
            unseen: true,
            from: Some("boss@x.com".into()),
            subject: Some("factura".into()),
            ..Default::default()
        };
        assert_eq!(
            build_search_command(&c).unwrap(),
            "UNSEEN FROM \"boss@x.com\" SUBJECT \"factura\""
        );
    }

    #[test]
    fn iso_dates_convert_to_imap_format() {
        let c = SearchCriteria {
            since: Some("2026-06-01".into()),
            before: Some("2026-06-27".into()),
            ..Default::default()
        };
        assert_eq!(
            build_search_command(&c).unwrap(),
            "SINCE 01-Jun-2026 BEFORE 27-Jun-2026"
        );
    }

    #[test]
    fn invalid_date_errors() {
        let c = SearchCriteria { since: Some("06/01/2026".into()), ..Default::default() };
        let err = build_search_command(&c).unwrap_err();
        assert!(err.contains("invalid date"));
    }

    #[test]
    fn quotes_are_escaped() {
        let c = SearchCriteria { subject: Some("he said \"hi\"".into()), ..Default::default() };
        assert_eq!(build_search_command(&c).unwrap(), "SUBJECT \"he said \\\"hi\\\"\"");
    }

    #[test]
    fn deserializes_from_json() {
        let c: SearchCriteria = serde_json::from_value(serde_json::json!({
            "unseen": true, "from": "a@b.com"
        })).unwrap();
        assert!(c.unseen);
        assert_eq!(c.from.as_deref(), Some("a@b.com"));
    }
}
```

- [ ] **Step 2: Correr los tests → fallan al compilar primero si los escribes antes; aquí: correr y verificar PASS**

Run: `cargo test --lib imap_search`
Expected: 7 passed.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/imap_search.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs
git commit -m "feat(imap): pure search-criteria to IMAP SEARCH builder"
```

---

## Task 2: `imap_mime.rs` — parseo de email (puro, sobre `mail-parser`)

**Files:**
- Modify: `src/libs/colmena/Cargo.toml` (add `mail-parser`)
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/imap_mime.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`

- [ ] **Step 1: Agregar dep**

En `src/libs/colmena/Cargo.toml`, en `[dependencies]`, agregar:
```toml
mail-parser = "0.11"
```

- [ ] **Step 2: Escribir `imap_mime.rs` (impl + fixtures de test)**

Declarar en `mod.rs`: `pub mod imap_mime;`.

```rust
//! Pure MIME parsing: raw RFC822 bytes -> structured email (headers, text body,
//! attachment metadata + bytes). Wraps `mail-parser`. No network.

use mail_parser::MessageParser;

/// Metadata for one attachment. `bytes` carries the decoded content so the node
/// can register it as a Colmena attachment when download is requested.
#[derive(Debug, Clone, PartialEq)]
pub struct AttachmentInfo {
    pub filename: String,
    pub mime: String,
    pub size: usize,
    pub bytes: Vec<u8>,
}

/// Structured result of parsing one email.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEmail {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub date: String,
    pub body_text: String,
    pub body_truncated: bool,
    pub attachments: Vec<AttachmentInfo>,
}

/// Parse raw RFC822 bytes. Prefers text/plain; falls back to HTML converted to
/// text. Truncates the body to `body_max_bytes` (UTF-8 safe), setting
/// `body_truncated`. Returns Err on unparseable input.
///
/// NOTE TO IMPLEMENTER: confirm the exact `mail-parser` 0.11 API. As of 0.11 the
/// shape is roughly: `MessageParser::default().parse(raw) -> Option<Message>`;
/// `Message::from()`/`to()` return `Option<&Address>` (use `.first()` +
/// `.address()`); `Message::subject() -> Option<&str>`; `Message::date() ->
/// Option<&DateTime>`; `Message::body_text(0) -> Option<Cow<str>>`;
/// `Message::body_html(0)`; `Message::attachments()` yields `&MessagePart` with
/// `.attachment_name() -> Option<&str>`, `.contents() -> &[u8]`, and a
/// content-type accessor. Adapt the accessors below to the real API; the TESTS
/// (which assert on `ParsedEmail`) are the contract.
pub fn parse_email(raw: &[u8], body_max_bytes: usize) -> Result<ParsedEmail, String> {
    let msg = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| "imap_read: failed to parse message".to_string())?;

    let from = msg
        .from()
        .and_then(|a| a.first())
        .and_then(|addr| addr.address())
        .unwrap_or("")
        .to_string();
    let to = msg
        .to()
        .and_then(|a| a.first())
        .and_then(|addr| addr.address())
        .unwrap_or("")
        .to_string();
    let subject = msg.subject().unwrap_or("").to_string();
    let date = msg.date().map(|d| d.to_rfc3339()).unwrap_or_default();

    // Prefer plain text; fall back to HTML stripped to text.
    let raw_body: String = match msg.body_text(0) {
        Some(t) => t.into_owned(),
        None => match msg.body_html(0) {
            Some(h) => strip_html(&h),
            None => String::new(),
        },
    };
    let (body_text, body_truncated) = truncate_utf8(&raw_body, body_max_bytes);

    let mut attachments = Vec::new();
    for part in msg.attachments() {
        let bytes = part.contents().to_vec();
        let filename = part.attachment_name().unwrap_or("attachment").to_string();
        // content-type as "type/subtype"; fall back to octet-stream.
        let mime = part
            .content_type()
            .map(|ct| match ct.subtype() {
                Some(st) => format!("{}/{}", ct.ctype(), st),
                None => ct.ctype().to_string(),
            })
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let size = bytes.len();
        attachments.push(AttachmentInfo { filename, mime, size, bytes });
    }

    Ok(ParsedEmail { from, to, subject, date, body_text, body_truncated, attachments })
}

/// Truncate to at most `max` bytes without splitting a UTF-8 char boundary.
fn truncate_utf8(s: &str, max: usize) -> (String, bool) {
    if s.len() <= max {
        return (s.to_string(), false);
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

/// Minimal HTML-to-text: drop tags, collapse whitespace. Good enough for an LLM
/// to read; not a full HTML renderer.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: &[u8] = b"From: alice@example.com\r\nTo: bob@example.com\r\nSubject: Hello\r\nDate: Mon, 01 Jun 2026 10:00:00 +0000\r\nContent-Type: text/plain\r\n\r\nThis is the body.";

    #[test]
    fn parses_plain_text_email() {
        let p = parse_email(PLAIN, 5120).unwrap();
        assert_eq!(p.from, "alice@example.com");
        assert_eq!(p.to, "bob@example.com");
        assert_eq!(p.subject, "Hello");
        assert!(p.body_text.contains("This is the body."));
        assert!(!p.body_truncated);
        assert!(p.attachments.is_empty());
    }

    #[test]
    fn truncates_long_body() {
        let mut raw = b"Subject: x\r\nContent-Type: text/plain\r\n\r\n".to_vec();
        raw.extend(std::iter::repeat(b'a').take(10_000));
        let p = parse_email(&raw, 100).unwrap();
        assert!(p.body_truncated);
        assert!(p.body_text.len() <= 100);
    }

    #[test]
    fn extracts_attachment_metadata_and_bytes() {
        // multipart/mixed with one text part and one attached text file.
        let raw = b"Subject: with attach\r\nContent-Type: multipart/mixed; boundary=BB\r\n\r\n--BB\r\nContent-Type: text/plain\r\n\r\nbody here\r\n--BB\r\nContent-Type: text/plain; name=\"note.txt\"\r\nContent-Disposition: attachment; filename=\"note.txt\"\r\n\r\nFILEDATA\r\n--BB--\r\n";
        let p = parse_email(raw, 5120).unwrap();
        assert!(p.body_text.contains("body here"));
        assert_eq!(p.attachments.len(), 1);
        assert_eq!(p.attachments[0].filename, "note.txt");
        assert_eq!(p.attachments[0].bytes, b"FILEDATA");
        assert_eq!(p.attachments[0].size, 8);
    }

    #[test]
    fn html_only_is_stripped_to_text() {
        let raw = b"Subject: h\r\nContent-Type: text/html\r\n\r\n<html><body><p>Hello <b>world</b></p></body></html>";
        let p = parse_email(raw, 5120).unwrap();
        assert!(p.body_text.contains("Hello"));
        assert!(p.body_text.contains("world"));
        assert!(!p.body_text.contains("<"));
    }

    #[test]
    fn strip_html_collapses_whitespace() {
        assert_eq!(strip_html("<p>a</p>\n  <p>b</p>"), "a b");
    }
}
```

- [ ] **Step 3: Correr tests; ajustar accesores de `mail-parser` si la API real difiere**

Run: `cargo test --lib imap_mime`
Expected: 5 passed. Si algún test falla por nombres de método de `mail-parser` (p.ej. `content_type`/`ctype`/`subtype`), ajusta SOLO los accesores en `parse_email` hasta que los tests (que son el contrato sobre `ParsedEmail`) pasen. No cambies las asserts salvo que un comportamiento sea genuinamente distinto (en ese caso, anótalo).

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/Cargo.toml src/libs/colmena/src/dag_engine/infrastructure/nodes/imap_mime.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs Cargo.lock
git commit -m "feat(imap): pure MIME parsing (text body + attachment metadata)"
```

---

## Task 3: `ImapNode` — struct, builders, output-shaping (puro) y `schema()`

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/imap.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`

- [ ] **Step 1: Escribir el esqueleto del nodo + la función pura de shaping con su test**

Declarar en `mod.rs`: `pub mod imap;`.

`imap.rs` (parte 1 — struct, builders, helpers puros, schema; el `execute` con red viene en Task 4):

```rust
//! `imap_read` node: read-only IMAP email retrieval. Connects over TLS, logs in
//! with an app password, EXAMINEs the mailbox (read-only), searches by structured
//! criteria, fetches matching messages with BODY.PEEK (never marks seen), and
//! returns headers + text body + attachment metadata. Optionally downloads
//! attachment bytes and registers them as Colmena attachments.

use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::infrastructure::nodes::imap_mime::ParsedEmail;
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;

#[derive(Default)]
pub struct ImapNode {
    storage: Option<Arc<dyn crate::storage::domain::OutputStorageRepository>>,
    attachment_resolver:
        Option<Arc<dyn crate::llm::domain::attachments::AttachmentStreamResolver>>,
}

impl ImapNode {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_storage(
        mut self,
        storage: Arc<dyn crate::storage::domain::OutputStorageRepository>,
    ) -> Self {
        self.storage = Some(storage);
        self
    }

    pub fn with_attachment_resolver(
        mut self,
        resolver: Arc<dyn crate::llm::domain::attachments::AttachmentStreamResolver>,
    ) -> Self {
        self.attachment_resolver = Some(resolver);
        self
    }

    /// Read a string field from inputs (priority) then config.
    fn get_str<'a>(inputs: &'a NodeInputs, config: &'a Value, key: &str) -> Option<&'a str> {
        inputs
            .get(key)
            .and_then(|v| v.as_str())
            .or_else(|| config.get(key).and_then(|v| v.as_str()))
    }

    /// Resolve `${ENV_VAR}` occurrences in a string. Mirrors the per-node helper
    /// used by http.rs/socketio.rs.
    fn resolve_env_vars(input: &str) -> Result<String, String> {
        let mut out = input.to_string();
        while let Some(start) = out.find("${") {
            let end = out[start..]
                .find('}')
                .map(|e| start + e)
                .ok_or_else(|| format!("imap_read: unterminated ${{ in '{input}'"))?;
            let var = &out[start + 2..end];
            let val = std::env::var(var)
                .map_err(|_| format!("imap_read: env var '{var}' not set"))?;
            out.replace_range(start..=end, &val);
        }
        Ok(out)
    }

    /// Shape parsed emails (+ optional attachment document_ids) into the node output.
    /// `doc_ids[i]` aligns with `emails[i].attachments`; an entry is `None` when the
    /// attachment was not downloaded. Pure — unit-testable.
    fn build_output(emails: &[ParsedEmail], doc_ids: &[Vec<Option<String>>]) -> Value {
        let messages: Vec<Value> = emails
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let atts: Vec<Value> = e
                    .attachments
                    .iter()
                    .enumerate()
                    .map(|(j, a)| {
                        let mut o = json!({
                            "filename": a.filename, "mime": a.mime, "size": a.size
                        });
                        if let Some(Some(id)) = doc_ids.get(i).and_then(|v| v.get(j)) {
                            o["document_id"] = json!(id);
                        }
                        o
                    })
                    .collect();
                json!({
                    "from": e.from, "to": e.to, "subject": e.subject, "date": e.date,
                    "body_text": e.body_text, "body_truncated": e.body_truncated,
                    "attachments": atts
                })
            })
            .collect();
        json!({ "output": { "count": messages.len(), "messages": messages } })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::infrastructure::nodes::imap_mime::{AttachmentInfo, ParsedEmail};

    fn sample(att: bool) -> ParsedEmail {
        ParsedEmail {
            from: "a@x.com".into(), to: "b@x.com".into(), subject: "s".into(),
            date: "2026-06-01T10:00:00+00:00".into(), body_text: "hi".into(),
            body_truncated: false,
            attachments: if att {
                vec![AttachmentInfo { filename: "f.pdf".into(), mime: "application/pdf".into(),
                                      size: 3, bytes: vec![1, 2, 3] }]
            } else { vec![] },
        }
    }

    #[test]
    fn build_output_without_attachments() {
        let out = ImapNode::build_output(&[sample(false)], &[vec![]]);
        assert_eq!(out["output"]["count"], 1);
        assert_eq!(out["output"]["messages"][0]["subject"], "s");
        assert_eq!(out["output"]["messages"][0]["attachments"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn build_output_includes_document_id_when_downloaded() {
        let out = ImapNode::build_output(&[sample(true)], &[vec![Some("doc_123".into())]]);
        let att = &out["output"]["messages"][0]["attachments"][0];
        assert_eq!(att["filename"], "f.pdf");
        assert_eq!(att["document_id"], "doc_123");
    }

    #[test]
    fn build_output_omits_document_id_when_not_downloaded() {
        let out = ImapNode::build_output(&[sample(true)], &[vec![None]]);
        let att = &out["output"]["messages"][0]["attachments"][0];
        assert!(att.get("document_id").is_none());
    }

    #[test]
    fn resolve_env_vars_substitutes() {
        std::env::set_var("IMAP_TEST_VAR", "secret");
        assert_eq!(ImapNode::resolve_env_vars("${IMAP_TEST_VAR}").unwrap(), "secret");
        std::env::remove_var("IMAP_TEST_VAR");
    }
}
```

> NOTE: `ExecutableNode` requires `execute` + `schema`. This task does NOT yet impl
> the trait (no `execute`), so the file compiles as plain methods. The trait impl
> lands in Task 4. If the crate's `warnings = "deny"` flags the unused
> `storage`/`attachment_resolver` fields before Task 4 wires them, add a temporary
> `#[allow(dead_code)]` on the struct and REMOVE it in Task 4 (note it in the commit).

- [ ] **Step 2: Correr tests**

Run: `cargo test --lib imap::`  (o `cargo test --lib imap_node` si el módulo se nombra distinto)
Expected: 4 passed.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/imap.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs
git commit -m "feat(imap): ImapNode struct, builders, output shaping (pure)"
```

---

## Task 4: `ImapNode::execute` + `schema()` — sesión IMAP (red) e impl del trait

**Files:**
- Modify: `src/libs/colmena/Cargo.toml` (add `async-imap`, `tokio-rustls`)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/imap.rs`

- [ ] **Step 1: Agregar deps**

En `Cargo.toml` `[dependencies]`:
```toml
async-imap = { version = "0.11", default-features = false, features = ["runtime-tokio"] }
tokio-rustls = "0.26"
```
> Confirma los nombres de feature de `async-imap` 0.11 (`runtime-tokio` vs `tokio`); ajusta para que compile con TLS sobre tokio usando rustls (el TLS del proyecto). Si `async-imap` no expone rustls directamente, usa `tokio-rustls` para construir el `TlsConnector` y pásale el stream a `async_imap::Client::new`.

- [ ] **Step 2: Implementar `execute` + `schema` (orquestación de red)**

Agregar a `imap.rs`. Esta es la parte **dependiente de la API de `async-imap` 0.11** — implementa la orquestación y CONFIRMA la API exacta del crate (los nombres abajo son la forma esperada; adáptalos). Se verifica vía la E2E real (Task 6), no por unit test (no hay servidor IMAP embebido).

```rust
use crate::dag_engine::infrastructure::nodes::imap_mime::parse_email;
use crate::dag_engine::infrastructure::nodes::imap_search::{build_search_command, SearchCriteria};
use futures::StreamExt;

impl ExecutableNode for ImapNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let err = |m: String| Box::new(std::io::Error::other(m)) as Box<dyn StdError + Send + Sync>;

        // 1. Config (inputs > config), resolve ${ENV}.
        let host = Self::resolve_env_vars(
            Self::get_str(inputs, config, "host").unwrap_or("imap.gmail.com"),
        ).map_err(err)?;
        let port = inputs.get("port").or_else(|| config.get("port"))
            .and_then(|v| v.as_u64()).unwrap_or(993) as u16;
        let username = Self::resolve_env_vars(
            Self::get_str(inputs, config, "username")
                .ok_or_else(|| err("imap_read: 'username' is required".into()))?,
        ).map_err(err)?;
        let password = Self::resolve_env_vars(
            Self::get_str(inputs, config, "password")
                .ok_or_else(|| err("imap_read: 'password' is required".into()))?,
        ).map_err(err)?;
        let mailbox = Self::resolve_env_vars(
            Self::get_str(inputs, config, "mailbox").unwrap_or("INBOX"),
        ).map_err(err)?;
        let max_results = inputs.get("max_results").or_else(|| config.get("max_results"))
            .and_then(|v| v.as_u64()).unwrap_or(20) as usize;
        let body_max_bytes = inputs.get("body_max_bytes").or_else(|| config.get("body_max_bytes"))
            .and_then(|v| v.as_u64()).unwrap_or(5120) as usize;
        let download_attachments = inputs.get("download_attachments")
            .or_else(|| config.get("download_attachments"))
            .and_then(|v| v.as_bool()).unwrap_or(false);

        // 2. Search criteria.
        let criteria: SearchCriteria = match inputs.get("search").or_else(|| config.get("search")) {
            Some(v) => serde_json::from_value(v.clone())
                .map_err(|e| err(format!("imap_read: invalid search: {e}")))?,
            None => SearchCriteria::default(),
        };
        let search_cmd = build_search_command(&criteria).map_err(err)?;

        if download_attachments && self.storage.is_none() {
            return Err(err("imap_read: download_attachments requires storage configured".into()));
        }

        // 3. Connect TLS + login. CONFIRM async-imap 0.11 API.
        //    Expected shape (adapt as needed):
        //      let tls = tokio_rustls::TlsConnector::from(Arc::new(rustls_client_config()));
        //      let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
        //      let tls_stream = tls.connect(server_name, tcp).await?;
        //      let client = async_imap::Client::new(tls_stream);
        //      let mut session = client.login(&username, &password).await.map_err(|e| e.0)?;
        //    Map a login failure to the actionable message below.
        let mut session = imap_connect_login(&host, port, &username, &password)
            .await
            .map_err(|e| err(format!(
                "imap_read: authentication/connection failed for {host}:{port} — verify the \
                 app password and that 2-Step Verification is enabled (Workspace admins may \
                 disable IMAP/app-passwords). Detail: {e}"
            )))?;

        // 4. EXAMINE (read-only) the mailbox.
        session.examine(&mailbox).await
            .map_err(|e| err(format!("imap_read: cannot open mailbox '{mailbox}': {e}")))?;

        // 5. UID SEARCH, take most-recent max_results.
        let mut uids: Vec<u32> = session.uid_search(&search_cmd).await
            .map_err(|e| err(format!("imap_read: SEARCH failed: {e}")))?
            .into_iter().collect();
        uids.sort_unstable();
        let take: Vec<u32> = uids.into_iter().rev().take(max_results).collect();

        // 6. Fetch + parse each (BODY.PEEK[] so flags never change).
        let mut emails: Vec<ParsedEmail> = Vec::new();
        for uid in &take {
            let mut stream = session.uid_fetch(uid.to_string(), "BODY.PEEK[]").await
                .map_err(|e| err(format!("imap_read: FETCH uid {uid} failed: {e}")))?;
            if let Some(item) = stream.next().await {
                let fetch = item.map_err(|e| err(format!("imap_read: fetch item error: {e}")))?;
                if let Some(body) = fetch.body() {
                    match parse_email(body, body_max_bytes) {
                        Ok(p) => emails.push(p),
                        Err(_) => { /* skip unparseable; counted via count */ }
                    }
                }
            }
            drop(stream);
        }
        let _ = session.logout().await;

        // 7. Optionally register attachment bytes -> document_ids.
        let mut doc_ids: Vec<Vec<Option<String>>> = Vec::with_capacity(emails.len());
        for e in &emails {
            let mut row = Vec::with_capacity(e.attachments.len());
            for a in &e.attachments {
                if download_attachments {
                    let id = self.register_attachment(&a.filename, &a.mime, &a.bytes).await
                        .map_err(|m| err(m))?;
                    row.push(Some(id));
                } else {
                    row.push(None);
                }
            }
            doc_ids.push(row);
        }

        Ok(Self::build_output(&emails, &doc_ids))
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "title": "imap_read",
            "description": "Read-only IMAP email retrieval (search + fetch text body + attachments).",
            "properties": {
                "host": {"type": "string", "default": "imap.gmail.com"},
                "port": {"type": "integer", "default": 993},
                "username": {"type": "string"},
                "password": {"type": "string", "description": "App password (use a secure_value)."},
                "mailbox": {"type": "string", "default": "INBOX"},
                "search": {"type": "object", "description": "Structured criteria: unseen, from, to, subject, body_contains, since, before."},
                "max_results": {"type": "integer", "default": 20},
                "body_max_bytes": {"type": "integer", "default": 5120},
                "download_attachments": {"type": "boolean", "default": false}
            },
            "required": ["username", "password"]
        })
    }

    fn description(&self) -> Option<&str> {
        Some("Read emails from an IMAP mailbox by structured search criteria; returns \
              headers, text body, and attachment metadata (downloadable on demand). \
              Read-only, does not mark messages as seen.")
    }
}
```

Implementa también el helper `register_attachment` y `imap_connect_login`:
- `register_attachment(&self, filename, mime, bytes) -> Result<String, String>`: usa `self.storage` (y/o `self.attachment_resolver`) para persistir los bytes y devolver un `document_id`. Sigue EXACTAMENTE el patrón que usa el nodo http / el pipeline de attachments para registrar bytes generados (lee cómo `image_generation`/`http` registran un artifact y reúsa ese camino; NO inventes un esquema de storage nuevo). Si la API de registro requiere más metadata, tómala de los parámetros disponibles.
- `imap_connect_login`: función `async` privada que hace TCP+TLS(rustls)+`async_imap::Client`+`login`, devolviendo la `Session`. Confirma la API 0.11.

> El `#[allow(dead_code)]` temporal de Task 3 (si lo agregaste) debe REMOVERSE aquí, ya que `storage`/`attachment_resolver` ahora se usan.

- [ ] **Step 3: Compilar (la red no se unit-testea; basta con que compile y pasen los tests puros)**

Run: `cargo build --lib 2>&1 | tail -20`
Expected: compila sin warnings (deny-warnings). Si `async-imap`/`mail-parser`/`tokio-rustls` exponen APIs distintas, ajusta el glue hasta compilar. NO toques los módulos puros (Tasks 1-2).

Run: `cargo test --lib imap`
Expected: los tests puros (imap_search, imap_mime, imap::tests) siguen verdes.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/Cargo.toml src/libs/colmena/src/dag_engine/infrastructure/nodes/imap.rs Cargo.lock
git commit -m "feat(imap): ImapNode execute (TLS connect, login, EXAMINE, search, fetch) + schema"
```

---

## Task 5: Registrar `imap_read` en el registry

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`

- [ ] **Step 1: Escribir un test de que el nodo está registrado**

Busca en `registry.rs` (o en sus tests) cómo se verifica que un `node_type` existe (probablemente `registry.get_node("http_request").is_some()` o similar). Agrega un test análogo:

```rust
#[test]
fn imap_read_node_is_registered() {
    let registry = NodeRegistry::new_with_secure_values(
        // usar el mismo constructor/los mismos args que el test existente más cercano,
        // p.ej. el que verifica http_request; copia su setup exacto.
    );
    assert!(registry.get_node("imap_read").is_some());
}
```
> Si no hay un test de registro al cual copiarle el setup, omite este test unitario y confía en la verificación de compilación + E2E; anótalo en el reporte.

- [ ] **Step 2: Registrar el nodo**

En `registry.rs`, junto al bloque de `http_request` (que inyecta storage/resolver), agregar:

```rust
            // --- Registrar Nodo IMAP ---
            let mut imap_node =
                crate::dag_engine::infrastructure::nodes::imap::ImapNode::new();
            if let Some(st) = storage.clone() {
                imap_node = imap_node.with_storage(st);
            }
            if let Some(resolver) = attachment_resolver.clone() {
                imap_node = imap_node.with_attachment_resolver(resolver);
            }
            nodes.insert("imap_read".to_string(), Arc::new(imap_node));
```

- [ ] **Step 3: Correr**

Run: `cargo test --lib imap_read_node_is_registered` (si lo agregaste) y `cargo build --lib`
Expected: PASS / compila limpio.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/registry.rs
git commit -m "feat(imap): register imap_read node with storage injection"
```

---

## Task 6: Grafo E2E + verificación contra Gmail real

**Files:**
- Create: `tests/graphs/external/imap_read_gmail.json`

- [ ] **Step 1: Crear el grafo**

Modela la estructura sobre un grafo existente con `llm_call` + `tool_configurations` + `node_schema` (lee `tests/graphs/agents/http_tool_node_schema_test.json`). Un `llm_call` (provider google, `gemini-2.5-flash`) con una tool `read_email` backed por `imap_read`, donde `host`/`port`/`username`/`password`/`mailbox` son `fixed` y solo `search`/`max_results` son LLM-visibles:

```json
{
  "nodes": {
    "agent": {
      "node_type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "system_message": "Eres un asistente que lee el correo del usuario. Usa read_email para buscar y resume lo que encuentres.",
        "tool_configurations": {
          "read_email": {
            "node_type": "imap_read",
            "description": "Lee correos del buzón del usuario por criterios de búsqueda.",
            "node_schema": {
              "host":     { "type": "string", "fixed": "imap.gmail.com" },
              "port":     { "type": "integer", "fixed": 993 },
              "username": { "type": "string", "fixed": "${GMAIL_USER}" },
              "password": { "type": "string", "fixed": "${GMAIL_APP_PASSWORD}" },
              "mailbox":  { "type": "string", "fixed": "INBOX" },
              "max_results": { "type": "integer", "fixed": 5 },
              "search": { "type": "object", "required": false,
                          "description": "Criterios: unseen, from, subject, since, before, body_contains" }
            }
          }
        }
      }
    }
  },
  "edges": []
}
```
Verifica `node_type` registrados: `grep -E '"(llm_call|imap_read)"' src/libs/colmena/src/dag_engine/infrastructure/registry.rs` (ambos deben aparecer). Valida JSON: `python3 -c "import json; json.load(open('tests/graphs/external/imap_read_gmail.json'))"`.

- [ ] **Step 2: Run en vivo (requiere app password real — NO commitear valores)**

```bash
set -a; source .env; set +a
export GMAIL_USER="tu-correo@gmail.com"
export GMAIL_APP_PASSWORD="xxxx xxxx xxxx xxxx"   # app password de Google (2FA activo)
mkdir -p /tmp/colmena_e2e
unset COLMENA_LOCAL
cargo run --bin dag_engine -- run tests/graphs/external/imap_read_gmail.json \
  --agent-session-id imap_e2e_001 2>&1 | tee /tmp/colmena_e2e/imap_read_gmail.sse
```
Expected: el agente llama `read_email`, recibe mensajes reales (status/contenido), y resume. En logs NO debe aparecer el app password. Presenta al usuario un reporte amigable (input, nº de correos, tokens, resumen) — sin pegar el SSE entero.

> Si no hay app password disponible al momento, marca este step como **pendiente** y deja el grafo committeado; el código se valida por compilación + los unit tests puros.

- [ ] **Step 3: Commit del grafo (sin secretos)**

```bash
git add tests/graphs/external/imap_read_gmail.json
git commit -m "test(imap): E2E graph reading Gmail via imap_read"
```

---

## Task 7: Documentación

**Files:**
- Create: `docs/developer_guide/50_imap_node.md`
- Modify: `docs/node_configurations.json`, `docs/node_as_tools_reference.json`, `docs/DEVELOPER_GUIDE.md`, `docs/CHANGELOG_2026-06.md`

- [ ] **Step 1: Dev guide `50_imap_node.md`**

Documentar (en español, estilo de los otros dev guides): qué hace `imap_read`, el esquema de config (§4 del spec), los criterios de búsqueda y su mapeo a SEARCH (§6), el comportamiento read-only (EXAMINE + BODY.PEEK), adjuntos (listar siempre + `download_attachments`), uso como tool con `node_schema+fixed` (password nunca visible al LLM), manejo de errores, y el setup operativo (§11: 2FA + app password; Workspace puede deshabilitarlo). Mencionar que enviar (SMTP) es un nodo futuro aparte.

- [ ] **Step 2: `node_configurations.json`**

Agregar la entrada del nodo `imap_read` con todos los campos de config (§4), matching el formato de las otras entradas. Valida JSON.

- [ ] **Step 3: `node_as_tools_reference.json`**

Agregar ejemplo de `imap_read` como tool: `host`/`port`/`username`/`password`/`mailbox` como `fixed`, `search` LLM-visible; nota de que el password nunca llega al LLM. Valida JSON.

- [ ] **Step 4: Índice + CHANGELOG**

En `DEVELOPER_GUIDE.md`, sección "8. Nodos", agregar: `- [**Nodo IMAP**](./developer_guide/50_imap_node.md) — `imap_read`: lectura read-only de correo por IMAP (app password), search estructurado, adjuntos.`

En `CHANGELOG_2026-06.md`, agregar una sección nueva (siguiente número disponible) "`imap_read` — lectura de correo IMAP" con Qué cambió / Por qué importa / Seguridad / Setup / Tests / Referencias / Estado, siguiendo el formato de las secciones existentes.

- [ ] **Step 5: Commit**

```bash
git add docs/developer_guide/50_imap_node.md docs/node_configurations.json \
        docs/node_as_tools_reference.json docs/DEVELOPER_GUIDE.md docs/CHANGELOG_2026-06.md
git commit -m "docs: imap_read node (dev guide, schemas, changelog, index)"
```

---

## Task 8: Verificación final pre-push

- [ ] **Step 1: Suite completa (paridad CI)**

Run: `cargo test --verbose 2>&1 | grep -E "^test result:|FAILED|panicked" | grep -vE "0 failed"`
Expected: vacío (sin fallos). Los tests `#[ignore]` (E2E real) no corren.

- [ ] **Step 2: Clippy + fmt**

Run: `cargo clippy --all-targets 2>&1 | tail -15 && cargo fmt --check`
Expected: sin warnings (deny-warnings), fmt limpio.

- [ ] **Step 3: Sweep ADP (aditivo)**

Confirmar que no se cambió ninguna firma pública existente; solo se agregó un nodo nuevo + deps. ADP worker no afectado.

---

## Self-Review (cobertura del spec)

- §2.1 read-only search+fetch → Tasks 1,4. ✅
- §2.2 IMAP genérico + defaults Gmail → Task 4 (config). ✅
- §2.3 auth app password (LOGIN) → Task 4. ✅
- §2.4 contenido (headers + texto + adjuntos listados/descargables) → Tasks 2,4. ✅
- §2.5 búsqueda estructurada → Task 1. ✅
- §3 arquitectura (3 archivos) → Tasks 1-4. ✅
- §4 esquema config → Task 4 (`schema()`) + Task 7 (docs). ✅
- §5 flujo no destructivo (EXAMINE + BODY.PEEK) → Task 4. ✅
- §6 mapeo SEARCH → Task 1. ✅
- §7 uso como tool (node_schema+fixed) → Task 6 (grafo) + Task 7 (docs). ✅
- §8 manejo de errores (login accionable, 0-resultados, parse skip, storage-faltante) → Task 4. ✅
- §9 testing (unit puros + E2E ignore) → Tasks 1,2,3 (unit) + Task 6 (E2E). ✅
- §10 compat ADP (aditivo, deps nuevas) → Task 8. ✅
- §11 setup operativo → Task 7 (docs). ✅
- §12 backlog (smtp_send, XOAUTH2, mutaciones, BODYSTRUCTURE, descarga por UID) → fuera de alcance, documentado en spec. ✅
