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
    Ok(d.format("%d-%b-%Y").to_string())
}

/// Escape a string for use inside an IMAP quoted string (RFC 3501 §4.3).
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
