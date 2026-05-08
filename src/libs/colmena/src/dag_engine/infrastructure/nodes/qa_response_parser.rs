//! Shared parser for the canonical ID-keyed Q/A resume-answer format used by
//! all suspend-flavored nodes (`suspend`, `secure_suspend`).
//!
//! Format: `Q[<id>]: <text>\nA[<id>]: <text>` repeated, line-start anchored,
//! order-independent, ID character set `[A-Za-z0-9_-]{1,64}`.

use std::collections::HashMap;
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub enum QaParseError {
    InvalidIdSyntax { token: String },
    UnknownId { id: String },
    DuplicateId { id: String },
    MissingId { id: String },
    EmptyAnswer { id: String },
    OrphanQuestion { id: String },
}

impl fmt::Display for QaParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdSyntax { token } => {
                write!(f, "qa_response: invalid id syntax in '{token}'")
            }
            Self::UnknownId { id } => {
                write!(f, "qa_response: A[{id}] is not in the expected id set")
            }
            Self::DuplicateId { id } => {
                write!(f, "qa_response: A[{id}] appears more than once")
            }
            Self::MissingId { id } => {
                write!(f, "qa_response: missing answer for id '{id}'")
            }
            Self::EmptyAnswer { id } => {
                write!(f, "qa_response: empty answer for A[{id}]")
            }
            Self::OrphanQuestion { id } => {
                write!(f, "qa_response: Q[{id}] has no matching A[{id}]")
            }
        }
    }
}

impl std::error::Error for QaParseError {}

const ID_MAX_LEN: usize = 64;

fn is_valid_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn validate_id(id: &str) -> Result<(), QaParseError> {
    if id.is_empty() || id.len() > ID_MAX_LEN || !id.chars().all(is_valid_id_char) {
        return Err(QaParseError::InvalidIdSyntax {
            token: id.to_string(),
        });
    }
    Ok(())
}

/// Attempt to parse a `Q[id]:` or `A[id]:` prefix at `offset`.
/// Returns `(kind, id, position_after_colon)` on success.
fn parse_prefix_at(answer: &str, offset: usize) -> Option<(char, String, usize)> {
    let bytes = answer.as_bytes();
    if offset >= bytes.len() {
        return None;
    }
    let kind = bytes[offset] as char;
    if kind != 'Q' && kind != 'A' {
        return None;
    }
    if bytes.get(offset + 1) != Some(&b'[') {
        return None;
    }
    // Find the next ']' or '\n' after the '['. We accept up to the first ']'
    // to support the `bad space` error case (validate_id will reject it).
    let search_start = offset + 2;
    let rel_end = answer[search_start..].find(|c: char| c == ']' || c == '\n')?;
    let close_byte = answer.as_bytes()[search_start + rel_end];
    if close_byte != b']' {
        return None;
    }
    let id_end = search_start + rel_end;
    if bytes.get(id_end + 1) != Some(&b':') {
        return None;
    }
    let id = &answer[search_start..id_end];
    Some((kind, id.to_string(), id_end + 2))
}

