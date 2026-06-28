//! `imap_read` node: read-only IMAP email retrieval. Connects over TLS, logs in
//! with an app password, EXAMINEs the mailbox (read-only), searches by structured
//! criteria, fetches matching messages with BODY.PEEK (never marks seen), and
//! returns headers + text body + attachment metadata. Optionally downloads
//! attachment bytes and registers them as Colmena attachments.

use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::infrastructure::nodes::imap_mime::{parse_email, ParsedEmail};
use crate::dag_engine::infrastructure::nodes::imap_search::{build_search_command, SearchCriteria};
use futures::StreamExt;
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;

#[derive(Default)]
pub struct ImapNode {
    storage: Option<Arc<dyn crate::storage::domain::OutputStorageRepository>>,
    attachment_resolver: Option<Arc<dyn crate::llm::domain::attachments::AttachmentStreamResolver>>,
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
            let val =
                std::env::var(var).map_err(|_| format!("imap_read: env var '{var}' not set"))?;
            out.replace_range(start..=end, &val);
        }
        Ok(out)
    }

    /// Shape parsed emails (+ optional attachment document_ids) into the node output.
    /// `doc_ids[i][j]` aligns with `emails[i].attachments[j]`; `None` = not downloaded.
    /// Pure — unit-testable.
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

    /// Read a string field (inputs > config), resolving `${ENV}` placeholders.
    fn resolve_str(
        inputs: &NodeInputs,
        config: &Value,
        key: &str,
    ) -> Result<Option<String>, String> {
        match Self::get_str(inputs, config, key) {
            Some(raw) => Ok(Some(Self::resolve_env_vars(raw)?)),
            None => Ok(None),
        }
    }

    /// Read an integer field (inputs > config). Accepts JSON numbers and numeric
    /// strings (so `${ENV}`-style overrides work). Returns `None` if absent.
    fn resolve_u64(inputs: &NodeInputs, config: &Value, key: &str) -> Result<Option<u64>, String> {
        let v = inputs.get(key).or_else(|| config.get(key));
        match v {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(n)) => n
                .as_u64()
                .map(Some)
                .ok_or_else(|| format!("imap_read: '{key}' must be a non-negative integer")),
            Some(Value::String(s)) => {
                let resolved = Self::resolve_env_vars(s)?;
                resolved
                    .trim()
                    .parse::<u64>()
                    .map(Some)
                    .map_err(|_| format!("imap_read: '{key}' must be an integer, got '{resolved}'"))
            }
            Some(other) => Err(format!(
                "imap_read: '{key}' must be an integer, got {other}"
            )),
        }
    }

    /// Read a boolean field (inputs > config). Accepts JSON bools and the
    /// strings "true"/"false". Returns `None` if absent.
    fn resolve_bool(
        inputs: &NodeInputs,
        config: &Value,
        key: &str,
    ) -> Result<Option<bool>, String> {
        let v = inputs.get(key).or_else(|| config.get(key));
        match v {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
                "true" => Ok(Some(true)),
                "false" => Ok(Some(false)),
                _ => Err(format!("imap_read: '{key}' must be a boolean, got '{s}'")),
            },
            Some(other) => Err(format!("imap_read: '{key}' must be a boolean, got {other}")),
        }
    }

    /// Build a rustls-based `TlsConnector` trusting the Mozilla webpki root set
    /// (same trust anchors the project's tungstenite/reqwest clients use).
    fn tls_connector() -> tokio_rustls::TlsConnector {
        let mut roots = tokio_rustls::rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = tokio_rustls::rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        tokio_rustls::TlsConnector::from(Arc::new(config))
    }

    /// Register downloaded attachment bytes as a Colmena attachment, returning a
    /// `document_id`.
    ///
    /// NOT YET WIRED. Producing a real `document_id` requires the
    /// `AttachmentRegistry.upsert(...)` path (see `tts.rs` / `image_generation.rs`)
    /// plus an `agent_session_id` — neither of which is threaded into this node's
    /// struct (it only carries `storage` + `attachment_resolver`). Wiring that is
    /// its own follow-up task; until then `download_attachments=true` is rejected
    /// up front in `execute`, so this helper is unreachable.
    #[allow(dead_code)]
    async fn register_attachment(
        &self,
        _filename: &str,
        _mime: &str,
        _bytes: &[u8],
    ) -> Result<String, String> {
        Err("imap_read: attachment download not yet implemented".to_string())
    }
}

