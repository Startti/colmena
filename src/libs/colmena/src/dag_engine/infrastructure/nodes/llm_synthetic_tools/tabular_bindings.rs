//! Polymorphic tabular-binding types for the `data_run_python` synthetic
//! LLM tool. A [`DataBinding`] describes ONE Python global to bind inside
//! the pandas sandbox, sourced from exactly one of four origins:
//! attachment (CSV/XLSX), Google Sheets, SQL `SELECT`, or inline JSON.
//!
//! This module owns pure types + validation only — no I/O, no async.
//! Fetching/dispatching per source lives in later tasks.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

// ── Binding type ─────────────────────────────────────────────────────

/// One tabular binding: a Python variable name plus exactly one data
/// source. The active source is determined structurally by
/// [`classify_binding`] — no explicit `source` discriminator field is
/// required from the LLM.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DataBinding {
    /// Python variable name to bind the records to (e.g. "products").
    /// Accepts UX aliases `binding_name` and `name`.
    #[serde(alias = "binding_name", alias = "name")]
    pub var: String,

    /// Attachment document id (CSV/XLSX source).
    #[serde(default)]
    pub attachment_id: Option<String>,

    /// Google Sheets spreadsheet id (gsheets source).
    #[serde(default)]
    pub spreadsheet_id: Option<String>,

    /// Google Sheets tab name (gsheets source). NOT aliased to
    /// `sheet_name` — that field is reserved for the attachment-XLSX tab
    /// selector below; the two sources are structurally distinct.
    #[serde(default)]
    pub sheet: Option<String>,

    /// Optional A1 range (gsheets source only). Omit to read the entire
    /// tab.
    #[serde(default)]
    pub range: Option<String>,

    /// SQL `SELECT` statement (sql source).
    #[serde(default)]
    pub query: Option<String>,

    /// Inline table data — an array of `{col: val}` objects, or a 2-D
    /// array whose first row is the header (inline source).
    #[serde(default)]
    pub data: Option<Value>,

    /// CSV delimiter override (attachment source, CSV only).
    #[serde(default)]
    pub delimiter: Option<String>,

    /// XLSX tab/sheet selector (attachment source, XLSX only). Distinct
    /// from `sheet` (gsheets tab) — do not conflate the two.
    #[serde(default)]
    pub sheet_name: Option<String>,

    /// Header row index override, 0-based (attachment source).
    #[serde(default)]
    pub header_row: Option<u32>,
}

/// The structural source a [`DataBinding`] resolves to.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum BindingKind {
    Attachment,
    Gsheets,
    Sql,
    Inline,
}

/// Classify a binding by inspecting which discriminator fields are
/// present. Exactly one of the four sources must be present:
/// `attachment_id`, `spreadsheet_id` + `sheet`, `query`, or `data`.
///
/// `spreadsheet_id` and `sheet` count as a single discriminator (the
/// gsheets source) — supplying only one of the two is treated as
/// "gsheets source present" for ambiguity-counting purposes, so it still
/// surfaces as an error when combined with another source, but the
/// error message doesn't distinguish "incomplete gsheets" from
/// "ambiguous"; that's fine for a structural classifier.
pub fn classify_binding(b: &DataBinding) -> Result<BindingKind, String> {
    let has_attachment = b.attachment_id.is_some();
    let has_gsheets = b.spreadsheet_id.is_some() || b.sheet.is_some();
    let has_sql = b.query.is_some();
    let has_inline = b.data.is_some();

    let present_count =
        [has_attachment, has_gsheets, has_sql, has_inline]
            .iter()
            .filter(|x| **x)
            .count();

    match present_count {
        1 => {
            if has_attachment {
                Ok(BindingKind::Attachment)
            } else if has_gsheets {
                Ok(BindingKind::Gsheets)
            } else if has_sql {
                Ok(BindingKind::Sql)
            } else {
                Ok(BindingKind::Inline)
            }
        }
        _ => Err(format!(
            "binding '{}' must have exactly one source: attachment_id | (spreadsheet_id+sheet) | query | data",
            b.var
        )),
    }
}

/// Validate a full list of bindings:
/// - non-empty
/// - every `var` is non-empty after trimming
/// - no duplicate `var`s
/// - every binding classifies to exactly one source
///
/// Returns a structured `invalid_args` JSON envelope on error, suitable
/// for returning directly as a tool error result.
pub fn validate_bindings(bindings: &[DataBinding]) -> Result<(), Value> {
    let invalid = |message: String| {
        Err(serde_json::json!({
            "error": "invalid_args",
            "message": message,
        }))
    };

    if bindings.is_empty() {
        return invalid("`bindings` must contain at least one entry".to_string());
    }

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for b in bindings {
        let trimmed = b.var.trim();
        if trimmed.is_empty() {
            return invalid("every binding must have a non-empty `var` name".to_string());
        }
        if !seen.insert(trimmed.to_string()) {
            return invalid(format!("duplicate binding var '{}'", trimmed));
        }
        if let Err(e) = classify_binding(b) {
            return invalid(e);
        }
    }

    Ok(())
}

