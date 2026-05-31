use crate::documents::domain::IdGenerator;
use std::sync::Mutex;

#[derive(Default)]
pub struct UlidIdGenerator;

impl UlidIdGenerator {
    fn short_ulid() -> String {
        let ulid = ulid::Ulid::new().to_string();
        ulid[..12].to_ascii_lowercase()
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
