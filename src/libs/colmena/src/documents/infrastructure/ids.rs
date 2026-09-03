use crate::documents::domain::IdGenerator;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Crockford base32 in lowercase — the alphabet `ulid` renders, minus `i`, `l`,
/// `o` and `u`. The sequence suffix is encoded with it so an id stays in a single
/// charset end to end.
const CROCKFORD_LOWER: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Monotonic for the lifetime of the process. Mixed into every id so that two ids
/// minted in the same millisecond differ regardless of how the random bits fall.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
pub struct UlidIdGenerator;

impl UlidIdGenerator {
    /// Builds a 22-character id body: 10 chars of ULID timestamp, 8 chars of ULID
    /// randomness (40 bits) and 4 chars of process-local sequence.
    ///
    /// The timestamp keeps ids roughly sortable, the random bits separate ids
    /// minted by different processes in the same millisecond, and the sequence
    /// removes same-process collisions: two ids can only repeat once the sequence
    /// wraps, which takes 2^20 ids inside one millisecond.
    ///
    /// The previous body was `ulid[..12]`, which kept the full timestamp and only
    /// 2 random chars — 1024 distinct values per millisecond. That collided in
    /// practice: 32 ids minted back to back collided 38% of the time.
    fn short_ulid() -> String {
        let ulid = ulid::Ulid::new().to_string().to_ascii_lowercase();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut id = String::with_capacity(22);
        id.push_str(&ulid[..18]);
        for group in (0..4).rev() {
            let bits = (sequence >> (group * 5)) & 0b1_1111;
            id.push(CROCKFORD_LOWER[bits as usize] as char);
        }
        id
    }
}

impl IdGenerator for UlidIdGenerator {
    fn new_artifact_id(&self) -> String {
        format!("art_{}", Self::short_ulid())
    }
    fn new_sheet_id(&self) -> String {
        format!("sheet_{}", Self::short_ulid())
    }
    fn new_table_id(&self) -> String {
        format!("tbl_{}", Self::short_ulid())
    }
    fn new_block_id(&self) -> String {
        format!("blk_{}", Self::short_ulid())
    }
    fn new_run_id(&self) -> String {
        format!("run_{}", Self::short_ulid())
    }
    fn new_row_id(&self) -> String {
        format!("row_{}", Self::short_ulid())
    }
    fn new_list_item_id(&self) -> String {
        format!("li_{}", Self::short_ulid())
    }
    fn new_slide_id(&self) -> String {
        format!("sl_{}", Self::short_ulid())
    }
    fn new_asset_id(&self) -> String {
        format!("asset_{}", Self::short_ulid())
    }
}

/// Deterministic counter-based generator for tests. Each category has its own counter.
pub struct CountingIdGenerator {
    counters: Mutex<[u64; 9]>,
}

impl Default for CountingIdGenerator {
    fn default() -> Self {
        Self {
            counters: Mutex::new([0; 9]),
        }
    }
}

impl CountingIdGenerator {
    fn next(&self, idx: usize) -> u64 {
        let mut g = self.counters.lock().unwrap();
        g[idx] += 1;
        g[idx]
    }
}

impl IdGenerator for CountingIdGenerator {
    fn new_artifact_id(&self) -> String {
        format!("art_{:02}", self.next(0))
    }
    fn new_sheet_id(&self) -> String {
        format!("sheet_{:02}", self.next(1))
    }
    fn new_table_id(&self) -> String {
        format!("tbl_{:02}", self.next(2))
    }
    fn new_block_id(&self) -> String {
        format!("blk_{:02}", self.next(3))
    }
    fn new_run_id(&self) -> String {
        format!("run_{:02}", self.next(4))
    }
    fn new_row_id(&self) -> String {
        format!("row_{:02}", self.next(5))
    }
    fn new_list_item_id(&self) -> String {
        format!("li_{:02}", self.next(6))
    }
    fn new_slide_id(&self) -> String {
        format!("sl_{:02}", self.next(7))
    }
    fn new_asset_id(&self) -> String {
        format!("asset_{:02}", self.next(8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Regression for the `[..12]` body, which kept only 2 random chars and made
    /// ids minted inside one millisecond collide (38% over 32 ids). The HTML
    /// end-to-end test failed on CI with "duplicate block id (across all slides)"
    /// because a document builds every block id in one tight burst.
    #[test]
    fn ulid_generator_ids_are_unique_in_a_tight_loop() {
        let generator = UlidIdGenerator;
        let count = 20_000;
        let ids: HashSet<String> = (0..count).map(|_| generator.new_block_id()).collect();
        assert_eq!(
            ids.len(),
            count,
            "{} of {} ids collided",
            count - ids.len(),
            count
        );
    }

    /// `IdGenerator` is `Send + Sync`, so the sequence has to hold across threads
    /// minting concurrently, not just inside one loop.
    #[test]
    fn ulid_generator_ids_are_unique_across_threads() {
        let per_thread = 2_000;
        let threads = 8;
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                std::thread::spawn(move || {
                    let generator = UlidIdGenerator;
                    (0..per_thread)
                        .map(|_| generator.new_block_id())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let ids: HashSet<String> = handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(
            ids.len(),
            threads * per_thread,
            "ids collided across threads"
        );
    }

    #[test]
    fn ulid_generator_prefixes_correctly() {
        let g = UlidIdGenerator;
        assert!(g.new_artifact_id().starts_with("art_"));
        assert!(g.new_sheet_id().starts_with("sheet_"));
        assert_ne!(g.new_artifact_id(), g.new_artifact_id());
    }

    #[test]
    fn counting_generator_is_deterministic() {
        let g = CountingIdGenerator::default();
        assert_eq!(g.new_artifact_id(), "art_01");
        assert_eq!(g.new_artifact_id(), "art_02");
        assert_eq!(g.new_sheet_id(), "sheet_01");
    }

    #[test]
    fn ulid_generator_new_slide_and_asset() {
        let g = UlidIdGenerator;
        assert!(g.new_slide_id().starts_with("sl_"));
        assert!(g.new_asset_id().starts_with("asset_"));
        assert_ne!(g.new_slide_id(), g.new_slide_id());
    }

    #[test]
    fn counting_generator_new_slide_and_asset() {
        let g = CountingIdGenerator::default();
        assert_eq!(g.new_slide_id(), "sl_01");
        assert_eq!(g.new_slide_id(), "sl_02");
        assert_eq!(g.new_asset_id(), "asset_01");
    }
}