#[async_trait::async_trait]
impl ExecutableNode for ImapNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // --- Resolve config (inputs > config, with ${ENV} expansion) ---
        let host = Self::resolve_str(inputs, config, "host")?
            .unwrap_or_else(|| "imap.gmail.com".to_string());
        let port = Self::resolve_u64(inputs, config, "port")?.unwrap_or(993);
        let port = u16::try_from(port)
            .map_err(|_| "imap_read: 'port' out of range (expected 1..=65535)".to_string())?;
        let username = Self::resolve_str(inputs, config, "username")?
            .ok_or_else(|| "imap_read: 'username' is required".to_string())?;
        let password = Self::resolve_str(inputs, config, "password")?
            .ok_or_else(|| "imap_read: 'password' is required".to_string())?;
        let mailbox =
            Self::resolve_str(inputs, config, "mailbox")?.unwrap_or_else(|| "INBOX".to_string());
        let max_results = Self::resolve_u64(inputs, config, "max_results")?.unwrap_or(20) as usize;
        let body_max_bytes =
            Self::resolve_u64(inputs, config, "body_max_bytes")?.unwrap_or(5120) as usize;
        let download_attachments =
            Self::resolve_bool(inputs, config, "download_attachments")?.unwrap_or(false);

        // Structured search → IMAP SEARCH command.
        let criteria: SearchCriteria = match inputs.get("search").or_else(|| config.get("search")) {
            Some(v) if !v.is_null() => serde_json::from_value(v.clone())
                .map_err(|e| format!("imap_read: invalid 'search' criteria: {e}"))?,
            _ => SearchCriteria::default(),
        };
        let search_cmd = build_search_command(&criteria)?;

        // Step 1: attachment download requires a working storage backend AND the
        // registry/session plumbing (not yet on this node) — reject early.
        if download_attachments {
            if self.storage.is_none() {
                return Err("imap_read: download_attachments requires storage configured".into());
            }
            return Err("imap_read: attachment download not yet implemented".into());
        }

        // Step 2: TCP + rustls TLS, build Client, login.
        let stream = tokio::net::TcpStream::connect((host.as_str(), port))
            .await
            .map_err(|e| Self::connect_error(&host, port, &e))?;
        let server_name = tokio_rustls::rustls::pki_types::ServerName::try_from(host.clone())
            .map_err(|e| format!("imap_read: invalid host name '{host}': {e}"))?;
        let tls_stream = Self::tls_connector()
            .connect(server_name, stream)
            .await
            .map_err(|e| Self::connect_error(&host, port, &e))?;

        let mut client = async_imap::Client::new(tls_stream);
        // Consume the server greeting before authenticating.
        client
            .read_response()
            .await
            .map_err(|e| Self::connect_error(&host, port, &e))?
            .ok_or_else(|| Self::connect_error(&host, port, &"no server greeting"))?;

        let mut session = client
            .login(&username, &password)
            .await
            .map_err(|(e, _client)| Self::connect_error(&host, port, &e))?;

        // Step 3: EXAMINE (read-only — never mutates \Seen or other flags).
        session
            .examine(&mailbox)
            .await
            .map_err(|e| format!("imap_read: cannot open mailbox '{mailbox}': {e}"))?;

        // Step 4: UID SEARCH → most-recent `max_results` UIDs.
        let uids = session
            .uid_search(&search_cmd)
            .await
            .map_err(|e| format!("imap_read: search failed ('{search_cmd}'): {e}"))?;
        let mut uids: Vec<u32> = uids.into_iter().collect();
        uids.sort_unstable();
        let selected: Vec<u32> = uids.into_iter().rev().take(max_results).collect();

        // Step 5: fetch each message read-only (BODY.PEEK[] never sets \Seen).
        let mut emails: Vec<ParsedEmail> = Vec::with_capacity(selected.len());
        for uid in &selected {
            let mut stream = session
                .uid_fetch(uid.to_string(), "BODY.PEEK[]")
                .await
                .map_err(|e| format!("imap_read: fetch failed for uid {uid}: {e}"))?;
            if let Some(item) = stream.next().await {
                let fetch =
                    item.map_err(|e| format!("imap_read: fetch failed for uid {uid}: {e}"))?;
                if let Some(body) = fetch.body() {
                    // Skip unparseable messages — don't fail the whole batch.
                    if let Ok(parsed) = parse_email(body, body_max_bytes) {
                        emails.push(parsed);
                    }
                }
            }
            // Drain the rest of the stream so the session is ready for the next
            // command (the borrow of `session` ends when `stream` is dropped).
            while stream.next().await.is_some() {}
        }

        // Step 6: logout (best-effort).
        let _ = session.logout().await;

        // Step 7: no attachment download in this path → all doc_ids are None.
        let doc_ids: Vec<Vec<Option<String>>> = emails
            .iter()
            .map(|e| vec![None; e.attachments.len()])
            .collect();

        // Step 8: shape output.
        Ok(Self::build_output(&emails, &doc_ids))
    }

    fn schema(&self) -> Value {
        json!({
            "inputs": {
                "search": "object (optional) — structured search criteria"
            },
            "outputs": { "output": "object — { count, messages: [...] }" },
            "config": {
                "host": "string (optional, default imap.gmail.com) — supports ${ENV_VAR}",
                "port": "integer (optional, default 993)",
                "username": "string (required) — supports ${ENV_VAR}",
                "password": "string (required) — app password; supports ${ENV_VAR}",
                "mailbox": "string (optional, default INBOX)",
                "search": "object (optional) — { unseen, from, to, subject, body_contains, since, before }",
                "max_results": "integer (optional, default 20) — most-recent N messages",
                "body_max_bytes": "integer (optional, default 5120) — text body truncation cap",
                "download_attachments": "boolean (optional, default false) — NOT yet implemented"
            },
            "required": ["username", "password"]
        })
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Read emails from an IMAP mailbox by structured search criteria; returns headers, \
             text body, and attachment metadata (downloadable on demand). Read-only, does not \
             mark messages as seen.",
        )
    }

    fn default_output(&self) -> Option<&str> {
        Some("output")
    }
}

