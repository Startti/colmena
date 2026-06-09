//! Shared JSON builders for `Location` and `Range` objects used in
//! every batchUpdate request emitted by the use cases. Crucially:
//! `tabId` is included WHENEVER set, omitted otherwise.
//!
//! Without `tabId`, Google Docs interprets `startIndex`/`endIndex`
//! against the FIRST tab — so writes on a multi-tab doc silently
//! land in the wrong tab. Every emission site must use these helpers.

use crate::gdocs::domain::TabId;
use serde_json::{json, Map, Value};

/// Build `{ index, tabId? }`.
pub fn location(index: u32, tab_id: &Option<TabId>) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), json!(index));
    if let Some(t) = tab_id {
        m.insert("tabId".into(), json!(t.0));
    }
    Value::Object(m)
}

/// Build `{ startIndex, endIndex, tabId? }`.
pub fn range(start: u32, end: u32, tab_id: &Option<TabId>) -> Value {
    let mut m = Map::new();
    m.insert("startIndex".into(), json!(start));
    m.insert("endIndex".into(), json!(end));
    if let Some(t) = tab_id {
        m.insert("tabId".into(), json!(t.0));
    }
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_omits_tab_id_when_none() {
        let v = location(5, &None);
        assert_eq!(v, json!({ "index": 5 }));
    }

    #[test]
    fn location_includes_tab_id_when_set() {
        let v = location(5, &Some(TabId("t.abc".into())));
        assert_eq!(v, json!({ "index": 5, "tabId": "t.abc" }));
    }

    #[test]
    fn range_omits_tab_id_when_none() {
        let v = range(1, 10, &None);
        assert_eq!(v, json!({ "startIndex": 1, "endIndex": 10 }));
    }

    #[test]
    fn range_includes_tab_id_when_set() {
        let v = range(1, 10, &Some(TabId("t.xyz".into())));
        assert_eq!(
            v,
            json!({ "startIndex": 1, "endIndex": 10, "tabId": "t.xyz" })
        );
    }
}
