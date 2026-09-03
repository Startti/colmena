# src/libs/colmena/src/documents/infrastructure/ids.rs

**Layer:** infrastructure  
**Purpose:** Provides two concrete implementations of the `IdGenerator` trait: a production ULID-based generator and a deterministic counting-based generator for testing.

## Symbols

- `UlidIdGenerator` (struct, pub) — Default-constructible generator producing prefixed ULID-based IDs for documents
- `UlidIdGenerator::short_ulid()` (fn, private) — Generates a 22-character lowercase id body: 10 chars of ULID timestamp, 8 chars of ULID randomness (40 bits) and 4 chars of process-local sequence
- `CROCKFORD_LOWER` (const, private) — Lowercase Crockford base32 alphabet used to encode the sequence suffix in the same charset the ULID renders
- `SEQUENCE` (static, private) — Process-local `AtomicU64` mixed into every id so ids minted in the same millisecond cannot repeat
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
- `tests::ulid_generator_ids_are_unique_in_a_tight_loop` (test) — Mints 20 000 ids back to back and asserts every one is distinct; regression for the truncated-ULID collision
- `tests::ulid_generator_ids_are_unique_across_threads` (test) — Mints 16 000 ids from 8 concurrent threads and asserts every one is distinct

## File-level notes

- Clean adapter implementation: domain trait (`IdGenerator`) has two infrastructure implementations following hexagonal architecture discipline.
- Both implementations are complete — all 9 trait methods defined in both adapters.
- Test coverage includes both generator types with determinism and prefix verification.
- No external dependencies beyond `ulid` crate (for production) and Rust stdlib.
- `CountingIdGenerator::next()` uses `Mutex::lock().unwrap()` — acceptable for test-only code but relies on mutex not being poisoned.
- ULID string slicing `ulid[..18]` is safe (ULIDs always produce 26 characters); no bounds panic risk.
- Uniqueness is structural inside a process, not merely probabilistic: the `SEQUENCE` suffix has to wrap (2^20 ids inside a single millisecond) before two ids can repeat. The 40 random bits cover the cross-process case, where no shared counter exists.
- The body was `ulid[..12]` until the collision fix. That kept the whole 10-char timestamp and only 2 random chars — 1024 distinct values per millisecond — so ids minted in one burst collided routinely (measured: 38% over 32 ids, 86% over 64). It surfaced as an intermittent `duplicate block id (across all slides)` failure in the HTML end-to-end test on faster CI machines.
