# src/libs/colmena/src/documents/infrastructure/ids.rs

**Layer:** infrastructure  
**Purpose:** Provides two concrete implementations of the `IdGenerator` trait: a production ULID-based generator and a deterministic counting-based generator for testing.

## Symbols

- `UlidIdGenerator` (struct, pub) — Default-constructible generator producing prefixed ULID-based IDs for documents
- `UlidIdGenerator::short_ulid()` (fn, private) — Generates a 12-character lowercase ULID string by truncating the full ULID
- `impl IdGenerator for UlidIdGenerator::new_artifact_id()` (fn) — Returns `art_` prefixed ULID-based ID
- `impl IdGenerator for UlidIdGenerator::new_sheet_id()` (fn) — Returns `sheet_` prefixed ULID-based ID
- `impl IdGenerator for UlidIdGenerator::new_table_id()` (fn) — Returns `tbl_` prefixed ULID-based ID
- `impl IdGenerator for UlidIdGenerator::new_block_id()` (fn) — Returns `blk_` prefixed ULID-based ID
- `impl IdGenerator for UlidIdGenerator::new_run_id()` (fn) — Returns `run_` prefixed ULID-based ID
- `impl IdGenerator for UlidIdGenerator::new_row_id()` (fn) — Returns `row_` prefixed ULID-based ID
- `impl IdGenerator for UlidIdGenerator::new_list_item_id()` (fn) — Returns `li_` prefixed ULID-based ID
- `impl IdGenerator for UlidIdGenerator::new_slide_id()` (fn) — Returns `sl_` prefixed ULID-based ID
- `impl IdGenerator for UlidIdGenerator::new_asset_id()` (fn) — Returns `asset_` prefixed ULID-based ID
- `CountingIdGenerator` (struct, pub) — Test-only deterministic counter-based generator with independent counters for each of 9 ID types
- `impl Default for CountingIdGenerator` (impl) — Initializes all 9 counters to zero
- `CountingIdGenerator::next()` (fn, private) — Atomically increments and returns counter at given index; used to generate sequential IDs
- `impl IdGenerator for CountingIdGenerator::new_artifact_id()` (fn) — Returns zero-padded counting ID for artifacts
- `impl IdGenerator for CountingIdGenerator::new_sheet_id()` (fn) — Returns zero-padded counting ID for sheets
- `impl IdGenerator for CountingIdGenerator::new_table_id()` (fn) — Returns zero-padded counting ID for tables
- `impl IdGenerator for CountingIdGenerator::new_block_id()` (fn) — Returns zero-padded counting ID for blocks
- `impl IdGenerator for CountingIdGenerator::new_run_id()` (fn) — Returns zero-padded counting ID for runs
- `impl IdGenerator for CountingIdGenerator::new_row_id()` (fn) — Returns zero-padded counting ID for rows
- `impl IdGenerator for CountingIdGenerator::new_list_item_id()` (fn) — Returns zero-padded counting ID for list items
- `impl IdGenerator for CountingIdGenerator::new_slide_id()` (fn) — Returns zero-padded counting ID for slides
- `impl IdGenerator for CountingIdGenerator::new_asset_id()` (fn) — Returns zero-padded counting ID for assets
- `tests::ulid_generator_prefixes_correctly` (test) — Verifies UlidIdGenerator produces correct prefixes and non-deterministic values
- `tests::counting_generator_is_deterministic` (test) — Verifies CountingIdGenerator produces expected sequential prefixed values
- `tests::ulid_generator_new_slide_and_asset` (test) — Verifies UlidIdGenerator handles slide and asset ID generation
- `tests::counting_generator_new_slide_and_asset` (test) — Verifies CountingIdGenerator handles slide and asset ID generation deterministically

## File-level notes

- Clean adapter implementation: domain trait (`IdGenerator`) has two infrastructure implementations following hexagonal architecture discipline.
- Both implementations are complete — all 9 trait methods defined in both adapters.
- Test coverage includes both generator types with determinism and prefix verification.
- No external dependencies beyond `ulid` crate (for production) and Rust stdlib.
- `CountingIdGenerator::next()` uses `Mutex::lock().unwrap()` — acceptable for test-only code but relies on mutex not being poisoned.
- ULID string slicing `ulid[..12]` is safe (ULIDs always produce 26 characters); no bounds panic risk.