impl ImapNode {
    /// Build the actionable connection/auth error message.
    fn connect_error(host: &str, port: u16, detail: &dyn std::fmt::Display) -> String {
        format!(
            "imap_read: authentication/connection failed for {host}:{port} — verify the app \
             password and that 2-Step Verification is enabled (Workspace admins may disable \
             IMAP/app-passwords). Detail: {detail}"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::infrastructure::nodes::imap_mime::{AttachmentInfo, ParsedEmail};

    fn sample(att: bool) -> ParsedEmail {
        ParsedEmail {
            from: "a@x.com".into(),
            to: "b@x.com".into(),
            subject: "s".into(),
            date: "2026-06-01T10:00:00+00:00".into(),
            body_text: "hi".into(),
            body_truncated: false,
            attachments: if att {
                vec![AttachmentInfo {
                    filename: "f.pdf".into(),
                    mime: "application/pdf".into(),
                    size: 3,
                    bytes: vec![1, 2, 3],
                }]
            } else {
                vec![]
            },
        }
    }

    #[test]
    fn build_output_without_attachments() {
        let out = ImapNode::build_output(&[sample(false)], &[vec![]]);
        assert_eq!(out["output"]["count"], 1);
        assert_eq!(out["output"]["messages"][0]["subject"], "s");
        assert_eq!(
            out["output"]["messages"][0]["attachments"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
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
        std::env::set_var("IMAP_TEST_VAR_T3", "secret");
        assert_eq!(
            ImapNode::resolve_env_vars("${IMAP_TEST_VAR_T3}").unwrap(),
            "secret"
        );
        std::env::remove_var("IMAP_TEST_VAR_T3");
    }
}