// ── Flexible bindings deserializer ───────────────────────────────────

/// Custom deserializer for `bindings` that accepts either:
///
/// 1. The canonical array form:
///    `[{"var":"products","query":"SELECT * FROM products"}, ...]`
///
/// 2. An object whose VALUES are binding-shaped objects (treat the key
///    as `var`):
///    `{"products": {"query":"SELECT * FROM products"}, ...}`
///
/// LLMs frequently emit form 2 despite the schema declaring
/// `Vec<DataBinding>`. This deserializer tolerates both shapes. Form 3
/// (values are bare strings) is rejected with a clear error — a bare
/// string cannot disambiguate which of the four sources is meant.
///
/// Ported from `gsheets_run_python.rs::deserialize_bindings_flexible`
/// and generalized to build [`DataBinding`] instead of `GsheetsBinding`.
pub fn deserialize_bindings_flexible<'de, D>(deserializer: D) -> Result<Vec<DataBinding>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, MapAccess, SeqAccess, Visitor};
    use std::fmt;

    struct BindingsVisitor;

    impl<'de> Visitor<'de> for BindingsVisitor {
        type Value = Vec<DataBinding>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an array of bindings or an object whose values are bindings")
        }

        // Canonical array form
        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut out: Vec<DataBinding> = Vec::new();
            while let Some(elem) = seq.next_element::<DataBinding>()? {
                out.push(elem);
            }
            Ok(out)
        }

        // LLM-hallucinated dict form
        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut out: Vec<DataBinding> = Vec::new();
            while let Some((key, value)) = map.next_entry::<String, Value>()? {
                match value {
                    // Form 2: value is binding-shaped object. Merge in `var` from key.
                    Value::Object(mut obj) => {
                        obj.insert("var".to_string(), Value::String(key.clone()));
                        let binding: DataBinding = serde_json::from_value(Value::Object(obj))
                            .map_err(de::Error::custom)?;
                        out.push(binding);
                    }
                    // Form 3: value is just a string. Ambiguous — reject.
                    Value::String(_) => {
                        return Err(de::Error::custom(format!(
                            "`bindings` entry '{key}' is a bare string; \
                             cannot determine its data source. Use the array form: \
                             [{{\"var\":\"{key}\",\"query\":\"...\"}}] (or attachment_id / \
                             spreadsheet_id+sheet / data)"
                        )));
                    }
                    _ => {
                        return Err(de::Error::custom(format!(
                            "`bindings` entry '{key}' has unexpected value type; \
                             expected an object with one of: attachment_id, \
                             spreadsheet_id+sheet, query, data"
                        )));
                    }
                }
            }
            Ok(out)
        }
    }

    deserializer.deserialize_any(BindingsVisitor)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn b(v: Value) -> DataBinding {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn classify_recognizes_each_source() {
        assert_eq!(
            classify_binding(&b(json!({"var":"a","attachment_id":"doc1"}))).unwrap(),
            BindingKind::Attachment
        );
        assert_eq!(
            classify_binding(&b(json!({"var":"a","spreadsheet_id":"1x","sheet":"Q4"}))).unwrap(),
            BindingKind::Gsheets
        );
        assert_eq!(
            classify_binding(&b(json!({"var":"a","query":"SELECT 1"}))).unwrap(),
            BindingKind::Sql
        );
        assert_eq!(
            classify_binding(&b(json!({"var":"a","data":[]}))).unwrap(),
            BindingKind::Inline
        );
    }

    #[test]
    fn classify_rejects_ambiguous_and_empty() {
        assert!(classify_binding(&b(
            json!({"var":"a","attachment_id":"d","query":"SELECT 1"})
        ))
        .is_err());
        assert!(classify_binding(&b(json!({"var":"a"}))).is_err());
    }

    #[test]
    fn validate_rejects_duplicate_and_empty_var() {
        let dup = vec![
            b(json!({"var":"x","query":"SELECT 1"})),
            b(json!({"var":"x","data":[]})),
        ];
        assert!(validate_bindings(&dup).is_err());
        let empty = vec![b(json!({"var":"","data":[]}))];
        assert!(validate_bindings(&empty).is_err());
    }
}
