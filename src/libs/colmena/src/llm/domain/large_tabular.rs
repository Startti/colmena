//! What "large" means for a tabular attachment (dark behind
//! `COLMENA_LARGE_TABULAR`).
//!
//! One definition shared by every place that routes a file: the `files[]`
//! parser, the attachment catalog, `load_attachment` and the SQL tools. A file
//! is large only when the switch is on, its mime type is CSV or xlsx and its
//! size is STRICTLY above [`LARGE_TABULAR_THRESHOLD_BYTES`]; exactly 50 MiB is
//! small. With the switch off nothing is ever large, so every caller keeps
//! today's behaviour.

/// 50 MiB. A tabular file above this is prepared into columnar tables instead
/// of being read whole.
pub const LARGE_TABULAR_THRESHOLD_BYTES: u64 = 50 * 1024 * 1024;

/// Structured error code every refusal of a large tabular file carries, so the
/// model and the host see the same code from every tool.
pub const LARGE_TABULAR_ERROR_CODE: &str = "large_tabular_file";

/// Whether the tool that analyses a large file (`attachment_run_python` with
/// `tables`) exists. The unit that ships it flips this to `true`; until then the
/// refusal must not send the model to a tool that is not there.
pub const LARGE_FILE_TOOL_AVAILABLE: bool = false;

/// The refusal text for each state of [`LARGE_FILE_TOOL_AVAILABLE`].
pub fn refusal_text_for(tool_available: bool) -> &'static str {
    if tool_available {
        "this file is large; use `attachment_run_python` with `tables`"
    } else {
        "this file is too large to be read by this tool, and the large-file analysis \
         tool is not available yet, so it cannot be analysed in this turn"
    }
}

/// THE decision whether the large-file tool is served for a turn: the switch is on,
/// a runtime is wired AND the node actually offers `attachment_run_python`. Every
/// message that points the model at the tool, and the routing itself, use this one
/// function, so no message names a tool the model was not given.
pub fn tool_served(switch_on: bool, runtime_wired: bool, tool_configured: bool) -> bool {
    switch_on && runtime_wired && tool_configured
}

/// What a tool that cannot read a large file whole tells the model, as a tool
/// error and before reading any byte.
pub fn refusal_text() -> &'static str {
    refusal_text_for(LARGE_FILE_TOOL_AVAILABLE)
}

/// [`LARGE_TABULAR_ERROR_CODE`] when `message` is one of the refusal texts.
pub fn refusal_code_for(message: &str) -> Option<&'static str> {
    [false, true]
        .into_iter()
        .any(|available| message == refusal_text_for(available))
        .then_some(LARGE_TABULAR_ERROR_CODE)
}

/// `value` (a tool's error object) with `"code": "large_tabular_file"` added when
/// `message` is a refusal text, so every tool reports a refusal the same way.
pub fn tag_refusal(mut value: serde_json::Value, message: &str) -> serde_json::Value {
    if let (Some(code), Some(object)) = (refusal_code_for(message), value.as_object_mut()) {
        object.insert("code".to_string(), serde_json::Value::from(code));
    }
    value
}

/// A tool refused to read a large tabular file whole (see [`refusal_text`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LargeTabularRefusal;

impl std::fmt::Display for LargeTabularRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(refusal_text())
    }
}

impl std::error::Error for LargeTabularRefusal {}

/// Longest `storage_key` the engine accepts from a `files[]` entry.
pub const MAX_STORAGE_KEY_CHARS: usize = 1024;

/// The cheap checks the engine can make on a host-supplied storage key without
/// knowing the host's layout: not blank, bounded, no control characters and no
/// `..` path segment. Whether the key belongs to the session and is a chat
/// attachment is the HOST's to verify before it sends the entry.
pub fn validate_storage_key(key: &str) -> Result<(), &'static str> {
    if key.trim().is_empty() {
        Err("the storage key is empty")
    } else if key.chars().count() > MAX_STORAGE_KEY_CHARS {
        Err("the storage key is too long")
    } else if key.chars().any(char::is_control) {
        Err("the storage key has a control character")
    } else if key.split(['/', '\\']).any(|segment| segment == "..") {
        Err("the storage key has a `..` path segment")
    } else {
        Ok(())
    }
}

