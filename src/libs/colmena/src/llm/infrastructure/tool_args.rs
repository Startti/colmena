//! Shared helper for serializing LLM tool-call arguments defensively.
//!
//! Provider adapters turn the arguments a model chose for a tool call into a
//! JSON string before handing them to the tool executor. Those arguments are
//! normally a `serde_json::Value`, which is effectively infallible to
//! serialize — but the adapters historically used
//! `serde_json::to_string(..).unwrap_or_default()`, whose fallback is an EMPTY
//! string (`""`). An empty string is not a valid JSON object, so if
//! serialization ever *did* fail (e.g. after a refactor changed the argument
//! type), the tool executor would silently receive garbage instead of a clear
//! signal — finding #7 in the code-audit ledger.

use serde::Serialize;

/// Serialize tool-call arguments to a JSON string, defensively.
///
/// On success returns the serialized JSON. On the (near-impossible for
/// `serde_json::Value`) error path it emits a `tracing::warn!` naming the tool
/// and returns `"{}"` — a valid empty JSON object the downstream tool executor
/// can parse — never an empty string.
///
/// # Example
/// ```ignore
/// let args = serde_json::json!({ "city": "Bogota" });
/// let s = serialize_tool_args(&args, "get_weather");
/// assert_eq!(s, r#"{"city":"Bogota"}"#);
/// ```
pub(crate) fn serialize_tool_args<T: Serialize>(args: &T, tool_name: &str) -> String {
    match serde_json::to_string(args) {
        Ok(json) => json,
        Err(error) => {
            tracing::warn!(
                tool = tool_name,
                error = %error,
                "failed to serialize tool-call arguments; falling back to empty JSON object",
            );
            "{}".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn valid_args_round_trip() {
        let v = serde_json::json!({ "a": 1, "b": "x" });
        assert_eq!(serialize_tool_args(&v, "my_tool"), r#"{"a":1,"b":"x"}"#);
    }

    #[test]
    fn unserializable_args_fall_back_to_empty_object_not_empty_string() {
        // A JSON object key must be a string (or something string-convertible
        // like an integer). A *tuple* key is not convertible, so
        // `serde_json::to_string` returns `Err` here — an input shape that
        // reliably forces the fallback path (unlike integer keys, which serde
        // happily stringifies).
        let mut bad: BTreeMap<(i32, i32), i32> = BTreeMap::new();
        bad.insert((1, 2), 3);

        let s = serialize_tool_args(&bad, "my_tool");

        // The historical `.unwrap_or_default()` returned "" (invalid JSON).
        // The fix must return a valid empty JSON object instead.
        assert_eq!(s, "{}");
    }
}
