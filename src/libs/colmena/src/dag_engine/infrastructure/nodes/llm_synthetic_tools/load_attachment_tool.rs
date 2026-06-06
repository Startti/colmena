//! The `load_attachment` synthetic tool. Returns a sentinel ToolResult that
//! AgentService intercepts to inject a synthetic `user` message carrying the
//! file. The tool definition embeds the per-session catalog in its description.

use crate::llm::domain::tools::{ParameterProperty, ToolDefinition, ToolParameters};
use crate::llm::domain::ConversationAttachment;
use crate::llm::domain::{LlmError, ToolCall, ToolResult};
use serde_json::json;
use std::collections::HashMap;

pub const LOAD_ATTACHMENT_TOOL_NAME: &str = "load_attachment";

/// Auto-injected system prelude when at least one attachment is registered and
/// `attachments_enabled` is true on the node. Tells the model that attachments
/// exist in this conversation and that it should use `load_attachment` proactively
/// when the user references uploaded content. Graph authors should NOT have to
/// duplicate these instructions in their own `system_message`.
pub const ATTACHMENTS_SYSTEM_PRELUDE: &str = "## Conversation Attachments\n\
This conversation has one or more documents attached to it. They are listed in \
the catalog below (and in the description of the `load_attachment` tool), each \
with a `document_id`, label, mime type, and size.\n\n\
You will NOT see document content automatically — the catalog only advertises \
which documents exist. To read a document's content, you must call \
load_attachment(document_id). To forward a document to a downstream tool (for \
example `http_request` multipart) without reading it yourself, pass the string \
\"$attachment:<document_id>\" in that tool's args.\n\n\
load_attachment results are ephemeral: the document content is available only \
for the turn in which you invoked the tool. Future turns will see a marker \
confirming the call happened, but not the content itself. Call load_attachment \
again if you need to re-read the document.\n\n\
Rules:\n\
- If the user asks about any uploaded document, call `load_attachment` with the \
matching `document_id` before answering — never guess at the contents.\n\
- Do not list, paraphrase, or summarise the attachments unless the user asks.\n\
- One `document_id` per call. Call the tool again if you need a second document.\n\
- If the user's question does not depend on any attachment, answer normally — \
do NOT call `load_attachment` preemptively.";

/// Build the `ToolDefinition` for `load_attachment`. The catalog is a snapshot
/// taken at the start of `llm_call.execute`. The caller is responsible for
/// passing only the entries that belong to the current provider.
///
/// When the catalog is empty, callers should NOT register this tool (mirrors
/// the load_skill pattern). This function still accepts an empty slice for
/// defensive use, producing a description that says no attachments are
/// available — but the recommended path is to skip the call.
pub fn build_load_attachment_tool_definition(catalog: &[ConversationAttachment]) -> ToolDefinition {
    let lines: Vec<String> = catalog
        .iter()
        .map(|a| format!("- {}", a.catalog_line()))
        .collect();
    let body = if lines.is_empty() {
        "No attachments are currently available in this conversation.".to_string()
    } else {
        format!("Available attachments:\n{}", lines.join("\n"))
    };

    let description = format!(
        "Load a document that has been attached to this conversation. Use this when you need to inspect the contents of a previously uploaded file. Each load attempt is a separate call; pass exactly one document_id per call.\n\n{}",
        body
    );

    let enum_values: Vec<String> = catalog.iter().map(|a| a.document_id.clone()).collect();

    let mut properties: HashMap<String, ParameterProperty> = HashMap::new();
    properties.insert(
        "document_id".to_string(),
        if enum_values.is_empty() {
            ParameterProperty::new(
                "string".to_string(),
                "Exact id from the available-attachments list above.".to_string(),
            )
        } else {
            ParameterProperty::new(
                "string".to_string(),
                "Exact id from the available-attachments list above.".to_string(),
            )
            .with_enum(enum_values)
        },
    );

    ToolDefinition {
        name: LOAD_ATTACHMENT_TOOL_NAME.to_string(),
        description,
        summary: Some("Materialize a registered attachment's content (with auto-summary for large files) into the conversation".to_string()),
        parameters: ToolParameters {
            schema_type: "object".to_string(),
            properties,
            required: vec!["document_id".to_string()],
        },
        input_schema_override: None,
    }
}

