//! The tool-result body the model gets when `load_attachment` fails. A refusal to
//! read a large file has its own code; every other failure keeps the code and the
//! text it always had.

use crate::llm::domain::large_tabular::refusal_code_for;

/// Body for a failed `load_attachment` of `document_id`. A refusal gets its own
/// code; every other failure is the exact string it always was.
pub(crate) fn load_failure_body(document_id: &str, error: &str) -> String {
    if let Some(code) = refusal_code_for(error) {
        return serde_json::json!({
            "error": code,
            "document_id": document_id,
            "reason": error,
        })
        .to_string();
    }
    format!(
        "{{\"error\":\"attachment_expired_unrecoverable\",\"document_id\":\"{}\",\"reason\":\"{}\"}}",
        document_id,
        error.replace('"', "'")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::large_tabular::{refusal_text_for, LARGE_TABULAR_ERROR_CODE};

    fn code(error: &str) -> String {
        let body: serde_json::Value = serde_json::from_str(&load_failure_body("d", error)).unwrap();
        assert_eq!(body["document_id"], "d");
        body["error"].as_str().unwrap().to_string()
    }

    #[test]
    fn only_a_refusal_has_its_own_code() {
        for available in [false, true] {
            assert_eq!(code(refusal_text_for(available)), LARGE_TABULAR_ERROR_CODE);
        }
    }

    /// Byte for byte what develop produces: the literals are copied from it.
    #[test]
    fn every_other_failure_is_the_exact_string_it_always_was() {
        assert_eq!(
            load_failure_body("d", "db down"),
            r#"{"error":"attachment_expired_unrecoverable","document_id":"d","reason":"db down"}"#
        );
        assert_eq!(
            load_failure_body("doc-1", "lookup failed: said \"x\""),
            r#"{"error":"attachment_expired_unrecoverable","document_id":"doc-1","reason":"lookup failed: said 'x'"}"#
        );
    }
}