pub fn parse_qa_response(
    answer: &str,
    expected_ids: &[&str],
) -> Result<HashMap<String, String>, QaParseError> {
    let mut answers: HashMap<String, String> = HashMap::new();
    let mut q_seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let bytes = answer.as_bytes();
    let mut line_start = 0usize;
    while line_start < bytes.len() {
        let line_end = answer[line_start..]
            .find('\n')
            .map(|n| line_start + n)
            .unwrap_or(bytes.len());

        if let Some((kind, id, after_colon)) = parse_prefix_at(answer, line_start) {
            validate_id(&id)?;

            if kind == 'Q' {
                q_seen.insert(id);
                line_start = if line_end < bytes.len() {
                    line_end + 1
                } else {
                    bytes.len()
                };
                continue;
            }

            // kind == 'A'
            let mut body_start = after_colon;
            if bytes.get(body_start) == Some(&b' ') {
                body_start += 1;
            }

            // Scan forward line by line for the next Q[ or A[ at line start.
            let mut scan = line_end;
            let body_end = loop {
                if scan >= bytes.len() {
                    break bytes.len();
                }
                // scan is positioned at a '\n' byte; the next line starts at scan+1.
                let s = scan + 1;
                if s >= bytes.len() {
                    break bytes.len();
                }
                if matches!(bytes[s] as char, 'Q' | 'A') && bytes.get(s + 1) == Some(&b'[') {
                    break s;
                }
                // Move scan to the next '\n' after position s.
                scan = answer[s..].find('\n').map(|n| s + n).unwrap_or(bytes.len());
            };

            let raw = &answer[body_start..body_end];
            let trimmed = raw.trim_end_matches('\n');
            if trimmed.trim().is_empty() {
                return Err(QaParseError::EmptyAnswer { id });
            }

            if !expected_ids.iter().any(|e| *e == id) {
                return Err(QaParseError::UnknownId { id });
            }
            if answers.contains_key(&id) {
                return Err(QaParseError::DuplicateId { id });
            }
            answers.insert(id, trimmed.to_string());

            line_start = body_end;
            continue;
        }

        line_start = if line_end < bytes.len() {
            line_end + 1
        } else {
            bytes.len()
        };
    }

    for id in &q_seen {
        if !answers.contains_key(id) && expected_ids.iter().any(|e| *e == id.as_str()) {
            return Err(QaParseError::OrphanQuestion { id: id.clone() });
        }
    }

    for expected in expected_ids {
        if !answers.contains_key(*expected) {
            return Err(QaParseError::MissingId {
                id: (*expected).to_string(),
            });
        }
    }

    Ok(answers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_id_pair() {
        let input = "Q[confirm]: Confirm?\nA[confirm]: yes";
        let out = parse_qa_response(input, &["confirm"]).unwrap();
        assert_eq!(out.get("confirm"), Some(&"yes".to_string()));
    }

    #[test]
    fn parses_multiple_ids_in_declared_order() {
        let input = "Q[user]: User?\nA[user]: alice\nQ[pass]: Pass?\nA[pass]: hunter2";
        let out = parse_qa_response(input, &["user", "pass"]).unwrap();
        assert_eq!(out.get("user"), Some(&"alice".to_string()));
        assert_eq!(out.get("pass"), Some(&"hunter2".to_string()));
    }

    #[test]
    fn parses_multiple_ids_in_reversed_order() {
        let input = "Q[pass]: P?\nA[pass]: hunter2\nQ[user]: U?\nA[user]: alice";
        let out = parse_qa_response(input, &["user", "pass"]).unwrap();
        assert_eq!(out.get("user"), Some(&"alice".to_string()));
        assert_eq!(out.get("pass"), Some(&"hunter2".to_string()));
    }

    #[test]
    fn preserves_internal_newlines_in_answer() {
        let input =
            "Q[k]: PEM?\nA[k]: -----BEGIN-----\nMIIEvQ\n-----END-----\nQ[fp]: FP?\nA[fp]: ab:cd";
        let out = parse_qa_response(input, &["k", "fp"]).unwrap();
        assert_eq!(
            out.get("k").unwrap(),
            "-----BEGIN-----\nMIIEvQ\n-----END-----"
        );
        assert_eq!(out.get("fp").unwrap(), "ab:cd");
    }

    #[test]
    fn tolerates_no_space_after_colon() {
        let input = "Q[x]:Confirm?\nA[x]:yes";
        let out = parse_qa_response(input, &["x"]).unwrap();
        assert_eq!(out.get("x"), Some(&"yes".to_string()));
    }

    #[test]
    fn does_not_validate_question_text_matches() {
        let input = "Q[x]: anything goes here\nA[x]: payload";
        let out = parse_qa_response(input, &["x"]).unwrap();
        assert_eq!(out.get("x"), Some(&"payload".to_string()));
    }

    #[test]
    fn errors_on_invalid_id_syntax() {
        let input = "Q[bad space]: hi\nA[bad space]: x";
        let err = parse_qa_response(input, &["bad space"]).unwrap_err();
        assert!(matches!(err, QaParseError::InvalidIdSyntax { .. }));
    }

    #[test]
    fn errors_on_unknown_id() {
        let input = "Q[wrong]: hi\nA[wrong]: x";
        let err = parse_qa_response(input, &["right"]).unwrap_err();
        assert!(matches!(err, QaParseError::UnknownId { .. }));
    }

    #[test]
    fn errors_on_duplicate_id() {
        let input = "Q[x]: hi\nA[x]: one\nQ[x]: hi\nA[x]: two";
        let err = parse_qa_response(input, &["x"]).unwrap_err();
        assert!(matches!(err, QaParseError::DuplicateId { .. }));
    }

    #[test]
    fn errors_on_missing_id() {
        let input = "Q[a]: hi\nA[a]: x";
        let err = parse_qa_response(input, &["a", "b"]).unwrap_err();
        match err {
            QaParseError::MissingId { id } => assert_eq!(id, "b"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn errors_on_empty_answer() {
        let input = "Q[x]: hi\nA[x]: \nQ[y]: hi\nA[y]: ok";
        let err = parse_qa_response(input, &["x", "y"]).unwrap_err();
        assert!(matches!(err, QaParseError::EmptyAnswer { .. }));
    }

    #[test]
    fn errors_on_orphan_q_without_a() {
        let input = "Q[x]: hi";
        let err = parse_qa_response(input, &["x"]).unwrap_err();
        assert!(matches!(
            err,
            QaParseError::OrphanQuestion { .. } | QaParseError::MissingId { .. }
        ));
    }
}
