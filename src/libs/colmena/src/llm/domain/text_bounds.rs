//! Pure, dependency-free text truncation helpers shared across the LLM
//! module. Extracted from [`DagToolExecutor`] so that every caller that has
//! to bound untrusted or oversized text uses the SAME primitive and emits the
//! SAME marker, instead of each growing its own variant.
//!
//! [`DagToolExecutor`]: crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor

/// Reserve, in bytes, for the truncation marker appended by [`head_truncate`].
/// The marker is short ASCII text; 96 bytes comfortably covers the largest
/// realistic byte counts.
pub(crate) const TRUNCATION_MARKER_RESERVE: usize = 96;

/// Truncate an oversized string by KEEPING ITS HEAD (the first
/// `max_string_bytes - marker` bytes, snapped to a UTF-8 char boundary) and
/// appending a `[truncated: showing first N of M bytes]` marker — instead of
/// discarding the content entirely. For a markdown table or any text whose
/// useful part is the beginning (headers, first rows), this preserves a
/// usable preview rather than handing the model a content-free placeholder.
///
/// The result is always `<= max_string_bytes` (the head budget already
/// reserves room for the marker).
pub(crate) fn head_truncate(s: &str, max_string_bytes: usize) -> String {
    let original_len = s.len();
    let head_budget = max_string_bytes.saturating_sub(TRUNCATION_MARKER_RESERVE);
    let mut end = head_budget.min(original_len);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[truncated: showing first {} of {} bytes]",
        &s[..end],
        end,
        original_len
    )
}

#[cfg(test)]
mod tests {
    use super::{head_truncate, TRUNCATION_MARKER_RESERVE};

    /// Approval test: proves the moved function reproduces the exact marker
    /// format `DagToolExecutor::head_truncate` produced before the move —
    /// this is what `dag_tool_executor.rs:4761,:4770` continue to assert
    /// against via the one-line delegation.
    #[test]
    fn head_truncate_moved_fn_matches_existing_marker() {
        let s = "A".repeat(10_000);
        let out = head_truncate(&s, 1_000);
        assert!(out.starts_with("AAAA"));
        assert!(out.len() <= 1_000);
        assert!(out.contains("[truncated: showing first"));
        assert!(out.contains("of 10000 bytes"));
    }

    #[test]
    fn head_truncate_respects_utf8_char_boundary() {
        // Multi-byte chars (é = 2 bytes) — cutting at a raw byte index could
        // split one; head_truncate must snap to a char boundary.
        let s = "é".repeat(5_000); // 10_000 bytes
        let out = head_truncate(&s, 1_001);
        // Valid UTF-8 (would have panicked on a bad slice) and under budget.
        assert!(out.len() <= 1_001);
        assert!(out.contains("[truncated: showing first"));
    }

    #[test]
    fn short_string_still_gets_marker_when_over_budget() {
        let s = "hello world";
        let out = head_truncate(s, 5 + TRUNCATION_MARKER_RESERVE);
        assert!(out.starts_with("hello"));
        assert!(out.contains("of 11 bytes"));
    }
}