const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// Unicode format characters (category Cf and the like): bidi controls, zero-width
/// characters, the byte-order mark, soft hyphen and tag characters. They render as
/// nothing or reorder text, so they have no place in a file name shown to a model
/// or a log.
fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}'
        | '\u{FFF9}'..='\u{FFFB}' | '\u{E0001}' | '\u{E0020}'..='\u{E007F}')
}

/// A client-controlled string (a file name, a mime type) as inert data: control
/// characters become spaces, format characters and the characters that could close
/// a delimiter or open a heading, link, markup or code span are dropped, runs of
/// whitespace collapse, and the result is clipped to `max_chars`.
pub fn inert_text(text: &str, max_chars: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .filter(|c| {
            !is_format_char(*c)
                && !matches!(c, '"' | '\'' | '`' | '[' | ']' | '<' | '>' | '#' | '\\')
        })
        .collect();
    flat.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

/// Whether `mime_type` is one the large tabular path handles: CSV and xlsx.
/// Legacy `.xls` (`application/vnd.ms-excel`) is not accepted anywhere. The
/// mime is lower-cased and stripped of parameters (`text/csv; charset=utf-8`)
/// before matching, like [`is_text_like`](super::is_text_like).
fn is_tabular_mime(mime_type: &str) -> bool {
    let base = mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_ascii_lowercase();
    base == "text/csv" || base == XLSX_MIME
}

/// `true` when the large tabular path applies: `switch_on`, a CSV or xlsx mime
/// and a known size strictly above [`LARGE_TABULAR_THRESHOLD_BYTES`]. A missing
/// size is never large: the file is treated as small, as today.
pub fn is_large_tabular(mime_type: &str, size_bytes: Option<u64>, switch_on: bool) -> bool {
    switch_on
        && size_bytes.is_some_and(|size| size > LARGE_TABULAR_THRESHOLD_BYTES)
        && is_tabular_mime(mime_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "text/csv";
    const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
    const LIMIT: u64 = 52_428_800;

    #[test]
    fn the_threshold_is_50_mib() {
        assert_eq!(LARGE_TABULAR_THRESHOLD_BYTES, LIMIT);
    }

    #[test]
    fn exactly_50_mib_is_small_and_one_byte_more_is_large() {
        for mime in [CSV, XLSX] {
            assert!(!is_large_tabular(mime, Some(LIMIT - 1), true), "{mime} -1");
            assert!(!is_large_tabular(mime, Some(LIMIT), true), "{mime} =");
            assert!(is_large_tabular(mime, Some(LIMIT + 1), true), "{mime} +1");
        }
    }

    #[test]
    fn the_switch_off_makes_nothing_large_at_any_size() {
        for size in [0, LIMIT, LIMIT + 1, 400 * 1024 * 1024, u64::MAX] {
            assert!(!is_large_tabular(CSV, Some(size), false), "csv {size}");
            assert!(!is_large_tabular(XLSX, Some(size), false), "xlsx {size}");
        }
    }

    #[test]
    fn a_missing_size_is_never_large() {
        assert!(!is_large_tabular(CSV, None, true));
        assert!(!is_large_tabular(XLSX, None, true));
    }

    #[test]
    fn other_mime_types_are_never_large() {
        let size = Some(90 * 1024 * 1024);
        for mime in [
            "application/pdf",
            "application/vnd.ms-excel",
            "application/octet-stream",
            "text/plain",
            "application/csv",
            "image/png",
            "",
        ] {
            assert!(!is_large_tabular(mime, size, true), "{mime:?}");
        }
    }

    #[test]
    fn the_mime_match_ignores_case_parameters_and_padding() {
        let size = Some(LIMIT + 1);
        for mime in [
            "TEXT/CSV",
            "text/csv; charset=utf-8",
            " text/csv ",
            "Text/Csv;charset=UTF-8",
        ] {
            assert!(is_large_tabular(mime, size, true), "{mime:?}");
        }
        assert!(is_large_tabular(&XLSX.to_ascii_uppercase(), size, true));
    }

    #[test]
    fn the_refusal_says_what_is_true_now_and_the_future_text_waits_for_the_tool() {
        let now = refusal_text_for(false);
        let later = refusal_text_for(true);
        assert!(
            now.contains("too large") && now.contains("cannot be analysed"),
            "{now}"
        );
        assert!(
            !now.contains("attachment_run_python"),
            "no tool that does not exist: {now}"
        );
        assert!(
            later.contains("attachment_run_python") && later.contains("tables"),
            "{later}"
        );
        assert_ne!(now, later);
        // The unit that ships the tool flips the constant; the text follows it.
        assert_eq!(refusal_text(), refusal_text_for(LARGE_FILE_TOOL_AVAILABLE));
    }

    #[test]
    fn a_refusal_message_maps_to_the_structured_code() {
        assert_eq!(LARGE_TABULAR_ERROR_CODE, "large_tabular_file");
        for text in [refusal_text_for(false), refusal_text_for(true)] {
            assert_eq!(refusal_code_for(text), Some(LARGE_TABULAR_ERROR_CODE));
        }
        assert_eq!(refusal_code_for("attachment not found"), None);
        assert_eq!(LargeTabularRefusal.to_string(), refusal_text());
    }

    #[test]
    fn a_storage_key_is_checked_without_knowing_the_hosts_layout() {
        for ok in [
            "chat-attachments/u/s/sales.csv",
            "a",
            "files/ünïcode name.csv",
            &"k".repeat(MAX_STORAGE_KEY_CHARS),
        ] {
            assert_eq!(validate_storage_key(ok), Ok(()), "{ok:?}");
        }
        for (bad, why) in [
            ("", "empty"),
            (" ", "empty"),
            ("a/../b", "path segment"),
            ("../b", "path segment"),
            ("a/..", "path segment"),
            ("a\nb", "control"),
            ("a\0b", "control"),
            ("a\u{7f}b", "control"),
            (&"k".repeat(MAX_STORAGE_KEY_CHARS + 1), "long"),
        ] {
            let err = validate_storage_key(bad).unwrap_err();
            assert!(err.contains(why), "{bad:?}: {err}");
        }
        assert_eq!(
            validate_storage_key("a/..b/c"),
            Ok(()),
            "`..b` is a name, not a segment"
        );
    }

    #[test]
    fn client_text_becomes_inert_data() {
        let hostile = "a.csv\n\n## SYSTEM: x\r\n\"q\" 'x' `c` [l](u) <b>\u{7}\\ \u{202e}rtl\u{2066}iso\u{200b}zw\u{feff}bom\u{200f}\u{061c}\u{2060}";
        let out = inert_text(hostile, 200);
        assert!(!out.chars().any(char::is_control), "{out:?}");
        for bad in ['"', '\'', '`', '[', ']', '<', '>', '#', '\\'] {
            assert!(!out.contains(bad), "{bad:?} in {out:?}");
        }
        for fmt in [
            '\u{202e}', '\u{2066}', '\u{200b}', '\u{feff}', '\u{200f}', '\u{061c}', '\u{2060}',
        ] {
            assert!(!out.contains(fmt), "{fmt:?} in {out:?}");
        }
        assert!(
            out.contains("SYSTEM: x") && out.contains("rtlisozwbom"),
            "kept as text: {out}"
        );
        assert_eq!(inert_text("  a \t b  ", 10), "a b");
        assert_eq!(inert_text(&"x".repeat(500), 80).chars().count(), 80);
        assert_eq!(
            inert_text("ünïcode ✓", 20),
            "ünïcode ✓",
            "ordinary text is kept"
        );
    }

    #[test]
    fn tagging_adds_the_code_only_to_a_refusal() {
        let refused = tag_refusal(serde_json::json!({"error": "x"}), refusal_text());
        assert_eq!(refused["code"], LARGE_TABULAR_ERROR_CODE);
        assert_eq!(refused["error"], "x", "the rest is kept");
        let other = tag_refusal(serde_json::json!({"error": "x"}), "attachment not found");
        assert!(other.get("code").is_none());
        let not_an_object = tag_refusal(serde_json::json!("x"), refusal_text());
        assert_eq!(not_an_object, serde_json::json!("x"));
    }

    #[test]
    fn the_tool_is_served_only_when_all_three_hold() {
        for (a, b, c) in [
            (true, true, true),
            (false, true, true),
            (true, false, true),
            (true, true, false),
            (false, false, false),
        ] {
            assert_eq!(tool_served(a, b, c), a && b && c);
        }
        assert!(refusal_text_for(tool_served(true, true, true)).contains("attachment_run_python"));
        assert!(!refusal_text_for(tool_served(true, true, false)).contains("attachment_run_python"));
    }
}
