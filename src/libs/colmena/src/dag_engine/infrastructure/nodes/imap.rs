//! `imap_read` node: read-only IMAP email retrieval. Connects over TLS, logs in
//! with an app password, EXAMINEs the mailbox (read-only), searches by structured
//! criteria, fetches matching messages with BODY.PEEK (never marks seen), and
//! returns headers + text body + attachment metadata. Optionally downloads
//! attachment bytes and registers them as Colmena attachments.

use crate::dag_engine::domain::node::NodeInputs;
use crate::dag_engine::infrastructure::nodes::imap_mime::ParsedEmail;
use serde_json::{json, Value};
use std::sync::Arc;

// TODO(Task 4): remove this `#[allow(dead_code)]` once the `ExecutableNode`
// trait impl (`execute`/`schema`) consumes the fields and helpers below.
#[allow(dead_code)]
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
    // TODO(Task 4): remove `#[allow(dead_code)]` once `execute` uses this helper.
    #[allow(dead_code)]
    fn get_str<'a>(inputs: &'a NodeInputs, config: &'a Value, key: &str) -> Option<&'a str> {
        inputs
            .get(key)
            .and_then(|v| v.as_str())
            .or_else(|| config.get(key).and_then(|v| v.as_str()))
    }

    /// Resolve `${ENV_VAR}` occurrences in a string. Mirrors the per-node helper
    /// used by http.rs/socketio.rs.
    // TODO(Task 4): remove `#[allow(dead_code)]` once `execute` uses this helper.
    #[allow(dead_code)]
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
    // TODO(Task 4): remove `#[allow(dead_code)]` once `execute` calls this in
    // non-test builds (currently only exercised by the unit tests).
    #[allow(dead_code)]
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
