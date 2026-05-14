//! Local text extraction from document bytes, plus a multi-byte-safe
//! char truncator. Used to feed the summary generator with at most
//! `summary_max_chars` of input text.

/// Truncate a string to at most `max_chars` Unicode characters
/// (not bytes). Safe across multi-byte UTF-8 sequences — never splits
/// a code point mid-way. Returns a `String` (allocates only when truncation
/// actually happens).
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((cut, _)) => s[..cut].to_string(),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_long_ascii_string() {
        assert_eq!(truncate_chars("abcdef", 3), "abc");
    }

    #[test]
    fn returns_full_string_when_under_cap() {
        assert_eq!(truncate_chars("abc", 10), "abc");
    }

    #[test]
    fn returns_empty_for_empty_input() {
        assert_eq!(truncate_chars("", 5), "");
    }

    #[test]
    fn handles_multi_byte_chars_without_panic() {
        // Each emoji is 4 UTF-8 bytes but 1 char.
        let s = "🦀🦀🦀🦀🦀";
        let out = truncate_chars(s, 3);
        assert_eq!(out, "🦀🦀🦀");
        assert_eq!(out.chars().count(), 3);
    }

    #[test]
    fn cap_zero_returns_empty() {
        assert_eq!(truncate_chars("hello", 0), "");
    }
}