/// Dispatch a `load_attachment` tool call. The returned `ToolResult` carries
/// either the LOAD_ATTACHMENT sentinel (when the document_id is in the
/// catalog) or an `unknown_document_id` error JSON (recoverable by the LLM).
pub fn dispatch_load_attachment(
    tool_call: &ToolCall,
    catalog: &[ConversationAttachment],
) -> Result<ToolResult, LlmError> {
    let args: serde_json::Value =
        serde_json::from_str(&tool_call.function.arguments).map_err(|e| {
            LlmError::InvalidToolCall {
                reason: format!("load_attachment: invalid arguments JSON: {}", e),
            }
        })?;

    let document_id = args
        .get("document_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| LlmError::InvalidToolCall {
            reason: "load_attachment: missing required parameter 'document_id'".to_string(),
        })?;

    let known = catalog.iter().any(|a| a.document_id == document_id);
    if !known {
        let err = json!({
            "error": "unknown_document_id",
            "document_id": document_id,
            "hint": "Check the available-attachments list in the tool description."
        });
        return Ok(ToolResult {
            tool_call_id: tool_call.id.clone(),
            output: err.to_string(),
            success: false,
            error: None,
        });
    }

    let sentinel = json!({
        "__colmena_status": "LOAD_ATTACHMENT",
        "document_id": document_id
    });
    Ok(ToolResult {
        tool_call_id: tool_call.id.clone(),
        output: sentinel.to_string(),
        success: true,
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::attachments::AttachmentSource;
    use crate::llm::domain::tools::FunctionCall;
    use crate::llm::domain::ProviderKind;
    use chrono::Utc;

    fn mk_attachment(id: &str, label: &str) -> ConversationAttachment {
        ConversationAttachment {
            agent_session_id: "s1".to_string(),
            document_id: id.to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: format!("{}.pdf", id),
            size_bytes: Some(1024),
            label: Some(label.to_string()),
            description: None,
            source: AttachmentSource::SignedUrl("u".to_string()),
            registered_at: Utc::now(),
            refreshed_at: Utc::now(),
            storage_key: None,
            origin: None,
            last_used_at: None,
        }
    }

    fn mk_call(args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: LOAD_ATTACHMENT_TOOL_NAME.to_string(),
                arguments: args.to_string(),
            },
            response: None,
        }
    }

    #[test]
    fn tool_definition_lists_each_attachment() {
        let cat = vec![mk_attachment("doc-1", "A"), mk_attachment("doc-2", "B")];
        let td = build_load_attachment_tool_definition(&cat);
        assert!(td.description.contains("doc-1"));
        assert!(td.description.contains("A"));
        assert!(td.description.contains("doc-2"));
    }

    #[test]
    fn tool_definition_enum_contains_every_id() {
        let cat = vec![mk_attachment("a", "A"), mk_attachment("b", "B")];
        let td = build_load_attachment_tool_definition(&cat);
        let enum_values = td
            .parameters
            .properties
            .get("document_id")
            .unwrap()
            .enum_values
            .clone()
            .unwrap();
        assert!(enum_values.contains(&"a".to_string()));
        assert!(enum_values.contains(&"b".to_string()));
    }

    #[test]
    fn tool_definition_empty_catalog_renders_no_attachments_message() {
        let td = build_load_attachment_tool_definition(&[]);
        assert!(td
            .description
            .contains("No attachments are currently available"));
    }

    #[test]
    fn dispatch_known_id_returns_sentinel() {
        let cat = vec![mk_attachment("doc-1", "A")];
        let call = mk_call(json!({"document_id": "doc-1"}));
        let r = dispatch_load_attachment(&call, &cat).unwrap();
        assert!(r.success);
        let parsed: serde_json::Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(parsed["__colmena_status"], "LOAD_ATTACHMENT");
        assert_eq!(parsed["document_id"], "doc-1");
    }

    #[test]
    fn dispatch_unknown_id_returns_error_json() {
        let cat = vec![mk_attachment("doc-1", "A")];
        let call = mk_call(json!({"document_id": "missing"}));
        let r = dispatch_load_attachment(&call, &cat).unwrap();
        assert!(!r.success);
        let parsed: serde_json::Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(parsed["error"], "unknown_document_id");
    }

    #[test]
    fn dispatch_missing_document_id_is_invalid_tool_call() {
        let cat = vec![mk_attachment("doc-1", "A")];
        let call = mk_call(json!({}));
        let err = dispatch_load_attachment(&call, &cat).unwrap_err();
        assert!(matches!(err, LlmError::InvalidToolCall { .. }));
    }
}

#[cfg(test)]
mod prelude_tests {
    use super::*;

    #[test]
    fn prelude_explains_no_autoinject_behavior() {
        assert!(
            ATTACHMENTS_SYSTEM_PRELUDE.contains("call load_attachment")
                || ATTACHMENTS_SYSTEM_PRELUDE.contains("load_attachment("),
            "prelude should instruct the model to call load_attachment"
        );
    }

    #[test]
    fn prelude_explains_ephemeral_load_attachment() {
        assert!(
            ATTACHMENTS_SYSTEM_PRELUDE.contains("ephemeral")
                || ATTACHMENTS_SYSTEM_PRELUDE.contains("only for this turn")
                || ATTACHMENTS_SYSTEM_PRELUDE.contains("not retained")
                || ATTACHMENTS_SYSTEM_PRELUDE.contains("turn only"),
            "prelude should warn that load_attachment results are ephemeral"
        );
    }
}
